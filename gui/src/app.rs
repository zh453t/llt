//! egui 界面：车次 / 车站 / 站站三种查询。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::epaint::MarginF32;
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui};
use egui_extras::{Column, DatePickerButton, TableBuilder};
use jiff::civil::Date;

use crate::config::Tab;
use crate::{config, fonts, theme};
use lltdb::date::{date_window, date_ymd, origin_ymd, today, weekday_cn, ymd};
use lltdb::extras::{DiningCar, Duty, ViaPoint};
use lltdb::{Db, OdResult, StationStop, Stop, TrainDetail, TrainSummary};

const OD_RESULT_LIMIT: usize = 500;
/// 输入/点击停止多久之后才落盘：逐键写盘既浪费，也会把半截输入写进配置。
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
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
    db_path: PathBuf,
    db_error: Option<String>,
    /// 数据库里记录的数据包版本（`20261108`），供标题栏显示数据日期。
    data_version: Option<i64>,

    tab: Tab,
    /// 三个查询页各自的查询日期（互不影响，各自落盘）。
    dates: Dates,

    train_query: String,
    train_results: Vec<TrainSummary>,

    station_query: String,
    /// 模糊搜索：把输入解析成同城组，匹配组内所有车站。
    station_fuzzy: bool,
    station_suggestions: Vec<String>,
    /// 同城组名候选（仅模糊模式）。
    station_city_suggestions: Vec<String>,
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
    /// 出发/到达解析失败时的同城组名候选。
    od_city_suggestions: Vec<String>,
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
    /// 上一帧「有没有输入框拿着键盘焦点」，只在变化时通知平台。
    ime_focus: bool,
    /// 上一帧的可用尺寸（逻辑点），用来发现横竖屏/分屏变化。
    last_screen: egui::Vec2,
    /// 点击输入框后还要补几次「弹键盘」：egui 每次点击都会 interrupt_ime，
    /// egui-winit 随之 set_ime_allowed(false)+(true)，Android 上先收后弹，
    /// 而「弹」会被系统忽略（焦点在我们的桥 View 上），所以要自己补一次。
    ime_reshow: u8,
    /// 最近一次读到的缩进。
    safe_area: crate::ScreenInsets,
    /// 上一次询问缩进的时间，用作最小间隔。
    inset_probe_at: Option<Instant>,

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
        fonts::install(&cc.egui_ctx);
        theme::install(&cc.egui_ctx, platform.pointer);
        // 输入法上屏发生在别的线程：先把界面句柄交给它，它才催得动帧。
        if let Some(ime) = &platform.ime {
            ime.attach_context(&cc.egui_ctx);
        }

        // 配置读取失败只降级，不阻断启动：返回默认值 + 一句提示。
        let (settings, config_warning) = config::load(&config_path);
        let data_page = crate::data::DataPage::new(platform.dirs.clone(), platform.files.clone());
        let mut app = Self {
            db: None,
            data: data_page,
            db_path,
            db_error: None,
            data_version: None,
            // 上次停留的标签页；第一次启动是第一位（站站）。
            tab: settings.ui.tab,
            dates: Dates {
                od: settings.od.date.unwrap_or_else(today),
                train: settings.train.date.unwrap_or_else(today),
                station: settings.station.date.unwrap_or_else(today),
            },
            train_query: settings.train.query.clone(),
            train_results: Vec::new(),
            station_query: settings.station.query.clone(),
            station_fuzzy: settings.station.fuzzy,
            station_suggestions: Vec::new(),
            station_city_suggestions: Vec::new(),
            station_options: Vec::new(),
            station_selected: BTreeSet::new(),
            station_board_for: None,
            station_board: Vec::new(),
            od_from: settings.od.from.clone(),
            od_to: settings.od.to.clone(),
            od_fuzzy: settings.od.fuzzy,
            od_city_suggestions: Vec::new(),
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
            platform,
            safe_area: crate::ScreenInsets::default(),
            inset_probe_at: None,
            config_path,
            persisted: settings,
            dirty_since: None,
            save_failed: false,
            ime_focus: false,
            last_screen: egui::Vec2::ZERO,
            ime_reshow: 0,
        };
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

    fn open_db(&mut self) {
        self.db = None;
        self.data_version = None;
        self.db_error = None;

        match Db::open(&self.db_path) {
            Ok(db) => {
                self.data_version = db.data_version();
                self.db = Some(db);
                // 配置里恢复出来的查询词要立刻查一次，否则页面上只有输入、没有结果。
                // `open_db` 也是错误页「重试」的入口，所以恢复逻辑放在这里而非 new()。
                self.refresh_restored_queries();
            }
            Err(err) => self.db_error = Some(err),
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
        let events: Vec<egui::Event> = ime
            .drain()
            .into_iter()
            .map(|input| match input {
                crate::ImeInput::Commit(text) => egui::Event::Ime(egui::ImeEvent::Commit(text)),
                crate::ImeInput::Preedit(text) => egui::Event::Ime(egui::ImeEvent::Preedit {
                    text,
                    active_range_chars: None,
                }),
                crate::ImeInput::Delete { before, after } => {
                    egui::Event::Ime(egui::ImeEvent::DeleteSurrounding {
                        before_chars: before,
                        after_chars: after,
                    })
                }
                crate::ImeInput::Enter => enter_key(),
            })
            .collect();
        if !events.is_empty() {
            ctx.input_mut(|input| input.events.extend(events));
        }
        // 焦点变化时过一次 JNI：Android 靠它把输入法连接挂到自己的 View 上、并弹键盘。
        // 键盘被系统收掉（返回键）后焦点不变，所以再补一条：界面里点一下就重弹，
        // 否则得先点别的输入框才回来。键盘上的点击不会进 egui，不会被这里反复触发。
        let focused = ctx.memory(|memory| memory.focused()).is_some();
        let clicked = ctx.input(|input| input.pointer.any_click());
        if focused != self.ime_focus {
            self.ime_focus = focused;
            self.ime_reshow = 0;
            ime.set_text_focus(focused);
        } else if focused && clicked {
            // 点输入框时 egui 会 request_focus → interrupt_ime，egui-winit 于是在
            // 这一帧末尾 `set_ime_allowed(false)` + `(true)`：Android 上先收键盘，
            // 而「再弹」因为焦点在我们的桥 View 上被系统忽略 —— 用户看到的就是
            // 「点一下就收起来」。所以这一帧结束后补两次 show（跨过那次 hide）。
            self.ime_reshow = 2;
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        } else if self.ime_reshow > 0 && focused {
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
            ui: config::UiSettings { tab: self.tab },
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
        }
    }

    /// 帧首调用：状态与上次落盘的快照不同时，停手 [`SAVE_DEBOUNCE`] 后写一次。
    ///
    /// 放在帧首而不是帧尾，是为了覆盖所有提前 `return` 的分支（库打不开时也会
    /// 记下当前状态）；快照最多落后一帧，对这几个开关和输入框没有影响。
    fn persist(&mut self, ctx: &egui::Context) {
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
            self.station_city_suggestions.clear();
            self.station_options.clear();
            self.station_selected.clear();
            self.station_board_for = None;
            self.station_board.clear();
            return;
        }

        let mut error = None;
        let mut suggestions = Vec::new();
        let mut cities = Vec::new();
        let mut fuzzy_selection = None;
        let mut exact_station = None;

        if let Some(db) = &self.db {
            match db.station_suggestions(&query, STATION_SUGGESTIONS) {
                Ok(rows) => suggestions = rows,
                Err(err) => error = Some(format!("车站查询失败：{err}")),
            }
            if self.station_fuzzy {
                match db.city_suggestions(&query, 8) {
                    Ok(rows) => cities = rows,
                    Err(err) => error = Some(format!("同城候选查询失败：{err}")),
                }
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
        self.station_city_suggestions = cities;

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
        self.od_city_suggestions.clear();
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
                format!("没有找到名为「{query}」的同城组或车站，试试候选。")
            } else {
                format!("没有名为「{query}」的车站，试试候选站名。")
            });
            match db.station_suggestions(query, 8) {
                Ok(rows) => self.od_suggestions = rows,
                Err(err) => self.error = Some(format!("车站查询失败：{err}")),
            }
            if self.od_fuzzy {
                match db.city_suggestions(query, 8) {
                    Ok(rows) => self.od_city_suggestions = rows,
                    Err(err) => self.error = Some(format!("同城候选查询失败：{err}")),
                }
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

        let mut error = None;
        let mut detail = None;
        let mut extras = TrainExtras::default();
        if let Some(db) = &self.db {
            match db.train_detail(index) {
                Ok(d) => detail = Some(d),
                Err(err) => error = Some(format!("读取车次详情失败：{err}")),
            }
            // 附加数据缺失（旧库）时返回空数组，不影响停站表。
            extras.duties = db.duties_for(index).unwrap_or_default();
            extras.via = db.via_points(index).unwrap_or_default();
            extras.dining = db.dining_cars(index).unwrap_or_default();
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
                ui.label(meta(format!("数据 {}", date_ymd(version))).weak())
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
        let height = theme::control_height();
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
    fn theme_button(&mut self, ui: &mut Ui) {
        let dark = ui.visuals().dark_mode;
        let (label, next_theme) = if dark {
            ("浅色", egui::Theme::Light)
        } else {
            ("深色", egui::Theme::Dark)
        };
        if ui.add(egui::Button::new(meta(label))).clicked() {
            ui.ctx().set_theme(next_theme);
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
        let (running, not_running): (Vec<&TrainSummary>, Vec<&TrainSummary>) = self
            .train_results
            .iter()
            .partition(|train| train.runs_on(selected_ymd));
        let rows: Vec<&TrainSummary> = running.iter().chain(not_running.iter()).copied().collect();
        let running_count = running.len();
        if !bar {
            show_counts(ui, None, running_count, rows.len() - running_count);
        }
        let selected = self.selected;
        let clicked = Self::train_rows_table(ui, &rows, running_count, selected, "train_results");
        if let Some(index) = clicked {
            self.select_train(index);
        }
    }

    fn train_rows_table(
        ui: &mut Ui,
        rows: &[&TrainSummary],
        running_count: usize,
        selected: Option<i64>,
        salt: &str,
    ) -> Option<i64> {
        let width = ui.available_width();
        let wide = width >= theme::TRAIN_COLUMNS_MIN_WIDTH;
        // 很窄（例如侧边详情把列表压到 430 点左右）时先收起开行日期，
        // 日期改在车次单元格的悬停提示里给出，避免日期被硬裁掉。
        let show_date = width >= 560.0;
        let header_top = ui.cursor().top();
        let mut clicked = None;
        let mut table = TableBuilder::new(ui)
            .id_salt(salt)
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .cell_layout(cell_layout())
            .column(Column::exact(34.0))
            .column(Column::initial(94.0).at_least(78.0));
        if wide {
            table = table.column(Column::initial(92.0).at_least(80.0));
        }
        table = table.column(Column::remainder().at_least(150.0));
        if wide {
            table = table
                .column(Column::initial(84.0).at_least(76.0))
                .column(Column::initial(58.0));
        }
        if show_date {
            table = table.column(Column::initial(168.0).at_least(150.0));
        }

        table
            .header(theme::list_header_height(), |mut header| {
                header.col(|ui| table_head(ui, "状态"));
                header.col(|ui| table_head(ui, "车次"));
                if wide {
                    header.col(|ui| table_head(ui, "类型"));
                }
                header.col(|ui| table_head(ui, "始发 → 终到"));
                if wide {
                    header.col(|ui| table_head_right(ui, "里程"));
                    header.col(|ui| table_head_right(ui, "天数"));
                }
                if show_date {
                    header.col(|ui| table_head(ui, "开行日期"));
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
                    row.col(|ui| {
                        status_cell(ui, running);
                    });
                    row.col(|ui| {
                        train_code_cell(ui, &train.code, (!show_date).then_some(window.as_str()));
                    });
                    if wide {
                        row.col(|ui| {
                            ui.add(egui::Label::new(&train.kind).truncate());
                        });
                    }
                    row.col(|ui| {
                        route_cell(ui, &train.first, &train.last);
                    });
                    if wide {
                        row.col(|ui| {
                            right_label(ui, format!("{} km", train.total_km));
                        });
                        row.col(|ui| {
                            right_label(ui, format!("{} 天", train.days));
                        });
                    }
                    if show_date {
                        row.col(|ui| {
                            ui.label(meta(window));
                        });
                    }
                    let r = row.response();
                    if r.clicked() {
                        clicked = Some(train.index);
                    }
                });
            });
        paint_header_line(ui, header_top, theme::list_header_height());

        clicked
    }

    fn station_ui(&mut self, ui: &mut Ui, part: TabPart) {
        let bar = self.bottom_bar();
        if part == TabPart::Results {
            self.station_results_ui(ui, bar);
            return;
        }
        let width = ui.available_width();
        let field_width = (width * 0.34).clamp(140.0, 240.0);
        let mut clear = false;
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            ui.label(RichText::new("车站").size(theme::FONT_BODY));
            let response = ui.add(query_input(
                &mut self.station_query,
                "例如 北京 / 北京南",
                field_width,
            ));
            self.take_focus(ui, FocusField::Station, &response);
            if response.changed() {
                changed = true;
            }
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
                    if width >= 430.0 {
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
            self.station_city_suggestions.clear();
            self.station_options.clear();
            self.station_selected.clear();
            self.station_board_for = None;
            self.station_board.clear();
            self.focus_request = Some(FocusField::Station);
        } else if changed {
            self.refresh_station();
        }

        // 同城组候选（模糊模式）与普通车站候选分开显示，点击后都走同一条解析路径。
        if self.station_fuzzy && !self.station_city_suggestions.is_empty() {
            let current = self.station_board_for.clone();
            let mut picked = None;
            ui.add_space(theme::GAP_XS);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(meta("同城组").weak());
                ui.add_space(theme::GAP_XS);
                for city in &self.station_city_suggestions {
                    let selected = current.as_deref() == Some(city.as_str());
                    if ui
                        .selectable_label(selected, RichText::new(city).size(theme::FONT_META))
                        .clicked()
                    {
                        picked = Some(city.clone());
                    }
                }
            });
            if let Some(city) = picked {
                self.station_query = city;
                self.refresh_station();
            }
        }

        if !self.station_suggestions.is_empty() {
            let current = self.station_board_for.clone();
            let mut picked = None;
            ui.add_space(theme::GAP_XS);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(meta("候选站").weak());
                ui.add_space(theme::GAP_XS);
                for station in &self.station_suggestions {
                    let selected = current.as_deref() == Some(station.as_str());
                    if ui
                        .selectable_label(selected, RichText::new(station).size(theme::FONT_META))
                        .clicked()
                    {
                        picked = Some(station.clone());
                    }
                }
            });
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
                    "试试上方的候选站名或同城组。",
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
        let (running, not_running): (Vec<&StationStop>, Vec<&StationStop>) = self
            .station_board
            .iter()
            .partition(|stop| stop.runs_on(origin_ymd(date, stop.day)));
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
    ) -> Option<i64> {
        // 窗口足够宽时才展示停留 / 第几天 / 类型，避免列被挤到互相覆盖；
        // 更窄时再收起本站里程，把宽度留给时刻和始发终到。
        let width = ui.available_width();
        let wide = width >= theme::STATION_COLUMNS_MIN_WIDTH;
        let show_km = width >= 560.0;
        // 极窄（例如 900 点窗口右侧详情占掉一半）时收起「始发 → 终到」，
        // 路线改在车次单元格的悬停提示里，避免表格横向溢出。
        // 同城多站时「车站」列优先于始发终到，路线再往后放。
        let show_route = width >= if show_station { 620.0 } else { 430.0 };
        let header_top = ui.cursor().top();
        let mut clicked = None;
        let mut table = TableBuilder::new(ui)
            .id_salt(salt)
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .cell_layout(cell_layout())
            .column(Column::exact(34.0))
            .column(Column::initial(96.0).at_least(80.0));
        if show_station {
            table = table.column(Column::initial(86.0).at_least(72.0));
        }
        table = table
            .column(Column::initial(76.0).at_least(72.0))
            .column(Column::initial(76.0).at_least(72.0));
        if wide {
            table = table
                .column(Column::initial(64.0))
                .column(Column::initial(58.0));
        }
        if show_km {
            table = table.column(Column::initial(82.0).at_least(74.0));
        }
        if wide {
            table = table.column(Column::initial(92.0).at_least(80.0));
        }
        if show_route {
            table = table.column(Column::remainder().at_least(110.0));
        }

        table
            .header(theme::list_header_height(), |mut header| {
                header.col(|ui| table_head(ui, "状态"));
                header.col(|ui| table_head(ui, "车次"));
                if show_station {
                    header.col(|ui| table_head(ui, "车站"));
                }
                header.col(|ui| table_head(ui, "到达"));
                header.col(|ui| table_head(ui, "发车"));
                if wide {
                    header.col(|ui| table_head_right(ui, "停留"));
                    header.col(|ui| table_head(ui, "第几天"));
                }
                if show_km {
                    header.col(|ui| table_head_right(ui, "本站里程"));
                }
                if wide {
                    header.col(|ui| table_head(ui, "类型"));
                }
                if show_route {
                    header.col(|ui| table_head(ui, "始发 → 终到"));
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
                    row.col(|ui| {
                        status_cell(ui, running);
                    });
                    let route = format!("{} → {}", stop.first, stop.last);
                    row.col(|ui| {
                        train_code_cell(ui, &stop.code, (!show_route).then_some(route.as_str()));
                    });
                    if show_station {
                        row.col(|ui| {
                            ui.add(egui::Label::new(&stop.station).truncate());
                        });
                    }
                    // 车站表的「+N天」说的是“这趟车从始发算起第几天”，
                    // 与「第几天」列同义，但窄屏收起那一列时它是唯一的跨天线索。
                    let day_offset = stop.day - 1;
                    row.col(|ui| {
                        time_cell(ui, &stop.arrive, day_offset);
                    });
                    row.col(|ui| {
                        time_cell(ui, &stop.depart, day_offset);
                    });
                    if wide {
                        row.col(|ui| {
                            right_label(ui, format!("{} 分", stop.dwell_min));
                        });
                        row.col(|ui| {
                            ui.label(format!("第 {} 天", stop.day));
                        });
                    }
                    if show_km {
                        row.col(|ui| {
                            right_label(ui, format!("{} km", stop.km));
                        });
                    }
                    if wide {
                        row.col(|ui| {
                            ui.add(egui::Label::new(&stop.kind).truncate());
                        });
                    }
                    if show_route {
                        row.col(|ui| {
                            route_cell(ui, &stop.first, &stop.last);
                        });
                    }
                    if row.response().clicked() {
                        clicked = Some(stop.train_index);
                    }
                });
            });
        paint_header_line(ui, header_top, theme::list_header_height());

        clicked
    }

    fn od_ui(&mut self, ui: &mut Ui, part: TabPart) {
        if part == TabPart::Results {
            self.od_results_ui(ui);
            return;
        }
        // 竖屏一行放：模糊 + 出发框 + ⇌ + 到达框；横屏（侧栏）把模糊挪到下一行，
        // 两个框因此能宽一些。站名最长 5 个汉字（5 × 15 点 = 75），下限仍取 96 点。
        let landscape = part == TabPart::Controls;
        let swap_width = theme::control_height();
        let mut changed = false;
        // 触摸屏上「模糊 / 查询」在底栏（`od_bar`），这里只留两个输入框与交换按钮。
        let bar = self.bottom_bar();
        // 这一行不换行：模糊 + 出发 + ⇌ + 到达 必须并排（换行会让到达框单独掉到下一行）。
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            // 「模糊」在输入框左边（竖屏）：它是「怎么匹配输入」的开关，贴着输入最直观。
            // 横屏的侧栏窄，它挪到输入框下面一行（见下面的 `if landscape`）。
            if !landscape {
                let fuzzy = ui
                    .checkbox(&mut self.od_fuzzy, meta("模糊"))
                    .on_hover_text("模糊搜索：出发/到达按城市或同城组匹配所有车站");
                if fuzzy.changed() {
                    changed = true;
                }
            }
            // 输入框宽度按「模糊之后还剩多少」现算：侧栏（横屏左栏）本来就窄，
            // 用估算常量会算多半行，到达框就被挤到下一行去了。
            // TextEdit 的 `desired_width` 不含它自己的左右内边距（合计约 8 点），
            // 所以每个框要多预留一份，否则两个框加起来会超出这一行。
            const INPUT_CHROME: f32 = 10.0;
            let rest = ui.available_width();
            let field_width =
                ((rest - swap_width - theme::GAP_S * 2.0) / 2.0 - INPUT_CHROME).clamp(96.0, 200.0);
            // 两个框不再带标签：占位符已经说明了是出发站还是到达站，省下的宽度给输入。
            let from = ui.add(query_input(&mut self.od_from, "出发站", field_width));
            self.take_focus(ui, FocusField::OdFrom, &from);
            if from.changed() {
                self.od_last = OdField::From;
                changed = true;
            }
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

            let to = ui.add(query_input(&mut self.od_to, "到达站", field_width));
            self.take_focus(ui, FocusField::OdTo, &to);
            if to.changed() {
                self.od_last = OdField::To;
                changed = true;
            }
            if advance {
                to.request_focus();
            }

            if !bar {
                // 边打边查，这个按钮只是兜底，别做成主操作。
                let can_query = !self.od_from.trim().is_empty() && !self.od_to.trim().is_empty();
                let button = egui::Button::new(meta("刷新"));
                if ui
                    .add_enabled(can_query, button)
                    .on_hover_text("重新查询（输入变化时会自动刷新）")
                    .clicked()
                {
                    changed = true;
                }
            }
        });
        ui.add_space(theme::GAP_XS);
        // 日期单独一行：放在输入框下面、车站筛选上面（竖屏与侧栏都这样）。
        ui.horizontal(|ui| date_picker(ui, &mut self.dates.od, "od_date", true));
        if landscape
            && ui
                .checkbox(&mut self.od_fuzzy, meta("模糊"))
                .on_hover_text("模糊搜索：出发/到达按城市或同城组匹配所有车站")
                .changed()
        {
            changed = true;
        }
        if changed {
            self.refresh_od();
        }

        if let Some(message) = &self.od_message {
            ui.add_space(theme::GAP_XS);
            ui.label(meta(message).weak());
        }

        if let Some(field) = self.od_missing {
            let mut picked = None;
            if self.od_fuzzy && !self.od_city_suggestions.is_empty() {
                ui.add_space(theme::GAP_XS);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(meta("同城组").weak());
                    ui.add_space(theme::GAP_XS);
                    for city in &self.od_city_suggestions {
                        if ui.add(egui::Button::new(meta(city))).clicked() {
                            picked = Some(city.clone());
                        }
                    }
                });
            }
            ui.add_space(theme::GAP_XS);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(meta("候选站").weak());
                ui.add_space(theme::GAP_XS);
                for station in &self.od_suggestions {
                    if ui.add(egui::Button::new(meta(station))).clicked() {
                        picked = Some(station.clone());
                    }
                }
            });
            if let Some(station) = picked {
                match field {
                    OdField::From => self.od_from = station,
                    OdField::To => self.od_to = station,
                }
                self.od_last = field;
                self.refresh_od();
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
        let (running, not_running): (Vec<&OdResult>, Vec<&OdResult>) = filtered
            .iter()
            .copied()
            .partition(|hit| hit.runs_on(origin_ymd(date, hit.depart_day)));
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
        let clicked = Self::od_rows_table(ui, &rows, running_count, selected, "od_results");
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

    fn od_rows_table(
        ui: &mut Ui,
        rows: &[&OdResult],
        running_count: usize,
        selected: Option<i64>,
        salt: &str,
    ) -> Option<i64> {
        let width = ui.available_width();
        // 竖屏（触摸）另用一套六列表：宁可横向滚动，也别把站名和时间挤成省略号。
        if theme::pointer() == theme::Pointer::Touch && width < theme::OD_TOUCH_SCROLL_WIDTH {
            return Self::od_rows_table_portrait(ui, rows, running_count, selected, salt);
        }
        let wide = width >= theme::OD_COLUMNS_MIN_WIDTH;
        // 区段就是查询条件本身，极窄时收起它，把空间留给时刻。
        let show_segment = width >= 560.0;
        // 极窄时把历时移到车次悬停提示里，保证五个基本列不横向溢出。
        let show_duration = width >= 450.0;
        let header_top = ui.cursor().top();
        let mut clicked = None;
        let mut table = TableBuilder::new(ui)
            .id_salt(salt)
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .cell_layout(cell_layout())
            .column(Column::exact(34.0))
            .column(Column::initial(92.0).at_least(78.0))
            .column(Column::initial(82.0).at_least(74.0))
            .column(Column::initial(82.0).at_least(74.0));
        if show_duration {
            // “18小时46分”“1天10小时05分”等最长历时需要约 104 点；
            // 再窄就省略号收尾，同样的道理：撑破列宽会把后面几列顶偏、把选中底色切断。
            table = table.column(Column::initial(112.0).at_least(104.0));
        }
        if wide {
            table = table
                .column(Column::initial(82.0).at_least(74.0))
                .column(Column::initial(88.0).at_least(80.0));
        }
        if show_segment {
            table = table.column(Column::remainder().at_least(110.0));
        }

        table
            .header(theme::list_header_height(), |mut header| {
                header.col(|ui| table_head(ui, "状态"));
                header.col(|ui| table_head(ui, "车次"));
                header.col(|ui| table_head(ui, "出发"));
                header.col(|ui| table_head(ui, "到达"));
                if show_duration {
                    header.col(|ui| table_head(ui, "历时"));
                }
                if wide {
                    header.col(|ui| table_head_right(ui, "里程"));
                    header.col(|ui| table_head(ui, "类型"));
                }
                if show_segment {
                    header.col(|ui| table_head(ui, "区段"));
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
                    row.col(|ui| {
                        status_cell(ui, running);
                    });
                    let duration = fmt_duration(hit.duration_min());
                    let segment = format!("{} → {}", hit.from_station, hit.to_station);
                    let mut tips = Vec::new();
                    if !show_segment {
                        tips.push(segment.clone());
                    }
                    if !show_duration {
                        tips.push(duration.clone());
                    }
                    let tip = (!tips.is_empty()).then(|| tips.join(" · "));
                    row.col(|ui| {
                        train_code_cell(ui, &hit.code, tip.as_deref());
                    });
                    row.col(|ui| {
                        // 出发日期就是查询日期，标「+N天」只会让人以为上车不是当天。
                        time_cell(ui, &hit.depart, 0);
                    });
                    row.col(|ui| {
                        time_cell(ui, &hit.arrive, hit.arrive_day - hit.depart_day);
                    });
                    if show_duration {
                        row.col(|ui| {
                            ui.add(egui::Label::new(&duration).truncate())
                                .on_hover_text(&duration);
                        });
                    }
                    if wide {
                        row.col(|ui| {
                            right_label(ui, format!("{} km", hit.km()));
                        });
                        row.col(|ui| {
                            ui.label(&hit.kind);
                        });
                    }
                    if show_segment {
                        row.col(|ui| {
                            ui.add(egui::Label::new(&segment).truncate())
                                .on_hover_text(format!(
                                    "{segment}
全程：{} → {}",
                                    hit.first, hit.last
                                ));
                        });
                    }
                    if row.response().clicked() {
                        clicked = Some(hit.train_index);
                    }
                });
            });
        paint_header_line(ui, header_top, theme::list_header_height());

        clicked
    }

    /// 竖屏（触摸）的站站结果表：`状态 车次 出发站 出发时间 到达站 到达时间`。
    ///
    /// 六列在 434 点宽的手机上放不下，所以固定列宽 + 横向滚动，而不是压缩它
    /// （压缩会把「北京朝阳」变成「北京…」、把时刻拆行）。
    fn od_rows_table_portrait(
        ui: &mut Ui,
        rows: &[&OdResult],
        running_count: usize,
        selected: Option<i64>,
        salt: &str,
    ) -> Option<i64> {
        // 六列 + 间距 ≈ 428 点：横屏右栏（约 450 点）放得下，最后一列不会被切；
        // 竖屏照旧横向滚动。「状态」两个汉字要 26 点，再小表头就被截成「…」。
        const STATUS: f32 = 30.0;
        const CODE: f32 = 74.0;
        const STATION: f32 = 78.0;
        const TIME: f32 = 64.0;
        let spacing = ui.spacing().item_spacing.x;
        let total = STATUS + CODE + STATION + TIME + STATION + TIME + spacing * 5.0;
        let pitch = theme::list_row_height() + ui.spacing().item_spacing.y;
        let content_height = theme::list_header_height() + rows.len() as f32 * pitch + theme::GAP_S;

        let mut clicked = None;
        // 滚动条做成「浮动 + 细」：平时不占位、几乎看不见，拖动时才浮出来一点。
        // 表格本来就是横向溢出可拖的，粗滚动条只会抢走一行高度和视觉重心。
        let mut scroll_style = egui::style::ScrollStyle::floating();
        scroll_style.bar_width = 8.0;
        scroll_style.floating_width = 4.0;
        ui.style_mut().spacing.scroll = scroll_style;
        // 只用一个「横竖都能滚」的区域：表格自己不再抢拖动。
        // 嵌套两个 ScrollArea 时，内层（表格的纵向滚动）会把手指拖动全吃掉，
        // 结果横向只能拖滚动条——这正是之前的毛病。
        egui::ScrollArea::both()
            .id_salt((salt, "scroll"))
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                // 给表格一个「比视口宽、比视口高」的容器，它才会按固定列宽铺开。
                ui.allocate_ui_with_layout(
                    egui::vec2(total, content_height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let header_top = ui.cursor().top();
                        TableBuilder::new(ui)
                            .id_salt(salt)
                            .striped(true)
                            .sense(Sense::click())
                            .drag_to_scroll(egui::scroll_area::DragScroll::Never)
                            .auto_shrink([false, false])
                            .cell_layout(cell_layout())
                            .column(Column::exact(STATUS))
                            .column(Column::exact(CODE))
                            .column(Column::exact(STATION))
                            .column(Column::exact(TIME))
                            .column(Column::exact(STATION))
                            .column(Column::exact(TIME))
                            .header(theme::list_header_height(), |mut header| {
                                header.col(|ui| table_head(ui, "状态"));
                                header.col(|ui| table_head(ui, "车次"));
                                header.col(|ui| table_head(ui, "出发站"));
                                header.col(|ui| table_head(ui, "出发时间"));
                                header.col(|ui| table_head(ui, "到达站"));
                                header.col(|ui| table_head(ui, "到达时间"));
                            })
                            .body(|body| {
                                body.rows(theme::list_row_height(), rows.len(), |mut row| {
                                    let hit = rows[row.index()];
                                    let running = row.index() < running_count;
                                    row.set_selected(selected == Some(hit.train_index));
                                    if running_count > 0 && row.index() == running_count {
                                        row.set_overline(true);
                                    }
                                    row.col(|ui| status_cell(ui, running));
                                    row.col(|ui| train_code_cell(ui, &hit.code, None));
                                    row.col(|ui| {
                                        ui.add(egui::Label::new(&hit.from_station).truncate())
                                            .on_hover_text(&hit.from_station);
                                    });
                                    // 出发日期就是查询日期，不标「+N」；到达才可能跨天。
                                    row.col(|ui| time_cell(ui, &hit.depart, 0));
                                    row.col(|ui| {
                                        ui.add(egui::Label::new(&hit.to_station).truncate())
                                            .on_hover_text(&hit.to_station);
                                    });
                                    row.col(|ui| {
                                        time_cell(ui, &hit.arrive, hit.arrive_day - hit.depart_day);
                                    });
                                    if row.response().clicked() {
                                        clicked = Some(hit.train_index);
                                    }
                                });
                            });
                        paint_header_line(ui, header_top, theme::list_header_height());
                    },
                );
            });

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
        // 模态窗按「去掉系统栏」的尺寸排版，否则整页会画到状态栏下面。
        let screen = self.safe_screen(ui.ctx());
        if via_page_open && self.via_page_size != Some(screen) {
            via_page_open = false;
        }
        let extras = &self.extras;
        let date = ymd(self.dates.get(self.tab));
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
            // 手机上整页只有**一个**滚动块：经由（窄条）→ 时刻表 → 杂项。
            // 标题栏固定在上面，任何时候都能返回。
            let has_via = !extras.via.is_empty();
            header_ui(
                ui,
                detail,
                true,
                previous_code(&previous, detail),
                &mut header,
            );
            ui.add_space(theme::GAP_S);
            // 经由条自带一条顶线；这时再画分隔线就成了两条并排的重复线。
            if !has_via {
                ui.separator();
                ui.add_space(theme::GAP_S);
            }
            egui::ScrollArea::vertical()
                .id_salt("detail_page_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if has_via {
                        via_page_trigger(ui, &extras.via, &mut via_page_open);
                        ui.add_space(theme::GAP_S);
                    }
                    // 停站表按行数撑开（而不是吃满剩余高度），这样它才会跟页面一起滚。
                    let pitch = theme::detail_row_height() + ui.spacing().item_spacing.y;
                    let table_height = theme::detail_header_height()
                        + detail.stops.len() as f32 * pitch
                        + theme::GAP_S;
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), table_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            if let Some(station) = stops_table_ui(ui, &detail.stops) {
                                link = Some(DetailLink::Station(station));
                            }
                        },
                    );
                    ui.add_space(theme::GAP_M);
                    extras_ui(ui, &detail.summary, extras, &detail.stops, date, &mut link);
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
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), stops_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            if let Some(station) = stops_table_ui(ui, &detail.stops) {
                                link = Some(DetailLink::Station(station));
                            }
                        },
                    );
                    if has_via {
                        ui.add_space(theme::GAP_S);
                        via_page_trigger(ui, &extras.via, &mut via_page_open);
                    }
                });
            });
        } else {
            // 宽屏侧栏：经由展开时独占下方的全部高度，停站表暂时让位。
            // 两者各分一半时都只剩几行，谁也看不清；收起经由停站表就回来。
            let has_via = !extras.via.is_empty();
            let via_open = has_via
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
            extras_ui(ui, &detail.summary, extras, &detail.stops, date, &mut link);
            ui.add_space(theme::GAP_S);
            ui.separator();
            ui.add_space(theme::GAP_S);
            if has_via {
                via_section_ui(
                    ui,
                    &extras.via,
                    &mut DetailToggles {
                        stations_only: &mut stations_only,
                        via_page_open: &mut via_page_open,
                    },
                    &detail.stops,
                    detail.summary.index,
                    via_open,
                );
            }
            if !via_open {
                if has_via {
                    ui.add_space(theme::GAP_S);
                    ui.separator();
                    ui.add_space(theme::GAP_S);
                }
                if let Some(station) = stops_table_ui(ui, &detail.stops) {
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
                self.station_city_suggestions.clear();
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
            self.station_city_suggestions.clear();
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
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_XS;
            if ui
                .add(egui::Button::new(meta(format!("← {label}"))))
                .on_hover_text(format!("回到上一处：{label}"))
                .clicked()
            {
                go_back = true;
            }
            ui.label(meta("刚才在停站表里点了车站").weak());
        });
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
        let query = normalize_code(code);
        let date = ymd(self.dates.get(self.tab));
        match db.search_trains(&query, None) {
            Ok(hits) => match hits
                .iter()
                .find(|train| train.runs_on(date))
                .or(hits.first())
            {
                Some(train) => {
                    let index = train.index;
                    self.select_train(index);
                }
                None => {
                    self.error = Some(format!("交路里的车次 {code} 在数据里查不到"));
                }
            },
            Err(err) => self.error = Some(format!("查询车次失败：{err}")),
        }
    }
}

impl eframe::App for LltApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
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
        if self.db.is_none() {
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

        // 横屏（矮而宽）不画头部：标签页和版本都在左栏里，纵向空间让给内容。
        let landscape = self.bottom_bar()
            && ui.available_height() < theme::LANDSCAPE_MAX_HEIGHT
            && ui.available_width() >= theme::LANDSCAPE_MIN_WIDTH
            && self.detail.is_none();
        if !landscape {
            egui::Panel::top("header")
                .frame(theme::panel_frame(ui))
                .show(ui, |ui| self.header(ui));
        }

        // 触摸窄屏（手机）不建底部面板：列表和面板各分一半后谁都看不清，
        // 改成「整车次一页」——详情直接replace掉列表，而不是叠一个会自己长高的模态。
        // 触摸宽屏（平板横屏）右栏够用，仍保留列表与详情并排。
        let touch_page = self.detail.is_some()
            && self.platform.pointer == theme::Pointer::Touch
            && ui.available_width() < WIDE_MIN_WIDTH;
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
                                    ui.label(meta(format!("数据 {}", date_ymd(version))).weak())
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

    /// 当前页有没有动作可放：车次页在没输入时只有一行空栏，不如收起来。
    fn action_bar_needed(&self) -> bool {
        match self.tab {
            Tab::Data => false,
            Tab::Train => !self.train_query.trim().is_empty(),
            _ => true,
        }
    }

    /// 底栏的动作行：把「不是输入框、又占表格上方空间」的控件收在这里。
    ///
    /// 每个标签页只放当前这页用得上的：查询 / 清空 / 模糊，右侧是这一页的
    /// 计数（原来那一行「当日开行 N 趟」）。数据页不需要动作行。
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

    /// 站站的动作行：模糊 / 查询（「交换」在两个输入框中间，不在这里）。
    fn od_bar(&mut self, ui: &mut Ui) {
        let mut changed = false;
        let can_query = !self.od_from.trim().is_empty() && !self.od_to.trim().is_empty();
        let (running, not_running) = self.running_split();
        let total = self.od_shown_count();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // 「刷新」只是兜底：三个查询页都边打边查，用户正常不会按它，
            // 所以做成 chip 大小、次要样式，别抢视觉重心。
            let button =
                egui::Button::new(meta("刷新")).min_size(egui::vec2(48.0, theme::chip_height()));
            if ui
                .add_enabled(can_query, button)
                .on_hover_text("重新查询（输入变化时会自动刷新）")
                .clicked()
            {
                changed = true;
            }
            counts_label(ui, total, running, not_running);
        });
        if changed {
            self.refresh_od();
        }
    }

    /// 按车次的动作行：清空 + 计数。
    fn train_bar(&mut self, ui: &mut Ui) {
        let height = theme::control_height();
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
        let height = theme::control_height();
        if !self.station_query.trim().is_empty()
            && ui
                .add(egui::Button::new(meta("清空")).min_size(egui::vec2(0.0, height)))
                .clicked()
        {
            self.station_query.clear();
            self.station_suggestions.clear();
            self.station_city_suggestions.clear();
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
                if train.runs_on(selected) {
                    (on + 1, off)
                } else {
                    (on, off + 1)
                }
            }),
            Tab::Station => self.station_board.iter().fold((0, 0), |(on, off), stop| {
                if stop.runs_on(origin_ymd(self.dates.get(self.tab), stop.day)) {
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
                    if hit.runs_on(origin_ymd(self.dates.get(self.tab), hit.depart_day)) {
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
        if self.via_page_open {
            self.via_page_open = false;
        } else if self.detail.is_some() {
            self.close_detail();
        } else if let Some(control) = &self.platform.control {
            control.exit();
        }
    }

    /// 「设置」页：三个查询页的开关 + 数据（子章节）。
    fn settings_ui(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .id_salt("settings_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.query_settings_ui(ui);
                ui.add_space(theme::GAP_M);
                ui.separator();
                ui.add_space(theme::GAP_M);
                self.data_ui(ui);
            });
    }

    /// 「查询」小节：三个查询页各自的模糊开关（车次页没有模糊）。
    fn query_settings_ui(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("查询")
                .size(theme::FONT_SECTION)
                .family(fonts::bold_family()),
        );
        ui.add_space(theme::GAP_XS);
        ui.checkbox(&mut self.od_fuzzy, meta("站站：模糊匹配同城车站"))
            .on_hover_text("出发/到达按城市或同城组匹配所有车站");
        ui.checkbox(&mut self.station_fuzzy, meta("车站：模糊匹配同城车站"))
            .on_hover_text("站名按城市/同城组匹配，包含同城所有车站");
        ui.label(meta("车次页没有模糊开关：车次号本身就是精确的前缀匹配。").weak());
        ui.add_space(theme::GAP_XS);
        ui.label(meta("查询日期分别在三个查询页里设置，各存一份。").weak());
    }

    /// 「数据」页：当前库、检查更新、下载重建、备份导入导出。
    fn data_ui(&mut self, ui: &mut Ui) {
        let outcome = self.data.ui(ui, &self.db_path);
        if outcome.reload_db {
            // 库被换掉了：重开连接并把恢复的查询重跑一遍（见 `open_db`）。
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
fn query_input<'a>(text: &'a mut String, hint: &str, width: f32) -> egui::TextEdit<'a> {
    let edit = egui::TextEdit::singleline(text)
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
        ui.label(meta(format!("+{offset_days}天")).weak());
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
        ui.label(bold(&detail.summary.code).size(theme::FONT_CODE));
        if compact {
            ui.add_space(theme::GAP_S);
            ui.add(egui::Label::new(RichText::new(&route).size(theme::FONT_BODY)).truncate())
                .on_hover_text(&route);
        }
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
fn stops_table_ui(ui: &mut Ui, stops: &[Stop]) -> Option<String> {
    let width = ui.available_width();
    let last_seq = stops.last().map(|stop| stop.seq);
    let show_km = width >= 380.0;
    let header_top = ui.cursor().top();
    let mut clicked = None;
    let mut table = TableBuilder::new(ui)
        .id_salt("train_stops")
        .striped(true)
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .cell_layout(cell_layout())
        // 站名吸收列宽变化，时刻、里程、天数保持固定，便于纵向比较。
        .column(Column::remainder().at_least(88.0))
        .column(Column::initial(66.0).at_least(62.0))
        .column(Column::initial(66.0).at_least(62.0));
    if show_km {
        table = table.column(Column::initial(66.0));
    }
    table = table.column(Column::initial(58.0).at_least(54.0));

    table
        .header(theme::detail_header_height(), |mut header| {
            header.col(|ui| table_head(ui, "车站"));
            header.col(|ui| table_head(ui, "到达"));
            header.col(|ui| table_head(ui, "发车"));
            if show_km {
                header.col(|ui| table_head_right(ui, "里程"));
            }
            header.col(|ui| table_head_right(ui, "天"));
        })
        .body(|body| {
            body.rows(theme::detail_row_height(), stops.len(), |mut row| {
                let index = row.index();
                let stop = &stops[index];
                // 不再按「第 N 天」画跨日分隔线：「天」列本身已经写明第几天，
                // 那条横线在密集表格里只会和斑马纹、行分隔线抢注意力。
                row.col(|ui| {
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
                });
                row.col(|ui| {
                    ui.label(RichText::new(&stop.arrive).monospace());
                });
                row.col(|ui| {
                    ui.label(RichText::new(&stop.depart).monospace());
                });
                if show_km {
                    row.col(|ui| {
                        right_label(ui, format!("{} km", stop.km));
                    });
                }
                row.col(|ui| {
                    right_label(ui, format!("第 {} 天", stop.day));
                });
            });
        });
    paint_header_line(ui, header_top, theme::detail_header_height());
    clicked
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

/// 交路串：`G1806/7 → G82/79 → [G1] · 2026-06-13 起`，当前车次高亮。
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
            let highlight = ui.visuals().selection.bg_fill;
            if extra_duties {
                ui.label(meta(format!("{}.", index + 1)).weak());
            }
            for (position, segment) in duty.segments.iter().enumerate() {
                if position > 0 {
                    ui.label(RichText::new("→").size(theme::FONT_BODY).weak());
                }
                let text = RichText::new(&segment.label).size(theme::FONT_BODY);
                if segment.current {
                    ui.label(
                        text.family(fonts::bold_family())
                            .background_color(highlight),
                    )
                    .on_hover_text("当前车次");
                } else if jumpable_train_code(&segment.code) {
                    // `override_text_color` 会盖掉 `Link` 的回退色，这里显式指定，
                    // 保证和停站表里的车站链接是同一支深蓝；悬停时 Link 自己加下划线。
                    let text = text.color(ui.visuals().hyperlink_color);
                    if ui
                        .add(egui::Link::new(text))
                        .on_hover_text(format!("查看 {} 的时刻表", segment.label))
                        .clicked()
                    {
                        clicked = Some(segment.code.clone());
                    }
                } else {
                    ui.label(text.weak())
                        .on_hover_text("回送 / 检测车次，没有独立时刻表");
                }
            }
            // 分隔符和日期作为一个整体换行，避免行末只剩一个「·」。
            ui.add(
                egui::Label::new(
                    meta(format!("· {}", date_window(duty.start_date, duty.end_date))).weak(),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            )
            .on_hover_text(format!(
                "该交路有效区间：{} ~ {}",
                date_ymd(duty.start_date),
                date_ymd(duty.end_date)
            ));
        });
    });
    clicked
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

/// 「27 点 · 1318 km · 车站 23」，折叠标题、窄屏入口和经由页共用。
fn via_summary_text(via: &[ViaPoint]) -> String {
    let total = via.last().map(|point| point.km).unwrap_or(0);
    let stations = via.iter().filter(|point| point.is_station).count();
    format!("{} 点 · {} km · 车站 {stations}", via.len(), total)
}

/// 整行可点击的折叠标题，返回当前是否展开。
///
/// 标题文字用子 UI 绘制，点击区域在这之后用 `interact` 叠在整行上，
/// 这样点到文字也能切换；悬停底色在绘制内容前按指针位置判断。
fn via_header_ui(ui: &mut Ui, id: egui::Id, summary: String) -> bool {
    let stored = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::hover());
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(4),
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    let weak = ui.visuals().weak_text_color();
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    collapse_arrow(&mut row, stored, weak);
    row.add_space(theme::GAP_XS);
    row.label(bold("经由").size(theme::FONT_BODY));
    row.add_space(theme::GAP_S);
    row.label(meta(summary).weak());
    drop(row);

    let response = ui.interact(rect, id.with("hit"), Sense::click());
    let open = if response.clicked() { !stored } else { stored };
    if response.clicked() {
        ui.data_mut(|data| data.insert_temp(id, open));
    }
    open
}

/// 交路 / 车型 / 餐车等信息行；经由由调用方在下方单独绘制。
fn extras_ui(
    ui: &mut Ui,
    train: &TrainSummary,
    extras: &TrainExtras,
    stops: &[Stop],
    date: i64,
    link: &mut Option<DetailLink>,
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
        detail_row(ui, "交路", |ui| {
            for (index, duty) in duties.iter().enumerate() {
                if index > 0 {
                    ui.add_space(theme::GAP_XS);
                }
                // 同号多区间时标出序号，避免两条交路看起来像一条。
                if let Some(code) = duty_chain_ui(ui, duty, multiple, index) {
                    *link = Some(DetailLink::Train(code));
                }
            }
        });
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
}

/// 详情里由界面回写的两个开关，集中传递，避免参数继续膨胀。
struct DetailToggles<'a> {
    stations_only: &'a mut bool,
    via_page_open: &'a mut bool,
}

/// 宽屏详情侧栏里的经由折叠项 + 图例 + 表格。
///
/// `fill` 为真时表格吃到面板剩余的全部高度（此时停站表不再绘制）；
/// 为假时只画折叠标题本身。图例行右侧的「放大」打开封顶尺寸的居中弹窗。
fn via_section_ui(
    ui: &mut Ui,
    via: &[ViaPoint],
    toggles: &mut DetailToggles<'_>,
    stops: &[Stop],
    train_index: i64,
    fill: bool,
) {
    if via.is_empty() {
        return;
    }
    let open = via_header_ui(ui, via_state_id(train_index), via_summary_text(via));
    if open {
        ui.add_space(theme::GAP_XS);
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
        let max_height = if fill {
            let raw = ui.available_height();
            let rows = ((raw - theme::detail_header_height()) / pitch)
                .floor()
                .max(1.0);
            theme::detail_header_height() + rows * pitch
        } else {
            let raw = (ui.available_height() * 0.5).clamp(104.0, 218.0);
            let rows = ((raw - theme::detail_header_height()) / pitch)
                .floor()
                .clamp(1.0, 6.0);
            theme::detail_header_height() + rows * pitch
        };
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
}

/// 窄屏时停站表下方的经由入口：整行可点，打开独立经由页。
fn via_page_trigger(ui: &mut Ui, via: &[ViaPoint], open: &mut bool) {
    if via.is_empty() {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::control_height()),
        Sense::hover(),
    );
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(4),
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    // 与表格内容之间加一条细线，避免入口像是浮在空白里。
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    let summary = via_summary_text(via);
    let weak = ui.visuals().weak_text_color();
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    row.add_space(theme::GAP_S);
    row.label(bold("经由").size(theme::FONT_BODY));
    row.add_space(theme::GAP_S);
    row.add(egui::Label::new(meta(summary.clone()).weak()).truncate())
        .on_hover_text(summary);
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        collapse_arrow(ui, false, weak);
        ui.add_space(theme::GAP_XS);
        ui.label(meta("查看").weak());
        ui.add_space(theme::GAP_S);
    });
    drop(row);

    // 子 UI 会挡住 allocate 的响应，和折叠标题一样在最后叠一层交互。
    let response = ui.interact(
        rect,
        ui.make_persistent_id("llt-via-page-trigger"),
        Sense::click(),
    );
    if response.clicked() {
        *open = true;
    }
    response.on_hover_text("打开经由页");
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
    // 窄屏整页和宽屏弹窗用不同的 Area id，避免复用对方的尺寸/位置状态。
    let response = egui::Modal::new(egui::Id::unique(("llt-via-page", compact)))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(120))
        .show(&ctx, |ui| {
            ui.set_min_size(page - egui::vec2(24.0, 24.0));
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
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = theme::chip_height();
                ui.spacing_mut().button_padding = theme::chip_padding();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_XS;
                    let total_km = extras.via.last().map(|point| point.km).unwrap_or(0);
                    let stations = extras.via.iter().filter(|point| point.is_station).count();
                    ui.label(
                        meta(format!(
                            "{}点 · {total_km}km · 车站 {stations}",
                            extras.via.len()
                        ))
                        .weak(),
                    );
                    ui.add_space(theme::GAP_S);
                    via_legend_ui(ui);
                });
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

/// 底栏里的计数：`68 条 · ● 68 · ○ 0`，比 `show_counts` 更短。
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
        ui.label(meta(format!("· {total} 条")).weak());
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
        }
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
