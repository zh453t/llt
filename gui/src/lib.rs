//! `llt-gui` 的库部分：界面、配置、字体、主题，以及一个与平台无关的启动函数。
//!
//! 桌面端由 `src/main.rs` 解析命令行后调用 [`run_desktop`]；Android 端由仓库里的
//! `android/` 独立包（cdylib）调用 [`options`] + [`run_with`]，先把 APK 资源里的
//! 数据库解到应用私有目录，再把路径交给界面。方案见 `docs/android.md`。

mod app;
pub mod columns;
pub mod config;
pub mod data;
pub mod files;
pub mod fonts;
pub mod theme;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::wgpu;

/// 平台侧（Android 的 `ime.rs`）也要用 egui 的类型，这里转出去一份。
pub use eframe::egui;

/// 窗口标题 / 任务列表里的应用名。
pub const APP_TITLE: &str = "路路通时刻表";

/// 默认窗口大小（逻辑点）。Android 上由系统决定尺寸，这个值不生效。
pub const DEFAULT_WINDOW_SIZE: [f32; 2] = [1600.0, 1000.0];

/// 屏幕四周被系统栏（状态栏 / 导航栏 / 刘海）遮住的宽度，单位是**物理像素**。
///
/// 桌面恒为 0；Android 的窗口是 edge-to-edge 的，顶栏会压在状态栏下面，
/// 所以要自己让开（egui-winit 只在 iOS 上填 `safe_area_insets`）。
/// 存物理像素、用的时候再除以 `pixels_per_point`，屏幕密度变化时不会算错。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScreenInsets {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

/// 系统栏缩进的来源。
///
/// 桌面是固定值（全 0）；Android 上窗口的 `WindowInsets` 要等第一次布局后才有值，
/// `android_main` 刚被调用时拿到的是 null，软键盘弹出/收起与旋转屏幕也会改它，
/// 所以给一个回调，由界面在需要时再问（见 `app.rs` 的 `safe_insets`）。
#[derive(Clone)]
pub enum Insets {
    Fixed(ScreenInsets),
    Probe(Arc<dyn Fn() -> ScreenInsets + Send + Sync>),
}

impl Insets {
    /// 固定缩进（桌面用）。
    pub fn fixed(insets: ScreenInsets) -> Self {
        Self::Fixed(insets)
    }

    /// 回调式缩进（Android 用）。
    pub fn probe(probe: impl Fn() -> ScreenInsets + Send + Sync + 'static) -> Self {
        Self::Probe(Arc::new(probe))
    }

    /// 是否是可反复询问的回调（Android）：键盘、导航栏、旋转都会改缩进。
    pub fn is_probe(&self) -> bool {
        matches!(self, Self::Probe(_))
    }

    /// 当前拿到的缩进；回调还没值时返回全 0。
    pub fn read(&self) -> ScreenInsets {
        match self {
            Self::Fixed(insets) => *insets,
            Self::Probe(probe) => probe(),
        }
    }
}

/// 系统输入法送进界面的一条输入。
///
/// 桌面根本没有这个枚举：egui-winit 会把 `WindowEvent::Ime` 自己转成
/// `egui::Event::Ime`。Android 那边没有这条路（见 [`ImeBridge`]），
/// 由平台侧自己收、自己交上来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeInput {
    /// 上屏的最终文字。中文是整词，不是逐字按键。
    Commit(String),
    /// 正在合成、还没上屏的串（拼音 / 候选状态）；空串表示合成结束。
    Preedit(String),
    /// 输入法要求删除光标前后若干字符。
    Delete { before: usize, after: usize },
    /// 回车 /「完成」。
    Enter,
    /// 用户自己把软键盘收起来了（返回键、键盘上的「收起」）。
    ///
    /// 系统只改窗口 insets，既不会通知输入法客户端，也不会让 egui 知道——输入框会
    /// 一直保持焦点，下一次重绘/点击又把键盘弹回来。平台侧报这一条，界面据此放开焦点。
    Hidden,
}

/// 平台输入法的接入点。
///
/// Android 的 `NativeActivity` 窗口里没有带 `InputConnection` 的 View，输入法只能退回
/// 「发按键」，而按键一次只带一个字符——中文要经合成、候选，最后整词 `commitText`
/// 上屏，没有 `InputConnection` 就整个丢掉。平台侧实现这个 trait，界面每帧取走事件、
/// 转成 `egui::Event` 推给输入框；桌面没有实现，传 `None` 即可。
pub trait ImeBridge: Send + Sync {
    /// 取走这一帧的上屏事件。
    fn drain(&self) -> Vec<ImeInput>;
    /// 界面里是否有输入框拿着键盘焦点。Android 靠它把输入法连接重新挂稳。
    fn set_text_focus(&self, focused: bool);
    /// 焦点没变、但用户又点了一下输入框：只把键盘叫回来，别重启输入会话。
    ///
    /// 直接再走一遍 [`Self::set_text_focus`] 会 `restartInput`，键盘会闪一下再收起。
    fn ensure_shown(&self);
    /// 把界面句柄交给平台侧。
    ///
    /// 输入法上屏发生在系统自己的线程里，egui 并不知道有新输入；界面空闲时
    /// 根本不重绘，中文字要等到下一次别的事件才显示出来（实测约 1 秒）。平台侧
    /// 收到输入法事件后要用这个句柄催一帧。
    fn attach_context(&self, ctx: &egui::Context);
}

/// 平台侧的应用级动作：返回键收完自己的页面之后，用它把控制权交回系统。
///
/// Android 上「返回」是 `activity.finish()`（左/右边缘滑动也是同一个返回事件）；
/// 桌面没有这个入口（关窗由系统标题栏负责）。
pub trait AppControl: Send + Sync {
    /// 请求退出应用。
    fn exit(&self);
}

/// 系统栏（状态栏 / 导航栏）的图标颜色。
///
/// Android 的窗口是 edge-to-edge 的：状态栏和导航栏的图标直接画在界面上面，深浅由系统
/// 决定。深色主题下系统默认的白色图标正好；浅色主题下不改就是白字白底，状态栏上的
/// 时间、信号、电量全部看不见（实测）。界面每帧把「当前是不是浅色」报一次，只在真的变了
/// 的时候过桥。桌面没有系统栏，传 `None`。
pub trait SystemBars: Send + Sync {
    /// `light = true`：界面是浅色，系统栏图标要画成深色。
    fn set_light(&self, light: bool);
}

/// 平台差异的唯一入口：界面只认这个结构体，不再各自传参。
///
/// 桌面与 Android 的差别全在这里：系统栏缩进、系统输入法、指针类型、数据目录、
/// 文件选择器、系统栏图标颜色、返回键。以后再加平台能力（剪贴板等）加字段即可，
/// `run_with` 与 `LltApp::new` 不必再改签名。
pub struct Platform {
    /// 系统栏（状态栏 / 导航栏 / 刘海 / 软键盘）占的像素；桌面恒为 0。
    pub insets: Insets,
    /// 系统输入法通道；桌面是 `None`（egui-winit 自己处理 IME）。
    pub ime: Option<Arc<dyn ImeBridge>>,
    /// 指针类型：决定控件尺寸、点击热区。
    pub pointer: theme::Pointer,
    /// 数据页用的目录（导入备份、导出备份、构建临时目录）。
    pub dirs: data::DataDirs,
    /// 系统文件选择器；桌面是 `None`（直接读写目录）。
    pub files: Option<Arc<dyn files::FilePicker>>,
    /// 返回键 / 退出应用；桌面是 `None`。
    pub control: Option<Arc<dyn AppControl>>,
    /// 系统栏图标颜色；桌面是 `None`（没有状态栏）。
    pub bars: Option<Arc<dyn SystemBars>>,
    /// 额外字体目录（Android 把内置的中文粗体解到应用目录，放这里；桌面为空）。
    pub font_dirs: Vec<PathBuf>,
}

impl Platform {
    /// 桌面：没有安全区、没有额外输入法通道、鼠标。
    ///
    /// 设了环境变量 `LLT_TOUCH=1` 就按触摸屏算——用来在桌面复现 Android 的
    /// 大尺寸/整页布局（窗口拉窄即可，见 `docs/android.md`）。
    pub fn desktop() -> Self {
        let pointer = if std::env::var_os("LLT_TOUCH").is_some() {
            theme::Pointer::Touch
        } else {
            theme::Pointer::Mouse
        };
        Self {
            insets: Insets::fixed(ScreenInsets::default()),
            ime: None,
            pointer,
            dirs: data::DataDirs::beside_exe(),
            files: None,
            control: None,
            bars: None,
            font_dirs: Vec::new(),
        }
    }
}

/// 桌面入口：解析 `--db` / `--config`，按「绿色版」语义定位数据库与配置文件。
pub fn run_desktop() -> eframe::Result {
    let args = Args::parse();
    let db_path = resolve_db_path(args.db);
    let config_path = config::resolve_path(args.config.as_deref());
    run_with(options(), db_path, config_path, Platform::desktop())
}

/// 窗口选项。桌面直接用它；Android 端拿到后只需补上 `android_app`。
///
/// 视口提示（默认尺寸、最小尺寸、按屏幕收敛）在 Android 上由 winit 忽略，
/// 不需要为两个平台各写一份。
pub fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_min_inner_size([480.0, 280.0])
            .with_clamp_size_to_monitor_size(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        ..Default::default()
    }
}

/// wgpu 设备的上限取适配器自己报的值。
///
/// 默认的 `wgpu::Limits::default()` 对某些后端偏高：Android 模拟器的 SwiftShader
///  Vulkan 只给 `max_uniform_buffer_binding_size = 16 KiB`（默认要 64 KiB），
/// 照抄默认值会直接开不了设备（`Limit ... is better than allowed`）。
/// 桌面显卡的上限本来就不低于默认值，这样改不会改变桌面行为。
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.device_descriptor = Arc::new(|adapter| wgpu::DeviceDescriptor {
        label: Some("llt-gui"),
        required_limits: adapter.limits(),
        ..Default::default()
    });
    eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// 用给定的路径启动界面。
///
/// 数据库与配置文件由调用方决定位置：桌面是「可执行文件旁」（绿色版），
/// Android 是应用私有目录（`internal_data_path()`），两条路径都不做隐式搜索。
/// 平台差异（安全区、输入法、指针类型）都在 [`Platform`] 里，桌面用
/// [`Platform::desktop`]。
pub fn run_with(
    options: eframe::NativeOptions,
    db_path: PathBuf,
    config_path: PathBuf,
    platform: Platform,
) -> eframe::Result {
    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::LltApp::new(
                cc,
                db_path,
                config_path,
                platform,
            )))
        }),
    )
}

/// 命令行参数：只认两个路径，其余忽略（保持单文件绿色运行的简单性）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub db: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

impl Args {
    /// 支持 `--db <path>` / `--db=<path>`、`--config <path>` / `--config=<path>`。
    pub fn parse() -> Self {
        Self::parse_from(std::env::args().skip(1))
    }

    pub fn parse_from(args: impl Iterator<Item = String>) -> Self {
        let mut args = args.peekable();
        let mut parsed = Self::default();
        while let Some(arg) = args.next() {
            // 空格分隔与 `=` 分隔两种写法都认；`=` 后面为空视作没给。
            let (flag, inline) = match arg.split_once('=') {
                Some((flag, value)) => (flag, Some(value)),
                None => (arg.as_str(), None),
            };
            if !matches!(flag, "--db" | "--config") {
                continue;
            }
            let value = match inline {
                Some(value) => Some(value.to_owned()),
                // 下一个参数又是选项时不吃掉它，否则 `--config --db x` 会丢掉 `--db`。
                None => match args.peek() {
                    Some(next) if !next.trim().is_empty() && !next.starts_with("--") => args.next(),
                    _ => None,
                },
            };
            let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
                continue;
            };
            if flag == "--db" {
                parsed.db = Some(PathBuf::from(value));
            } else {
                parsed.config = Some(PathBuf::from(value));
            }
        }
        parsed
    }
}

/// 解析数据库路径：`--db` → `$LLT_DB` → 可执行文件旁 → 当前目录 → 仓库 `out/timetable`。
///
/// 查找逻辑在 `lltdb::path`（与 CLI 共用一份实现）。
fn resolve_db_path(explicit: Option<PathBuf>) -> PathBuf {
    match lltdb::path::find(explicit.as_deref(), false) {
        Ok(located) => located.path,
        // 找不到时仍返回一个“预期的位置”，让界面上显示的路径有参考价值。
        Err(_) => explicit.unwrap_or_else(default_db_path),
    }
}

/// 界面上展示的默认位置：可执行文件旁。
fn default_db_path() -> PathBuf {
    exe_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(lltdb::path::DB_FILE)
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        Args::parse_from(list.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn parses_both_argument_spellings() {
        let parsed = args(&["--db", "a.sqlite", "--config=b.json"]);
        assert_eq!(parsed.db, Some(PathBuf::from("a.sqlite")));
        assert_eq!(parsed.config, Some(PathBuf::from("b.json")));

        let parsed = args(&["--db=c.sqlite", "--config", "d.json"]);
        assert_eq!(parsed.db, Some(PathBuf::from("c.sqlite")));
        assert_eq!(parsed.config, Some(PathBuf::from("d.json")));
    }

    #[test]
    fn ignores_unknown_and_empty_arguments() {
        assert_eq!(args(&[]).db, None);
        assert_eq!(args(&["--verbose"]).db, None);
        // 空的 `--config=` 视作没给。
        assert_eq!(args(&["--config="]).config, None);
        // 后面紧跟另一个选项时，不把它当成路径吃掉。
        let parsed = args(&["--config", "--db", "a.sqlite"]);
        assert_eq!(parsed.config, None);
        assert_eq!(parsed.db.as_deref(), Some(Path::new("a.sqlite")));
    }
}
