//! egui 界面：车次 / 车站 / 站站三种查询。

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::epaint::MarginF32;
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui};
use egui_extras::{Column, DatePickerButton, TableBuilder};
use jiff::civil::Date;

use crate::config::Tab;
use crate::{columns, config, fonts, theme};
use lltdb::date::{date_window, date_ymd, origin_ymd, today, weekday_cn, ymd};
use lltdb::extras::{DiningCar, Duty, DutySegment, ViaPoint};
use lltdb::{Db, OdResult, StationStop, Stop, TrainDetail, TrainSummary};

const OD_RESULT_LIMIT: usize = 500;
/// 输入/点击停止多久之后才落盘：逐键写盘既浪费，也会把半截输入写进配置。
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// 触摸屏上按住列分隔线多久才开始调整列宽（秒）。
///
/// 手指从行上开始拖动通常是滚动，不能因为擦到列边就把列宽改了；按住这一小会儿
/// 才进入「调整列宽」模式。鼠标没有这个限制。
const RESIZE_LONG_PRESS: f64 = 0.4;
const STATION_SUGGESTIONS: usize = 24;
/// 宽屏布局（右侧详情栏、自动聚焦输入框）的最小窗口宽度。
use theme::WIDE_MIN_WIDTH;
/// 读系统栏缩进的最小间隔：软键盘、导航栏、旋转屏幕都会改它，但不必逐帧过 JNI。
const INSET_PROBE_INTERVAL: Duration = Duration::from_millis(150);

// 系统栏缩进用 `egui::epaint::MarginF32`（4 个 f32），**不要**用 `egui::Margin`：
// 后者字段是 `i8`，手机上软键盘能有 350 多点高，换算时会被截断到 127，
// 底栏就停在键盘后面不动。`MarginF32` 还自带 `Rect - MarginF32` 与 `sum()`。

/// 标签页的文案与焦点规则。可写入配置的那份定义在 [`config::Tab`]（带 serde + 默认值）。
impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Od => "站站",
            Tab::Train => "车次",
            Tab::Station => "车站",
            Tab::Data => "设置",
        }
    }
}

/// 详情里点出去的去处。
#[derive(Clone, PartialEq, Eq)]
enum DetailLink {
    /// 停站表里的车站：切到「按车站查询」，只看这一站。
    Station(String),
    /// 交路里的另一个车次（段首车次号）：就地换详情。
    Train(String),
    /// 详情标题旁的「← G105」：回到上一个看过的车次。
    PreviousTrain(i64),
}

/// 从详情跳到车站页之前的现场：够一键回到「原来那一页、那一趟车」。
struct ReturnPoint {
    tab: Tab,
    station_query: String,
    station_fuzzy: bool,
    /// 跳走时正在看的车次（重新打开的详情）。
    train_index: i64,
    /// 给按钮用的文案：上一个车站名（原来不在车站页时是上一页的名字）。
    label: String,
}

/// 交路里以 `0` 开头的是回送 / 空送车，`DJ` 是检测车，都没有独立时刻表可查。
fn jumpable_train_code(code: &str) -> bool {
    let code = code.trim();
    !code.is_empty() && !code.starts_with('0') && !code.to_ascii_uppercase().starts_with("DJ")
}

/// 需要自动接管的输入框：切标签页 / 启动 / 清空后把焦点还给它。
#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusField {
    Train,
    Station,
    OdFrom,
    OdTo,
}

/// 三个查询页各自的查询日期。
///
/// 分开存：站站常用「今天」，车次可能在看某天的具体车，车站可能翻到周末，
/// 一个全局日期会让三个页面互相打架。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dates {
    od: Date,
    train: Date,
    station: Date,
}

impl Dates {
    fn get(&self, tab: Tab) -> Date {
        match tab {
            Tab::Train => self.train,
            Tab::Station => self.station,
            _ => self.od,
        }
    }
}

/// 一个查询页要画哪一半。
///
/// 竖屏是「控件在上、结果在下」的一整页；横屏把两半拆到左右两栏
/// （左栏控件、右栏结果），表格才能拿到完整高度。
#[derive(Clone, Copy, PartialEq, Eq)]
enum TabPart {
    /// 竖屏：控件 + 结果上下排（原有行为）。
    Stacked,
    /// 横屏左栏：只画控件。
    Controls,
    /// 横屏右栏：只画结果。
    Results,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OdField {
    From,
    To,
}

/// 设置页「表格列」里展开的是哪一张表：手风琴，一次只开一个。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnTable {
    Od,
    Train,
    Station,
    Stops,
}

/// 同城查询/车站筛选里的一个车站。
#[derive(Clone)]
struct StationOption {
    index: i64,
    name: String,
}

/// 车次详情里的附加数据（交路 / 经由 / 餐车）。
///
/// 旧库（`tools/decode_llt.py` 生成）没有这些表，三个字段都会是空数组，
/// 详情面板相应只显示停站表，不占额外空间。
#[derive(Default)]
struct TrainExtras {
    duties: Vec<Duty>,
    via: Vec<ViaPoint>,
    dining: Vec<DiningCar>,
    /// 交路明细：`(交路 id, 段序号)` → 该段车次当天首末站的时刻。查不到就没有这个键。
    ///
    /// 选中车次时一次查完（`duty_runs`），界面每帧只查表——展开交路不能逐帧翻库。
    runs: HashMap<(i64, i64), DutyRun>,
}

/// 交路里一段车次的首末站与时刻，展开「交路明细」时逐行显示。
#[derive(Clone, Debug)]
struct DutyRun {
    from: String,
    /// 首站发车时刻（`hh:mm`，数据里就是这个写法）。
    from_time: String,
    to: String,
    /// 末站到达时刻。
    to_time: String,
}

impl TrainExtras {
    /// 只保留查询日期有效的记录；当天没有有效记录时退回全部，避免整块消失。
    fn for_date(&self, date: i64) -> (Vec<&Duty>, Vec<&DiningCar>, bool) {
        let duties: Vec<&Duty> = self
            .duties
            .iter()
            .filter(|duty| duty.runs_on(date))
            .collect();
        let dining: Vec<&DiningCar> = self.dining.iter().filter(|car| car.runs_on(date)).collect();
        let matched = !duties.is_empty() || !dining.is_empty();
        if matched {
            (duties, dining, true)
        } else {
            (
                self.duties.iter().collect(),
                self.dining.iter().collect(),
                false,
            )
        }
    }
}

pub struct LltApp {
    db: Option<Db>,
    /// 后台正在替换库文件：连接已被主动放开（Windows 上不释放句柄就改不了名），
    /// 但界面不能因此掉进「还没有时刻表数据」页。
    db_suspended: bool,
    db_path: PathBuf,
    db_error: Option<String>,
    /// 数据库里记录的数据包版本（`20261108`），供标题栏显示数据日期。
    data_version: Option<i64>,
    /// 开行周期表：`rule_index -> (period, mask)`，打开数据库时一次读全。
    rules: std::collections::HashMap<i64, (i64, i64)>,
    /// 详情里开行计划日历正在看的月份（`yyyymm`），换车次时按查询日期重置。
    plan_month: i64,
    /// 桌面端的开行计划弹窗是否打开，以及打开时的窗口尺寸（变了就关）。
    plan_page_open: bool,
    plan_page_size: Option<egui::Vec2>,

    tab: Tab,
    /// 深浅色偏好（`ThemeChoice`），改了就落盘。
    theme: config::ThemeChoice,
    /// 三个查询页各自的查询日期（互不影响，各自落盘）。
    dates: Dates,

    train_query: String,
    train_results: Vec<TrainSummary>,

    station_query: String,
    /// 模糊搜索：把输入解析成同城组，匹配组内所有车站。
    station_fuzzy: bool,
    /// 自定义列：`None` = 按窗口宽度自动（默认）。
    od_columns: Option<Vec<columns::OdColumn>>,
    train_columns: Option<Vec<columns::TrainColumn>>,
    station_columns: Option<Vec<columns::StationColumn>>,
    stop_columns: Option<Vec<columns::StopColumn>>,
    /// 设置页「表格列」展开的是哪张表（`None` = 都收起）。
    columns_open: Option<ColumnTable>,
    station_suggestions: Vec<String>,
    /// 当前同城组的全部车站，供车站筛选。
    station_options: Vec<StationOption>,
    /// 车站筛选里勾选的车站下标。
    station_selected: BTreeSet<i64>,
    station_board_for: Option<String>,
    station_board: Vec<StationStop>,

    od_from: String,
    od_to: String,
    /// 模糊搜索：出发/到达都按同城组匹配。
    od_fuzzy: bool,
    /// 结果表按实际出发站/到达站筛选。
    od_filter_from: Option<String>,
    od_filter_to: Option<String>,
    od_last: OdField,
    od_missing: Option<OdField>,
    od_suggestions: Vec<String>,
    od_results: Vec<OdResult>,
    od_truncated: bool,
    od_message: Option<String>,

    selected: Option<i64>,
    detail: Option<TrainDetail>,
    extras: TrainExtras,
    /// 经由列表是否只看车站（记住用户选择，切换车次时不重置）。
    via_stations_only: bool,
    /// 窄屏时经由改成独立页面弹出，这里记录页面是否打开。
    via_page_open: bool,
    /// 打开经由页时的窗口尺寸；尺寸变化时关掉页面，避免模态停在旧位置。
    via_page_size: Option<egui::Vec2>,
    /// 下一次绘制时要抢焦点的输入框；宽屏生效，用完即清。
    focus_request: Option<FocusField>,
    /// 从详情跳去车站页之前的现场，用来一键回到原处。
    return_point: Option<ReturnPoint>,
    /// 上一个看过的车次（索引、车次号），详情标题旁显示「← G105」。
    train_back: Option<(i64, String)>,
    error: Option<String>,
    window_fitted: bool,
    /// 平台差异：安全区、输入法、指针类型、数据目录。
    platform: crate::Platform,
    /// 「数据」页的状态（当前库、线上版本、后台任务、备份列表）。
    data: crate::data::DataPage,
    /// 上一帧拿着键盘焦点的控件（而不是「有没有焦点」）：从出发框跳到到达框时
    /// 焦点一直是 `Some`，但输入法连接需要重挂一次。
    ime_focus: Option<egui::Id>,
    /// 上一帧的可用尺寸（逻辑点），用来发现横竖屏/分屏变化。
    last_screen: egui::Vec2,
    /// 点击输入框后还要补几次「弹键盘」：egui 每次点击都会 interrupt_ime，
    /// egui-winit 随之 set_ime_allowed(false)+(true)，Android 上先收后弹，
    /// 而「弹」会被系统忽略（焦点在我们的桥 View 上），所以要自己补一次。
    ime_reshow: u8,
    /// 上一次读到的缩进。
    safe_area: crate::ScreenInsets,
    /// 上一次询问缩进的时间，用作最小间隔。
    inset_probe_at: Option<Instant>,
    /// 上一次报给平台的「系统栏图标是否要深色」（浅色主题）；`None` = 还没报过。
    bars_light: Option<bool>,
    /// 上一次从界面状态里读出来的列宽（落盘快照用；`on_exit` 拿不到 `ctx`）。
    column_widths: config::ColumnWidths,
    /// 详情页的身份：每打开一次车次就加一。
    ///
    /// 详情里的滚动区、停站表都把它拼进 `id_salt`，于是换车次就是一份全新的滚动状态——
    /// 新打开的车次从页首开始，不会继承上一趟车翻到的位置。
    detail_gen: u64,

    /// 配置文件路径：默认紧挨可执行文件，见 [`config::resolve`]。
    config_path: PathBuf,
    /// 上一次交给写盘的状态；与当前状态不同就会触发一次去抖保存。
    persisted: config::Settings,
    /// 第一次发现改动的时间，用于 [`SAVE_DEBOUNCE`] 去抖。
    dirty_since: Option<Instant>,
    /// 已经提示过保存失败：同一条错误只打扰一次，写成功后再复位。
    save_failed: bool,
}

impl LltApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        db_path: PathBuf,
        config_path: PathBuf,
        platform: crate::Platform,
    ) -> Self {
        fonts::install(&cc.egui_ctx, &platform.font_dirs);
        theme::install(&cc.egui_ctx, platform.pointer);
        // 输入法上屏发生在别的线程：先把界面句柄交给它，它才催得动帧。
        if let Some(ime) = &platform.ime {
            ime.attach_context(&cc.egui_ctx);
        }

        // 配置读取失败只降级，不阻断启动：返回默认值 + 一句提示。
        let (settings, config_warning) = config::load(&config_path);
        // 上次选的深浅色（默认跟随系统）。
        cc.egui_ctx.set_theme(settings.ui.theme.preference());
        let data_page = crate::data::DataPage::new(platform.dirs.clone(), platform.files.clone());
        let mut app = Self {
            db: None,
            db_suspended: false,
            data: data_page,
            db_path,
            db_error: None,
            data_version: None,
            // 上次停留的标签页；第一次启动是第一位（站站）。
            tab: settings.ui.tab,
            theme: settings.ui.theme,
            dates: Dates {
                od: settings.od.date.unwrap_or_else(today),
                train: settings.train.date.unwrap_or_else(today),
                station: settings.station.date.unwrap_or_else(today),
            },
            train_query: settings.train.query.clone(),
            train_results: Vec::new(),
            station_query: settings.station.query.clone(),
            station_fuzzy: settings.station.fuzzy,
            od_columns: settings
                .columns
                .od
                .as_deref()
                .map(|keys| columns::parse(keys, columns::OdColumn::from_key))
                .filter(|picked| !picked.is_empty()),
            train_columns: settings
                .columns
                .train
                .as_deref()
                .map(|keys| columns::parse(keys, columns::TrainColumn::from_key))
                .filter(|picked| !picked.is_empty()),
            station_columns: settings
                .columns
                .station
                .as_deref()
                .map(|keys| columns::parse(keys, columns::StationColumn::from_key))
                .filter(|picked| !picked.is_empty()),
            stop_columns: settings
                .columns
                .stops
                .as_deref()
                .map(|keys| columns::parse(keys, columns::StopColumn::from_key))
                .filter(|picked| !picked.is_empty()),
            columns_open: None,
            station_suggestions: Vec::new(),
            station_options: Vec::new(),
            station_selected: BTreeSet::new(),
            station_board_for: None,
            station_board: Vec::new(),
            od_from: settings.od.from.clone(),
            od_to: settings.od.to.clone(),
            od_fuzzy: settings.od.fuzzy,
            od_filter_from: None,
            od_filter_to: None,
            od_last: OdField::From,
            od_missing: None,
            od_suggestions: Vec::new(),
            od_results: Vec::new(),
            od_truncated: false,
            od_message: None,
            selected: None,
            detail: None,
            extras: TrainExtras::default(),
            via_stations_only: settings.detail.via_stations_only,
            via_page_open: false,
            via_page_size: None,
            // 恢复到上次停留的标签页；焦点在构造完再按当前页决定（见下）。
            focus_request: None,
            return_point: None,
            train_back: None,
            error: None,
            window_fitted: false,
            rules: std::collections::HashMap::new(),
            plan_month: 0,
            plan_page_open: false,
            plan_page_size: None,
            platform,
            safe_area: crate::ScreenInsets::default(),
            inset_probe_at: None,
            column_widths: settings.columns.widths.clone(),
            detail_gen: 0,
            bars_light: None,
            config_path,
            persisted: settings,
            dirty_since: None,
            save_failed: false,
            ime_focus: None,
            last_screen: egui::Vec2::ZERO,
            ime_reshow: 0,
        };
        // 列宽记忆：把配置里记住的列宽塞回界面状态（绘制时按它排），
        // 同时记住「按哪套布局存的」——窗口尺寸变了就不再硬套。
        seed_column_widths(&cc.egui_ctx, &app.persisted.columns.widths);
        // 打开就能打字：焦点落在恢复出来的那一页的输入框。
        app.focus_request = Some(app.focus_field(app.tab));
        app.open_db();
        app.data.refresh(&app.db_path);
        // 提示放在 open_db 之后：恢复查询会先清空 `error`。
        if let Some(warning) = config_warning {
            app.error = Some(warning);
        }
        app
    }

    /// 某一天这趟车开不开行：区间内还要过开行周期那一关。
    ///
    /// 口径与 `docs/llt-format-reverse-engineering.md` §12 一致；`rule_index` 查不到
    /// （老库没有 `train_rules`）时退回「区间内天天开行」。
    fn running(&self, date: i64, from: i64, to: i64, rule_index: Option<i64>) -> bool {
        let (period, mask) = rule_index
            .and_then(|index| self.rules.get(&index).copied())
            .unwrap_or((1, 0));
        lltdb::date::run_state(from, to, date, period, mask) == lltdb::date::RunState::Running
    }

    fn open_db(&mut self) {
        self.db = None;
        self.db_suspended = false;
        self.data_version = None;
        self.db_error = None;

        match Db::open(&self.db_path) {
            Ok(db) => {
                self.data_version = db.data_version();
                // 开行周期表很小（131 行），一次读全，界面按 rule_index 取。
                self.rules = db.all_rules();
                self.db = Some(db);
                // 配置里恢复出来的查询词要立刻查一次，否则页面上只有输入、没有结果。
                // `open_db` 也是错误页「重试」的入口，所以恢复逻辑放在这里而非 new()。
                self.refresh_restored_queries();
                self.demo_preselect();
            }
            Err(err) => self.db_error = Some(err),
        }
    }

    /// 调试用：`LLT_DEMO_TRAIN=G531` 时启动直接查这趟车并打开详情。
    ///
    /// 截图验证详情页（尤其开行计划日历）时省掉「点行」这一步——自动化点击
    /// 在不同窗口尺寸下很容易打偏，而这里走的是同一条 `select_train` 路径。
    fn demo_preselect(&mut self) {
        let Ok(code) = std::env::var("LLT_DEMO_TRAIN") else {
            return;
        };
        if code.trim().is_empty() || self.db.is_none() {
            return;
        }
        self.tab = Tab::Train;
        self.train_query = code;
        self.refresh_train_search();
        if let Some(first) = self.train_results.first().map(|train| train.index) {
            self.select_train(first);
        }
    }

    /// 把配置文件恢复的查询词跑一遍；空查询跳过，重试成功后会再走一次。
    fn refresh_restored_queries(&mut self) {
        if !self.train_query.trim().is_empty() {
            self.refresh_train_search();
        }
        if !self.station_query.trim().is_empty() {
            self.refresh_station();
        }
        if !self.od_from.trim().is_empty() && !self.od_to.trim().is_empty() {
            self.refresh_od();
        }
    }

    // ------------------------------------------------------------ 输入法

    /// 把平台输入法这一帧的上屏事件塞进 egui。
    ///
    /// 必须赶在所有控件之前：`TextEdit` 是在自己的 `show` 里读 `input.events` 的，
    /// 事件晚一步推进去，这一帧就打不出来。桌面没有这个通道（`ime` 为 `None`），
    /// 走 egui-winit 的 `WindowEvent::Ime`。
    fn pump_ime(&mut self, ctx: &egui::Context) {
        let Some(ime) = self.platform.ime.clone() else {
            return;
        };
        let mut dismissed = false;
        let events: Vec<egui::Event> = ime
            .drain()
            .into_iter()
            .filter_map(|input| match input {
                crate::ImeInput::Commit(text) => {
                    Some(egui::Event::Ime(egui::ImeEvent::Commit(text)))
                }
                crate::ImeInput::Preedit(text) => Some(egui::Event::Ime(egui::ImeEvent::Preedit {
                    text,
                    active_range_chars: None,
                })),
                crate::ImeInput::Delete { before, after } => {
                    Some(egui::Event::Ime(egui::ImeEvent::DeleteSurrounding {
                        before_chars: before,
                        after_chars: after,
                    }))
                }
                crate::ImeInput::Enter => Some(enter_key()),
                // 用户自己把键盘收了（返回键 / 键盘上的「收起」）。系统不会告诉 egui，
                // 输入框会一直保持焦点、光标还在闪，之后再点一下界面又把键盘弹回来。
                // 这里替它放手：焦点一没，下面那段「补两次 show」也就不会触发。
                crate::ImeInput::Hidden => {
                    dismissed = true;
                    None
                }
            })
            .collect();
        if dismissed {
            // `stop_text_input` 只放开焦点，不动输入框里的文字。
            ctx.memory_mut(|memory| memory.stop_text_input());
            self.ime_reshow = 0;
        }
        let typed = !events.is_empty();
        if typed {
            ctx.input_mut(|input| input.events.extend(events));
        }
        // 焦点变化时过一次 JNI：Android 靠它把输入法连接挂到自己的 View 上、并弹键盘。
        //
        // 三件事都会让 egui 调 `Memory::request_focus` → `interrupt_ime`，
        // egui-winit 收到后就在帧末 `set_ime_allowed(false)` + `(true)`：
        // Android 上等于先收键盘，「再弹」又被系统忽略（焦点在我们的桥 View 上）。
        // 用户看到的是「打一个字键盘就没了」/「回车跳格后键盘没了」。所以这三处之后
        // 都补两次 show（跨过那一帧的 hide）：
        //   1. 焦点换到另一个输入框（`Some(id)` → `Some(other)` 也算）；
        //   2. 点了一下界面（egui 每次点击都会 request_focus）；
        //   3. 刚刚输入了内容（打字本身不该影响键盘）。
        let focused = ctx.memory(|memory| memory.focused());
        let clicked = ctx.input(|input| input.pointer.any_click());
        if focused != self.ime_focus {
            self.ime_focus = focused;
            self.ime_reshow = 0;
            ime.set_text_focus(focused.is_some());
            if focused.is_some() {
                self.ime_reshow = 2;
                ctx.request_repaint_after(std::time::Duration::from_millis(40));
            }
        } else if focused.is_some() && (clicked || typed) {
            self.ime_reshow = 2;
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        } else if self.ime_reshow > 0 && focused.is_some() {
            self.ime_reshow -= 1;
            ime.ensure_shown();
            if self.ime_reshow > 0 {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
    }

    // ------------------------------------------------------------ 配置

    /// 切到某个标签页时该抢焦点的输入框（站站落在还空着的那一侧）。
    fn focus_field(&self, tab: Tab) -> FocusField {
        match tab {
            Tab::Train => FocusField::Train,
            Tab::Station => FocusField::Station,
            Tab::Data => self.focus_field(Tab::Od),
            Tab::Od if self.od_from.trim().is_empty() => FocusField::OdFrom,
            Tab::Od if self.od_to.trim().is_empty() => FocusField::OdTo,
            Tab::Od => FocusField::OdFrom,
        }
    }

    /// 当前需要落盘的状态。
    fn settings(&self) -> config::Settings {
        config::Settings {
            version: config::CONFIG_VERSION,
            ui: config::UiSettings {
                tab: self.tab,
                theme: self.theme,
            },
            train: config::TrainSettings {
                query: self.train_query.clone(),
                date: Some(self.dates.train),
            },
            station: config::StationSettings {
                query: self.station_query.clone(),
                fuzzy: self.station_fuzzy,
                date: Some(self.dates.station),
            },
            od: config::OdSettings {
                from: self.od_from.clone(),
                to: self.od_to.clone(),
                fuzzy: self.od_fuzzy,
                date: Some(self.dates.od),
            },
            detail: config::DetailSettings {
                via_stations_only: self.via_stations_only,
            },
            columns: config::ColumnSettings {
                od: self
                    .od_columns
                    .as_deref()
                    .map(|picked| columns::keys(picked, columns::OdColumn::key)),
                train: self
                    .train_columns
                    .as_deref()
                    .map(|picked| columns::keys(picked, columns::TrainColumn::key)),
                station: self
                    .station_columns
                    .as_deref()
                    .map(|picked| columns::keys(picked, columns::StationColumn::key)),
                stops: self
                    .stop_columns
                    .as_deref()
                    .map(|picked| columns::keys(picked, columns::StopColumn::key)),
                widths: self.column_widths.clone(),
            },
        }
    }

    /// 从界面状态里读各表当前的列宽（只记拖过、或者从配置里恢复回来的）。
    ///
    /// 宽度的工作副本在 egui 的界面状态里（`table_widths` 那一套），这里读出来取整；
    /// 这一轮还没画过的表沿用上次的值，不要抹掉。
    fn live_column_widths(&self, ctx: &egui::Context) -> config::ColumnWidths {
        let mut widths = config::ColumnWidths::default();
        for table in WidthTable::ALL {
            let stored: Option<Vec<f32>> =
                ctx.data(|data| data.get_temp(egui::Id::unique((table.salt(), "widths"))));
            let viewport: Option<i32> =
                ctx.data(|data| data.get_temp(egui::Id::unique(("table_viewport", table.salt()))));
            let live = match (stored, viewport) {
                (Some(stored), Some(viewport)) if !stored.is_empty() => Some(config::ColumnWidth {
                    viewport,
                    widths: stored
                        .iter()
                        .map(|width| width.round().clamp(1.0, f32::from(u16::MAX)) as u16)
                        .collect(),
                }),
                _ => None,
            };
            let kept = live.or_else(|| table.get(&self.persisted.columns.widths).cloned());
            table.set(&mut widths, kept);
        }
        widths
    }

    /// 帧首调用：状态与上次落盘的快照不同时，停手 [`SAVE_DEBOUNCE`] 后写一次。
    ///
    /// 放在帧首而不是帧尾，是为了覆盖所有提前 `return` 的分支（库打不开时也会
    /// 记下当前状态）；快照最多落后一帧，对这几个开关和输入框没有影响。
    fn persist(&mut self, ctx: &egui::Context) {
        self.column_widths = self.live_column_widths(ctx);
        let snapshot = self.settings();
        if snapshot == self.persisted {
            self.dirty_since = None;
            return;
        }
        let now = Instant::now();
        let since = *self.dirty_since.get_or_insert(now);
        if now.duration_since(since) < SAVE_DEBOUNCE {
            // 没有输入时界面不再重绘，这里补一次唤醒，保证去抖到点。
            ctx.request_repaint_after(SAVE_DEBOUNCE);
            return;
        }
        self.store(&snapshot);
        self.dirty_since = None;
    }

    /// 写盘并记录「已处理过的快照」。
    fn store(&mut self, snapshot: &config::Settings) {
        if let Err(err) = config::save(&self.config_path, snapshot) {
            // 只读目录（Program Files、只读共享盘）会一直失败：提示一次就够。
            if !self.save_failed {
                self.save_failed = true;
                self.error = Some(format!(
                    "保存配置失败（{}）：{err}。可以用 --config 或环境变量 LLT_CONFIG 换一个可写位置。",
                    self.config_path.display()
                ));
            }
        } else {
            self.save_failed = false;
        }
        // 无论成功与否都记作「已处理」，否则失败时会每 500 ms 重试一次。
        self.persisted = snapshot.clone();
    }

    // ------------------------------------------------------------ 查询

    fn refresh_train_search(&mut self) {
        self.error = None;
        let query = normalize_code(&self.train_query);
        if query.is_empty() {
            self.train_results.clear();
            return;
        }

        let mut error = None;
        let results = if let Some(db) = &self.db {
            match db.search_trains(&query, None) {
                Ok(rows) => rows,
                Err(err) => {
                    error = Some(format!("车次查询失败：{err}"));
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        self.error = error;
        self.train_results = results;
    }

    fn refresh_station(&mut self) {
        self.error = None;
        let query = normalize_station(&self.station_query);
        if query.is_empty() {
            self.station_suggestions.clear();
            self.station_options.clear();
            self.station_selected.clear();
            self.station_board_for = None;
            self.station_board.clear();
            return;
        }

        let mut error = None;
        let mut suggestions = Vec::new();
        let mut fuzzy_selection = None;
        let mut exact_station = None;

        if let Some(db) = &self.db {
            match db.station_suggestions(&query, STATION_SUGGESTIONS) {
                Ok(rows) => suggestions = rows,
                Err(err) => error = Some(format!("车站查询失败：{err}")),
            }
            if self.station_fuzzy {
                match db.same_city_selection(&query) {
                    Ok(selection) => fuzzy_selection = selection,
                    Err(err) => error = Some(format!("同城查询失败：{err}")),
                }
            } else {
                match db.find_station(&query) {
                    Ok(Some(name)) => match db.station_index(&name) {
                        Ok(Some(index)) => exact_station = Some((name, index)),
                        Ok(None) => {}
                        Err(err) => error = Some(format!("车站查询失败：{err}")),
                    },
                    Ok(None) => {}
                    Err(err) => error = Some(format!("车站查询失败：{err}")),
                }
            }
        }
        self.error = error;
        self.station_suggestions = suggestions;

        if let Some((title, stations)) = fuzzy_selection {
            self.apply_station_selection(title, stations);
        } else if let Some((name, index)) = exact_station {
            self.apply_station_selection(name.clone(), vec![(index, name)]);
        } else {
            self.station_options.clear();
            self.station_selected.clear();
            self.station_board_for = None;
            self.station_board.clear();
        }
    }

    /// 写入同城组/单站选择，并加载所有选中车站的时刻表。
    fn apply_station_selection(&mut self, title: String, stations: Vec<(i64, String)>) {
        self.station_board_for = Some(title);
        self.station_options = stations
            .into_iter()
            .map(|(index, name)| StationOption { index, name })
            .collect();
        self.station_selected = self
            .station_options
            .iter()
            .map(|option| option.index)
            .collect();
        self.reload_station_board();
    }

    /// 按当前车站筛选重新加载时刻表。
    fn reload_station_board(&mut self) {
        self.station_board.clear();
        let indices: Vec<i64> = self
            .station_options
            .iter()
            .filter(|option| self.station_selected.contains(&option.index))
            .map(|option| option.index)
            .collect();
        if indices.is_empty() {
            return;
        }

        let mut error = None;
        let mut board = Vec::new();
        if let Some(db) = &self.db {
            match db.station_board_multi(&indices) {
                Ok(rows) => board = rows,
                Err(err) => error = Some(format!("读取车站时刻表失败：{err}")),
            }
        }
        self.station_board = board;
        if let Some(err) = error {
            self.error = Some(err);
        }
    }

    fn refresh_od(&mut self) {
        self.error = None;
        self.od_results.clear();
        self.od_truncated = false;
        self.od_message = None;
        self.od_missing = None;
        self.od_suggestions.clear();
        self.od_filter_from = None;
        self.od_filter_to = None;

        let from = normalize_station(&self.od_from);
        let to = normalize_station(&self.od_to);
        if from.is_empty() || to.is_empty() {
            return;
        }

        let Some(db) = &self.db else { return };

        let mut error = None;
        let mut from_selection = None;
        let mut to_selection = None;
        if self.od_fuzzy {
            match db.same_city_selection(&from) {
                Ok(selection) => from_selection = selection,
                Err(err) => error = Some(format!("同城查询失败：{err}")),
            }
            match db.same_city_selection(&to) {
                Ok(selection) => to_selection = selection,
                Err(err) => error = Some(format!("同城查询失败：{err}")),
            }
        } else {
            match db.find_station(&from) {
                Ok(Some(name)) => match db.station_index(&name) {
                    Ok(Some(index)) => from_selection = Some((name.clone(), vec![(index, name)])),
                    Ok(None) => {}
                    Err(err) => error = Some(format!("车站查询失败：{err}")),
                },
                Ok(None) => {}
                Err(err) => error = Some(format!("车站查询失败：{err}")),
            }
            match db.find_station(&to) {
                Ok(Some(name)) => match db.station_index(&name) {
                    Ok(Some(index)) => to_selection = Some((name.clone(), vec![(index, name)])),
                    Ok(None) => {}
                    Err(err) => error = Some(format!("车站查询失败：{err}")),
                },
                Ok(None) => {}
                Err(err) => error = Some(format!("车站查询失败：{err}")),
            }
        }
        self.error = error;

        let missing = match (&from_selection, &to_selection) {
            (Some(_), Some(_)) => None,
            (None, Some(_)) => Some(OdField::From),
            (Some(_), None) => Some(OdField::To),
            (None, None) => Some(self.od_last),
        };
        if let Some(field) = missing {
            let query = if field == OdField::From { &from } else { &to };
            self.od_missing = Some(field);
            self.od_message = Some(if self.od_fuzzy {
                format!("没有找到名为「{query}」的同城组或车站，点输入框挑候选。")
            } else {
                format!("没有名为「{query}」的车站，点输入框挑候选站。")
            });
            match db.station_suggestions(query, 8) {
                Ok(rows) => self.od_suggestions = rows,
                Err(err) => self.error = Some(format!("车站查询失败：{err}")),
            }
            return;
        }
        let (Some((from_title, from_stations)), Some((to_title, to_stations))) =
            (from_selection, to_selection)
        else {
            return;
        };

        let from_indices: Vec<i64> = from_stations.iter().map(|(index, _)| *index).collect();
        let to_indices: Vec<i64> = to_stations.iter().map(|(index, _)| *index).collect();
        let from_set: BTreeSet<i64> = from_indices.iter().copied().collect();
        let to_set: BTreeSet<i64> = to_indices.iter().copied().collect();
        if from_set == to_set {
            self.od_message =
                Some("出发与到达是同城/同一车站，请选择不同的城市或车站。".to_owned());
            return;
        }

        match db.od_search_multi(&from_indices, &to_indices, OD_RESULT_LIMIT) {
            Ok((rows, truncated)) => {
                self.od_results = rows;
                self.od_truncated = truncated;
                if self.od_results.is_empty() {
                    self.od_message =
                        Some(format!("没有找到 {from_title} → {to_title} 的直达车次。"));
                }
            }
            Err(err) => self.error = Some(format!("站站查询失败：{err}")),
        }
    }

    fn select_train(&mut self, index: i64) {
        // 换车次时把日历拨回查询日期那一月，顺手关掉弹窗。
        self.plan_month = 0;
        self.plan_page_open = false;
        self.plan_page_size = None;
        if let Some(detail) = &self.detail {
            if detail.summary.index == index {
                self.selected = Some(index);
                return;
            }
            // 记住上一个车次，详情标题旁给一条回头路「← G105」。
            self.train_back = Some((detail.summary.index, detail.summary.code.clone()));
        }
        self.via_page_open = false;
        self.via_page_size = None;
        self.error = None;
        // 新页面从头开始：详情里的滚动区按 `detail_gen` 换 id，上一趟车的滚动位置不继承。
        self.detail_gen = self.detail_gen.wrapping_add(1);

        let mut error = None;
        let mut detail = None;
        let mut extras = TrainExtras::default();
        let date = ymd(self.dates.get(self.tab));
        if let Some(db) = &self.db {
            match db.train_detail(index) {
                Ok(d) => detail = Some(d),
                Err(err) => error = Some(format!("读取车次详情失败：{err}")),
            }
            // 附加数据缺失（旧库）时返回空数组，不影响停站表。
            extras.duties = db.duties_for(index).unwrap_or_default();
            extras.via = db.via_points(index).unwrap_or_default();
            extras.dining = db.dining_cars(index).unwrap_or_default();
            extras.runs = duty_runs(db, &extras.duties, date);
        }
        if let Some(err) = error {
            self.error = Some(err);
        }
        if let Some(detail) = detail {
            self.selected = Some(index);
            self.detail = Some(detail);
            self.extras = extras;
        }
    }

    /// 收起详情：连带清掉经由页与「上一个车次」，避免下次打开时串味。
    fn close_detail(&mut self) {
        self.selected = None;
        self.detail = None;
        self.extras = TrainExtras::default();
        self.via_page_open = false;
        self.via_page_size = None;
        self.train_back = None;
    }

    // ------------------------------------------------------------ 界面

    fn header(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        let touch = self.platform.pointer == theme::Pointer::Touch;
        // 触摸屏：标签页挪到底栏（见 `body`），顶栏只留一行「标题 + 日期 + 主题」。
        // 手机再小一号标题，抵掉一部分安全区。
        let title_size = if touch && width < WIDE_MIN_WIDTH {
            theme::FONT_SECTION
        } else {
            theme::FONT_TITLE
        };
        ui.horizontal(|ui| {
            if touch {
                // 顶栏只放一行控件，36 点足够手指点中，也省下 8 点给表格。
                ui.spacing_mut().interact_size.y = theme::header_height();
            }
            ui.label(
                RichText::new("路路通时刻表")
                    .size(title_size)
                    .family(fonts::bold_family()),
            );
            if width > 300.0
                && let Some(version) = self.data_version
            {
                ui.add_space(theme::GAP_XS);
                ui.label(theme::mono(format!("数据 {}", date_ymd(version))).weak())
                    .on_hover_text(format!("数据包版本 {version}（库内 meta.data_version）"));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.theme_button(ui);
                if !touch && width > 980.0 {
                    let file_name = self
                        .db_path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| self.db_path.display().to_string());
                    ui.label(meta(file_name).weak())
                        .on_hover_text(self.db_path.display().to_string());
                }
            });
        });
        if touch {
            return;
        }

        ui.add_space(theme::GAP_S);
        self.tab_bar(ui);
    }

    /// 标签页那一行。触摸窄屏与宽屏共用，只有日期控件的位置不同。
    fn tab_bar(&mut self, ui: &mut Ui) {
        // 自己起一个水平布局：宽屏时调用方本来就在 `ui.horizontal` 里，
        // 嵌套只是把标签页圈成一组；触摸窄屏那一行是竖的，这里必须横过来。
        ui.horizontal(|ui| {
            // 标签页之间只靠内边距分隔，避免多余的分隔线。
            ui.spacing_mut().item_spacing.x = 2.0;
            let tabs = [
                (Tab::Od, "站站"),
                (Tab::Train, "车次"),
                (Tab::Station, "车站"),
                (Tab::Data, "设置"),
            ];
            let entered = self.tab;
            for (tab, label) in tabs {
                if ui
                    .selectable_value(
                        &mut self.tab,
                        tab,
                        RichText::new(label).size(theme::FONT_BODY),
                    )
                    .clicked()
                {
                    // 点标签页（含重复点当前页）就把焦点交回该页的输入框。
                    self.focus_request = Some(self.focus_field(tab));
                }
            }
            if self.tab != entered && self.tab == Tab::Data {
                // 进数据页时重读一次：库可能刚被命令行或别的进程换掉。
                self.data.refresh(&self.db_path);
            }
        });
    }

    /// 底栏里的标签页：四等分、整块可点（屏幕上最小的点击单位也够大）。
    fn tab_bar_segments(&mut self, ui: &mut Ui) {
        let tabs = [
            (Tab::Od, "站站"),
            (Tab::Train, "车次"),
            (Tab::Station, "车站"),
            (Tab::Data, "设置"),
        ];
        let entered = self.tab;
        let height = theme::bar_height();
        ui.columns(tabs.len(), |columns| {
            for (index, (tab, label)) in tabs.iter().enumerate() {
                let column = &mut columns[index];
                let selected = self.tab == *tab;
                let button = egui::Button::selectable(
                    selected,
                    RichText::new(*label).size(theme::FONT_BODY),
                );
                let width = column.available_width();
                if column.add_sized([width, height], button).clicked() {
                    self.tab = *tab;
                    self.focus_request = Some(self.focus_field(*tab));
                }
            }
        });
        if self.tab != entered && self.tab == Tab::Data {
            // 进数据页时重读一次：库可能刚被命令行或别的进程换掉。
            self.data.refresh(&self.db_path);
        }
    }

    /// 浅色 / 深色切换。
    /// 切换深浅色并记住选择（头部按钮与设置页共用同一个入口）。
    fn set_theme(&mut self, ctx: &egui::Context, choice: config::ThemeChoice) {
        self.theme = choice;
        ctx.set_theme(choice.preference());
    }

    fn theme_button(&mut self, ui: &mut Ui) {
        let dark = ui.visuals().dark_mode;
        let (label, next) = if dark {
            ("浅色", config::ThemeChoice::Light)
        } else {
            ("深色", config::ThemeChoice::Dark)
        };
        if ui.add(egui::Button::new(meta(label))).clicked() {
            self.set_theme(ui.ctx(), next);
        }
    }

    fn error_banner(&mut self, ui: &mut Ui) {
        let mut dismiss = false;
        if let Some(error) = &self.error {
            let color = ui.visuals().error_fg_color;
            ui.horizontal(|ui| {
                ui.label(RichText::new(error).size(theme::FONT_META).color(color));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("忽略").clicked() {
                        dismiss = true;
                    }
                });
            });
            ui.add_space(theme::GAP_S);
        }
        if dismiss {
            self.error = None;
        }
    }

    /// 把焦点交给输入框：只在启动、切标签页、清空这三个「用户刚做完动作」的时刻触发，
    /// 不做逐帧抢占，否则点复选框、日期控件、筛选 chips 都会被抢回去。
    ///
    /// 窄屏不接管：小窗口常对应触屏，弹出行内键盘比省一次点击更烦。
    fn take_focus(&mut self, ui: &Ui, field: FocusField, response: &egui::Response) {
        if self.focus_request != Some(field) {
            return;
        }
        self.focus_request = None;
        if ui.ctx().content_rect().width() >= WIDE_MIN_WIDTH {
            response.request_focus();
        }
    }

    fn train_ui(&mut self, ui: &mut Ui, part: TabPart) {
        let bar = self.bottom_bar();
        let width = ui.available_width();
        let field_width = (width * 0.34).clamp(140.0, 240.0);
        let mut clear = false;
        if part == TabPart::Results {
            self.train_results_ui(ui, bar);
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            ui.label(RichText::new("车次").size(theme::FONT_BODY));
            let response = ui.add(query_input(
                "train",
                &mut self.train_query,
                "例如 G1 / K54 / 1461",
                field_width,
            ));
            // 车次页的日期紧挨输入框右边：看过哪天的车一目了然。
            date_picker(ui, &mut self.dates.train, "train_date", false);
            self.take_focus(ui, FocusField::Train, &response);
            if response.changed() {
                self.refresh_train_search();
            }
            // 触摸屏上「清空 + 计数」在底栏（`train_bar`）。
            if !bar && !self.train_query.trim().is_empty() {
                if ui.add(egui::Button::new(meta("清空"))).clicked() {
                    clear = true;
                }
                ui.label(meta(format!("{} 条匹配", self.train_results.len())).weak());
            }
        });
        if clear {
            self.train_query.clear();
            self.train_results.clear();
            // 清空是为了立刻重输，焦点留在（还给）输入框。
            self.focus_request = Some(FocusField::Train);
        }
        if part == TabPart::Stacked {
            self.train_results_ui(ui, bar);
        }
    }

    /// 按车次的结果半页：空状态或（分「当日开行 / 不开行」的）车次表。
    fn train_results_ui(&mut self, ui: &mut Ui, bar: bool) {
        ui.add_space(theme::GAP_S);

        if self.train_query.trim().is_empty() {
            empty_state(
                ui,
                "车次",
                "输入车次号开始查询，支持前缀匹配（G1）和车次号（54）。",
            );
            return;
        }
        if self.train_results.is_empty() {
            empty_state(
                ui,
                "没有匹配的车次",
                "试试完整车次（G1）或去掉前缀的车次号（54）。",
            );
            return;
        }

        let selected_ymd = ymd(self.dates.train);
        let (running, not_running): (Vec<&TrainSummary>, Vec<&TrainSummary>) =
            self.train_results.iter().partition(|train| {
                self.running(
                    selected_ymd,
                    train.date_from,
                    train.date_to,
                    train.rule_index,
                )
            });
        let rows: Vec<&TrainSummary> = running.iter().chain(not_running.iter()).copied().collect();
        let running_count = running.len();
        if !bar {
            show_counts(ui, None, running_count, rows.len() - running_count);
        }
        let selected = self.selected;
        let clicked = Self::train_rows_table(
            ui,
            &rows,
            running_count,
            selected,
            "train_results",
            self.train_columns.as_deref(),
        );
        if let Some(index) = clicked {
            self.select_train(index);
        }
    }

    /// 车次结果表。列同上，`custom = None` 时按宽度自动挑。
    fn train_rows_table(
        ui: &mut Ui,
        rows: &[&TrainSummary],
        running_count: usize,
        selected: Option<i64>,
        salt: &str,
        custom: Option<&[columns::TrainColumn]>,
    ) -> Option<i64> {
        use columns::TrainColumn as Col;
        let specs: Vec<columns::Spec<Col>> = match custom {
            Some(picked) => picked.iter().map(|column| column.spec()).collect(),
            None => Col::auto(ui.available_width())
                .iter()
                .map(|column| column.spec())
                .collect(),
        };
        let pitch = theme::list_row_height() + ui.spacing().item_spacing.y;
        let height = theme::list_header_height() + rows.len() as f32 * pitch + theme::GAP_S;
        let mut clicked = None;
        scrollable_table(
            ui,
            salt,
            height,
            theme::list_header_height(),
            |ui, content_width| {
                let (computed, _) =
                    column_widths(&specs, content_width, ui.spacing().item_spacing.x);
                let mut widths = table_widths(ui, salt, &specs, computed, Col::key);
                apply_resize_drag(ui, salt, &specs, &mut widths, Col::resizable);
                let active = table_active_column(ui, salt);
                let table = exact_columns(
                    TableBuilder::new(ui)
                        .id_salt(salt)
                        .striped(true)
                        .sense(Sense::click())
                        .drag_to_scroll(egui::scroll_area::DragScroll::Never)
                        .auto_shrink([false, false])
                        .cell_layout(cell_layout()),
                    &widths,
                );
                // 单元格右边界直接取自表头实际画出来的位置：滚动、内边距都由它兜底，
                // 不用自己拿宽度累加（累加对不上只会在滚动时偏几十点）。
                let mut edges: Vec<f32> = Vec::with_capacity(specs.len());
                let mut cells: Vec<egui::Rect> = Vec::with_capacity(specs.len());
                let mut first_row: Option<egui::Rect> = None;
                table
                    .header(theme::list_header_height(), |mut header| {
                        for (index, spec) in specs.iter().enumerate() {
                            header.col(|ui| {
                                edges.push(ui.max_rect().right());
                                cells.push(ui.max_rect());
                                column_head(ui, spec, active == Some(index));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(theme::list_row_height(), rows.len(), |mut row| {
                            let train = rows[row.index()];
                            let running = row.index() < running_count;
                            row.set_selected(selected == Some(train.index));
                            if running_count > 0 && row.index() == running_count {
                                row.set_overline(true);
                            }
                            let window = date_window(train.date_from, train.date_to);
                            for spec in &specs {
                                row.col(|ui| match spec.column {
                                    Col::Status => status_cell(ui, running),
                                    Col::Code => {
                                        train_code_cell(ui, &train.code, Some(window.as_str()))
                                    }
                                    Col::Kind => {
                                        ui.add(egui::Label::new(&train.kind).truncate());
                                    }
                                    Col::Route => route_cell(ui, &train.first, &train.last),
                                    Col::Km => right_label(ui, format!("{} km", train.total_km)),
                                    Col::Days => right_label(ui, format!("{} 天", train.days)),
                                    Col::Date => {
                                        ui.label(meta(window.clone()));
                                    }
                                });
                            }
                            if row.index() == 0 {
                                first_row = Some(row.response().rect);
                            }
                            if row.response().clicked() {
                                clicked = Some(train.index);
                            }
                        });
                    });
                table_header_drag(ui, salt, &specs, &cells, Col::key, Col::resizable);
                table_resize_handles(
                    ui,
                    salt,
                    &specs,
                    &mut widths,
                    &edges,
                    first_row,
                    theme::list_header_height(),
                    Col::resizable,
                );
            },
        );
        clicked
    }

    fn station_ui(&mut self, ui: &mut Ui, part: TabPart) {
        let bar = self.bottom_bar();
        if part == TabPart::Results {
            self.station_results_ui(ui, bar);
            return;
        }
        let mut clear = false;
        let mut changed = false;
        let mut input_response = None;
        // 不换行：车站标签 + 输入框 + 模糊 + 日期 必须并排（换行说明宽度算多了）。
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            ui.label(RichText::new("车站").size(theme::FONT_BODY));
            // 输入框宽度按「剩下的宽度 − 模糊 − 日期 − 间距」现算；96 点刚好放 5 个汉字。
            let rest = ui.available_width();
            let reserve = 52.0 + 112.0 + theme::GAP_S * 2.0;
            let field_width = (rest - reserve).clamp(90.0, 240.0);
            // 车站输入框不放占位符：候选站就在下面，写例子只是多一层噪音。
            let response = ui.add(query_input(
                "station",
                &mut self.station_query,
                "",
                field_width,
            ));
            self.take_focus(ui, FocusField::Station, &response);
            if response.changed() {
                changed = true;
            }
            input_response = Some(response);
            // 触摸屏上「清空 / 模糊 / 计数」在底栏（`station_bar`）。
            if !bar
                && !self.station_query.trim().is_empty()
                && ui.add(egui::Button::new(meta("清空"))).clicked()
            {
                clear = true;
            }
            // 「模糊」紧挨输入框右侧：它是「怎么匹配这个输入」的开关，跟输入放一起最直观；
            // 触摸屏也从底栏挪到这儿（底栏只剩清空与计数）。
            let fuzzy = ui
                .checkbox(&mut self.station_fuzzy, meta("模糊"))
                .on_hover_text("模糊搜索：按城市/同城组匹配，包含同城所有车站");
            if fuzzy.changed() {
                changed = true;
            }
            // 日期在最后（不带星期，「今天」从弹窗里点）：一行要同时放下输入框、
            // 模糊和日期，多两个字就会换行。
            date_picker(ui, &mut self.dates.station, "station_date", false);
            if let Some(title) = self.station_board_for.as_ref().filter(|_| !bar) {
                let text = if self.station_options.len() > 1 {
                    if ui.available_width() >= 160.0 {
                        format!(
                            "{} · {} 站 · {} 趟经停",
                            title,
                            self.station_selected.len(),
                            self.station_board.len()
                        )
                    } else {
                        format!(
                            "{} 站 · {} 趟",
                            self.station_selected.len(),
                            self.station_board.len()
                        )
                    }
                } else {
                    format!("{} · {} 趟经停", title, self.station_board.len())
                };
                ui.label(meta(text).weak());
            }
        });
        if clear {
            self.station_query.clear();
            self.station_suggestions.clear();
            self.station_options.clear();
            self.station_selected.clear();
            self.station_board_for = None;
            self.station_board.clear();
            self.focus_request = Some(FocusField::Station);
        } else if changed {
            self.refresh_station();
        }

        if let Some(input) = input_response {
            let picked = candidates_popup(
                ui,
                &input,
                "station_candidates",
                &self.station_suggestions,
                self.station_board_for.as_deref(),
            );
            if let Some(station) = picked {
                self.station_query = station;
                self.refresh_station();
            }
        }

        // 同城组内的车站筛选；勾选变化后按新集合重新查一次停站。
        if self.station_options.len() > 1 {
            let mut toggle = None;
            let mut select_all = false;
            let mut select_none = false;
            ui.add_space(theme::GAP_XS);
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = theme::chip_height();
                ui.spacing_mut().button_padding = theme::chip_padding();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(meta("筛选").weak());
                    ui.add_space(theme::GAP_XS);
                    for option in &self.station_options {
                        let selected = self.station_selected.contains(&option.index);
                        if ui
                            .selectable_label(
                                selected,
                                RichText::new(&option.name).size(theme::FONT_META),
                            )
                            .clicked()
                        {
                            toggle = Some(option.index);
                        }
                    }
                    ui.add_space(theme::GAP_S);
                    if ui.add(egui::Button::new(meta("全选"))).clicked() {
                        select_all = true;
                    }
                    if ui.add(egui::Button::new(meta("全不选"))).clicked() {
                        select_none = true;
                    }
                });
            });
            if let Some(index) = toggle {
                if !self.station_selected.remove(&index) {
                    self.station_selected.insert(index);
                }
                self.reload_station_board();
            } else if select_all {
                self.station_selected = self
                    .station_options
                    .iter()
                    .map(|option| option.index)
                    .collect();
                self.reload_station_board();
            } else if select_none {
                self.station_selected.clear();
                self.reload_station_board();
            }
        }

        if part == TabPart::Stacked {
            self.station_results_ui(ui, bar);
        }
    }

    /// 按车站的结果半页：空状态或按到达时刻排序的停站表。
    fn station_results_ui(&mut self, ui: &mut Ui, bar: bool) {
        ui.add_space(theme::GAP_S);
        if self.station_board_for.is_none() {
            if self.station_query.trim().is_empty() {
                empty_state(
                    ui,
                    "车站",
                    "输入车站名或城市名，查询在该站/同城停靠的列车。",
                );
            } else {
                empty_state(
                    ui,
                    "没有找到这个车站或同城组",
                    "点输入框从候选站里挑一个；模糊模式下输入城市名可查同城所有车站。",
                );
            }
            return;
        }
        if self.station_selected.is_empty() {
            empty_state(ui, "未选择车站", "在车站筛选里勾选至少一个车站。");
            return;
        }
        if self.station_board.is_empty() {
            empty_state(ui, "所选车站暂无经停列车", "换一个车站，或检查数据库。");
            return;
        }

        let date = self.dates.get(self.tab);
        let (running, not_running): (Vec<&StationStop>, Vec<&StationStop>) =
            self.station_board.iter().partition(|stop| {
                self.running(
                    origin_ymd(date, stop.day),
                    stop.date_from,
                    stop.date_to,
                    stop.rule_index,
                )
            });
        let rows: Vec<&StationStop> = running.iter().chain(not_running.iter()).copied().collect();
        let running_count = running.len();
        if !bar {
            show_counts(ui, None, running_count, rows.len() - running_count);
        }
        let selected = self.selected;
        let show_station = self.station_selected.len() > 1;
        let clicked = Self::station_rows_table(
            ui,
            &rows,
            running_count,
            selected,
            "station_board",
            show_station,
            self.station_columns.as_deref(),
        );
        if let Some(index) = clicked {
            self.select_train(index);
        }
    }

    fn station_rows_table(
        ui: &mut Ui,
        rows: &[&StationStop],
        running_count: usize,
        selected: Option<i64>,
        salt: &str,
        show_station: bool,
        custom: Option<&[columns::StationColumn]>,
    ) -> Option<i64> {
        use columns::StationColumn as Col;
        let specs: Vec<columns::Spec<Col>> = match custom {
            Some(picked) => picked.iter().map(|column| column.spec()).collect(),
            None => Col::auto(ui.available_width(), show_station)
                .iter()
                .map(|column| column.spec())
                .collect(),
        };
        let pitch = theme::list_row_height() + ui.spacing().item_spacing.y;
        let height = theme::list_header_height() + rows.len() as f32 * pitch + theme::GAP_S;
        let mut clicked = None;
        scrollable_table(
            ui,
            salt,
            height,
            theme::list_header_height(),
            |ui, content_width| {
                let (computed, _) =
                    column_widths(&specs, content_width, ui.spacing().item_spacing.x);
                let mut widths = table_widths(ui, salt, &specs, computed, Col::key);
                apply_resize_drag(ui, salt, &specs, &mut widths, Col::resizable);
                let active = table_active_column(ui, salt);
                let table = exact_columns(
                    TableBuilder::new(ui)
                        .id_salt(salt)
                        .striped(true)
                        .sense(Sense::click())
                        .drag_to_scroll(egui::scroll_area::DragScroll::Never)
                        .auto_shrink([false, false])
                        .cell_layout(cell_layout()),
                    &widths,
                );
                // 单元格右边界直接取自表头实际画出来的位置：滚动、内边距都由它兜底，
                // 不用自己拿宽度累加（累加对不上只会在滚动时偏几十点）。
                let mut edges: Vec<f32> = Vec::with_capacity(specs.len());
                let mut cells: Vec<egui::Rect> = Vec::with_capacity(specs.len());
                let mut first_row: Option<egui::Rect> = None;
                table
                    .header(theme::list_header_height(), |mut header| {
                        for (index, spec) in specs.iter().enumerate() {
                            header.col(|ui| {
                                edges.push(ui.max_rect().right());
                                cells.push(ui.max_rect());
                                column_head(ui, spec, active == Some(index));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(theme::list_row_height(), rows.len(), |mut row| {
                            let stop = rows[row.index()];
                            let running = row.index() < running_count;
                            row.set_selected(selected == Some(stop.train_index));
                            if running_count > 0 && row.index() == running_count {
                                row.set_overline(true);
                            }
                            let route = format!("{} → {}", stop.first, stop.last);
                            let day_offset = stop.day - 1;
                            for spec in &specs {
                                row.col(|ui| match spec.column {
                                    Col::Status => status_cell(ui, running),
                                    Col::Code => {
                                        train_code_cell(ui, &stop.code, Some(route.as_str()))
                                    }
                                    Col::Station => station_name_cell(ui, &stop.station),
                                    Col::Arrive => time_cell(ui, &stop.arrive, day_offset),
                                    Col::Depart => time_cell(ui, &stop.depart, day_offset),
                                    Col::Dwell => right_label(ui, format!("{} 分", stop.dwell_min)),
                                    Col::Day => right_label(ui, format!("第 {} 天", stop.day)),
                                    Col::Km => right_label(ui, format!("{} km", stop.km)),
                                    Col::Kind => {
                                        ui.add(egui::Label::new(&stop.kind).truncate());
                                    }
                                    Col::Route => route_cell(ui, &stop.first, &stop.last),
                                });
                            }
                            if row.index() == 0 {
                                first_row = Some(row.response().rect);
                            }
                            if row.response().clicked() {
                                clicked = Some(stop.train_index);
                            }
                        });
                    });
                table_header_drag(ui, salt, &specs, &cells, Col::key, Col::resizable);
                table_resize_handles(
                    ui,
                    salt,
                    &specs,
                    &mut widths,
                    &edges,
                    first_row,
                    theme::list_header_height(),
                    Col::resizable,
                );
            },
        );
        clicked
    }

    fn od_ui(&mut self, ui: &mut Ui, part: TabPart) {
        if part == TabPart::Results {
            self.od_results_ui(ui);
            return;
        }
        // 输入行只放「出发框 ⇌ 到达框」（模糊挪到日期那一行，见下）。站名最长 5 个
        // 汉字（5 × 15 点 = 75），所以输入框下限取 96 点。
        let swap_width = theme::control_height();
        let mut changed = false;
        // 候选站下拉要贴在被编辑 / 没解析出来的那个输入框下面，所以留着两个 Response。
        let mut from_response = None;
        let mut to_response = None;
        // 触摸屏上「清空 / 计数」在底栏（`od_bar`），这里只留两个输入框与交换按钮。
        // 这一行不换行：出发 + ⇌ + 到达 必须并排（换行会让到达框单独掉到下一行）。
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            // 输入框宽度按「模糊之后还剩多少」现算：侧栏（横屏左栏）本来就窄，
            // 用估算常量会算多半行，到达框就被挤到下一行去了。
            // TextEdit 的 `desired_width` 不含它自己的左右内边距（合计约 8 点），
            // 所以每个框要多预留一份，否则两个框加起来会超出这一行。
            const INPUT_CHROME: f32 = 10.0;
            let rest = ui.available_width();
            let field_width =
                ((rest - swap_width - theme::GAP_S * 2.0) / 2.0 - INPUT_CHROME).clamp(96.0, 200.0);
            // 两个框不再带标签：占位符已经说明了是出发站还是到达站，省下的宽度给输入。
            let from = ui.add(query_input(
                "od-from",
                &mut self.od_from,
                "出发站",
                field_width,
            ));
            self.take_focus(ui, FocusField::OdFrom, &from);
            if from.changed() {
                self.od_last = OdField::From;
                changed = true;
            }
            from_response = Some(from.clone());
            // 在出发框按 Enter 等于「出发填完了」：焦点直接落到到达框。
            // 单行输入框按 Enter 本来就会失焦，所以判 lost_focus + Enter 两个条件。
            // 不受宽屏限制：此时键盘已经在这个输入框上，跳一格不会额外弹键盘。
            let advance = from.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            // 交换放在两个框中间：箭头方向就是「对调」的意思。
            // 字号按「字形行高 + 按钮内边距 ≤ 控件高度」反推，而不是写死 22 点：
            // 写死时桌面（控件 30 点、按钮内边距 5+5）会被撑到 36 点，整行就对不齐。
            let glyph = swap_glyph_size(ui, swap_width);
            let swap = ui
                .add(
                    egui::Button::new(RichText::new("⇌").size(glyph))
                        .min_size(egui::vec2(swap_width, swap_width))
                        .wrap_mode(egui::TextWrapMode::Extend),
                )
                .on_hover_text("对调出发站与到达站");
            if swap.clicked() {
                std::mem::swap(&mut self.od_from, &mut self.od_to);
                changed = true;
            }

            let to = ui.add(query_input("od-to", &mut self.od_to, "到达站", field_width));
            self.take_focus(ui, FocusField::OdTo, &to);
            if to.changed() {
                self.od_last = OdField::To;
                changed = true;
            }
            if advance {
                to.request_focus();
            }
            to_response = Some(to.clone());
        });
        ui.add_space(theme::GAP_XS);
        // 日期 + 「模糊站名」同一行：放在输入框下面、车站筛选上面（竖屏与侧栏都一样）。
        ui.horizontal(|ui| {
            date_picker(ui, &mut self.dates.od, "od_date", true);
            ui.add_space(theme::GAP_S);
            if ui
                .checkbox(&mut self.od_fuzzy, meta("模糊站名"))
                .on_hover_text("模糊搜索：出发/到达按城市或同城组匹配所有车站")
                .changed()
            {
                changed = true;
            }
        });
        if changed {
            self.refresh_od();
        }

        if let Some(message) = &self.od_message {
            ui.add_space(theme::GAP_XS);
            ui.label(meta(message).weak());
        }

        // 候选站：贴在「没解析出来」的那个输入框下方，和车站页同一套下拉。
        // 原来是在这里铺两排 chip（同城组 + 候选站），解析不出结果时会把结果区往下压。
        if let Some(field) = self.od_missing {
            let (input, salt) = match field {
                OdField::From => (from_response, "od_candidates_from"),
                OdField::To => (to_response, "od_candidates_to"),
            };
            if let Some(input) = input {
                let picked = candidates_popup(ui, &input, salt, &self.od_suggestions, None);
                if let Some(station) = picked {
                    match field {
                        OdField::From => self.od_from = station,
                        OdField::To => self.od_to = station,
                    }
                    self.od_last = field;
                    self.refresh_od();
                }
            }
            return;
        }

        if self.od_results.is_empty() {
            if self.od_message.is_none() {
                empty_state(ui, "站站", "输入出发站和到达站，查询可直达的列车。");
            }
            return;
        }

        // 同城结果里按实际出发站/到达站筛选；两侧互为约束：
        // 选定出发站后，没有对应直达车次的到达站 chip 会被禁用（反向同理）。
        let from_options: BTreeSet<String> = self
            .od_results
            .iter()
            .map(|hit| hit.from_station.clone())
            .collect();
        let to_options: BTreeSet<String> = self
            .od_results
            .iter()
            .map(|hit| hit.to_station.clone())
            .collect();
        // 先清掉与另一侧筛选不兼容的选择，避免筛出空列表。
        let mut to_available = od_to_available(&self.od_results, self.od_filter_from.as_deref());
        if self
            .od_filter_to
            .as_ref()
            .is_some_and(|name| !to_available.contains(name))
        {
            self.od_filter_to = None;
        }
        let mut from_available = od_from_available(&self.od_results, self.od_filter_to.as_deref());
        if self
            .od_filter_from
            .as_ref()
            .is_some_and(|name| !from_available.contains(name))
        {
            self.od_filter_from = None;
            // 出发站被清掉后，到达站重新全部可用。
            from_available = od_from_available(&self.od_results, self.od_filter_to.as_deref());
            to_available = od_to_available(&self.od_results, None);
        }

        if from_options.len() > 1 || to_options.len() > 1 {
            let had_filter = self.od_filter_from.is_some() || self.od_filter_to.is_some();
            ui.add_space(theme::GAP_XS);
            // 这一块是「筛子」，不是主操作：整块压矮一档，别把表格挤下去。
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = theme::chip_height();
                ui.spacing_mut().button_padding = theme::chip_padding();
                ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                ui.spacing_mut().item_spacing.y = 3.0;
                // 出发、到达各占一行：「筛选」两个字省了，方向由 chip 前面的标签说明；
                // 分成两行后每侧都能多放几个站名，不再混在一起换行。
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                    ui.spacing_mut().item_spacing.y = 3.0;
                    Self::filter_chips(
                        ui,
                        "出发",
                        &from_options,
                        &from_available,
                        &mut self.od_filter_from,
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                    ui.spacing_mut().item_spacing.y = 3.0;
                    Self::filter_chips(
                        ui,
                        "到达",
                        &to_options,
                        &to_available,
                        &mut self.od_filter_to,
                    );
                    if had_filter {
                        ui.add_space(theme::GAP_S);
                        if ui.add(egui::Button::new(meta("清除筛选"))).clicked() {
                            self.od_filter_from = None;
                            self.od_filter_to = None;
                        }
                    }
                });
            });
        }

        if part == TabPart::Stacked {
            self.od_results_ui(ui);
        }
    }

    /// 站站的结果半页：筛选后的直达车次表（触摸竖屏走六列可横向滚动的版本）。
    fn od_results_ui(&mut self, ui: &mut Ui) {
        let filtered: Vec<&OdResult> = self
            .od_results
            .iter()
            .filter(|hit| {
                self.od_filter_from
                    .as_ref()
                    .is_none_or(|name| &hit.from_station == name)
                    && self
                        .od_filter_to
                        .as_ref()
                        .is_none_or(|name| &hit.to_station == name)
            })
            .collect();
        if filtered.is_empty() {
            ui.add_space(theme::GAP_XS);
            empty_state(
                ui,
                "当前筛选没有车次",
                "调整出发站/到达站筛选，或清除筛选。",
            );
            return;
        }

        let date = self.dates.get(self.tab);
        let (running, not_running): (Vec<&OdResult>, Vec<&OdResult>) =
            filtered.iter().copied().partition(|hit| {
                self.running(
                    origin_ymd(date, hit.depart_day),
                    hit.date_from,
                    hit.date_to,
                    hit.rule_index,
                )
            });
        let rows: Vec<&OdResult> = running.iter().chain(not_running.iter()).copied().collect();
        let running_count = running.len();
        if !self.bottom_bar() {
            // 触摸屏上这一行在底栏（`od_bar` 的计数），桌面仍留在表格上方。
            let mut count = format!("{} 趟直达", filtered.len());
            if self.od_truncated {
                count = format!("仅显示前 {OD_RESULT_LIMIT} 趟直达");
            } else if filtered.len() != self.od_results.len() {
                count = format!(
                    "{} 趟直达（共 {} 趟）",
                    filtered.len(),
                    self.od_results.len()
                );
            }
            show_counts(ui, Some(count), running_count, rows.len() - running_count);
        }
        let selected = self.selected;
        let clicked = Self::od_rows_table(
            ui,
            &rows,
            running_count,
            selected,
            "od_results",
            self.od_columns.as_deref(),
        );
        if let Some(index) = clicked {
            self.select_train(index);
        }
    }

    /// 一站一 chips 的筛选；`全部` 取消筛选。
    ///
    /// `options` 是结果里出现过的全部车站，`available` 是与另一侧筛选兼容的子集；
    /// 不在 `available` 里的车站会禁用并说明原因。
    fn filter_chips(
        ui: &mut Ui,
        label: &str,
        options: &BTreeSet<String>,
        available: &BTreeSet<String>,
        selected: &mut Option<String>,
    ) {
        ui.label(meta(label).weak());
        if ui
            .selectable_label(
                selected.is_none(),
                RichText::new("全部").size(theme::FONT_META),
            )
            .clicked()
        {
            *selected = None;
        }
        for option in options {
            let active = selected.as_deref() == Some(option.as_str());
            let enabled = available.contains(option);
            let response = ui.add_enabled(
                enabled,
                egui::Button::selectable(active, RichText::new(option).size(theme::FONT_META)),
            );
            if response.clicked() {
                *selected = Some(option.clone());
            }
            if !enabled {
                response.on_disabled_hover_text(format!("另一侧已选车站没有到{option}的直达车次"));
            }
        }
    }

    /// 站站结果表。
    ///
    /// 列由「设置 → 表格列」决定（`custom = None` 时按窗口宽度自动挑）。
    /// 初始列宽 + 外层两向滚动：手机竖屏六列约 428 点放不下，宁可横向滑，
    /// 也别把站名和时间挤成省略号；桌面端还可以拖动列分隔线微调。
    fn od_rows_table(
        ui: &mut Ui,
        rows: &[&OdResult],
        running_count: usize,
        selected: Option<i64>,
        salt: &str,
        custom: Option<&[columns::OdColumn]>,
    ) -> Option<i64> {
        use columns::OdColumn as Col;
        let specs: Vec<columns::Spec<Col>> = match custom {
            Some(picked) => picked.iter().map(|column| column.spec()).collect(),
            None => Col::auto(ui.available_width())
                .iter()
                .map(|column| column.spec())
                .collect(),
        };
        let pitch = theme::list_row_height() + ui.spacing().item_spacing.y;
        let height = theme::list_header_height() + rows.len() as f32 * pitch + theme::GAP_S;
        let mut clicked = None;
        scrollable_table(
            ui,
            salt,
            height,
            theme::list_header_height(),
            |ui, content_width| {
                let (computed, _) =
                    column_widths(&specs, content_width, ui.spacing().item_spacing.x);
                let mut widths = table_widths(ui, salt, &specs, computed, Col::key);
                apply_resize_drag(ui, salt, &specs, &mut widths, Col::resizable);
                let active = table_active_column(ui, salt);
                let table = exact_columns(
                    TableBuilder::new(ui)
                        .id_salt(salt)
                        .striped(true)
                        .sense(Sense::click())
                        .drag_to_scroll(egui::scroll_area::DragScroll::Never)
                        .auto_shrink([false, false])
                        .cell_layout(cell_layout()),
                    &widths,
                );
                // 单元格右边界直接取自表头实际画出来的位置：滚动、内边距都由它兜底，
                // 不用自己拿宽度累加（累加对不上只会在滚动时偏几十点）。
                let mut edges: Vec<f32> = Vec::with_capacity(specs.len());
                let mut cells: Vec<egui::Rect> = Vec::with_capacity(specs.len());
                let mut first_row: Option<egui::Rect> = None;
                table
                    .header(theme::list_header_height(), |mut header| {
                        for (index, spec) in specs.iter().enumerate() {
                            header.col(|ui| {
                                edges.push(ui.max_rect().right());
                                cells.push(ui.max_rect());
                                column_head(ui, spec, active == Some(index));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(theme::list_row_height(), rows.len(), |mut row| {
                            let hit = rows[row.index()];
                            let running = row.index() < running_count;
                            row.set_selected(selected == Some(hit.train_index));
                            if running_count > 0 && row.index() == running_count {
                                row.set_overline(true);
                            }
                            for spec in &specs {
                                row.col(|ui| od_cell(ui, spec.column, hit, running));
                            }
                            if row.index() == 0 {
                                first_row = Some(row.response().rect);
                            }
                            if row.response().clicked() {
                                clicked = Some(hit.train_index);
                            }
                        });
                    });
                table_header_drag(ui, salt, &specs, &cells, Col::key, Col::resizable);
                table_resize_handles(
                    ui,
                    salt,
                    &specs,
                    &mut widths,
                    &edges,
                    first_row,
                    theme::list_header_height(),
                    Col::resizable,
                );
            },
        );
        clicked
    }

    /// `compact` 为真时（窗口窄、详情在底部），改为左右分栏：
    /// 左边是车次信息，右边是停站表；经由不再内嵌，而是在停站表下方
    /// 放一个入口，点开后用独立页面显示。更窄时改为上下排列。
    fn detail_ui(&mut self, ui: &mut Ui, compact: bool, close_label: &str) {
        let Some(detail) = &self.detail else {
            ui.label("未选择车次");
            return;
        };

        let mut close = false;
        // 详情里的跳转先记下来，等 `self.detail` 的借用结束再执行。
        let mut link: Option<DetailLink> = None;
        // 标题栏的「← 上一个车次」：只在换过车次时出现。
        let previous = self.train_back.clone();
        let mut stations_only = self.via_stations_only;
        let mut via_page_open = self.via_page_open;
        // 桌面端的开行计划弹窗：和经由一样，点入口才开。
        let mut plan_page_open = self.plan_page_open;
        let mut plan_page_size = self.plan_page_size;
        // 模态窗按「去掉系统栏」的尺寸排版，否则整页会画到状态栏下面。
        let screen = self.safe_screen(ui.ctx());
        if via_page_open && self.via_page_size != Some(screen) {
            via_page_open = false;
        }
        let extras = &self.extras;
        let date = ymd(self.dates.get(self.tab));
        // 开行计划日历：周期表只读、月份要写回界面状态（两处是不同的字段）。
        let mut plan = RunPlan {
            rules: &self.rules,
            month: &mut self.plan_month,
            open: &mut plan_page_open,
        };
        let open = format!(
            "开行 {}",
            date_window(detail.summary.date_from, detail.summary.date_to)
        );
        let mut header = HeaderState {
            open,
            close_label,
            close: false,
            back: None,
        };

        if compact && ui.available_width() < theme::STACKED_DETAIL_MAX_WIDTH {
            // 手机上整页：标题栏 + 经由入口固定在上面，下面只有一个滚动块
            // （时刻表 → 杂项）。经由不跟着滚——它是一条入口/摘要，翻停站表时
            // 滑走就再也找不回来，还得先滚回顶部。
            let has_via = !extras.via.is_empty();
            header_ui(
                ui,
                detail,
                true,
                previous_code(&previous, detail),
                &mut header,
            );
            ui.add_space(theme::GAP_S);
            if has_via {
                // 入口自带上下两条细线，这里再补 `ui.separator()` 就成了双线。
                via_page_trigger(ui, &extras.via, &mut via_page_open);
            } else {
                ui.separator();
            }
            ui.add_space(theme::GAP_S);
            // 整页只有一个滚动块，而且是两向的：
            // - 纵向滚整页。手指按在停站表上往下拖，滚的也是整页——以前给表格单独
            //   套一层横向滚动区，那一层的拖动会把手指整段吃掉，按在表上怎么拖页面都不动。
            // - 横向滚停站表溢出的列。整页共用同一个滚动量，横向滚动条浮在视口底边。
            // 杂项（含日历）反过来抵消掉横向滚动量、按视口宽度铺：表格右侧溢出不会把
            // 它们撑宽，横滚时也不跟着左右跑（见下面的 `extras_left`）。
            let page_left = ui.available_rect_before_wrap().left();
            egui::ScrollArea::both()
                .id_salt(("detail_page_scroll", self.detail_gen))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // 滚动区内容带一圈 `content_margin`，表格和杂项都从这里起算。
                    let margin = f32::from(ui.spacing().scroll.content_margin.left);
                    let viewport = ui.available_width();
                    let stale = column_layout_stale(ui, "train_stops");
                    let natural = stops_table_width(ui, self.stop_columns.as_deref(), viewport);
                    // 用户把列拖宽后表格内容宽度要跟上去；窗口尺寸变了的那一帧
                    // 回到自然宽度，让列重新自适应。
                    let needed = if stale {
                        natural
                    } else {
                        remembered_table_width(ui, "train_stops").unwrap_or(natural)
                    };
                    let table_width = needed.max(viewport);
                    // 停站表按行数撑开（而不是吃满剩余高度），竖向跟着页面滚。
                    if let Some(station) =
                        stops_table_ui(ui, &detail.stops, self.stop_columns.as_deref(), table_width)
                    {
                        link = Some(DetailLink::Station(station));
                    }
                    ui.add_space(theme::GAP_M);
                    // 杂项画在一个「摆回视口左沿」的子 Ui 里：宽度还是视口宽，
                    // 位置钉在不滚时的内容左沿（`page_left` + 滚动区内边距），
                    // 高度照旧跟在停站表下面。内容整体被横向滚走多少与这里无关。
                    let extras_left = page_left + margin;
                    let top = ui.cursor().top();
                    let extras_rect = egui::Rect::from_min_max(
                        egui::pos2(extras_left, top),
                        egui::pos2(
                            extras_left + viewport,
                            ui.max_rect().bottom().max(top + 1.0),
                        ),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(extras_rect), |ui| {
                        extras_ui(
                            ui,
                            &detail.summary,
                            extras,
                            &detail.stops,
                            date,
                            &mut link,
                            &mut plan,
                            true,
                        );
                    });
                });
        } else if compact {
            let height = ui.available_height();
            let left_width = (ui.available_width() * 0.42).clamp(260.0, 460.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(left_width, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        header_ui(
                            ui,
                            detail,
                            true,
                            previous_code(&previous, detail),
                            &mut header,
                        );
                        ui.add_space(theme::GAP_S);
                        ui.separator();
                        ui.add_space(theme::GAP_S);
                        egui::ScrollArea::vertical()
                            .id_salt("detail_info_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                extras_ui(
                                    ui,
                                    &detail.summary,
                                    extras,
                                    &detail.stops,
                                    date,
                                    &mut link,
                                    &mut plan,
                                    true,
                                );
                            });
                    },
                );
                ui.separator();
                ui.vertical(|ui| {
                    // 右栏：停站表占满，经由入口固定在表格下方。
                    let total = ui.available_height();
                    let has_via = !extras.via.is_empty();
                    let trigger_height = if has_via {
                        theme::control_height() + theme::GAP_S
                    } else {
                        0.0
                    };
                    let stops_height = (total - trigger_height).max(80.0);
                    let picked = scrollable_table(
                        ui,
                        ("train_stops_page", self.detail_gen),
                        stops_height,
                        theme::detail_header_height(),
                        |ui, content_width| {
                            stops_table_ui(
                                ui,
                                &detail.stops,
                                self.stop_columns.as_deref(),
                                content_width,
                            )
                        },
                    );
                    if let Some(station) = picked {
                        link = Some(DetailLink::Station(station));
                    }
                    if has_via {
                        ui.add_space(theme::GAP_S);
                        via_page_trigger(ui, &extras.via, &mut via_page_open);
                    }
                });
            });
        } else {
            // 宽屏侧栏：抬头 → 杂项 → 一条入口（经由 | 开行计划）→ 表格。
            // 经由点开就在这条入口下面就地展开，展开时独占下方全部高度、停站表暂时让位
            // （两者各分一半时都只剩几行，谁也看不清）。
            let has_via = !extras.via.is_empty();
            let mut via_open = has_via
                && ui.data(|data| {
                    data.get_temp::<bool>(via_state_id(detail.summary.index))
                        .unwrap_or(false)
                });
            header_ui(
                ui,
                detail,
                false,
                previous_code(&previous, detail),
                &mut header,
            );
            ui.add_space(theme::GAP_S);
            ui.separator();
            ui.add_space(theme::GAP_S);
            // 宽屏的开行计划入口在下面的入口条里，和经由同一行。
            extras_ui(
                ui,
                &detail.summary,
                extras,
                &detail.stops,
                date,
                &mut link,
                &mut plan,
                false,
            );
            ui.add_space(theme::GAP_S);
            // 入口条：经由占剩下的宽度（它带一串摘要），开行计划按自身文字宽度。
            let summary = via_entry_summary(&extras.via);
            let mut entries = Vec::new();
            if has_via {
                entries.push(Entry {
                    id: "via-inline",
                    title: "经由",
                    summary: &summary,
                    hint: "",
                    arrow: Some(via_open),
                    width: None,
                    hover: if via_open {
                        "收起经由表"
                    } else {
                        "在下面展开经由表"
                    },
                });
            }
            let plan_entry = plan_entry(None);
            let plan_width = entry_natural_width(ui, &plan_entry);
            entries.push(Entry {
                width: Some(plan_width),
                ..plan_entry
            });
            match entry_bar_ui(ui, &entries) {
                Some("via-inline") => {
                    via_open = !via_open;
                    ui.data_mut(|data| {
                        data.insert_temp(via_state_id(detail.summary.index), via_open);
                    });
                }
                Some("run-plan") => plan_page_open = true,
                _ => {}
            }
            ui.add_space(theme::GAP_S);
            if has_via && via_open {
                via_inline_ui(
                    ui,
                    &extras.via,
                    &mut DetailToggles {
                        stations_only: &mut stations_only,
                        via_page_open: &mut via_page_open,
                    },
                    &detail.stops,
                    detail.summary.index,
                );
            } else {
                // 桌面侧栏也统一走两向滚动：列多时横着滑，纵向照旧滚。
                let height = ui.available_height();
                let picked = scrollable_table(
                    ui,
                    ("train_stops_side", self.detail_gen),
                    height,
                    theme::detail_header_height(),
                    |ui, content_width| {
                        stops_table_ui(
                            ui,
                            &detail.stops,
                            self.stop_columns.as_deref(),
                            content_width,
                        )
                    },
                );
                if let Some(station) = picked {
                    link = Some(DetailLink::Station(station));
                }
            }
        }

        if via_page_open && !extras.via.is_empty() {
            // 刚打开时记下尺寸；之后窗口一变化就关页，避免模态位置和大小错位。
            if self.via_page_size.is_none() {
                self.via_page_size = Some(screen);
            }
            via_page_ui(
                ui,
                detail,
                extras,
                &mut stations_only,
                &mut via_page_open,
                compact,
                screen,
            );
        }
        if plan_page_open {
            // 刚打开时记下尺寸；窗口一变就关页，避免弹窗位置与大小错位。
            if plan_page_size.is_none() {
                plan_page_size = Some(screen);
            }
            run_plan_page_ui(
                ui,
                &detail.summary,
                &self.rules,
                &mut self.plan_month,
                &mut plan_page_open,
                screen,
                date,
            );
        }
        self.plan_page_open = plan_page_open;
        self.plan_page_size = if plan_page_open { plan_page_size } else { None };
        if header.close {
            close = true;
        }
        if header.back.is_some() {
            link = header.back;
        }
        self.via_stations_only = stations_only;
        self.via_page_open = via_page_open;
        if !via_page_open {
            self.via_page_size = None;
        }
        if close {
            self.close_detail();
        }
        if let Some(link) = link {
            self.follow_detail_link(link, compact);
        }
    }

    /// 表头拖动换序的结果落回列设置（下一帧生效，并存进配置）。
    ///
    /// 表格只认「当前这一帧的列序」，换序的愿望写在界面状态里；在这里统一读出来，
    /// 免得四个表的绘制函数都要往回传一个「换过的列序」。
    fn apply_column_order(&mut self, ctx: &egui::Context) {
        fn take(ctx: &egui::Context, salt: &str) -> Option<Vec<String>> {
            ctx.data_mut(|data| data.remove_temp(egui::Id::unique((salt, "column_order"))))
        }
        if let Some(keys) = take(ctx, WidthTable::Od.salt()) {
            let picked = columns::parse(&keys, columns::OdColumn::from_key);
            if !picked.is_empty() && self.od_columns.as_ref() != Some(&picked) {
                self.od_columns = Some(picked);
            }
        }
        if let Some(keys) = take(ctx, WidthTable::Train.salt()) {
            let picked = columns::parse(&keys, columns::TrainColumn::from_key);
            if !picked.is_empty() && self.train_columns.as_ref() != Some(&picked) {
                self.train_columns = Some(picked);
            }
        }
        if let Some(keys) = take(ctx, WidthTable::Station.salt()) {
            let picked = columns::parse(&keys, columns::StationColumn::from_key);
            if !picked.is_empty() && self.station_columns.as_ref() != Some(&picked) {
                self.station_columns = Some(picked);
            }
        }
        if let Some(keys) = take(ctx, WidthTable::Stops.salt()) {
            let picked = columns::parse(&keys, columns::StopColumn::from_key);
            if !picked.is_empty() && self.stop_columns.as_ref() != Some(&picked) {
                self.stop_columns = Some(picked);
            }
        }
    }

    /// 执行详情里的跳转。
    ///
    /// - 车站：切到「按车站查询」并只查这一站。宽屏保留右侧详情，方便对照原车次；
    ///   窄屏先收起详情——底部面板要占掉大半高度，留着会把新的车站表压得只剩两三行。
    /// - 车次：就地换详情，不动左侧列表。交路本身就是来回的路径，
    ///   跳过去以后还能点回来；列表保留用户原先的查询结果。
    fn follow_detail_link(&mut self, link: DetailLink, compact: bool) {
        match link {
            DetailLink::Station(name) => {
                // 记下现场：跳过去以后左上角出现「← 上一个车站」。
                // 文案只说车站这一维：原来在车站页就写站名，否则写上一页的名字；
                // 不混车次号——在车站页看到车次号，会让人以为是要跳去那趟车。
                if let Some(detail) = &self.detail {
                    let label = if self.tab == Tab::Station && !self.station_query.trim().is_empty()
                    {
                        self.station_query.trim().to_string()
                    } else {
                        self.tab.label().to_string()
                    };
                    self.return_point = Some(ReturnPoint {
                        tab: self.tab,
                        station_query: self.station_query.clone(),
                        station_fuzzy: self.station_fuzzy,
                        train_index: detail.summary.index,
                        label,
                    });
                }
                if compact {
                    self.close_detail();
                }
                self.tab = Tab::Station;
                self.station_query = name;
                // 点的是具体车站，就该只查这一站，不要展开同城组。
                self.station_fuzzy = false;
                self.refresh_station();
            }
            DetailLink::Train(code) => self.jump_to_train(&code),
            DetailLink::PreviousTrain(index) => self.select_train(index),
        }
    }

    /// 回到跳走之前的那一页：标签页、车站查询词与模糊开关、正在看的车次。
    fn go_back(&mut self) {
        let Some(point) = self.return_point.take() else {
            return;
        };
        self.tab = point.tab;
        if self.station_query != point.station_query || self.station_fuzzy != point.station_fuzzy {
            self.station_query = point.station_query;
            self.station_fuzzy = point.station_fuzzy;
            self.refresh_station();
        }
        self.select_train(point.train_index);
    }

    /// 「← 天津」：跳到别的车站以后的一键回退（文案只写车站这一维）。
    fn return_banner(&mut self, ui: &mut Ui) {
        let Some(point) = &self.return_point else {
            return;
        };
        let label = point.label.clone();
        let mut go_back = false;
        // 不再补一句「刚才在停站表里点了车站」：按钮本身就是 `← 天津`，
        // 悬停提示也写了去向，旁边再挂一句解释只是噪音。
        if ui
            .add(egui::Button::new(meta(format!("← {label}"))))
            .on_hover_text(format!("回到上一处：{label}"))
            .clicked()
        {
            go_back = true;
        }
        ui.add_space(theme::GAP_XS);
        if go_back {
            self.go_back();
        }
    }

    /// 按交路段的车次号打开车次详情。
    ///
    /// 同一个车次号可能有多段开行区间（如 `G1057/G1…` 有两条），优先选当天开行的那一条；
    /// 都不开行时退回第一条，详情里仍能看到完整信息。
    fn jump_to_train(&mut self, code: &str) {
        let Some(db) = &self.db else {
            return;
        };
        let date = ymd(self.dates.get(self.tab));
        match find_train(db, code, date) {
            Ok(Some(index)) => self.select_train(index),
            Ok(None) => self.error = Some(format!("交路里的车次 {code} 在数据里查不到")),
            Err(err) => self.error = Some(format!("查询车次失败：{err}")),
        }
    }
}

impl eframe::App for LltApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // 表头拖动换序：表格那边只把新顺序写进界面状态，这里落回列设置。
        self.apply_column_order(ui.ctx());
        // 横竖屏切换 / 分屏改尺寸时，系统不一定会再推一帧；尺寸一变就自己请求一次，
        // 否则会看到旧宽度排出来的画面，得等下一次触摸才刷新。
        let screen = ui.ctx().content_rect().size();
        if self.last_screen != screen {
            self.last_screen = screen;
            ui.ctx().request_repaint();
        }
        self.pump_ime(ui.ctx());
        self.handle_back(ui.ctx());
        self.persist(ui.ctx());
        self.sync_system_bars(ui.ctx());
        if !self.window_fitted {
            self.window_fitted = true;
            fit_window(ui.ctx());
        }

        // Android 的窗口是 edge-to-edge 的：先给整个屏幕铺一层底色，再把内容
        // 框缩进安全区，免得上栏被状态栏盖住、底部表格被导航条盖住。
        let full = ui.max_rect();
        let insets = self.safe_insets(ui.ctx());
        if insets == MarginF32::ZERO {
            self.body(ui);
            return;
        }
        ui.painter().rect_filled(full, 0.0, ui.visuals().panel_fill);
        // `Rect - MarginF32` 是 epaint 自带实现：左上角加、右下角减。
        let safe = full - insets;
        ui.scope_builder(egui::UiBuilder::new().max_rect(safe), |ui| self.body(ui));
    }

    /// 正常关窗时兜底一次（`persist` 的去抖窗口内可能还没写）。
    ///
    /// 签名随 eframe 的 `glow` feature 变化：本仓库只启用 wgpu 默认 feature，
    /// 所以是无参版本；哪天 feature 统一把 `glow` 打开，这里会编译失败提醒改签名。
    fn on_exit(&mut self) {
        let snapshot = self.settings();
        if snapshot != self.persisted {
            self.store(&snapshot);
        }
    }
}

impl LltApp {
    /// 系统栏图标跟着界面深浅色走。
    ///
    /// 窗口是 edge-to-edge 的，状态栏/导航栏的图标画在界面上面：深色主题下系统默认的
    /// 白色正好，浅色主题下不改就是白字白底，状态栏上的时间、信号、电量全看不见。
    /// 每帧比一下 egui **实际生效**的主题（`跟随系统` 时由系统决定，切深浅色时会跟着变），
    /// 变了才过一次桥，不逐帧走 JNI。
    fn sync_system_bars(&mut self, ctx: &egui::Context) {
        let Some(bars) = &self.platform.bars else {
            return;
        };
        let light = ctx.theme() == egui::Theme::Light;
        if self.bars_light == Some(light) {
            return;
        }
        self.bars_light = Some(light);
        bars.set_light(light);
    }

    /// 系统栏缩进换算成逻辑点（egui 的 `Margin` 单位）。
    ///
    /// 有回调时最多每 [`INSET_PROBE_INTERVAL`] 问一次：`WindowInsets` 会因
    /// 软键盘弹出、导航栏隐藏、旋转屏幕而变，这些都不是应用能直接感知的事件；
    /// 帧本来就在跟着事件走，所以「每帧最多问一次」就够了，而且不逐帧过 JNI。
    fn safe_insets(&mut self, ctx: &egui::Context) -> MarginF32 {
        let due = self
            .inset_probe_at
            .is_none_or(|at| at.elapsed() >= INSET_PROBE_INTERVAL);
        if (self.platform.insets.is_probe() && due)
            || self.safe_area == crate::ScreenInsets::default()
        {
            self.safe_area = self.platform.insets.read();
            self.inset_probe_at = Some(Instant::now());
        }
        self.insets_points(ctx)
    }

    /// 缩进换算成逻辑点。只读，不触发询问。
    fn insets_points(&self, ctx: &egui::Context) -> MarginF32 {
        if self.safe_area == crate::ScreenInsets::default() {
            return MarginF32::ZERO;
        }
        let scale = 1.0 / ctx.pixels_per_point();
        let points = |value: u32| (value as f32 * scale).round();
        MarginF32 {
            left: points(self.safe_area.left),
            right: points(self.safe_area.right),
            top: points(self.safe_area.top),
            bottom: points(self.safe_area.bottom),
        }
    }

    /// 去掉系统栏后的逻辑屏幕尺寸，给模态窗（经由页 / 放大弹窗）排版用。
    ///
    /// egui 的 `content_rect()` 在 Android 上不会扣掉状态栏和导航栏（egui-winit
    /// 只在 iOS 上填 `safe_area_insets`），所以这里自己扣。
    fn safe_screen(&self, ctx: &egui::Context) -> egui::Vec2 {
        let full = ctx.content_rect().size();
        let insets = self.insets_points(ctx);
        (full - insets.sum()).max(egui::vec2(240.0, 240.0))
    }

    /// 除系统栏缩进以外的主体布局。
    fn body(&mut self, ui: &mut Ui) {
        if self.db.is_none() && !self.db_suspended {
            // 没有库时直接落到数据页：第一次启动（Android 不再内置数据库）与
            // 「库被删了」都该在同一个地方解决——下载 / 导入。
            egui::Panel::top("header")
                .frame(theme::panel_frame(ui))
                .show(ui, |ui| self.header(ui));
            egui::CentralPanel::default()
                .frame(theme::panel_frame(ui))
                .show(ui, |ui| {
                    // 库文件压根不存在时别把 SQLite 的原话怼到脸上：那是「还没下载」，
                    // 不是「坏了」。真正的打开失败（文件在但结构不对）才显示原文。
                    let missing = !self.db_path.is_file();
                    if missing {
                        ui.label(
                            RichText::new("还没有时刻表数据")
                                .size(theme::FONT_SECTION)
                                .family(fonts::bold_family()),
                        );
                        ui.label(meta("下载一份数据，或导入之前的备份。"));
                        ui.add_space(theme::GAP_S);
                    }
                    if let Some(error) = self.db_error.as_ref().filter(|_| !missing) {
                        ui.label(
                            RichText::new(error)
                                .size(theme::FONT_META)
                                .color(ui.visuals().error_fg_color),
                        );
                        ui.add_space(theme::GAP_S);
                    }
                    self.data_ui(ui);
                });
            return;
        }

        // 触摸窄屏（手机）不建底部面板：列表和面板各分一半后谁都看不清，
        // 改成「整车次一页」——详情直接 replace 掉列表，而不是叠一个会自己长高的模态。
        // 触摸宽屏（平板横屏）右栏够用，仍保留列表与详情并排。
        let touch_page = self.detail.is_some()
            && self.platform.pointer == theme::Pointer::Touch
            && ui.available_width() < WIDE_MIN_WIDTH;

        // 横屏（矮而宽）不画头部：标签页和版本都在左栏里，纵向空间让给内容。
        let landscape = self.bottom_bar()
            && ui.available_height() < theme::LANDSCAPE_MAX_HEIGHT
            && ui.available_width() >= theme::LANDSCAPE_MIN_WIDTH
            && self.detail.is_none();
        // 手机上看整页详情时也不画头部：那一页自己有车次抬头和「返回」，
        // 上面再顶一条「路路通时刻表 + 数据 + 浅色」就是白占一行（竖屏横屏都一样）。
        if !landscape && !touch_page {
            egui::Panel::top("header")
                .frame(theme::panel_frame(ui))
                .show(ui, |ui| self.header(ui));
        }
        if self.detail.is_some() && !touch_page {
            let available = ui.available_width();
            if available >= WIDE_MIN_WIDTH {
                // 宽屏：右侧详情栏占窗口宽度的约 36%，仍保证至少 320 点。
                let detail_width = (available * 0.36).clamp(380.0, 560.0);
                egui::Panel::right("detail_right")
                    .resizable(true)
                    .default_size(detail_width)
                    .min_size(320.0)
                    .max_size(640.0)
                    .frame(theme::detail_frame(ui))
                    .show(ui, |ui| self.detail_ui(ui, false, "关闭"));
            } else {
                // 窄屏：右侧栏会把结果表挤得不可用，改为底部详情面板。
                let detail_height = (ui.available_height() * 0.62).clamp(180.0, 560.0);
                egui::Panel::bottom("detail_bottom")
                    .resizable(true)
                    .default_size(detail_height)
                    .min_size(90.0)
                    .max_size(560.0)
                    .frame(theme::detail_frame(ui))
                    .show(ui, |ui| self.detail_ui(ui, true, "关闭"));
            }
        }

        // 横屏：改成左右两栏——左栏标签 + 控件 + 动作，右栏只放结果表。
        if landscape && !touch_page {
            // 0.44 是量出来的：站站那行要「模糊 + 两个 96 点的框 + ⇌ + 间距」≈ 312 点，
            // 侧栏内容宽（扣掉左右各 12 点留白）得比它再宽一点才不会换行。
            let side_width = (ui.available_width() * 0.44).clamp(312.0, 400.0);
            egui::Panel::left("query_side")
                .resizable(false)
                .exact_size(side_width)
                .frame(theme::panel_frame(ui))
                .show(ui, |ui| {
                    // 底部：版本 + 主题（头部在横屏隐藏了，这里补上）+ 动作行。
                    egui::Panel::bottom("side_actions")
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            ui.add_space(theme::GAP_XS);
                            ui.separator();
                            ui.horizontal(|ui| {
                                if let Some(version) = self.data_version {
                                    ui.label(
                                        theme::mono(format!("数据 {}", date_ymd(version))).weak(),
                                    )
                                    .on_hover_text("数据包版本（库内 meta.data_version）");
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    self.theme_button(ui);
                                });
                            });
                            if self.action_bar_needed() {
                                self.action_bar(ui);
                            }
                        });
                    egui::ScrollArea::vertical()
                        .id_salt("side_controls")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.tab_bar_segments(ui);
                            ui.add_space(theme::GAP_XS);
                            ui.separator();
                            ui.add_space(theme::GAP_XS);
                            self.error_banner(ui);
                            self.tab_ui(ui, TabPart::Controls);
                        });
                });
            egui::CentralPanel::default()
                .frame(theme::panel_frame(ui))
                .show(ui, |ui| self.tab_ui(ui, TabPart::Results));
            return;
        }

        // 触摸屏的底栏：标签页在最底下，动作行在它上面。
        // 之所以放底部：标签页与「查询/清空/模糊」这些都不是输入框，却是表格上方
        // 最占高度的东西；挪到拇指能够到的地方，表格就能多几行。
        if self.bottom_bar() && !touch_page {
            egui::Panel::bottom("tab_bar_bottom")
                .frame(theme::bar_frame(ui))
                .show(ui, |ui| self.tab_bar_segments(ui));
            if self.action_bar_needed() {
                egui::Panel::bottom("action_bar")
                    .frame(theme::bar_frame(ui))
                    .show(ui, |ui| self.action_bar(ui));
            }
        }

        egui::CentralPanel::default()
            .frame(theme::panel_frame(ui))
            .show(ui, |ui| {
                if touch_page {
                    // 整页详情：一页就是这一趟车，顶栏留着标签页与日期。
                    self.detail_page_ui(ui);
                    return;
                }
                self.error_banner(ui);
                self.tab_ui(ui, TabPart::Stacked);
            });
    }

    /// 按当前标签页画它的某一半（竖屏整页 / 横屏左栏控件 / 横屏右栏结果）。
    fn tab_ui(&mut self, ui: &mut Ui, part: TabPart) {
        match self.tab {
            Tab::Train => self.train_ui(ui, part),
            Tab::Station => {
                if part != TabPart::Results {
                    self.return_banner(ui);
                }
                self.station_ui(ui, part);
            }
            Tab::Od => self.od_ui(ui, part),
            Tab::Data => {
                if part != TabPart::Controls {
                    self.settings_ui(ui);
                }
            }
        }
    }

    /// 是否是触摸屏（底栏只在触摸屏上出现；桌面保持原来的三行头部）。
    fn bottom_bar(&self) -> bool {
        self.platform.pointer == theme::Pointer::Touch
    }

    /// 当前页有没有动作可放：没输入时底栏只剩一条空栏，不如收起来。
    ///
    /// 三个查询页的底栏都是「清空 + 开行统计」，输入为空时两样都没有。
    fn action_bar_needed(&self) -> bool {
        match self.tab {
            Tab::Data => false,
            Tab::Train => !self.train_query.trim().is_empty(),
            Tab::Od => !self.od_from.trim().is_empty() || !self.od_to.trim().is_empty(),
            Tab::Station => !self.station_query.trim().is_empty(),
        }
    }

    /// 底栏的动作行：把「不是输入框、又占表格上方空间」的控件收在这里。
    ///
    /// 三个查询页统一成同一行：左边「清空」，右跟这一页的开行统计
    /// （`● 当日开行 ○ 当日不开行`，原来那一行「当日开行 N 趟」）。
    /// 数据页不需要动作行。
    fn action_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            match self.tab {
                Tab::Od => self.od_bar(ui),
                Tab::Train => self.train_bar(ui),
                Tab::Station => self.station_bar(ui),
                Tab::Data => {}
            }
        });
    }

    /// 站站的动作行：清空 + 计数（和车次 / 车站两页同一个格式：左清空、右计数）。
    fn od_bar(&mut self, ui: &mut Ui) {
        let height = theme::bar_height() - 8.0;
        let has_query = !self.od_from.trim().is_empty() || !self.od_to.trim().is_empty();
        if has_query
            && ui
                .add(egui::Button::new(meta("清空")).min_size(egui::vec2(0.0, height)))
                .clicked()
        {
            self.clear_od();
        }
        let (running, not_running) = self.running_split();
        let total = self.od_shown_count();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            counts_label(ui, total, running, not_running);
        });
    }

    /// 站站页的清空：两个站名都清掉、结果跟着空，焦点还给出发框。
    fn clear_od(&mut self) {
        self.od_from.clear();
        self.od_to.clear();
        self.od_last = OdField::From;
        self.focus_request = Some(FocusField::OdFrom);
        self.refresh_od();
    }

    /// 按车次的动作行：清空 + 计数。
    fn train_bar(&mut self, ui: &mut Ui) {
        let height = theme::bar_height() - 8.0;
        let has_query = !self.train_query.trim().is_empty();
        if has_query
            && ui
                .add(egui::Button::new(meta("清空")).min_size(egui::vec2(0.0, height)))
                .clicked()
        {
            self.train_query.clear();
            self.train_results.clear();
            // 清空是为了立刻重输，焦点留在（还给）输入框。
            self.focus_request = Some(FocusField::Train);
        }
        let total = self.train_results.len();
        let (running, not_running) = self.running_split();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            counts_label(ui, total, running, not_running);
        });
    }

    /// 按车站的动作行：清空 / 模糊 + 计数。
    fn station_bar(&mut self, ui: &mut Ui) {
        let height = theme::bar_height() - 8.0;
        if !self.station_query.trim().is_empty()
            && ui
                .add(egui::Button::new(meta("清空")).min_size(egui::vec2(0.0, height)))
                .clicked()
        {
            self.station_query.clear();
            self.station_suggestions.clear();
            self.station_options.clear();
            self.station_selected.clear();
            self.station_board_for = None;
            self.station_board.clear();
            self.focus_request = Some(FocusField::Station);
        }
        let (running, not_running) = self.running_split();
        let total = self.station_board.len();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            counts_label(ui, total, running, not_running);
        });
    }

    /// 当前列表里「当日开行 / 不开行」的条数（只统计已经在手上的结果）。
    fn running_split(&self) -> (usize, usize) {
        let selected = ymd(self.dates.get(self.tab));
        match self.tab {
            Tab::Train => self.train_results.iter().fold((0, 0), |(on, off), train| {
                if self.running(selected, train.date_from, train.date_to, train.rule_index) {
                    (on + 1, off)
                } else {
                    (on, off + 1)
                }
            }),
            Tab::Station => self.station_board.iter().fold((0, 0), |(on, off), stop| {
                let day = origin_ymd(self.dates.get(self.tab), stop.day);
                if self.running(day, stop.date_from, stop.date_to, stop.rule_index) {
                    (on + 1, off)
                } else {
                    (on, off + 1)
                }
            }),
            Tab::Od => self
                .od_results
                .iter()
                .filter(|hit| self.od_filtered(hit))
                .fold((0, 0), |(on, off), hit| {
                    let day = origin_ymd(self.dates.get(self.tab), hit.depart_day);
                    if self.running(day, hit.date_from, hit.date_to, hit.rule_index) {
                        (on + 1, off)
                    } else {
                        (on, off + 1)
                    }
                }),
            _ => (0, 0),
        }
    }

    /// 站站结果是否通过了「出发站 / 到达站」筛选（两侧互为约束）。
    fn od_filtered(&self, hit: &OdResult) -> bool {
        self.od_filter_from
            .as_ref()
            .is_none_or(|name| &hit.from_station == name)
            && self
                .od_filter_to
                .as_ref()
                .is_none_or(|name| &hit.to_station == name)
    }

    /// 筛选后还剩多少趟（底栏计数用，不区分开行/不开行）。
    fn od_shown_count(&self) -> usize {
        self.od_results
            .iter()
            .filter(|hit| self.od_filtered(hit))
            .count()
    }

    /// 系统「返回」（Android 的左/右边缘滑动、返回键都是它）。
    ///
    /// 顺序与常见手机应用一致：先关最上面那层自己开的页面（经由 → 车次详情），
    /// 都没有时才把控制权交回系统退出应用。软键盘由系统自己先收，不会走到这里。
    fn handle_back(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.key_pressed(egui::Key::BrowserBack)) {
            return;
        }
        if self.columns_open.is_some() {
            // 触摸屏的表格列弹窗：返回键先收弹窗，再轮到详情/退出。
            self.columns_open = None;
        } else if self.via_page_open {
            self.via_page_open = false;
        } else if self.detail.is_some() {
            self.close_detail();
        } else if let Some(control) = &self.platform.control {
            control.exit();
        }
    }

    /// 「设置」页：三个查询页的开关 + 数据（子章节）。
    fn settings_ui(&mut self, ui: &mut Ui) {
        let mut outcome = crate::data::DataOutcome::default();
        egui::ScrollArea::vertical()
            .id_salt("settings_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 设置页条目多，纵向间距比别处再紧一档（8 → 4）。
                ui.spacing_mut().item_spacing.y = theme::GAP_XS;
                self.query_settings_ui(ui);
                ui.add_space(theme::GAP_XS);
                ui.separator();
                ui.add_space(theme::GAP_XS);
                self.columns_settings_ui(ui);
                ui.add_space(theme::GAP_XS);
                ui.separator();
                ui.add_space(theme::GAP_XS);
                // 数据是设置的一个子章节：复用它的内容，但滚动容器由本页统一提供。
                self.data.contents(ui, &self.db_path, &mut outcome);
            });
        self.apply_data_outcome(outcome);
        // 触摸屏的列清单是整页弹窗：放在滚动区外画，避免被设置页的滚动裁剪。
        if theme::pointer() == theme::Pointer::Touch {
            let screen = self.safe_screen(ui.ctx());
            self.columns_modal(ui.ctx(), screen);
        }
    }

    /// 「表格列」小节：四张表各一个按钮，点开在下方展开竖排列清单。
    ///
    /// 清单里勾选控制显示、按住行拖动调整顺序；勾「自动」回到按窗口宽度挑列。
    /// 改动逐帧读，下一帧就生效；落盘由 `persist` 的快照比较负责。
    fn columns_settings_ui(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("表格列")
                .size(theme::FONT_SECTION)
                .family(fonts::bold_family()),
        );
        ui.label(meta("点击按钮以配置相应表格的列。").weak());
        ui.label(
            meta("在表格中按住表头的列分隔线并拖动可调整列宽；水平拖动表头可交换列的顺序。").weak(),
        );
        ui.add_space(theme::GAP_XS);

        let width = ui.available_width();
        // 自动列按当前宽度算：既显示在按钮上的列数里，也是展开后「自动」状态的勾选。
        let od_auto = columns::OdColumn::auto(width);
        let train_auto = columns::TrainColumn::auto(width);
        let station_auto = columns::StationColumn::auto(width, self.station_selected.len() > 1);
        let stop_auto = columns::StopColumn::auto(width);
        let entries = [
            (
                "站站",
                ColumnTable::Od,
                self.od_columns.as_ref().map_or(od_auto.len(), Vec::len),
            ),
            (
                "车次",
                ColumnTable::Train,
                self.train_columns
                    .as_ref()
                    .map_or(train_auto.len(), Vec::len),
            ),
            (
                "车站",
                ColumnTable::Station,
                self.station_columns
                    .as_ref()
                    .map_or(station_auto.len(), Vec::len),
            ),
            (
                "停站表",
                ColumnTable::Stops,
                self.stop_columns.as_ref().map_or(stop_auto.len(), Vec::len),
            ),
        ];
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(theme::GAP_XS, theme::GAP_XS);
            for (label, table, count) in entries {
                let open = self.columns_open == Some(table);
                let button = egui::Button::new(setting_label(format!("{label} {count}")))
                    .selected(open)
                    .frame_when_inactive(true);
                if ui.add(button).clicked() {
                    self.columns_open = if open { None } else { Some(table) };
                }
            }
        });

        // 触摸屏（Android）改成整页弹窗，见 `columns_modal`：设置页本身在滚动，
        // 清单展开在按钮下面时，按钮容易滚出屏幕，手指也容易点错。
        if theme::pointer() == theme::Pointer::Touch {
            return;
        }

        let Some(open) = self.columns_open else {
            return;
        };
        ui.add_space(theme::GAP_XS);
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| match open {
                ColumnTable::Od => column_list(
                    ui,
                    "columns_od",
                    &mut self.od_columns,
                    od_auto,
                    columns::OdColumn::ALL,
                    columns::OdColumn::spec,
                    false,
                ),
                ColumnTable::Train => column_list(
                    ui,
                    "columns_train",
                    &mut self.train_columns,
                    train_auto,
                    columns::TrainColumn::ALL,
                    columns::TrainColumn::spec,
                    false,
                ),
                ColumnTable::Station => column_list(
                    ui,
                    "columns_station",
                    &mut self.station_columns,
                    station_auto,
                    columns::StationColumn::ALL,
                    columns::StationColumn::spec,
                    false,
                ),
                ColumnTable::Stops => column_list(
                    ui,
                    "columns_stops",
                    &mut self.stop_columns,
                    stop_auto,
                    columns::StopColumn::ALL,
                    columns::StopColumn::spec,
                    false,
                ),
            });
    }

    /// 触摸屏（Android）的表格列清单：整页弹窗。
    ///
    /// 设置页本身是一个长滚动区，清单展开在按钮下面时按钮很容易被滚出屏幕；
    /// 弹窗盖住整页，勾选和拖动都只在一个固定区域里，手指不用来回找。
    fn columns_modal(&mut self, ctx: &egui::Context, screen: egui::Vec2) {
        let Some(open) = self.columns_open else {
            return;
        };
        let title = match open {
            ColumnTable::Od => "站站",
            ColumnTable::Train => "车次",
            ColumnTable::Station => "车站",
            ColumnTable::Stops => "停站表",
        };
        // 手机上一屏放得下整张清单：弹窗高度按行数算，下面是空的就不要再撑满整屏。
        let row_count = match open {
            ColumnTable::Od => columns::OdColumn::ALL.len(),
            ColumnTable::Train => columns::TrainColumn::ALL.len(),
            ColumnTable::Station => columns::StationColumn::ALL.len(),
            ColumnTable::Stops => columns::StopColumn::ALL.len(),
        } + 1; // 「自动」行
        let list_height_needed = row_count as f32 * (theme::popup_column_row_height() + 2.0);
        let page_w = (screen.x - 24.0).max(240.0);
        let page_h = (theme::control_height() + theme::GAP_XS + list_height_needed + 24.0)
            .min(screen.y - 24.0)
            .max(240.0);
        let page = egui::vec2(page_w, page_h);
        let size_key = (page.x.round() as i32, page.y.round() as i32);
        let inner = page - egui::vec2(24.0, 24.0);
        let auto_width = inner.x;
        let visual = ctx.style_of(ctx.theme()).visuals.clone();
        let frame = egui::Frame::new()
            .fill(visual.panel_fill)
            .stroke(visual.widgets.noninteractive.bg_stroke)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(12);
        let mut close = false;
        let _ = egui::Modal::new(egui::Id::unique(("llt-columns-page", size_key)))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(140))
            .show(ctx, |ui| {
                ui.set_min_size(inner);
                ui.horizontal(|ui| {
                    ui.label(bold(format!("表格列 · {title}")).size(theme::FONT_SECTION));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(setting_label("完成"))).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(theme::GAP_XS);
                let list_height =
                    (inner.y - theme::control_height() - theme::GAP_XS - 8.0).max(120.0);
                egui::ScrollArea::vertical()
                    .id_salt("columns_modal_scroll")
                    .max_height(list_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| match open {
                        ColumnTable::Od => column_list(
                            ui,
                            "columns_od",
                            &mut self.od_columns,
                            columns::OdColumn::auto(auto_width),
                            columns::OdColumn::ALL,
                            columns::OdColumn::spec,
                            true,
                        ),
                        ColumnTable::Train => column_list(
                            ui,
                            "columns_train",
                            &mut self.train_columns,
                            columns::TrainColumn::auto(auto_width),
                            columns::TrainColumn::ALL,
                            columns::TrainColumn::spec,
                            true,
                        ),
                        ColumnTable::Station => column_list(
                            ui,
                            "columns_station",
                            &mut self.station_columns,
                            columns::StationColumn::auto(
                                auto_width,
                                self.station_selected.len() > 1,
                            ),
                            columns::StationColumn::ALL,
                            columns::StationColumn::spec,
                            true,
                        ),
                        ColumnTable::Stops => column_list(
                            ui,
                            "columns_stops",
                            &mut self.stop_columns,
                            columns::StopColumn::auto(auto_width),
                            columns::StopColumn::ALL,
                            columns::StopColumn::spec,
                            true,
                        ),
                    });
            });
        // 打开它的那一下点击可能被背景层当成「点了外面」，所以只认「完成」按钮
        // 和系统返回键（见 `handle_back`）。
        if close {
            self.columns_open = None;
        }
    }

    /// 「查询」小节：深浅色 + 两个模糊开关。
    ///
    /// 两行的标签列同宽、控件同高，右边才对得齐；选项都做成按钮/勾选框，
    /// 而不是只有选中的那一个才有底色（未选中的看起来像正文，不像可点的东西）。
    fn query_settings_ui(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("查询")
                .size(theme::FONT_SECTION)
                .family(fonts::bold_family()),
        );
        ui.add_space(theme::GAP_XS);
        let label_width = theme::SETTING_LABEL_WIDTH;
        ui.horizontal(|ui| {
            setting_label_cell(ui, "深浅色", label_width);
            for (choice, label) in [
                (config::ThemeChoice::System, "跟随系统"),
                (config::ThemeChoice::Light, "浅色"),
                (config::ThemeChoice::Dark, "深色"),
            ] {
                // `Button::selectable` 只在选中时画边框，未选中的看起来像正文；
                // `frame_when_inactive(true)` 让三个选项都长成按钮。
                if ui
                    .add(
                        egui::Button::new(setting_label(label))
                            .selected(self.theme == choice)
                            .frame_when_inactive(true),
                    )
                    .clicked()
                {
                    self.set_theme(ui.ctx(), choice);
                }
            }
        });
        ui.add_space(theme::GAP_XS);
        ui.horizontal(|ui| {
            setting_label_cell(ui, "模糊查询", label_width);
            ui.checkbox(&mut self.od_fuzzy, setting_label("站站"))
                .on_hover_text("出发 / 到达按城市或同城组匹配，包含同城所有车站");
            ui.add_space(theme::GAP_S);
            ui.checkbox(&mut self.station_fuzzy, setting_label("车站"))
                .on_hover_text("按站名所在城市/同城组匹配，包含同城所有车站");
        });
    }

    /// 「数据」页：当前库、检查更新、下载重建、备份导入导出。
    fn data_ui(&mut self, ui: &mut Ui) {
        let outcome = self.data.ui(ui, &self.db_path);
        self.apply_data_outcome(outcome);
    }

    /// 数据页的副作用：替换库文件前放开连接，完成后重开并重跑查询。
    fn apply_data_outcome(&mut self, outcome: crate::data::DataOutcome) {
        if outcome.close_db && self.db.is_some() {
            self.db = None;
            self.db_suspended = true;
        }
        if outcome.reload_db {
            self.db_suspended = false;
            self.open_db();
            self.data.refresh(&self.db_path);
        }
    }

    /// Android（触摸）上的车次详情：整页，而不是把列表压成半屏的底部面板。
    ///
    /// 手机的逻辑宽度只有约 440 点，底部面板和列表各分一半后停站表只剩几行。
    /// 这里直接把列表换成详情（不是叠一个模态）：中央面板本来就给了确定的宽高，
    /// 「按行取整」的停站表才有稳定高度。之前用模态时，模态是按内容自动长高的，
    /// 停站表又拿 `available_height()` 算行数，两者互相喂——手指按住不放（每帧都在
    /// 重绘）时整页会一路往上跑。
    fn detail_page_ui(&mut self, ui: &mut Ui) {
        self.detail_ui(ui, true, "返回");
    }
}

// ---------------------------------------------------------------- 辅助函数

/// 某个查询页的日期选择。`salt` 区分三个页面各自的日历弹窗状态。
///
/// `with_weekday` 为假时只画按钮本身（星期与「今天」从弹窗里也能看到），
/// 用在横屏侧栏这种窄地方。
fn date_picker(ui: &mut Ui, date: &mut Date, salt: &'static str, with_weekday: bool) {
    ui.add(
        DatePickerButton::new(date)
            .id_salt(salt)
            .show_icon(false)
            .highlight_weekends(false)
            .calendar_week(false)
            .start_end_years(2000..=2100),
    );
    if !with_weekday {
        return;
    }
    ui.label(meta(weekday_cn(*date)).weak());
    let current_date = today();
    if *date != current_date {
        ui.add_space(theme::GAP_XS);
        if ui.add(egui::Button::new(meta("今天"))).clicked() {
            *date = current_date;
        }
    }
}

/// 交换按钮里「⇌」的字号：按「字形行高 + 按钮内边距 ≤ 控件高度」反推。
///
/// 写死字号会出事：桌面控件高 30、按钮内边距 5+5，22 点字的行高约 27，
/// 加起来 37 点，按钮比输入框高一截，整行就对不齐了。
fn swap_glyph_size(ui: &Ui, button_size: f32) -> f32 {
    let padding = 2.0 * ui.spacing().button_padding.y;
    let row_height = |glyph: f32| {
        ui.ctx()
            .fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(glyph)))
    };
    let mut glyph = (button_size * 0.7).round();
    while glyph > 8.0 && row_height(glyph) + padding > button_size {
        glyph -= 1.0;
    }
    glyph
}

/// 查询输入框。触摸屏上文字居中（水平和垂直），鼠标端保持默认的左对齐。
/// 查询输入框。
///
/// `salt` 必须给一个固定值：这几个输入框所在的布局会随结果有无、底栏位置变化
/// （`TabPart` 的 Input / Results / Stacked 三态），自动 id 跟着位置走，一刷新
/// 就变成“另一个输入框”——egui 的焦点随即丢掉，表现是**只能打进第一个字符**
/// （之后的输入法上屏事件因为没有焦点全被丢弃）。
fn query_input<'a>(
    salt: &'static str,
    text: &'a mut String,
    hint: &str,
    width: f32,
) -> egui::TextEdit<'a> {
    let edit = egui::TextEdit::singleline(text)
        .id(egui::Id::unique(salt))
        .hint_text(hint)
        .desired_width(width);
    if theme::pointer() == theme::Pointer::Touch {
        // 44 点的框里文字贴左上角很难看，居中更像系统输入框。
        edit.align(egui::Align2::CENTER_CENTER)
    } else {
        edit
    }
}

/// 首帧根据屏幕大小收敛一次窗口尺寸，避免高 DPI/远程桌面下窗口超出屏幕。
fn fit_window(ctx: &egui::Context) {
    let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) else {
        return;
    };
    if monitor.x < 2.0 || monitor.y < 2.0 {
        return;
    }

    // monitor_size 已经是逻辑点（egui 点），不要再除以 pixels_per_point。
    // 留出标题栏和任务栏的余量，避免窗口底部超出屏幕。
    let desired = egui::vec2(crate::DEFAULT_WINDOW_SIZE[0], crate::DEFAULT_WINDOW_SIZE[1]);
    let target = egui::vec2(
        desired.x.min((monitor.x * 0.94).max(480.0)),
        desired.y.min((monitor.y * 0.84).max(300.0)),
    );
    let current = ctx.input(|i| {
        i.viewport()
            .inner_rect
            .map(|rect| rect.size())
            .unwrap_or(desired)
    });

    if (target.x - current.x).abs() > 2.0 || (target.y - current.y).abs() > 2.0 {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(target));
        // 自动缩小后把窗口挪到屏幕左上角附近，避免沿用原来的大窗口坐标。
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(16.0, 16.0)));
    }
}

/// 出发站已定时，还能作为到达站出现的车站。
fn od_to_available(hits: &[OdResult], from: Option<&str>) -> BTreeSet<String> {
    hits.iter()
        .filter(|hit| from.is_none_or(|name| hit.from_station == name))
        .map(|hit| hit.to_station.clone())
        .collect()
}

/// 到达站已定时，还能作为出发站出现的车站。
fn od_from_available(hits: &[OdResult], to: Option<&str>) -> BTreeSet<String> {
    hits.iter()
        .filter(|hit| to.is_none_or(|name| hit.to_station == name))
        .map(|hit| hit.from_station.clone())
        .collect()
}

/// 半角/全角归一化，并去掉所有空白。
fn normalize_code(input: &str) -> String {
    normalize_width(input)
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// 按车次号找一趟车，返回 `trains` 表主键。
///
/// 同号多区间（同一车次号有多个开行区间）时优先取「当天在开」的那条，否则取第一条——
/// 交路跳转与交路明细里的站名时刻用的是同一口径。
fn find_train(db: &Db, code: &str, date: i64) -> rusqlite::Result<Option<i64>> {
    let query = normalize_code(code);
    let hits = db.search_trains(&query, None)?;
    Ok(hits
        .iter()
        .find(|train| train.runs_on(date))
        .or(hits.first())
        .map(|train| train.index))
}

/// 交路明细要用的「每段车次：出发站/时刻 → 到达站/时刻」。
///
/// 选中车次时一次查完（每段两个轻查），查不到的那段不放进 map——展开后
/// 那一行就只写车次号（回送 / 检测车、旧库缺数据都是这种情况）。
fn duty_runs(db: &Db, duties: &[Duty], date: i64) -> HashMap<(i64, i64), DutyRun> {
    let mut runs = HashMap::new();
    for duty in duties {
        for segment in &duty.segments {
            let Ok(Some(index)) = find_train(db, &segment.code, date) else {
                continue;
            };
            let Ok(detail) = db.train_detail(index) else {
                continue;
            };
            let (Some(first), Some(last)) = (detail.stops.first(), detail.stops.last()) else {
                continue;
            };
            runs.insert(
                (duty.id, segment.seq),
                DutyRun {
                    from: first.station.clone(),
                    from_time: first.depart.clone(),
                    to: last.station.clone(),
                    to_time: last.arrive.clone(),
                },
            );
        }
    }
    runs
}

/// 站名只做全角转半角并去掉首尾空白，中文本身不改动。
fn normalize_station(input: &str) -> String {
    normalize_width(input).trim().to_owned()
}

fn normalize_width(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect()
}

/// 时刻单元格：时间用等宽字体，「+N天」用更小一档的弱化文字紧跟其后。
///
/// 跨日时刻如果把「23:58 +2天」凑成一段等宽文字，宽度会超过列宽（74 点）；
/// egui_extras 的单元格背景是按“列宽与内容宽度取并集”铺的，一旦内容超宽，
/// 该单元格的占位就被撑到列外、后面几列整体右移——同一张表里跨日的行于是
/// 看起来“排版不一样”，选中行的底色还会被撑开的缝隙切成几段。
///
/// `offset_days` 是相对该行基准日的天数，为 0 时不画后缀。基准日各表不同：
/// 车站表是查询日期（即“本站第几天 - 1”），站站查询的到达是**上车日期**——
/// 出发日期本来就等于查询日期，所以出发列不需要后缀，到达列才用“当天 / 次日到”。
fn time_cell(ui: &mut Ui, time: &str, offset_days: i64) {
    ui.spacing_mut().item_spacing.x = 2.0;
    ui.label(RichText::new(time).monospace());
    if offset_days > 0 {
        // 站站/车站结果表的列很窄，「+1」就够了（「天」在详情/表头里有说明）。
        ui.label(meta(format!("+{offset_days}")).weak());
    }
}

fn fmt_duration(minutes: i64) -> String {
    let minutes = minutes.max(0);
    let days = minutes / 1440;
    let hours = (minutes % 1440) / 60;
    let mins = minutes % 60;

    if days > 0 {
        format!("{days}天{hours}小时{mins:02}分")
    } else if hours > 0 {
        format!("{hours}小时{mins:02}分")
    } else {
        format!("{mins}分")
    }
}

/// 辅助信息（13 号字）：表头、日期区间、计数、字段名。
/// 一次「回车」按键事件；输入法的「完成」也走它，与实体回车等价。
fn enter_key() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn meta(text: impl Into<String>) -> RichText {
    RichText::new(text).size(theme::FONT_META)
}

/// 设置页里的选项文字：用正文号，比弱化小字好读。
fn setting_label(text: impl Into<String>) -> RichText {
    RichText::new(text).size(theme::FONT_BODY)
}

/// 设置页「标签 + 控件」一行里的标签格：固定宽度，右一列的控件才能对得齐。
fn setting_label_cell(ui: &mut Ui, text: &str, width: f32) {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(setting_label(text));
        },
    );
}

/// 真粗体：`RichText::strong()` 只改颜色，真正加粗要指定字体族。
fn bold(text: impl Into<String>) -> RichText {
    RichText::new(text).family(fonts::bold_family())
}

fn table_head(ui: &mut Ui, text: &str) {
    let color = theme::meta_color(ui);
    ui.label(bold(text).size(theme::FONT_META).color(color));
}

fn table_head_right(ui: &mut Ui, text: &str) {
    let color = theme::meta_color(ui);
    right_label(ui, bold(text).size(theme::FONT_META).color(color));
}

/// 详情标题区：车次号（粗体）+ 线路 + 开行区间 + 关闭按钮。
///
/// 不随宽度改变结构：车次号一行、线路一行、开行区间一行，窄侧栏与底部面板一致。
/// 上一个看过的车次；和当前车次相同（理论上不会）时不显示，避免出现「← 自己」。
fn previous_code<'a>(
    previous: &'a Option<(i64, String)>,
    detail: &TrainDetail,
) -> Option<(i64, &'a str)> {
    previous
        .as_ref()
        .filter(|(index, _)| *index != detail.summary.index)
        .map(|(index, code)| (*index, code.as_str()))
}

/// 详情标题栏。`previous` 是上一个看过的车次（索引、车次号），
/// 有值时在「关闭」左边给一条「← G105」的回头路。
/// 详情标题栏的入参与回执。
///
/// 摞成结构体是为了不把参数写成八股：`open` 是开行区间文案，`close_label` 决定
/// 按钮写「关闭」还是「返回」（面板 / 整页），`close` 与 `back` 是这一帧的回执。
struct HeaderState<'a> {
    open: String,
    close_label: &'a str,
    close: bool,
    back: Option<DetailLink>,
}

fn header_ui(
    ui: &mut Ui,
    detail: &TrainDetail,
    compact: bool,
    previous: Option<(i64, &str)>,
    state: &mut HeaderState<'_>,
) {
    let route = format!("{} → {}", detail.summary.first, detail.summary.last);
    ui.horizontal(|ui| {
        // 先把右侧按钮/上一个车次的宽度量出来留好：`right_to_left` 是叠加，
        // 不预留的话长车次号（K1061/K1064）会把「始发 → 终到」压到按钮底下。
        let reserve = {
            let pad = 2.0 * ui.spacing().button_padding.x;
            let measure = |text: &str| {
                ui.ctx().fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(
                            text.to_owned(),
                            egui::FontId::proportional(theme::FONT_META),
                            egui::Color32::PLACEHOLDER,
                        )
                        .size()
                        .x
                })
            };
            let close = measure(state.close_label) + pad;
            let previous = previous
                .map(|(_, code)| measure(&format!("← {code}")) + theme::GAP_S)
                .unwrap_or(0.0);
            close + previous + theme::GAP_S
        };
        let title_width = (ui.available_width() - reserve).max(96.0);
        // 标题块的高度：单行取车次那一行，双行取两行之和；再按行高（= 右侧按钮高）
        // 上下各留一半，让文字与按钮在垂直方向对齐。
        let line_height = |id: egui::FontId| ui.ctx().fonts_mut(|fonts| fonts.row_height(&id));
        let code_h = line_height(egui::FontId::new(theme::FONT_CODE, fonts::bold_family()));
        let route_h = line_height(egui::FontId::proportional(theme::FONT_BODY));
        let content_h = code_h + route_h;
        let single_line_h = code_h.max(route_h);
        let row_height = theme::control_height().max(content_h);
        ui.allocate_ui_with_layout(
            egui::vec2(title_width, row_height),
            Layout::top_down(Align::Min),
            |ui| {
                // 一行放不下就整体换行（车次一行、始发 → 终到一行），
                // 而不是把「哈尔滨西」从中间掰开或被返回按钮压住。
                let route_width = ui.ctx().fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(
                            route.clone(),
                            egui::FontId::proportional(theme::FONT_BODY),
                            egui::Color32::PLACEHOLDER,
                        )
                        .size()
                        .x
                });
                let code_width = ui.ctx().fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(
                            detail.summary.code.clone(),
                            egui::FontId::new(theme::FONT_CODE, fonts::bold_family()),
                            egui::Color32::PLACEHOLDER,
                        )
                        .size()
                        .x
                });
                let fits = code_width + theme::GAP_S + route_width <= title_width;
                let two_lines = compact && !fits;
                let content = if two_lines { content_h } else { single_line_h };
                // 单行时按「行盒」居中，字形（等宽数字 + 汉字）看上去仍比右侧按钮低
                // 约 6 点：24 点字的下缘比 13 点按钮文字更低。往上抬一档补偿掉。
                let bias = if two_lines { 0.0 } else { 9.0 };
                ui.add_space(((row_height - content) / 2.0 - bias).max(0.0));
                if two_lines {
                    // 两行：车次在上、始发 → 终到在下，靠左对齐。
                    ui.label(bold(&detail.summary.code).size(theme::FONT_CODE));
                    ui.add(egui::Label::new(
                        RichText::new(&route).size(theme::FONT_BODY),
                    ))
                    .on_hover_text(&route);
                } else if compact {
                    ui.horizontal(|ui| {
                        ui.label(bold(&detail.summary.code).size(theme::FONT_CODE));
                        ui.add_space(theme::GAP_S);
                        ui.add(egui::Label::new(
                            RichText::new(&route).size(theme::FONT_BODY),
                        ))
                        .on_hover_text(&route);
                    });
                } else {
                    ui.label(bold(&detail.summary.code).size(theme::FONT_CODE));
                }
            },
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(egui::Button::new(meta(state.close_label))).clicked() {
                state.close = true;
            }
            if let Some((index, code)) = previous {
                // `override_text_color` 会盖掉 `Link` 的回退色，这里显式指定。
                let text = RichText::new(format!("← {code}"))
                    .size(theme::FONT_META)
                    .color(ui.visuals().hyperlink_color);
                if ui
                    .add(egui::Link::new(text))
                    .on_hover_text(format!("回到上一个车次 {code}"))
                    .clicked()
                {
                    state.back = Some(DetailLink::PreviousTrain(index));
                }
            }
        });
    });
    if !compact {
        ui.add_space(2.0);
        ui.label(RichText::new(&route).size(theme::FONT_BODY));
    }
    ui.add_space(2.0);
    ui.label(meta(state.open.clone()).weak());
}

/// 停站表：车站 / 到达 / 发车 / 里程 / 天。
/// 停站表。返回被点中的车站名（只有站名字形是入口，其余单元格不响应点击）。
/// 停站表。
///
/// 列由「设置 → 表格列」决定（`custom = None` 时按宽度自动挑：窄屏先收起里程）。
/// 这里**只画表**、不自己建滚动区：窄屏整页由外层的两向滚动块管，宽屏右栏和桌面侧栏
/// 由调用方包一层 [`scrollable_table`]。`content_width` 是外层给的内容宽度，
/// 列比它宽时表格就按内容宽度画，横着滑。
fn stop_specs(
    custom: Option<&[columns::StopColumn]>,
    content_width: f32,
) -> Vec<columns::Spec<columns::StopColumn>> {
    match custom {
        Some(picked) => picked.iter().map(|column| column.spec()).collect(),
        None => columns::StopColumn::auto(content_width)
            .iter()
            .map(|column| column.spec())
            .collect(),
    }
}

/// 停站表要占多宽：列比可用宽度窄时就是可用宽度（铺满），否则是各列自然宽度之和。
fn stops_table_width(ui: &Ui, custom: Option<&[columns::StopColumn]>, content_width: f32) -> f32 {
    let specs = stop_specs(custom, content_width);
    let (_, total) = column_widths(&specs, content_width, ui.spacing().item_spacing.x);
    total
}

fn stops_table_ui(
    ui: &mut Ui,
    stops: &[Stop],
    custom: Option<&[columns::StopColumn]>,
    content_width: f32,
) -> Option<String> {
    use columns::StopColumn as Col;
    let specs: Vec<columns::Spec<Col>> = stop_specs(custom, content_width);
    let spacing = ui.spacing().item_spacing.x;
    let (widths, _) = column_widths(&specs, content_width, spacing);
    let width = content_width;
    let pitch = theme::detail_row_height() + ui.spacing().item_spacing.y;
    let height = theme::detail_header_height() + stops.len() as f32 * pitch + theme::GAP_S;
    // 首站开车时刻：均速的起点（首站本身没有均速可言，显示 ---）。
    let origin_depart = stops.first().map(|stop| stop.depart_min).unwrap_or(0);
    let last_seq = stops.last().map(|stop| stop.seq);
    let mut clicked = None;
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        Layout::top_down(Align::Min),
        |ui| {
            let header_top = ui.cursor().top();
            let mut widths = table_widths(ui, "train_stops", &specs, widths, Col::key);
            apply_resize_drag(ui, "train_stops", &specs, &mut widths, Col::resizable);
            let active = table_active_column(ui, "train_stops");
            let table = exact_columns(
                TableBuilder::new(ui)
                    .id_salt("train_stops")
                    .striped(true)
                    .drag_to_scroll(egui::scroll_area::DragScroll::Never)
                    .auto_shrink([false, false])
                    .cell_layout(cell_layout()),
                &widths,
            );
            let mut edges: Vec<f32> = Vec::with_capacity(specs.len());
            let mut cells: Vec<egui::Rect> = Vec::with_capacity(specs.len());
            let mut first_row: Option<egui::Rect> = None;
            table
                .header(theme::detail_header_height(), |mut header| {
                    for (index, spec) in specs.iter().enumerate() {
                        header.col(|ui| {
                            edges.push(ui.max_rect().right());
                            cells.push(ui.max_rect());
                            column_head(ui, spec, active == Some(index));
                        });
                    }
                })
                .body(|mut body| {
                    // 整页内容宽度要跟着实际列宽走（用户可能把列拖宽），
                    // 否则窄屏整页的横向滚动区会把表格右边裁掉。
                    let drawn = body.widths();
                    let total =
                        drawn.iter().sum::<f32>() + spacing * drawn.len().saturating_sub(1) as f32;
                    remember_table_width(body.ui_mut(), "train_stops", total);
                    body.rows(theme::detail_row_height(), stops.len(), |mut row| {
                        let index = row.index();
                        let stop = &stops[index];
                        // 不再按「第 N 天」画跨日分隔线：「天」列本身已经写明第几天，
                        // 那条横线在密集表格里只会和斑马纹、行分隔线抢注意力。
                        for spec in &specs {
                            row.col(|ui| match spec.column {
                                Col::Station => {
                                    let is_terminal = stop.seq == 1 || Some(stop.seq) == last_seq;
                                    let text = if is_terminal {
                                        bold(&stop.station)
                                    } else {
                                        RichText::new(&stop.station)
                                    };
                                    let tip = format!(
                                        "{} · 停留 {} 分
点击查看该站的时刻表",
                                        stop.station, stop.dwell_min
                                    );
                                    if link_cell(ui, text, tip) {
                                        clicked = Some(stop.station.clone());
                                    }
                                }
                                Col::Arrive => {
                                    ui.label(RichText::new(&stop.arrive).monospace());
                                }
                                Col::Depart => {
                                    ui.label(RichText::new(&stop.depart).monospace());
                                }
                                // 始发/终到没有停留：`---` 比「0 分」少一点误会。
                                Col::Dwell => right_label(ui, dwell_text(stop.dwell_min)),
                                Col::Km => right_label(ui, format!("{} km", stop.km)),
                                Col::Day => right_label(ui, format!("第 {} 天", stop.day)),
                                Col::Speed => {
                                    right_label(ui, speed_text(index, stop, origin_depart))
                                }
                            });
                        }
                        if index == 0 {
                            first_row = Some(row.response().rect);
                        }
                    });
                });
            table_header_drag(ui, "train_stops", &specs, &cells, Col::key, Col::resizable);
            table_resize_handles(
                ui,
                "train_stops",
                &specs,
                &mut widths,
                &edges,
                first_row,
                theme::detail_header_height(),
                Col::resizable,
            );
            paint_header_line(ui, header_top, theme::detail_header_height());
        },
    );
    clicked
}

/// 停留时长：始发/终到站没有意义，显示 `---`。
fn dwell_text(dwell_min: i64) -> String {
    if dwell_min > 0 {
        format!("{dwell_min} 分")
    } else {
        "---".to_owned()
    }
}

/// 均速：本站里程 ÷ 从始发开车到本站到达的时间。
///
/// 首站（`index == 0`）没有区间可言，按约定显示 `---`；里程或时间算不出正数时同样。
fn speed_text(index: usize, stop: &Stop, origin_depart: i64) -> String {
    if index == 0 || stop.km <= 0 {
        return "---".to_owned();
    }
    let minutes = stop.arrive_min - origin_depart;
    if minutes <= 0 {
        return "---".to_owned();
    }
    let speed = stop.km as f64 * 60.0 / minutes as f64;
    format!("{speed:.0} km/h")
}

/// 详情里的「标签 + 值」行：标签固定窄列、弱化，值可以换行。
fn detail_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = theme::GAP_S;
        ui.allocate_ui_with_layout(egui::vec2(38.0, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.add_space(1.0);
            ui.label(meta(label).weak());
        });
        ui.vertical(|ui| add(ui));
    });
}

/// 交路串里的一个车次号：当前车次加粗高亮、查得到的画成链接、其余弱化。
///
/// 交路串和展开后的明细框共用这一个画法，「点击→跳转」由调用方处理。
fn duty_segment_ui(ui: &mut Ui, segment: &DutySegment) -> Option<String> {
    let text = RichText::new(&segment.label).size(theme::FONT_BODY);
    if segment.current {
        ui.label(
            text.family(fonts::bold_family())
                .background_color(ui.visuals().selection.bg_fill),
        )
        .on_hover_text("当前车次");
        None
    } else if jumpable_train_code(&segment.code) {
        // `override_text_color` 会盖掉 `Link` 的回退色，这里显式指定，
        // 保证和停站表里的车站链接是同一支深蓝；悬停时 Link 自己加下划线。
        let text = text.color(ui.visuals().hyperlink_color);
        let clicked = ui
            .add(egui::Link::new(text))
            .on_hover_text(format!("查看 {} 的时刻表", segment.label))
            .clicked();
        clicked.then(|| segment.code.clone())
    } else {
        ui.label(text.weak())
            .on_hover_text("回送 / 检测车次，没有独立时刻表");
        None
    }
}

/// 交路串：`G1806/7 → G82/79 → [G1]`，当前车次高亮
///（有效区间不写在这里了：展开后写在明细框的尾部）。
///
/// 除当前车次外，其它车次都可以点过去（`0` 开头的回送车与 `DJ` 检测车除外），
/// 返回被点中的段首车次号。
fn duty_chain_ui(ui: &mut Ui, duty: &Duty, extra_duties: bool, index: usize) -> Option<String> {
    let mut clicked = None;
    // 交路只有一行文字，不需要按钮级别的行高。
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 22.0;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_XS;
            if extra_duties {
                ui.label(meta(format!("{}.", index + 1)).weak());
            }
            for (position, segment) in duty.segments.iter().enumerate() {
                if position > 0 {
                    ui.label(RichText::new("→").size(theme::FONT_BODY).weak());
                }
                if let Some(code) = duty_segment_ui(ui, segment) {
                    clicked = Some(code);
                }
            }
        });
    });
    clicked
}

/// 「交路明细」展开状态按车次保存：换车次就回到收起，不留上次的展开。
fn duty_fold_id(train_index: i64) -> egui::Id {
    egui::Id::unique(("llt-duty-fold", train_index))
}

/// 交路明细框：圆角描边的一段长方形，一行一段车次，尾部写交路有效期。
///
/// 每行是「车次号 +（出发站 出发时刻 → 到达站 到达时刻）」；车次号的可点性、
/// 当前车次高亮都与交路串一致，查不到那趟车时只写车次号。框撑满内容列宽宽，
/// 四周内边距一样。
fn duty_box_ui(
    ui: &mut Ui,
    duties: &[&Duty],
    runs: &HashMap<(i64, i64), DutyRun>,
    multiple: bool,
    link: &mut Option<DetailLink>,
) {
    let frame = egui::Frame::new()
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(theme::GAP_S as i8));
    frame.show(ui, |ui| {
        // 框要撑满父容器，而 `Frame` 默认按内容收缩。
        ui.set_min_width(ui.available_width());
        // 一行一段车次，就一行字的高度——不然每一行都会被抬到控件高度（触摸屏 44 点）。
        ui.spacing_mut().interact_size.y = 20.0;
        for (index, duty) in duties.iter().enumerate() {
            if index > 0 {
                ui.add_space(theme::GAP_S);
            }
            for (position, segment) in duty.segments.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                    // 同号多区间时标出序号，与交路串上的序号对得上。
                    if multiple && position == 0 {
                        ui.label(meta(format!("{}.", index + 1)).weak());
                    }
                    if let Some(code) = duty_segment_ui(ui, segment) {
                        *link = Some(DetailLink::Train(code));
                    }
                    if let Some(run) = runs.get(&(duty.id, segment.seq)) {
                        ui.label(
                            meta(format!(
                                "({} {} → {} {})",
                                run.from, run.from_time, run.to, run.to_time
                            ))
                            .weak(),
                        )
                        .on_hover_text(format!(
                            "{} {} 开，{} {} 到（按查询当天的数据）",
                            run.from, run.from_time, run.to, run.to_time
                        ));
                    }
                });
            }
            ui.label(
                meta(format!(
                    "有效期 {}",
                    date_window(duty.start_date, duty.end_date)
                ))
                .weak(),
            )
            .on_hover_text(format!(
                "该交路有效区间：{} ~ {}",
                date_ymd(duty.start_date),
                date_ymd(duty.end_date)
            ));
        }
    });
}

/// 交路行的折叠三角按钮：18×18 的点击区，悬停有底色。
fn duty_fold_button_ui(ui: &mut Ui, open: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let color = if hovered {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    paint_collapse_arrow(ui, rect, open, color);
    response
        .on_hover_text(if open {
            "收起交路明细"
        } else {
            "展开交路明细（每段车次的首末站与时刻）"
        })
        .clicked()
}

/// 把 `京包张包段/包西线` 拆成线路集合。
fn line_set(line: &str) -> std::collections::HashSet<&str> {
    line.split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

/// 经由表：站点 / 线路 / 里程。
///
/// 三种标记互不干扰：
/// - 停站：本站是车次的办客停站，站名用强调色；
/// - 换线：与上一行不是同一条线路，线路名用警示色；
/// - 非办客点（线路所 / 信号所 / 未收录站点）：弱化斜体，勾上「只看车站」后隐藏。
fn via_table_ui(
    ui: &mut Ui,
    via: &[ViaPoint],
    stations_only: bool,
    stops: &[Stop],
    id: i64,
    max_height: f32,
    salt: &'static str,
) {
    let stop_names: std::collections::HashSet<&str> =
        stops.iter().map(|stop| stop.station.as_str()).collect();

    // 过滤后仍然按原始顺序显示，换线判断基于「上一行显示的点」，
    // 这样隐藏线路所之后，新线路的首个可见站依然会被标出来。
    let shown: Vec<&ViaPoint> = via
        .iter()
        .filter(|point| !stations_only || point.is_station)
        .collect();

    let width = ui.available_width();
    // 触摸屏（手机）线路列可能按 `/` 拆成多行，行高要跟着变。
    let multiline = theme::pointer() == theme::Pointer::Touch;
    let show_km = width >= 300.0;
    let station_width = (width * 0.32).clamp(96.0, 132.0);
    let header_top = ui.cursor().top();
    // max_height 是「表头 + 表体」的总高度；max_scroll_height 只约束表体。
    let pitch = theme::detail_row_height() + ui.spacing().item_spacing.y;
    let body_max = (max_height - theme::detail_header_height()).max(pitch);
    let mut table = TableBuilder::new(ui)
        .id_salt(("via_table", id, salt, width.round() as i32))
        .striped(true)
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .max_scroll_height(body_max)
        .cell_layout(cell_layout())
        .column(Column::initial(station_width).at_least(96.0))
        .column(Column::remainder().at_least(96.0));
    if show_km {
        table = table.column(Column::initial(76.0));
    }

    table
        .header(theme::detail_header_height(), |mut header| {
            header.col(|ui| table_head(ui, "站点"));
            header.col(|ui| table_head(ui, "线路"));
            if show_km {
                header.col(|ui| table_head_right(ui, "里程"));
            }
        })
        .body(|mut body| {
            let draw = |mut row: egui_extras::TableRow<'_, '_>, index: usize| {
                let point = shown[index];
                let is_stop = stop_names.contains(point.station.as_str());
                // 跨线站的线路名会写成 `京包张包段/包西线`（两条线共用本站）。
                // 因此换线 = 当前线路集合里有上一行没有的线路；这样
                // 「A」→「A/B」会标在共用站，「A/B」→「B」则不会重复标。
                let current = line_set(&point.line);
                let new_lines: Vec<&str> = if index == 0 {
                    Vec::new()
                } else {
                    let previous = line_set(&shown[index - 1].line);
                    current
                        .iter()
                        .filter(|line| !previous.contains(*line))
                        .copied()
                        .collect()
                };
                let changed = !new_lines.is_empty();

                row.col(|ui| {
                    let text = RichText::new(&point.station);
                    let text = if !point.is_station {
                        text.weak().italics()
                    } else if is_stop {
                        text.color(ui.visuals().hyperlink_color)
                    } else {
                        text
                    };
                    ui.add(egui::Label::new(text).truncate())
                        .on_hover_text(if point.is_station {
                            if is_stop {
                                "本站停靠"
                            } else {
                                "通过（不停站）"
                            }
                        } else {
                            "非办客站点（线路所 / 信号所 / 未收录等）"
                        });
                });
                row.col(|ui| {
                    let warn = ui.visuals().warn_fg_color;
                    let tip = if changed {
                        format!("由此进入 {}", new_lines.join(" / "))
                    } else {
                        point.line.clone()
                    };
                    // 跨线站的线路名写作 `A/B`（共用本站）。触摸屏上不再截成「A/…」，
                    // 而是按 `/` 拆开、每条线占一行；行高由下面的循环按段数放大。
                    if multiline && point.line.contains('/') {
                        // 用一个带换行符的 Label，而不是 ui.vertical + 多个 Label：
                        // 前者的垂直居中与其它单元格完全一致；ui.vertical 在单元格里
                        // 会贴顶，上下内边距一多一少。
                        let joined = point
                            .line
                            .split('/')
                            .map(str::trim)
                            .filter(|segment| !segment.is_empty())
                            .collect::<Vec<_>>()
                            .join(
                                "
",
                            );
                        let mut text = RichText::new(joined);
                        if changed {
                            text = text.color(warn);
                        }
                        ui.add(egui::Label::new(text)).on_hover_text(tip);
                    } else {
                        let text = if changed {
                            RichText::new(&point.line).color(warn)
                        } else {
                            RichText::new(&point.line)
                        };
                        ui.add(egui::Label::new(text).truncate()).on_hover_text(tip);
                    }
                });
                if show_km {
                    row.col(|ui| {
                        right_label(ui, format!("{} km", point.km));
                    });
                }
            };
            if multiline {
                // 线路被拆成多行的行要更高，只能逐行给高度（放弃虚拟化；
                // 经由点最多百来条，比表格里几千行轻得多）。
                for (index, point) in shown.iter().enumerate() {
                    let segments = point
                        .line
                        .split('/')
                        .filter(|segment| !segment.trim().is_empty())
                        .count()
                        .max(1);
                    let extra = (segments as f32 - 1.0) * (theme::FONT_BODY + 6.0);
                    body.row(theme::detail_row_height() + extra, |row| {
                        draw(row, index);
                    });
                }
            } else {
                body.rows(theme::detail_row_height(), shown.len(), |row| {
                    let index = row.index();
                    draw(row, index);
                });
            }
        });
    paint_header_line(ui, header_top, theme::detail_header_height());
}

/// 经由的图例：圆点颜色和表格一一对应，文字保持常规颜色。
fn via_legend_ui(ui: &mut Ui) {
    let accent = ui.visuals().hyperlink_color;
    let warn = ui.visuals().warn_fg_color;
    let weak = ui.visuals().weak_text_color();
    legend_item(ui, accent, "停站", "车次在该站办理客运，站名用蓝色");
    legend_item(ui, warn, "换线", "由此进入另一条线路，线路名用橙色");
    // 不只线路所：数据里没有收录、或不办理客运的车站都会落到这一类，
    // 所以图例用「非办客」而不是具体的「线路所」。
    legend_item(
        ui,
        weak,
        "非办客",
        "不办理客运的站点：线路所、信号所、未收录的货运站等，可被「只看车站」隐藏",
    );
}

fn legend_item(ui: &mut Ui, color: Color32, label: &str, tip: &str) {
    // 包一层 horizontal：放进 `horizontal_wrapped` 时整项作为一组换行，
    // 不会把「非办客」拆成「非办」+「客」。
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(RichText::new("●").size(10.0).color(color));
        ui.label(meta(label)).on_hover_text(tip);
    });
    ui.add_space(theme::GAP_XS);
}

/// 折叠标题的三角箭头（不依赖字体里的几何符号）。
fn collapse_arrow(ui: &mut Ui, open: bool, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 18.0), Sense::hover());
    paint_collapse_arrow(ui, rect, open, color);
}

/// 在 `rect` 里画折叠三角（`open` 朝下、否则朝右），不分配空间。
fn paint_collapse_arrow(ui: &Ui, rect: egui::Rect, open: bool, color: Color32) {
    let center = rect.center();
    let radius = 3.5;
    let points = if open {
        vec![
            egui::pos2(center.x - radius, center.y - radius * 0.55),
            egui::pos2(center.x + radius, center.y - radius * 0.55),
            egui::pos2(center.x, center.y + radius * 0.9),
        ]
    } else {
        vec![
            egui::pos2(center.x - radius * 0.55, center.y - radius),
            egui::pos2(center.x - radius * 0.55, center.y + radius),
            egui::pos2(center.x + radius * 0.9, center.y),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

/// 宽屏侧栏的经由折叠状态按车次保存。
fn via_state_id(train_index: i64) -> egui::Id {
    egui::Id::unique(("llt-via", train_index))
}

/// 经由入口的一行摘要：`61 途经点 · 1463 km`。
///
/// 只写这两项：点数、里程在经由页里能数出来，但入口收起时它们是最有用的两个量；
/// 「车站 N」就省了——那要看「只看车站」勾上以后的条数，和入口无关。
fn via_entry_summary(via: &[ViaPoint]) -> String {
    let total = via.last().map(|point| point.km).unwrap_or(0);
    format!("{} 途经点 · {total} km", via.len())
}

/// 量一行文字的宽度（不换行、不截断），用来给固定宽的格子定宽。
fn text_width(ui: &Ui, text: &str, font: egui::FontId) -> f32 {
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font, egui::Color32::PLACEHOLDER)
            .size()
            .x
    })
}

/// 明细页入口条里的一格：折叠三角 + 标题 + 弱化摘要 + 尾部「查看 ▸」。
///
/// 以前这三处入口各写了一份（窄屏的经由、宽屏的经由折叠标题、桌面的开行计划）：
/// 行高两套、细线一处两条一处一条、箭头一处左一处右，叠在一起就是「双线 + 错位」。
/// 现在统一成这一个格子，差别只剩字段。
struct Entry<'a> {
    /// 悬停/点击用的 id，同时也是点中后的回传值。
    id: &'a str,
    /// 粗体标题。
    title: &'a str,
    /// 标题后面的弱化摘要，空串表示不画。
    summary: &'a str,
    /// 右端的弱化提示（「查看」），空串表示不画（连箭头一起）。
    hint: &'a str,
    /// 左端的折叠三角：`Some(展开状态)`；`None` 表示这一格不折叠。
    arrow: Option<bool>,
    /// 固定宽度；`None` = 分掉剩下的宽度。
    width: Option<f32>,
    /// 整格的悬停提示，空串表示没有。
    hover: &'a str,
}

/// 一格入口的自然宽度：标题 + 摘要 + 尾部「查看 ▸」+ 左右内边距。
///
/// 两个入口并排时，带长摘要的那格（经由）分剩下的宽度，另一格用这个宽度就够。
fn entry_natural_width(ui: &Ui, entry: &Entry<'_>) -> f32 {
    let arrow_width = 14.0;
    let mut width = 2.0 * theme::GAP_S
        + text_width(
            ui,
            entry.title,
            egui::FontId::new(theme::FONT_BODY, fonts::bold_family()),
        );
    if entry.arrow.is_some() {
        width += arrow_width + theme::GAP_XS;
    }
    if !entry.summary.is_empty() {
        width += theme::GAP_S
            + text_width(
                ui,
                entry.summary,
                egui::FontId::proportional(theme::FONT_META),
            );
    }
    if !entry.hint.is_empty() {
        width += theme::GAP_S
            + text_width(ui, entry.hint, egui::FontId::proportional(theme::FONT_META))
            + theme::GAP_XS
            + arrow_width;
    }
    width
}

/// 入口条：一条上细线 + 一行入口 + 一条下细线，返回被点中那一格的 `id`。
///
/// 细线由这里统一画，调用方前后**只留空白**，不要再补 `ui.separator()`。
/// 格子之间留 `GAP_M`，悬停底色各管各的 —— 两个入口并排时不会看成一整条。
fn entry_bar_ui<'a>(ui: &mut Ui, entries: &'a [Entry<'a>]) -> Option<&'a str> {
    if entries.is_empty() {
        return None;
    }
    let height = theme::control_height();
    let total = ui.available_width();
    let gap = theme::GAP_M;
    let gaps = gap * (entries.len() - 1) as f32;
    let fixed: f32 = entries.iter().filter_map(|entry| entry.width).sum();
    let flex_count = entries.iter().filter(|entry| entry.width.is_none()).count();
    let flex = if flex_count == 0 {
        0.0
    } else {
        ((total - fixed - gaps) / flex_count as f32).max(48.0)
    };

    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, height), Sense::hover());
    let hairline = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    ui.painter().hline(rect.x_range(), rect.top(), hairline);
    ui.painter().hline(rect.x_range(), rect.bottom(), hairline);

    let mut clicked = None;
    let mut left = rect.left();
    for entry in entries {
        let width = entry.width.unwrap_or(flex);
        let cell =
            egui::Rect::from_min_size(egui::pos2(left, rect.top()), egui::vec2(width, height));
        if entry_cell_ui(ui, cell, entry) {
            clicked = Some(entry.id);
        }
        left += width + gap;
    }
    clicked
}

/// 入口条里的一格；返回是否被点中。
///
/// 文字用子 UI 画，点击区域在这之后用 `interact` 叠在整个格子上——点到文字也能中；
/// 悬停底色在画内容之前按指针位置判断。
fn entry_cell_ui(ui: &mut Ui, cell: egui::Rect, entry: &Entry<'_>) -> bool {
    let hovered = ui.rect_contains_pointer(cell);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(
            cell,
            egui::CornerRadius::same(4),
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    let weak = ui.visuals().weak_text_color();
    // 尾部「查看 ▸」先量出来，摘要按剩下的宽度截断，长摘要不会压到箭头底下。
    let arrow_width = 14.0;
    let trailing = if entry.hint.is_empty() {
        0.0
    } else {
        text_width(ui, entry.hint, egui::FontId::proportional(theme::FONT_META))
            + theme::GAP_XS
            + arrow_width
            + theme::GAP_S
    };
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(cell)
            .layout(Layout::left_to_right(Align::Center)),
    );
    // 间距全由下面显式 `add_space` 给：留默认的 `item_spacing` 会和它叠成两倍，
    // `entry_natural_width` 算出来的宽度就对不上，格子会挤到叠字。
    row.spacing_mut().item_spacing.x = 0.0;
    row.add_space(theme::GAP_S);
    if let Some(open) = entry.arrow {
        collapse_arrow(&mut row, open, weak);
        row.add_space(theme::GAP_XS);
    }
    row.label(bold(entry.title).size(theme::FONT_BODY));
    if !entry.summary.is_empty() {
        row.add_space(theme::GAP_S);
        let used = row.cursor().left() - cell.left();
        let budget = (cell.width() - used - trailing).max(0.0);
        row.allocate_ui_with_layout(
            egui::vec2(budget, cell.height()),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.add(egui::Label::new(meta(entry.summary).weak()).truncate());
            },
        );
    }
    if !entry.hint.is_empty() {
        row.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(theme::GAP_S);
            collapse_arrow(ui, false, weak);
            ui.add_space(theme::GAP_XS);
            ui.label(meta(entry.hint).weak());
        });
    }
    drop(row);

    let response = ui.interact(
        cell,
        ui.make_persistent_id(("llt-entry", entry.id)),
        Sense::click(),
    );
    let clicked = response.clicked();
    if !entry.hover.is_empty() {
        response.on_hover_text(entry.hover);
    }
    clicked
}

/// 交路 / 车型 / 餐车等信息行；经由由调用方在下方单独绘制。
/// 开行计划日历要用的两样东西：周期表与正在看的月份。
///
/// 拆成结构体是因为 `extras_ui` 的参数已经到极限，而这两项又必须可变
/// （翻月要写回界面状态）。
struct RunPlan<'a> {
    rules: &'a std::collections::HashMap<i64, (i64, i64)>,
    /// `yyyymm`。
    month: &'a mut i64,
    /// 桌面端点「开行计划」入口后置真，由详情页画弹窗。
    open: &'a mut bool,
}

fn extras_ui(
    ui: &mut Ui,
    train: &TrainSummary,
    extras: &TrainExtras,
    stops: &[Stop],
    date: i64,
    link: &mut Option<DetailLink>,
    plan: &mut RunPlan<'_>,
    // 桌面：开行计划是否由本函数在尾上画一行入口。宽屏把它挪到下面的入口条
    // （和经由并排一行），那时传 `false`。
    plan_entry: bool,
) {
    let (duties, dining, matched) = extras.for_date(date);
    ui.spacing_mut().item_spacing.y = 6.0;

    // 用时：首末站之间的运行时间。跨天时补一个 (n 天) 说明，
    // 避免「25小时38分」这类读数看不出实际跨了几天。
    let run_min = stops.last().map(|stop| stop.arrive_min).unwrap_or(0)
        - stops.first().map(|stop| stop.depart_min).unwrap_or(0);
    let span_days = train.days.max(run_min / 1440 + 1);
    // 统一按小时计（可超过 24），跨天时再补 (n 天)，避免「2天11小时」与
    // 「59小时」两种口径混用。
    let hours = run_min / 60;
    let minutes = run_min % 60;
    let duration = if hours > 0 {
        format!("{hours}小时{minutes:02}分")
    } else {
        format!("{minutes}分")
    };
    let duration = if span_days > 1 {
        format!("{duration} ({span_days}天)")
    } else {
        duration
    };
    detail_row(ui, "用时", |ui| {
        ui.label(RichText::new(&duration).size(theme::FONT_BODY))
            .on_hover_text("首站发车到末站到达，括号内为跨天天数");
    });

    if !duties.is_empty() {
        let multiple = duties.len() > 1;
        let fold_id = duty_fold_id(train.index);
        let mut open = ui.data(|data| data.get_temp::<bool>(fold_id).unwrap_or(false));
        detail_row(ui, "交路", |ui| {
            // 串占左边、折叠三角贴右：串换行时三角不动，不会被挤到下一行。
            ui.horizontal_top(|ui| {
                let chain_width = (ui.available_width() - 18.0 - theme::GAP_XS).max(0.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(chain_width, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        for (index, duty) in duties.iter().enumerate() {
                            if index > 0 {
                                ui.add_space(theme::GAP_XS);
                            }
                            // 同号多区间时标出序号，避免两条交路看起来像一条。
                            if let Some(code) = duty_chain_ui(ui, duty, multiple, index) {
                                *link = Some(DetailLink::Train(code));
                            }
                        }
                    },
                );
                if duty_fold_button_ui(ui, open) {
                    open = !open;
                }
            });
            // 展开后在交路行与车型之间插一个明细框（每一行一段车次）。
            if open {
                ui.add_space(theme::GAP_S);
                duty_box_ui(ui, &duties, &extras.runs, multiple, link);
            }
        });
        ui.data_mut(|data| data.insert_temp(fold_id, open));
    }

    // 车型：优先用交路里的编组（如 CR400BF-S型重联），没有就用车次类别。
    let formation = duties
        .iter()
        .map(|duty| duty.formation.trim())
        .find(|formation| !formation.is_empty());
    let company = duties
        .iter()
        .map(|duty| duty.company.trim())
        .find(|company| !company.is_empty());

    detail_row(ui, "车型", |ui| {
        match formation {
            Some(formation) => {
                ui.add(egui::Label::new(RichText::new(formation).size(theme::FONT_BODY)).wrap())
                    .on_hover_text(formation);
            }
            // 没有编组信息时退回车次类别，保证这一行总有内容。
            None => {
                ui.label(RichText::new(&train.kind).size(theme::FONT_BODY));
            }
        }
    });
    if let Some(company) = company {
        detail_row(ui, "担当", |ui| {
            ui.label(RichText::new(company).size(theme::FONT_BODY));
        });
    }

    if !dining.is_empty() {
        let cars: Vec<&str> = dining.iter().map(|car| car.coach.trim()).collect();
        let text = cars.join("、");
        detail_row(ui, "餐车", |ui| {
            ui.add(egui::Label::new(RichText::new(&text).size(theme::FONT_BODY)).truncate())
                .on_hover_text(text);
        });
    }

    if !matched && (!duties.is_empty() || !dining.is_empty()) {
        detail_row(ui, "", |ui| {
            ui.label(
                meta("查询日期不在上述区间内，显示的是全部区间").color(ui.visuals().warn_fg_color),
            );
        });
    }

    // 手机整页：月历直接内嵌在这页里（本来就是滚动的一页，再开一层弹窗反而绕）；
    // 桌面只留一行入口——宽屏时入口在下面的入口条里（见 `detail_ui`），
    // 窄窗口的底部面板没那么宽，入口就接在杂项后面。
    if theme::pointer() == theme::Pointer::Touch {
        ui.add_space(theme::GAP_S);
        run_plan_ui(ui, train, plan, true, date);
    } else if plan_entry {
        ui.add_space(theme::GAP_S);
        if run_plan_entry(ui) {
            *plan.open = true;
        }
    }
}

/// 桌面端的开行计划弹窗：封顶 340×70% 的居中窗口，内容与手机端内嵌的日历一致。
fn run_plan_page_ui(
    ui: &mut Ui,
    train: &TrainSummary,
    rules: &std::collections::HashMap<i64, (i64, i64)>,
    month: &mut i64,
    open: &mut bool,
    screen: egui::Vec2,
    date: i64,
) {
    // 只约束宽度：日历是定高的，高度交给内容，免得弹窗下面拖一大片空白。
    let width = (screen.x - 24.0).min(340.0);
    // 与经由页同一个理由：`Area` 按 id 记尺寸，窗口/转屏改宽度后必须换一份状态。
    let size_key = (width.round() as i32, screen.y.round() as i32);
    let frame = egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(12);
    let mut plan = RunPlan {
        rules,
        month,
        open: &mut false,
    };
    let mut close = false;
    let _ = egui::Modal::new(egui::Id::unique(("llt-run-plan-page", size_key)))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(120))
        .show(ui.ctx(), |ui| {
            ui.set_min_width(width - 24.0);
            ui.set_max_width(width - 24.0);
            ui.horizontal(|ui| {
                ui.label(bold("开行计划").size(theme::FONT_SECTION));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(meta("关闭"))).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(theme::GAP_S);
            run_plan_ui(ui, train, &mut plan, false, date);
        });
    // 与详情整页同一个理由：打开它的那一下点击还没结束，模态背景层会当成
    // 「点了外面」，于是弹窗刚开就被关。这里只认自己的「关闭」按钮。
    if close {
        *open = false;
    }
}

/// 「开行计划」入口格：桌面上占一行入口，点开看月历。
///
/// 摘要（`2026-07-01 起 · 月历`）不画：同一页的抬头下面已经写过一遍开行区间，
/// 再挂一串灰字只是噪声。`width` 给定时就是和经由并排时的固定宽。
fn plan_entry(width: Option<f32>) -> Entry<'static> {
    Entry {
        id: "run-plan",
        title: "开行计划",
        summary: "",
        hint: "查看",
        arrow: None,
        width,
        hover: "按日历看这趟车每天开不开行",
    }
}

/// 桌面端单独一行的「开行计划」入口（窄窗口的底部详情面板）。
fn run_plan_entry(ui: &mut Ui) -> bool {
    entry_bar_ui(ui, &[plan_entry(None)]) == Some("run-plan")
}

/// 开行计划日历：一天一格，可翻月。
///
/// 数据完全来自本地：`date_from`/`date_to` 两个日期 + `t.rule` 里的一行
/// 「周期 / 掩码」（见 `docs/llt-format-reverse-engineering.md` §12）。
/// 用 egui 自己的绘图与 jiff 的日期运算，不引第三方日历控件。
/// 开行计划月历。
///
/// 月历的「当下」是**查询日期**（不是今天）：详情是按查询日期算开不开行的，
/// 圈出的那一天、以及首次打开落在哪个月，都跟着查询日期走。
fn run_plan_ui(
    ui: &mut Ui,
    train: &TrainSummary,
    plan: &mut RunPlan<'_>,
    with_title: bool,
    date: i64,
) {
    let (period, mask) = train
        .rule_index
        .and_then(|index| plan.rules.get(&index).copied())
        .unwrap_or((1, 0));
    if *plan.month <= 0 {
        *plan.month = date / 100;
    }

    let cell = theme::plan_cell_size(ui.available_width());
    // 星期表头与日期格固定 7 列、格间距 2 点：月份选择器要和这块网格右对齐，
    // 不能按弹窗/侧栏的整个宽度右对齐（那样会跑到日历右边去）。
    let grid_width = cell * 7.0 + 2.0 * 6.0;
    ui.allocate_ui_with_layout(
        egui::vec2(grid_width, theme::control_height()),
        Layout::left_to_right(Align::Center),
        |ui| {
            // 弹窗自己有标题，这里只留区间与翻月。
            if with_title {
                ui.label(bold("开行计划").size(theme::FONT_BODY));
                ui.add_space(theme::GAP_S);
            }
            // 开行区间在标题下面已经写过一遍（`开行 2026-11-25 ~ 2080-12-31`），
            // 这里不再重复：重复的区间会把翻月挤到只剩省略号。
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // 翻月：方形按钮、与控件同高，字形放大到 19 点再加粗。
                // 之前用 13 点的小字跟日历格子同宽，落在按钮里像两个小点，
                // 几项之间又只隔 4 点，整排挤在一起。
                let step = theme::control_height();
                ui.spacing_mut().item_spacing.x = theme::GAP_S;
                ui.spacing_mut().interact_size = egui::vec2(step, step);
                // 箭头字号比按钮默认文字大，内边距收窄一点，按钮才不会被撑得比同行控件高。
                ui.spacing_mut().button_padding = egui::vec2(theme::GAP_XS, 1.0);
                let arrow = |glyph: &str| {
                    RichText::new(glyph)
                        .size(theme::FONT_BODY + 4.0)
                        .family(fonts::bold_family())
                };
                // `interact_size` 只管高度，按钮宽度还得显式给：不给就是「一个字 + 内边距」，
                // 于是 19 点的箭头被塞进 13 点的窄条里（实测 13×37）。
                if ui
                    .add(egui::Button::new(arrow("›")).min_size(egui::vec2(step, step)))
                    .on_hover_text("下一个月")
                    .clicked()
                {
                    *plan.month = shift_month(*plan.month, 1);
                }
                ui.label(
                    RichText::new(month_label(*plan.month))
                        .size(theme::FONT_BODY)
                        .weak(),
                );
                if ui
                    .add(egui::Button::new(arrow("‹")).min_size(egui::vec2(step, step)))
                    .on_hover_text("上一个月")
                    .clicked()
                {
                    *plan.month = shift_month(*plan.month, -1);
                }
            });
        },
    );

    let year = (*plan.month / 100) as i16;
    let month = (*plan.month % 100) as i8;
    let Ok(first) = jiff::civil::Date::new(year, month, 1) else {
        return;
    };
    let days = i64::from(first.days_in_month());
    // 周一为第一列，与「工作日/周末」的直觉一致。
    let lead = i64::from(first.weekday().to_monday_zero_offset());
    // 查询日期那格画一圈：这里是「查的哪天」，不是「今天」。
    let query_ymd = date;

    let visuals = ui.visuals().clone();
    let running_fg = visuals.hyperlink_color;
    let running_bg = visuals.selection.bg_fill;
    let weak_fg = visuals.weak_text_color();
    let dim_fg = weak_fg.gamma_multiply(0.55);
    let font = egui::FontId::proportional(theme::FONT_META);

    ui.add_space(theme::GAP_XS);
    ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
    // 星期表头：只写一个字，和格子同宽，对齐天然成立。
    ui.horizontal(|ui| {
        for name in ["一", "二", "三", "四", "五", "六", "日"] {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(cell, cell * 0.7), Sense::hover());
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                name,
                font.clone(),
                weak_fg,
            );
        }
    });
    let weeks = (lead + days + 6) / 7;
    for week in 0..weeks {
        ui.horizontal(|ui| {
            for weekday in 0..7 {
                let index = week * 7 + weekday - lead;
                if !(0..days).contains(&index) {
                    // 月首、月末的空格：占位保持列对齐。
                    ui.allocate_exact_size(egui::vec2(cell, cell), Sense::hover());
                    continue;
                }
                let day = index + 1;
                let date = year as i64 * 10_000 + month as i64 * 100 + day;
                let state =
                    lltdb::date::run_state(train.date_from, train.date_to, date, period, mask);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(cell, cell), Sense::hover());
                let painter = ui.painter();
                let running = state == lltdb::date::RunState::Running;
                if running {
                    painter.rect_filled(rect.shrink(1.0), 5.0, running_bg);
                }
                let color = if running {
                    running_fg
                } else if state == lltdb::date::RunState::Stopped {
                    // 规则停开：和「区间外」区分开，画一条斜杠。
                    painter.line_segment(
                        [
                            egui::pos2(rect.left() + 5.0, rect.bottom() - 5.0),
                            egui::pos2(rect.right() - 5.0, rect.top() + 5.0),
                        ],
                        egui::Stroke::new(1.0, dim_fg),
                    );
                    weak_fg
                } else {
                    dim_fg
                };
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    day.to_string(),
                    font.clone(),
                    color,
                );
                if date == query_ymd {
                    painter.rect_stroke(
                        rect.shrink(1.0),
                        5.0,
                        egui::Stroke::new(1.0, running_fg),
                        egui::StrokeKind::Inside,
                    );
                }
                let note = if date == query_ymd {
                    " · 查询日"
                } else {
                    ""
                };
                response.on_hover_text(format!("{} · {}{note}", date_ymd(date), state.label()));
            }
        });
    }

    ui.add_space(theme::GAP_XS);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::GAP_XS;
        legend_swatch(ui, SwatchKind::Filled, running_bg, running_fg, "开行");
        legend_swatch(ui, SwatchKind::Struck, weak_fg, weak_fg, "规则停开");
        legend_swatch(ui, SwatchKind::Plain, dim_fg, dim_fg, "区间外 / 已过期");
        legend_swatch(ui, SwatchKind::Plain, running_fg, running_fg, "查询日");
    });
}

/// 图例小方块的三种画法，与格子里的表现一一对应。
enum SwatchKind {
    /// 开行：实心块。
    Filled,
    /// 规则停开：一条斜杠。
    Struck,
    /// 区间外 / 已过期：只描边。
    Plain,
}

fn legend_swatch(ui: &mut Ui, kind: SwatchKind, fill: Color32, color: Color32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(11.0, 11.0), Sense::hover());
    let painter = ui.painter();
    match kind {
        SwatchKind::Filled => {
            painter.rect_filled(rect, 3.0, fill);
        }
        SwatchKind::Struck => {
            painter.line_segment(
                [
                    egui::pos2(rect.left() + 1.0, rect.bottom() - 1.0),
                    egui::pos2(rect.right() - 1.0, rect.top() + 1.0),
                ],
                egui::Stroke::new(1.5, color),
            );
        }
        SwatchKind::Plain => {
            painter.rect_stroke(
                rect,
                3.0,
                egui::Stroke::new(1.0, color),
                egui::StrokeKind::Inside,
            );
        }
    }
    ui.label(meta(text).color(color));
}

/// `yyyymm` 前后挪若干个月。
fn shift_month(month: i64, delta: i64) -> i64 {
    let year = month / 100;
    let index = year * 12 + (month % 100) - 1 + delta;
    index.div_euclid(12) * 100 + index.rem_euclid(12) + 1
}

/// `yyyymm` -> `2026 年 11 月`。
fn month_label(month: i64) -> String {
    format!("{} 年 {} 月", month / 100, month % 100)
}

/// 详情里由界面回写的两个开关，集中传递，避免参数继续膨胀。
struct DetailToggles<'a> {
    stations_only: &'a mut bool,
    via_page_open: &'a mut bool,
}

/// 宽屏侧栏就地展开的经由表（点入口条上的「经由」后出现在它下面）。
///
/// 入口那一行不在这里画：它和「开行计划」并排在 `entry_bar_ui` 的一条线上，
/// 所以本函数只管展开之后的工具行与表格。
fn via_inline_ui(
    ui: &mut Ui,
    via: &[ViaPoint],
    toggles: &mut DetailToggles<'_>,
    stops: &[Stop],
    train_index: i64,
) {
    if via.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = 24.0;
        ui.spacing_mut().item_spacing.x = theme::GAP_XS;
        ui.checkbox(toggles.stations_only, meta("只看车站"));
        ui.add_space(theme::GAP_M);
        via_legend_ui(ui);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(egui::Button::new(meta("放大")).min_size(egui::vec2(48.0, 24.0)))
                .on_hover_text("在居中的大窗口里查看经由")
                .clicked()
            {
                *toggles.via_page_open = true;
            }
        });
    });
    ui.add_space(theme::GAP_XS);
    // 高度按整行取整，避免表格底部出现半截行。
    let pitch = theme::detail_row_height() + ui.spacing().item_spacing.y;
    let rows = ((ui.available_height() - theme::detail_header_height()) / pitch)
        .floor()
        .max(1.0);
    let max_height = theme::detail_header_height() + rows * pitch;
    via_table_ui(
        ui,
        via,
        *toggles.stations_only,
        stops,
        train_index,
        max_height,
        "inline",
    );
}

/// 窄屏时停站表上方的经由入口：整条可点，打开独立经由页。
///
/// 行高、悬停底色、上下两条细线都在 `entry_bar_ui` 里，和宽屏那条「经由 + 开行计划」
/// 入口用的是同一个格子。调用方前后只要各留 `GAP_S`，**不要**再补 `ui.separator()`
/// ——那样会变成两条线，且入口看上去离上面的细线远、离下面的近。
fn via_page_trigger(ui: &mut Ui, via: &[ViaPoint], open: &mut bool) {
    if via.is_empty() {
        return;
    }
    let summary = via_entry_summary(via);
    let entries = [Entry {
        id: "via-page",
        title: "经由",
        summary: &summary,
        hint: "查看",
        arrow: None,
        width: None,
        hover: "打开经由页",
    }];
    if entry_bar_ui(ui, &entries).is_some() {
        *open = true;
    }
}

/// 独立经由页。
///
/// 窄屏时覆盖整个内容区，给经由表足够高度；宽屏时封顶 760×82% 居中显示。
/// 页面用 `Modal` 绘制：背景变暗并拦截下层输入，点返回/关闭、点遮罩或按 Esc 关闭。
/// 窗口尺寸变化时由调用方关闭，避免模态停在旧位置。
fn via_page_ui(
    ui: &mut Ui,
    detail: &TrainDetail,
    extras: &TrainExtras,
    stations_only: &mut bool,
    open: &mut bool,
    compact: bool,
    screen: egui::Vec2,
) {
    let ctx = ui.ctx().clone();
    // 触摸屏（手机）走更精简的抬头与可换行的线路列。
    let touch = theme::pointer() == theme::Pointer::Touch;
    // 窄屏留 12 点边距，基本就是整页；宽屏改成封顶 760×82% 的居中弹窗。
    let page = if compact {
        (screen - egui::vec2(24.0, 24.0)).max(egui::vec2(240.0, 240.0))
    } else {
        egui::vec2(
            (screen.x - 24.0).min(760.0),
            ((screen.y - 24.0) * 0.82).max(320.0),
        )
    };
    let frame = egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(12);
    // Area 会按 id 记住上一帧的尺寸（`AreaState::size`）。横屏开过经由再转回
    // 竖屏时，如果 id 不变就会沿用横屏那份 760 点的宽度，弹窗直接溢出屏幕——
    // 「转屏时先关掉」也挡不住，因为下次打开还是同一个 id。所以把屏幕尺寸编进 id：
    // 每种尺寸各算各的尺寸状态，横竖屏互不影响。
    let size_key = (page.x.round() as i32, page.y.round() as i32);
    let inner = page - egui::vec2(24.0, 24.0);
    let response = egui::Modal::new(egui::Id::unique(("llt-via-page", compact, size_key)))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(120))
        .show(&ctx, |ui| {
            // 只给下限，别再压上限：`set_max_width` 会把内容钉在「屏宽 - 48」
            // 上，正好比经由表「显示里程列」的阈值（300 点）少半格，里程列就没了。
            ui.set_min_size(inner);
            // 抬头压成两行：`返回 经由 车次 路由` 一行，`点数 · 里程 · 只看车站 · 图例` 一行。
            // 这几行原本各占一整行（44 点），表格因此少了将近两行的显示高度。
            let route = format!("{} → {}", detail.summary.first, detail.summary.last);
            ui.horizontal(|ui| {
                ui.spacing_mut().interact_size.y = theme::header_height();
                // 「只看车站」固定在右端：先把它的宽度留出来，左边标题区才不会
                // 被截断的站名盖住（`with_layout(right_to_left)` 是叠加，不会挤）。
                let reserve = 104.0;
                let title_width = (ui.available_width() - reserve).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(title_width, theme::header_height()),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(if compact { "返回" } else { "关闭" })
                                        .size(theme::FONT_BODY),
                                )
                                .min_size(egui::vec2(56.0, theme::header_height())),
                            )
                            .clicked()
                        {
                            *open = false;
                        }
                        ui.add_space(theme::GAP_S);
                        // Android：抬头只留粗体车次（路由在表格里已经写全，不必重复）；
                        // 桌面保持「经由 + 车次 + 路由」的完整抬头。
                        if touch {
                            ui.label(bold(&detail.summary.code).size(theme::FONT_SECTION));
                        } else {
                            ui.label(bold("经由").size(theme::FONT_SECTION));
                            ui.add_space(theme::GAP_S);
                            ui.label(bold(&detail.summary.code).size(theme::FONT_BODY));
                            ui.add_space(theme::GAP_XS);
                            ui.add(egui::Label::new(meta(&route)).truncate())
                                .on_hover_text(&route);
                        }
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().interact_size.y = theme::chip_height();
                    ui.checkbox(stations_only, meta("只看车站"));
                });
            });
            ui.add_space(2.0);
            // 只留图例：第二行原来还写「61点 · 1463km · 车站 38」，可这三个数
            // 表里数得出来，抬头上也已经只有车次，留着只是把表格往下压一行。
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                via_legend_ui(ui);
            });
            ui.add_space(theme::GAP_XS);
            // 整页也按整行取整，避免首屏最下面出现半截行。
            let height = ui.available_height();
            let pitch = theme::detail_row_height() + ui.spacing().item_spacing.y;
            let rows = ((height - theme::detail_header_height()) / pitch)
                .floor()
                .max(1.0);
            let max_height = theme::detail_header_height() + rows * pitch;
            via_table_ui(
                ui,
                &extras.via,
                *stations_only,
                &detail.stops,
                detail.summary.index,
                max_height,
                "page",
            );
        });
    if response.backdrop_response.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *open = false;
    }
}

/// 两级空状态：标题 + 支持说明，纵向放在内容区偏上位置，避免大面积空白。
fn empty_state(ui: &mut Ui, title: &str, hint: &str) {
    let top = ((ui.available_height() - 80.0) * 0.34).clamp(32.0, 180.0);
    ui.add_space(top);
    ui.vertical_centered(|ui| {
        ui.label(bold(title).size(theme::FONT_SECTION));
        ui.add_space(theme::GAP_S);
        ui.label(RichText::new(hint).size(theme::FONT_HINT).weak());
    });
}

/// 表格单元格统一布局：水平靠左、垂直居中。
///
/// `egui_extras::TableBuilder` 的单元格默认继承外层 `ui.layout()`，在
/// 垂直面板里就是 `top_down(Align::Min)`，会让表头和内容都贴在 cell 左上角；
/// 显式指定这个布局后，单行的 `ui.label` 也会垂直居中。
/// 表格里的可跳转文字：常态用链接色，悬停补一条下划线 + 手型光标。
///
/// 用 `Label` 而不是 `egui::Link`，是为了保留 `.truncate()`——
/// 窄列里的长站名要截断，链接不能把列撑破。
fn link_cell(ui: &mut Ui, text: RichText, tip: impl Into<egui::WidgetText>) -> bool {
    let color = ui.visuals().hyperlink_color;
    let label = ui.add(
        egui::Label::new(text.color(color))
            .truncate()
            .selectable(false),
    );
    // 只有字形本身是入口；命中区向外放宽一点（鼠标 2/4，触摸 6/8），
    // 免得「缩小热区」变成「点不中」。放宽量仍小于行高的一半，不会串到相邻行。
    let pad = theme::link_hot_pad();
    let hit = label.rect.expand2(pad);
    let response = ui
        .interact(hit, ui.next_auto_id(), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip);
    if response.hovered() || response.has_focus() {
        let rect = label.rect;
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 1.0,
            egui::Stroke::new(1.0, color),
        );
    }
    response.clicked()
}

fn cell_layout() -> Layout {
    Layout::left_to_right(Align::Center)
}

/// 一张表的列清单：上半是已显示的列（按用户顺序），下半是没显示的列。
///
/// `picked = None`（自动）时清单显示当前宽度下自动挑的列；任何勾选/拖动都会切成
/// 自定义，并照眼前这份加减。勾选把列排到已显示块的末尾；取消勾选把它送到清单最后；
/// 拖动只在已显示的行之间换顺序（没显示的行没有拖动把手）。
fn column_list<C: Copy + PartialEq + Send + Sync + 'static>(
    ui: &mut Ui,
    salt: &'static str,
    picked: &mut Option<Vec<C>>,
    auto: Vec<C>,
    all: &[C],
    spec: fn(C) -> columns::Spec<C>,
    compact: bool,
) {
    if compact {
        // 弹窗里行距再紧一档：13 行一屏能放下，少滚一次。
        ui.spacing_mut().item_spacing.y = 2.0;
    }
    let visible: Vec<C> = picked.clone().unwrap_or(auto);

    // 顺序：显示中的列先按用户顺序排；没显示的按声明顺序兜底。之后整份顺序都会
    // 被拖动改写并存起来，所以「没显示」的列也可以先拖到想要的位置再勾上。
    let order_id = egui::Id::unique((salt, "order"));
    let mut order: Vec<C> = ui.data(|data| data.get_temp(order_id)).unwrap_or_default();
    order.retain(|column| all.contains(column));
    for (index, column) in visible.iter().enumerate() {
        if order.contains(column) {
            continue;
        }
        // 新出现的列（比如窗口变宽后自动加进来的）接在上一个可见列后面。
        let at = if index == 0 {
            0
        } else {
            order
                .iter()
                .position(|other| other == &visible[index - 1])
                .map_or(order.len(), |position| position + 1)
        };
        order.insert(at.min(order.len()), *column);
    }
    for column in all {
        if !order.contains(column) {
            order.push(*column);
        }
    }

    // 「自动」行：勾着表示按窗口宽度挑列；点它清掉自定义。已经是自动时点它不做事，
    // 想改成自定义直接勾选/拖动任意一列即可。
    let auto_on = picked.is_none();
    if column_row(ui, "自动", auto_on, false, false, compact)
        .response
        .clicked()
        && !auto_on
    {
        *picked = None;
    }

    let drag_id = egui::Id::unique((salt, "drag"));
    // 存的是 `Option<usize>`，取出来也是 `Option<Option<usize>>`，要再摊平一次。
    let mut drag_from: Option<usize> = ui
        .data(|data| data.get_temp::<Option<usize>>(drag_id))
        .flatten();
    let mut drag_started = None;
    let mut row_rects: Vec<egui::Rect> = Vec::new();
    let mut toggled: Option<C> = None;

    for (index, column) in order.iter().enumerate() {
        let checked = visible.contains(column);
        let cell = column_row(
            ui,
            spec(*column).head,
            checked,
            checked,
            drag_from == Some(index),
            compact,
        );
        row_rects.push(cell.rect);
        if cell.response.drag_started() {
            drag_started = Some(index);
        }
        if cell.response.clicked() {
            toggled = Some(*column);
        }
    }

    if let Some(column) = toggled {
        if visible.contains(&column) {
            // 取消勾选：至少留一列，最后一列点了就当没点。取消的列排到清单最后，
            // 想再勾回来也能一眼找到；显示中的列顺序保持不变。
            if visible.len() > 1 {
                order.retain(|other| other != &column);
                order.push(column);
                *picked = Some(
                    order
                        .iter()
                        .copied()
                        .filter(|other| visible.contains(other) && other != &column)
                        .collect(),
                );
            }
        } else {
            // 勾选：排到已显示块的末尾（即清单里第一个没显示的项之前）。
            order.retain(|other| other != &column);
            let at = visible.len().min(order.len());
            order.insert(at, column);
            *picked = Some(
                order
                    .iter()
                    .copied()
                    .filter(|other| visible.contains(other) || other == &column)
                    .collect(),
            );
        }
    } else {
        if drag_from.is_none() {
            drag_from = drag_started;
        }
        if let Some(from) = drag_from {
            if ui.input(|input| input.pointer.any_down()) {
                if let Some(pointer) = ui.ctx().pointer_latest_pos() {
                    // 只有指针越过相邻行的中线才换位：拖着自己那行的下半边不会误触发。
                    let mut to = from;
                    for (index, rect) in row_rects.iter().enumerate() {
                        let center = rect.center().y;
                        if index > from && pointer.y > center {
                            to = index;
                        } else if index < from && pointer.y < center {
                            to = index;
                            break;
                        }
                    }
                    if to != from && from < order.len() {
                        let column = order.remove(from);
                        // 只在已显示块内换位：拖出这块就贴到已显示列的末尾。
                        let to = to.min(visible.len().saturating_sub(1)).min(order.len());
                        order.insert(to, column);
                        *picked = Some(
                            order
                                .iter()
                                .copied()
                                .filter(|other| visible.contains(other))
                                .collect(),
                        );
                        drag_from = Some(to);
                    }
                }
            } else {
                drag_from = None;
            }
        }
    }

    ui.data_mut(|data| data.insert_temp(order_id, order));
    ui.data_mut(|data| data.insert_temp(drag_id, drag_from));
}

/// 列清单的一行：左边拖动把手 + 复选框 + 名称。
///
/// 整行一个 `click_and_drag` 命中区：触摸时点一下切换显示、按住拖动换顺序，
/// 不用瞄着那个小复选框点。隐藏的列不可拖（顺序只对显示中的列有意义）。
fn column_row(
    ui: &mut Ui,
    label: &str,
    checked: bool,
    draggable: bool,
    dragging: bool,
    compact: bool,
) -> ColumnRow {
    let height = if compact {
        theme::popup_column_row_height()
    } else {
        theme::column_row_height()
    };
    let sense = if draggable {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), sense);

    let fill = if dragging {
        Some(ui.visuals().selection.bg_fill)
    } else if response.hovered() {
        Some(ui.visuals().widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 6.0, fill);
    }

    let check = if compact {
        theme::popup_checkbox_size()
    } else {
        theme::checkbox_size()
    };
    let pad = 6.0;
    let grip = if draggable { check } else { 0.0 };
    let box_rect = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + pad + grip + check * 0.5 + if draggable { 6.0 } else { 0.0 },
            rect.center().y,
        ),
        egui::vec2(check, check),
    );

    // 拖动把手：六个小点，比一条横线更像「可抓」，触摸时也看得清。
    if draggable {
        let color = if dragging {
            theme::accent(ui)
        } else {
            theme::meta_color(ui)
        };
        let dot = if dragging { 2.0 } else { 1.6 };
        let center_x = rect.left() + pad + grip * 0.5;
        for row in 0..3 {
            for col in 0..2 {
                let offset = egui::vec2(
                    (col as f32 - 0.5) * dot * 4.0,
                    (row as f32 - 1.0) * dot * 4.0,
                );
                ui.painter().circle_filled(
                    egui::pos2(center_x, rect.center().y) + offset,
                    dot,
                    color,
                );
            }
        }
    }

    if checked {
        ui.painter().rect_filled(box_rect, 4.0, theme::accent(ui));
        let stroke = egui::Stroke::new((check * 0.11).max(1.5), theme::on_accent_text(ui));
        let point =
            |x: f32, y: f32| egui::pos2(box_rect.left() + check * x, box_rect.top() + check * y);
        ui.painter()
            .line_segment([point(0.22, 0.52), point(0.42, 0.72)], stroke);
        ui.painter()
            .line_segment([point(0.42, 0.72), point(0.78, 0.30)], stroke);
    } else {
        ui.painter().rect_stroke(
            box_rect,
            4.0,
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
    }

    let color = if checked {
        ui.visuals().strong_text_color()
    } else {
        theme::meta_color(ui)
    };
    ui.painter().text(
        egui::pos2(box_rect.right() + theme::GAP_S, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(theme::FONT_BODY),
        color,
    );

    ColumnRow { response, rect }
}

struct ColumnRow {
    response: egui::Response,
    rect: egui::Rect,
}

/// 输入框下方的候选站下拉列表（车站页与站站页共用）。
///
/// 候选多的时候这比在输入框下面铺一排 chip 省得多：chip 排横着换行，单字查询的 24 个
/// 候选在 393 点的手机上要 4 行、347 点上要 5 行（150–190 点，竖屏结果区的两到三成）。
/// 列表浮在结果上面，最多 8 行、再多自己滚，选完就收。
///
/// 开合交给 egui 的 popup 记忆：点输入框 / 打字时打开，点别处或 Esc 关闭
/// （`CloseOnClickOutside` 不会因为点列表里的项而关掉，挑选由这里处理）。
/// 返回用户点中的那个站名。
fn candidates_popup(
    ui: &mut Ui,
    input: &egui::Response,
    salt: &'static str,
    stations: &[String],
    current: Option<&str>,
) -> Option<String> {
    if stations.is_empty() {
        return None;
    }
    let popup_id = ui.make_persistent_id(salt);
    let mut open_command = None;
    if input.clicked() || input.changed() {
        open_command = Some(egui::SetOpenCommand::Bool(true));
    }
    let width = input.rect.width().max(200.0);
    let row_height = theme::list_row_height();
    // 高度上限按窗口高度给：手机上键盘还占着下半屏，列表再高就顶到键盘上沿了。
    // 一头一尾各留 4 行打底（候选少时不要缩成一条），最多 8 行左右。
    let max_height =
        (ui.ctx().content_rect().height() * 0.35).clamp(row_height * 4.0, row_height * 8.0);
    let mut picked = None;
    egui::Popup::from_response(input)
        .id(popup_id)
        .open_memory(open_command)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(theme::GAP_XS)
        .frame(egui::Frame::popup(ui.style()))
        .show(|ui| {
            ui.set_min_width(width);
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for station in stations {
                        let selected = current == Some(station.as_str());
                        // 整行可点、整行高亮：`frame_when_inactive(selected)` 让未选中的
                        // 行平时不画框，悬停/选中才有底色。
                        let row = egui::Button::new(RichText::new(station).size(theme::FONT_BODY))
                            .selected(selected)
                            .frame_when_inactive(selected)
                            .min_size(egui::vec2(ui.available_width(), row_height));
                        if ui.add(row).clicked() {
                            picked = Some(station.clone());
                        }
                    }
                });
        });
    if picked.is_some() {
        // 选完就结束编辑：键盘与列表一起收起（否则新查询又会带出新的候选）。
        ui.ctx().memory_mut(|memory| memory.stop_text_input());
        egui::Popup::close_id(ui.ctx(), popup_id);
    }
    picked
}

/// 一张「横竖都能滚」的列表表格：外层负责滚动，表格只负责铺行列。
///
/// `TableBuilder` 自带的滚动区只有纵向：列宽超出视口时右边直接被裁掉，手势也拖不出
/// 横向滚动；而「外层管横向 + 内层管纵向」两个 `ScrollArea` 也不行——内层会把手指拖动
/// 整段吃掉。所以四张表统一成：外层一个两向 `ScrollArea` 负责滚动，表格自己
/// `drag_to_scroll(Never)`，列宽由 [`resizable_columns`] 铺（初始宽度按可用宽度算，
/// 用户可以拖分隔线改）。
///
/// `height` 是内容高度（表头 + 全部行）；`build` 拿到的是**内容可用宽度**——已经扣掉
/// 滚动条，按它算列宽才不会因为多算 8 点而凭空多出一条横向滚动条。
fn scrollable_table<R>(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    height: f32,
    header_height: f32,
    build: impl FnOnce(&mut Ui, f32) -> R,
) -> R {
    // 滚动条做成「浮动 + 细」：平时不占位、几乎看不见，拖动时才浮出来一点。
    // 注意必须**继承**当前 style 再改宽度：新建一份 `ScrollStyle::floating()`
    // 会把 `kinetic.rubber_band` 一并拉回默认的「开」，边界橡皮筋就又回来了。
    let mut scroll_style = ui.spacing().scroll;
    scroll_style.bar_width = 8.0;
    scroll_style.floating_width = 4.0;
    // `scope` 让样式改动只活在这个滚动区里，不污染同一页后面的控件。
    ui.scope(|ui| {
        ui.style_mut().spacing.scroll = scroll_style;
        egui::ScrollArea::both()
            .id_salt((salt, "scroll"))
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                let width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(width, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let header_top = ui.cursor().top();
                        let value = build(ui, width);
                        paint_header_line(ui, header_top, header_height);
                        value
                    },
                )
                .inner
            })
            .inner
    })
    .inner
}

/// 算每列宽度与内容总宽。
///
/// 自然宽度之和小于可用宽度时，多出来的空间按 `grow` 分给文本列（站名、区段）；
/// 一个可长宽的列都没有时全给最后一列。放不下则保持自然宽度，交给外层横向滚动——
/// 时刻和站名宁可横着滑，也不挤成省略号。
fn column_widths<C>(specs: &[columns::Spec<C>], available: f32, spacing: f32) -> (Vec<f32>, f32) {
    let gaps = spacing * specs.len().saturating_sub(1) as f32;
    let natural: f32 = specs.iter().map(|spec| spec.width).sum::<f32>() + gaps;
    let mut widths: Vec<f32> = specs.iter().map(|spec| spec.width).collect();
    let slack = available - natural;
    if slack > 0.0 && !widths.is_empty() {
        let growers: u32 = specs.iter().map(|spec| u32::from(spec.grow)).sum();
        if growers == 0 {
            if let Some(last) = widths.last_mut() {
                *last += slack;
            }
        } else {
            for (width, spec) in widths.iter_mut().zip(specs) {
                if spec.grow > 0 {
                    *width += slack * f32::from(spec.grow) / growers as f32;
                }
            }
        }
    }
    let total = widths.iter().sum::<f32>() + gaps;
    (widths, total.max(available))
}

/// 按列宽铺列。宽度由 [`table_widths`] 统一管，拖动改的是那份宽度。
fn exact_columns<'a>(mut table: TableBuilder<'a>, widths: &[f32]) -> TableBuilder<'a> {
    for width in widths {
        table = table.column(Column::exact(*width));
    }
    table
}

/// 有「列宽记忆」的四张表：配置里按 od / train / station / stops 分开存，
/// 界面状态里按绘制用的 `salt` 分开存，两边靠这张表对应。
#[derive(Debug, Clone, Copy)]
enum WidthTable {
    Od,
    Train,
    Station,
    Stops,
}

impl WidthTable {
    const ALL: [Self; 4] = [Self::Od, Self::Train, Self::Station, Self::Stops];

    /// 绘制这些表时用的 `salt`（与 `table_widths` 的入参一致）。
    fn salt(self) -> &'static str {
        match self {
            Self::Od => "od_results",
            Self::Train => "train_results",
            Self::Station => "station_board",
            Self::Stops => "train_stops",
        }
    }

    fn get(self, widths: &config::ColumnWidths) -> Option<&config::ColumnWidth> {
        match self {
            Self::Od => widths.od.as_ref(),
            Self::Train => widths.train.as_ref(),
            Self::Station => widths.station.as_ref(),
            Self::Stops => widths.stops.as_ref(),
        }
    }

    fn set(self, widths: &mut config::ColumnWidths, value: Option<config::ColumnWidth>) {
        match self {
            Self::Od => widths.od = value,
            Self::Train => widths.train = value,
            Self::Station => widths.station = value,
            Self::Stops => widths.stops = value,
        }
    }
}

/// 把配置里记住的列宽塞回界面状态，并记下「是按哪套布局量的」。
fn seed_column_widths(ctx: &egui::Context, widths: &config::ColumnWidths) {
    for table in WidthTable::ALL {
        let Some(saved) = table.get(widths) else {
            continue;
        };
        if saved.widths.is_empty() {
            continue;
        }
        let values: Vec<f32> = saved.widths.iter().map(|width| f32::from(*width)).collect();
        let viewport = saved.viewport;
        let salt = table.salt();
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::unique((salt, "widths")), values);
            data.insert_temp(egui::Id::unique((salt, "widths_seeded")), viewport);
        });
    }
}

/// 每张表的列宽。
///
/// 首次、可视宽度变化、列集合变化时按内容重算；其余时候保留用户拖出来的宽度
/// （存在 egui 界面状态里，按 `salt` 区分；也会写进配置，下次启动照旧）。
/// `computed` 是这一帧算出的自适应宽度。
fn table_widths<C: Copy>(
    ui: &Ui,
    salt: &str,
    specs: &[columns::Spec<C>],
    computed: Vec<f32>,
    key: fn(C) -> &'static str,
) -> Vec<f32> {
    let id = egui::Id::unique((salt, "widths"));
    let keys_id = egui::Id::unique((salt, "widths_keys"));
    let seeded_id = egui::Id::unique((salt, "widths_seeded"));
    let keys: Vec<&'static str> = specs.iter().map(|spec| key(spec.column)).collect();
    let previous: Option<Vec<&'static str>> = ui.data(|data| data.get_temp(keys_id));
    ui.data_mut(|data| data.insert_temp(keys_id, keys.clone()));
    // 启动时从配置恢复的那一份：只有「按同一套布局量的」才认，认完就清掉标记，
    // 之后窗口尺寸再变照旧重新自适应。
    let viewport = column_viewport(ui);
    let seeded = ui.data(|data| {
        data.get_temp::<i32>(seeded_id)
            .is_some_and(|saved| (saved - viewport).abs() <= VIEWPORT_SLACK)
            && data
                .get_temp::<Vec<f32>>(id)
                .is_some_and(|widths| widths.len() == specs.len())
    });
    ui.data_mut(|data| data.remove::<i32>(seeded_id));
    let changed = column_layout_changed(ui, salt);
    let stored: Option<Vec<f32>> = ui.data(|data| data.get_temp(id));
    let stored = stored.filter(|widths| widths.len() == specs.len());
    let stored = stored.and_then(|widths| {
        // 列序变了但列还是那几列（表头拖动、设置页拖清单）：宽度跟着自己那一列走，
        // 不能整份丢掉——用户刚调好的宽度不能因为换个位置就回到默认。
        // 真换了列（取消一个又勾上一个）就对不上号了，交给下面重算。
        if let Some(previous) = previous.filter(|previous| *previous != keys) {
            let same_set =
                previous.len() == keys.len() && keys.iter().all(|key| previous.contains(key));
            if !same_set {
                return None;
            }
            let moved: Vec<f32> = keys
                .iter()
                .map(|key| {
                    let index = previous.iter().position(|old| old == key).unwrap_or(0);
                    widths.get(index).copied().unwrap_or_default()
                })
                .collect();
            ui.data_mut(|data| data.insert_temp(id, moved.clone()));
            return Some(moved);
        }
        Some(widths)
    });
    match stored {
        Some(widths) if !changed || seeded => widths,
        _ => {
            ui.data_mut(|data| data.insert_temp(id, computed.clone()));
            computed
        }
    }
}

/// 指针按下到现在多久（秒）。调宽与表头换序共用一份计时。
///
/// egui 没有公开 `press_start_time()`，自己按 `salt` 记一笔。
fn pointer_held(ui: &Ui, salt: &str) -> f64 {
    let press_id = egui::Id::unique((salt, "resize_press_at"));
    let now = ui.input(|input| input.time);
    if ui.input(|input| input.pointer.any_pressed()) {
        ui.data_mut(|data| data.insert_temp(press_id, now));
    } else if !ui.input(|input| input.pointer.any_down()) {
        ui.data_mut(|data| data.remove::<f64>(press_id));
    }
    ui.data(|data| data.get_temp::<f64>(press_id))
        .map_or(0.0, |start| now - start)
}

/// 表头文字水平拖动换列序：指针越过相邻列的中线就把这两列换位。
///
/// 换序结果只写进界面状态（`("column_order", salt)`），由 `LltApp` 下一帧落回列设置——
/// 表格这边只认「这一帧的列序」，不直接改配置。触摸屏和调宽一样要先按住
/// [`RESIZE_LONG_PRESS`] 秒，否则从表头起手的横拖本来是滚动。
fn table_header_drag<C: Copy>(
    ui: &mut Ui,
    salt: &str,
    specs: &[columns::Spec<C>],
    cells: &[egui::Rect],
    key: fn(C) -> &'static str,
    movable: fn(C) -> bool,
) {
    let touch = theme::pointer() == theme::Pointer::Touch;
    let armed = !touch || pointer_held(ui, salt) >= RESIZE_LONG_PRESS;
    let drag_id = egui::Id::unique((salt, "head_drag"));
    // 存的是 `Option<usize>`，取出来也是 `Option<Option<usize>>`，要再摊平一次。
    let mut dragged: Option<usize> = ui.data(|data| data.get_temp(drag_id)).flatten();
    if !ui.input(|input| input.pointer.any_down()) {
        dragged = None;
    }
    for (index, spec) in specs.iter().enumerate() {
        if !movable(spec.column) {
            continue;
        }
        let Some(cell) = cells.get(index).copied() else {
            continue;
        };
        // id 用列名而不是下标：换完序这列会挪到别的格子，拖动不能断。
        let response = ui.interact(
            cell,
            ui.make_persistent_id((salt, "head", key(spec.column))),
            Sense::click_and_drag(),
        );
        if response.dragged() && armed {
            ui.set_cursor_icon(egui::CursorIcon::Grabbing);
            dragged = Some(index);
        } else if response.hovered() && armed {
            ui.set_cursor_icon(egui::CursorIcon::Grab);
        }
    }
    let pointer = ui.input(|input| input.pointer.interact_pos());
    if let (Some(index), Some(pointer)) = (dragged, pointer) {
        // 只跟紧挨着的那一列换：越过它的中线才动手，原地小幅抖动不会误触发。
        let movable_at = |target: usize| specs.get(target).is_some_and(|spec| movable(spec.column));
        let mut target = None;
        if let Some(next) = cells.get(index + 1) {
            if pointer.x > next.center().x && movable_at(index + 1) {
                target = Some(index + 1);
            }
        }
        if target.is_none() {
            if let Some(previous) = index.checked_sub(1) {
                if pointer.x < cells[previous].center().x && movable_at(previous) {
                    target = Some(previous);
                }
            }
        }
        if let Some(target) = target {
            let mut order: Vec<String> = specs
                .iter()
                .map(|spec| key(spec.column).to_owned())
                .collect();
            order.swap(index, target);
            ui.data_mut(|data| {
                data.insert_temp(egui::Id::unique((salt, "column_order")), order);
            });
            dragged = Some(target);
        }
    }
    ui.data_mut(|data| data.insert_temp(drag_id, dragged));
}

/// 正在拖动的那一列（表头加粗、分隔线加粗用）。读的是上一帧记下的结果。
fn table_active_column(ui: &Ui, salt: &str) -> Option<usize> {
    let read = |key: &str| {
        ui.data(|data| data.get_temp::<Option<usize>>(egui::Id::unique((salt, key))))
            .flatten()
    };
    // 调宽、换序都算「正在动这一列」：表头加粗变强调色，分隔线也加粗——
    // 手指按住不放时得看得出在改哪一列。都是上一帧记下的结果（绘制在写入之前）。
    read("resize_active").or_else(|| read("head_drag"))
}

/// 拖列宽时，在**画表之前**把这一帧的指针位移落进列宽。
///
/// 拖动的判定本来在表画完之后（要表的实际几何），可那样列宽永远晚一帧生效：
/// 手指快速滑动时，分隔线（下一帧才挪过去）会明显跟不上手指。这里先按锚点算一遍，
/// 表就按新宽度画；`table_resize_handles` 里再算一次只是补上「松手那一帧」
/// （那时表已经按旧宽度画完了），同一个绝对公式，重复应用无副作用。
fn apply_resize_drag<C: Copy>(
    ui: &Ui,
    salt: &str,
    specs: &[columns::Spec<C>],
    widths: &mut [f32],
    resizable: fn(C) -> bool,
) {
    let anchor: Option<ResizeAnchor> = ui
        .data(|data| data.get_temp(egui::Id::unique((salt, "resize_anchor"))))
        .flatten();
    let Some(anchor) = anchor else {
        return;
    };
    let Some(spec) = specs
        .get(anchor.index)
        .filter(|spec| resizable(spec.column))
    else {
        return;
    };
    let touch = theme::pointer() == theme::Pointer::Touch;
    if touch && pointer_held(ui, salt) < RESIZE_LONG_PRESS {
        return;
    }
    let (pointer, down, released) = ui.input(|input| {
        (
            input.pointer.interact_pos(),
            input.pointer.any_down(),
            input.pointer.any_released(),
        )
    });
    let Some(pointer) = pointer else {
        return;
    };
    if !down && !released {
        return;
    }
    if let Some(width) = widths.get_mut(anchor.index) {
        let wanted = (anchor.width + (pointer.x - anchor.pointer_x)).max(spec.min);
        if (*width - wanted).abs() > 0.01 {
            *width = wanted;
            ui.data_mut(|data| {
                data.insert_temp(egui::Id::unique((salt, "widths")), widths.to_vec());
            });
        }
    }
}

/// 拖列宽时的锚点：按下那一刻在哪一列、当时多宽、指针在哪。
#[derive(Debug, Clone, Copy)]
struct ResizeAnchor {
    index: usize,
    width: f32,
    pointer_x: f32,
}

/// 列分隔线的拖动调整。
///
/// 触摸屏上只在**表头**那一条横带里认分隔线，而且要先按住约 [`RESIZE_LONG_PRESS`]
/// 秒才进入调整：手指从行上开始拖动是滚动，不能因为擦到列边就把列宽改了。
/// 鼠标没有长按限制，整根分隔线都能拖。
///
/// 拖动中的那一列表头加粗、分隔线加粗成强调色。
fn table_resize_handles<C: Copy>(
    ui: &mut Ui,
    salt: &str,
    specs: &[columns::Spec<C>],
    widths: &mut [f32],
    edges: &[f32],
    first_row: Option<egui::Rect>,
    header_height: f32,
    resizable: fn(C) -> bool,
) {
    let Some(first_row) = first_row else {
        return;
    };
    let touch = theme::pointer() == theme::Pointer::Touch;
    let line_bottom = ui.min_rect().bottom();
    let spacing = ui.spacing().item_spacing.x;
    let grab = theme::resize_grab_radius();
    // 只有表头那一条横带能调宽：行上的拖动（哪怕贴着分隔线）全部留给外层
    // 滚动区，表格该横滚/竖滚照常。触摸时还要先按住约 0.4 秒。
    // 表头顶从第一行上沿反推——第一行的命中坐标是准的（点行是好的）。
    let top = first_row.top() - header_height;
    let band_bottom = first_row.top();

    let armed = !touch || pointer_held(ui, salt) >= RESIZE_LONG_PRESS;

    let idle = ui.visuals().widgets.noninteractive.bg_stroke;
    let hot = ui.visuals().widgets.active.bg_stroke;
    let widths_id = egui::Id::unique((salt, "widths"));
    let anchor_id = egui::Id::unique((salt, "resize_anchor"));
    // 按下那一刻记住「哪一列、多宽、指针在哪」，之后按**绝对位移**算宽度：
    // 只累加 `pointer.delta()` 有两个毛病——触摸屏上按住的那 0.4 秒里移动的部分
    // 会被整段丢掉（列宽比手指走过的距离少一截），松手那一帧的位移也来不及算。
    let pointer = ui.input(|input| input.pointer.interact_pos());
    let mut anchor: Option<ResizeAnchor> = ui.data(|data| data.get_temp(anchor_id)).flatten();
    let mut active = None;
    for (index, spec) in specs.iter().enumerate() {
        let center = edges.get(index).copied().unwrap_or_default() + spacing * 0.5;
        if !resizable(spec.column) {
            continue;
        }
        let rect = egui::Rect::from_min_max(
            egui::pos2(center - grab, top),
            egui::pos2(center + grab, band_bottom),
        );
        let response = ui.interact(
            rect,
            ui.make_persistent_id((salt, "resize", index)),
            Sense::click_and_drag(),
        );
        if response.hovered() {
            ui.set_cursor_icon(egui::CursorIcon::ResizeColumn);
        }
        if response.is_pointer_button_down_on() && anchor.is_none() {
            anchor = Some(ResizeAnchor {
                index,
                width: widths[index],
                pointer_x: pointer.map_or(0.0, |pointer| pointer.x),
            });
        }
        // 松手那一帧也算：`dragged()` 已经为假，但最后一段位移还没落进宽度里。
        let dragging = (response.dragged() || response.drag_stopped())
            && armed
            && anchor.is_some_and(|anchor| anchor.index == index);
        if dragging {
            active = Some(index);
            let anchor = anchor.expect("dragging 蕴含 anchor");
            if let Some(pointer) = pointer {
                // 松手那一帧也补一次：那时表已经按旧宽度画完了，这条记录留给下一帧。
                widths[index] = (anchor.width + (pointer.x - anchor.pointer_x)).max(spec.min);
                ui.data_mut(|data| data.insert_temp(widths_id, widths.to_vec()));
            }
        }
        if response.is_pointer_button_down_on() && armed {
            active = Some(index);
        }
        if response.double_clicked() {
            ui.data_mut(|data| data.remove::<Vec<f32>>(widths_id));
        }
        let stroke = if dragging {
            egui::Stroke::new(hot.width.max(2.0), theme::accent(ui))
        } else if response.hovered() || response.is_pointer_button_down_on() {
            hot
        } else {
            idle
        };
        // 平时分隔线只画在表头这一段（就是拖动的把手）；拖动中的那一列才恢复
        // 贯穿整个表格的强调色竖线，方便看清自己正在调哪一列的宽度。
        let line_end = if dragging { line_bottom } else { band_bottom };
        ui.painter().line_segment(
            [egui::pos2(center, top), egui::pos2(center, line_end)],
            stroke,
        );
    }
    if !ui.input(|input| input.pointer.any_down()) {
        anchor = None;
    }
    ui.data_mut(|data| data.insert_temp(anchor_id, anchor));
    ui.data_mut(|data| {
        data.insert_temp(egui::Id::unique((salt, "resize_active")), active);
    });
}

/// 表格的可视宽度。只在窗口 / 面板尺寸变化时变，滚动条、列宽拖动的变化不算。
fn column_viewport(ui: &Ui) -> i32 {
    (ui.clip_rect().width() * 2.0).round() as i32
}

/// 可视宽度变了多少才算「换了布局」（逻辑点 ×2）。
///
/// 浮动滚动条出现/消失只会让可视宽度差 6 点左右，窗口或面板真的变了才会超出这个量；
/// 差得少的时候不该把用户拖好的列宽丢掉。
const VIEWPORT_SLACK: i32 = 32;

/// 列宽由 `TableBuilder` 按 `id_salt` 记着，窗口变宽变窄时不会自己重新分配；
/// 这里按「可视宽度」记一笔，变化超过 [`VIEWPORT_SLACK`] 就返回 `true`，
/// 让调用方 `reset()` 重新自适应。
///
/// 判据不用内容宽度：拖动列宽或出现滚动条都会改变内容宽度，那会把用户刚拖好的
/// 列宽又重置掉。
fn column_layout_changed(ui: &Ui, salt: &str) -> bool {
    let viewport = column_viewport(ui);
    let id = egui::Id::unique(("table_viewport", salt));
    let changed = ui
        .data(|data| data.get_temp::<i32>(id))
        .is_none_or(|previous| (previous - viewport).abs() > VIEWPORT_SLACK);
    // 记最新的值（差得少也算更新），下次比较才不会一直拿旧值。
    ui.data_mut(|data| data.insert_temp(id, viewport));
    changed
}

/// 只查询「这一帧会不会重置列宽」，不写回。
///
/// 窄屏整页要先知道结果，再决定内容宽度：重置帧要用按视口算出的自然宽度，
/// 不然会把上一轮拖宽的表宽又带进新窗口。
fn column_layout_stale(ui: &Ui, salt: &str) -> bool {
    let id = egui::Id::unique(("table_viewport", salt));
    ui.data(|data| data.get_temp::<i32>(id))
        .is_none_or(|previous| (previous - column_viewport(ui)).abs() > VIEWPORT_SLACK)
}

/// 记下停站表实际铺开的宽度。
///
/// 窄屏整页只有一个两向滚动区，内容宽度由表宽决定；不跟着记的话，把列拖宽后
/// 表格右边会被页面裁掉。
fn remember_table_width(ui: &Ui, salt: &str, width: f32) {
    ui.data_mut(|data| data.insert_temp(egui::Id::unique(("table_width", salt)), width));
}

fn remembered_table_width(ui: &Ui, salt: &str) -> Option<f32> {
    ui.data(|data| data.get_temp::<f32>(egui::Id::unique(("table_width", salt))))
}

/// 表头格：`spec.right` 决定右对齐（数字列表头也跟着靠右）。
fn column_head<C>(ui: &mut Ui, spec: &columns::Spec<C>, hot: bool) {
    if hot {
        // 正在调宽的那一列：表头加粗 + 强调色，告诉手指改的是哪一列。
        let text = bold(spec.head)
            .size(theme::FONT_META)
            .color(theme::accent(ui));
        if spec.right {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(text);
            });
        } else {
            ui.label(text);
        }
    } else if spec.right {
        table_head_right(ui, spec.head);
    } else {
        table_head(ui, spec.head);
    }
}

/// 站站结果表的一格。
///
/// 车次的悬停里带上「区段 · 历时 · 全程」，这样用户只留几列时也不至于丢信息。
fn od_cell(ui: &mut Ui, column: columns::OdColumn, hit: &OdResult, running: bool) {
    use columns::OdColumn as Col;
    match column {
        Col::Status => status_cell(ui, running),
        Col::Code => {
            let tip = format!(
                "{} → {} · 历时 {}
全程：{} → {}",
                hit.from_station,
                hit.to_station,
                fmt_duration(hit.duration_min()),
                hit.first,
                hit.last
            );
            train_code_cell(ui, &hit.code, Some(tip.as_str()));
        }
        Col::From => station_name_cell(ui, &hit.from_station),
        // 出发日期就是查询日期，标「+N 天」只会让人以为上车不是当天。
        Col::Depart => time_cell(ui, &hit.depart, 0),
        Col::To => station_name_cell(ui, &hit.to_station),
        Col::Arrive => time_cell(ui, &hit.arrive, hit.arrive_day - hit.depart_day),
        Col::Duration => {
            let text = fmt_duration(hit.duration_min());
            ui.add(egui::Label::new(text.as_str()).truncate())
                .on_hover_text(text);
        }
        Col::Km => right_label(ui, format!("{} km", hit.km())),
        Col::Kind => {
            ui.add(egui::Label::new(&hit.kind).truncate());
        }
        Col::Segment => {
            let text = format!("{} → {}", hit.from_station, hit.to_station);
            ui.add(egui::Label::new(text.as_str()).truncate())
                .on_hover_text(format!(
                    "{text}
全程：{} → {}",
                    hit.first, hit.last
                ));
        }
        Col::Date => {
            ui.label(meta(date_window(hit.date_from, hit.date_to)));
        }
    }
}

/// 站名一格：窄列用省略号收尾，悬停看全名。
fn station_name_cell(ui: &mut Ui, name: &str) {
    ui.add(egui::Label::new(name).truncate())
        .on_hover_text(name);
}

/// 列表表头下方的细线：给表头一个明确的下边界，避免表头“浮”在数据行上。
fn paint_header_line(ui: &Ui, header_top: f32, height: f32) {
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .hline(ui.max_rect().x_range(), header_top + height, stroke);
}

/// 表格里的数值列右对齐，便于纵向比较。
fn right_label(ui: &mut Ui, text: impl Into<egui::WidgetText>) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(text);
    });
}

fn train_code_cell(ui: &mut Ui, code: &str, extra: Option<&str>) {
    let response = ui.add(egui::Label::new(bold(code).size(theme::FONT_BODY)).truncate());
    match extra {
        Some(extra) => response.on_hover_text(format!("{code} · {extra}")),
        None => response.on_hover_text(code),
    };
}

/// 始发 / 终到合并显示；窄列用省略号，悬停看全文。
fn route_cell(ui: &mut Ui, first: &str, last: &str) {
    let text = format!("{first} → {last}");
    ui.add(egui::Label::new(text.as_str()).truncate())
        .on_hover_text(&text);
}

/// 在合并后的列表上方显示两组数量；列表本身按“开行在前、不开行在后”排序。
fn show_counts(ui: &mut Ui, prefix: Option<String>, running: usize, not_running: usize) {
    let accent = theme::accent(ui);
    let weak = theme::meta_color(ui);
    // 计数只有一行文字，用比控件更矮的行高，避免结果列表上方出现多余留白。
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 22.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            if let Some(prefix) = prefix {
                ui.label(meta(prefix));
                ui.add_space(theme::GAP_XS);
                ui.label(RichText::new("·").weak());
                ui.add_space(theme::GAP_XS);
            }
            ui.label(RichText::new("●").size(10.0).color(accent));
            ui.label(meta(format!("当日开行 {running} 趟")));
            ui.add_space(theme::GAP_M);
            ui.label(RichText::new("○").size(10.0).color(weak));
            ui.label(meta(format!("当日不开行 {not_running} 趟")).weak());
        });
    });
    ui.add_space(theme::GAP_XS);
}

/// 底栏里的计数：`● 68 ○ 0`。两者之和就是总数，不再另写一遍「N 条」。
fn counts_label(ui: &mut Ui, total: usize, running: usize, not_running: usize) {
    if total == 0 {
        return;
    }
    // 调用方多半是 `right_to_left` 布局，这里再套一层从左到右，
    // 否则每个 label 都会往前插，读出来是反的（"3 ○ 76 ●"）。
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let accent = theme::accent(ui);
        let weak = theme::meta_color(ui);
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new("●").size(9.0).color(accent));
        ui.label(meta(running.to_string()));
        ui.label(RichText::new("○").size(9.0).color(weak));
        ui.label(meta(not_running.to_string()).weak());
    });
}

/// 合并列表中的状态圆点：● 当日开行，○ 当日不开行。
fn status_cell(ui: &mut Ui, running: bool) {
    let (symbol, color) = if running {
        ("●", ui.visuals().hyperlink_color)
    } else {
        ("○", ui.visuals().weak_text_color())
    };
    ui.centered_and_justified(|ui| {
        ui.label(RichText::new(symbol).size(12.0).color(color))
            .on_hover_text(if running {
                "当日开行"
            } else {
                "当日不开行"
            })
    });
}

#[cfg(test)]
mod swap_glyph_tests {
    use super::*;

    /// 「⇌」不能把交换按钮撑得比输入框高：两种指针各验一次。
    ///
    /// 这正是桌面端「输入框对不齐」的根因（写死 22 点字 + 按钮内边距 5+5 = 37 > 30）。
    #[test]
    fn swap_glyph_fits_the_button() {
        for pointer in [theme::Pointer::Mouse, theme::Pointer::Touch] {
            let ctx = egui::Context::default();
            theme::install(&ctx, pointer);
            let mut measured = None;
            let mut output = ctx.run_ui(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let size = theme::control_height();
                    let glyph = swap_glyph_size(ui, size);
                    let padding = 2.0 * ui.spacing().button_padding.y;
                    let row = ui
                        .ctx()
                        .fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(glyph)));
                    measured = Some((glyph, row + padding, size));
                });
            });
            // 字体纹理增量必须被消费掉，否则 epaint 的 TexturesDelta 在 drop 时 panic。
            output.textures_delta.clear();
            let (glyph, needed, size) = measured.expect("没画出界面");
            assert!(
                needed <= size,
                "{pointer:?}: 字形 {glyph} 点需要 {needed} 点，超过控件高度 {size}"
            );
            assert!(glyph >= 12.0, "{pointer:?}: 字形太小（{glyph}）");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(date_from: i64, date_to: i64) -> TrainSummary {
        TrainSummary {
            index: 1,
            code: "X1".to_owned(),
            kind: String::new(),
            first: String::new(),
            last: String::new(),
            total_km: 0,
            days: 1,
            date_from,
            date_to,
            rule_index: None,
        }
    }

    #[test]
    fn run_state_honours_period() {
        // 周期 7 天、掩码 0b1111：前四天开行，后三天停开。
        let on = lltdb::date::RunState::Running;
        let off = lltdb::date::RunState::Stopped;
        assert_eq!(
            lltdb::date::run_state(20261101, 20261130, 20261101, 7, 15),
            on
        );
        assert_eq!(
            lltdb::date::run_state(20261101, 20261130, 20261104, 7, 15),
            on
        );
        assert_eq!(
            lltdb::date::run_state(20261101, 20261130, 20261105, 7, 15),
            off
        );
        assert_eq!(
            lltdb::date::run_state(20261101, 20261130, 20261108, 7, 15),
            on
        );
        // 查询日期跨过区间两端。
        assert_eq!(
            lltdb::date::run_state(20261101, 20261130, 20261031, 7, 15),
            lltdb::date::RunState::NotYet
        );
        assert_eq!(
            lltdb::date::run_state(20261101, 20261130, 20261201, 7, 15),
            lltdb::date::RunState::Expired
        );
    }

    #[test]
    fn month_shifts_across_year() {
        assert_eq!(shift_month(202612, 1), 202701);
        assert_eq!(shift_month(202601, -1), 202512);
        assert_eq!(shift_month(202611, 0), 202611);
        assert_eq!(shift_month(202601, 12), 202701);
    }

    #[test]
    fn running_window_is_inclusive() {
        let train = summary(20261124, 20261125);
        assert!(!train.runs_on(20261123));
        assert!(train.runs_on(20261124));
        assert!(train.runs_on(20261125));
        assert!(!train.runs_on(20261126));
    }
}
