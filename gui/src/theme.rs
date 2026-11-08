//! 统一的视觉规范：字号层级、控件尺寸、间距与配色。
//!
//! 界面代码只从这里取值，避免各区域各写一套字号/间距/颜色。
//! 两套主题（浅色 / 深色）共用同一套尺寸，只替换颜色。

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, Vec2,
};

// ---------------------------------------------------------------- 字号层级

/// 应用标题 / 区块大标题。
pub const FONT_TITLE: f32 = 20.0;
/// 空状态标题、详情里的车次号（`FONT_CODE`）。
pub const FONT_SECTION: f32 = 17.0;
/// 详情车次号。
pub const FONT_CODE: f32 = 24.0;
/// 正文、表格内容、按钮。
pub const FONT_BODY: f32 = 15.0;
/// 辅助信息：表头、日期区间、计数、字段名。
pub const FONT_META: f32 = 13.0;
/// 空状态说明、较长的一段辅助文字。
pub const FONT_HINT: f32 = 14.0;
/// 表格里的时刻列：等宽，保证数字纵向对齐。
pub const FONT_MONO: f32 = 14.0;

// ---------------------------------------------------------------- 尺寸与间距
//
// 尺寸随指针类型取值（见 [`Pointer`]）：鼠标用紧凑的一套，触摸屏用大一号的一套。
// 数值都留在下面这几个常量里，界面只调访问函数。

/// 鼠标下的输入框 / 按钮 / 日期选择器高度。
const CONTROL_HEIGHT_MOUSE: f32 = 30.0;
/// 触摸屏的控件高度：手指落点约 9 mm，30 点太紧。
const CONTROL_HEIGHT_TOUCH: f32 = 37.0;
/// 鼠标下的结果列表行高。
const LIST_ROW_HEIGHT_MOUSE: f32 = 28.0;
const LIST_ROW_HEIGHT_TOUCH: f32 = 32.0;
/// 鼠标下的列表表头高度。
const LIST_HEADER_HEIGHT_MOUSE: f32 = 30.0;
const LIST_HEADER_HEIGHT_TOUCH: f32 = 34.0;
/// 鼠标下的详情（停站表 / 经由表）行高。
const DETAIL_ROW_HEIGHT_MOUSE: f32 = 26.0;
const DETAIL_ROW_HEIGHT_TOUCH: f32 = 30.0;
/// 详情表格表头高度。
const DETAIL_HEADER_HEIGHT_MOUSE: f32 = 26.0;
const DETAIL_HEADER_HEIGHT_TOUCH: f32 = 30.0;

// ---------------------------------------------------------------- 指针类型

/// 指针类型：决定控件尺寸、点击热区与滚动条宽度。
///
/// 触摸屏没有悬停，也没有滚轮；手指落点又比光标粗得多，所以同一套界面在两端的
/// 尺寸必须不同。一个进程只有一种输入设备，`install` 时写一次即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pointer {
    #[default]
    Mouse,
    Touch,
}

/// 当前进程的指针类型，由 [`install`] 写入。
static POINTER: std::sync::OnceLock<Pointer> = std::sync::OnceLock::new();

/// 当前进程的指针类型；没调用过 [`install`]（例如单元测试）时按鼠标算。
pub fn pointer() -> Pointer {
    POINTER.get().copied().unwrap_or_default()
}

/// 顶栏高度：手机上一行 `标题 + 日期 + 主题` 就够，不必跟控件一样高。
pub fn header_height() -> f32 {
    match pointer() {
        Pointer::Mouse => CONTROL_HEIGHT_MOUSE,
        Pointer::Touch => 36.0,
    }
}

/// 输入框、按钮、日期选择器的统一高度。
pub fn control_height() -> f32 {
    match pointer() {
        Pointer::Mouse => CONTROL_HEIGHT_MOUSE,
        Pointer::Touch => CONTROL_HEIGHT_TOUCH,
    }
}

/// 等宽字：数据版本、时间戳、文件大小、路径、文件名这类需要对整齐的内容用它。
///
/// 中文字符仍然落到 CJK 字体（Hack 没有汉字），但数字、日期、路径本身会对齐。
pub fn mono(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(FONT_MONO)
        .family(egui::FontFamily::Monospace)
}

/// 底栏按钮高度：比普通控件再高一点，拇指更容易点中（桌面跟控件同高）。
pub fn bar_height() -> f32 {
    match pointer() {
        Pointer::Mouse => CONTROL_HEIGHT_MOUSE,
        Pointer::Touch => CONTROL_HEIGHT_TOUCH + 6.0,
    }
}

/// 查询结果列表行高。
pub fn list_row_height() -> f32 {
    match pointer() {
        Pointer::Mouse => LIST_ROW_HEIGHT_MOUSE,
        Pointer::Touch => LIST_ROW_HEIGHT_TOUCH,
    }
}

/// 列表表头高度。
pub fn list_header_height() -> f32 {
    match pointer() {
        Pointer::Mouse => LIST_HEADER_HEIGHT_MOUSE,
        Pointer::Touch => LIST_HEADER_HEIGHT_TOUCH,
    }
}

/// 筛选 chip 的高度与内边距：比控件矮一档，一行能放更多个，也不至于点不中。
pub fn chip_height() -> f32 {
    match pointer() {
        Pointer::Mouse => 22.0,
        Pointer::Touch => 30.0,
    }
}

/// 筛选 chip 的内边距（左右, 上下）。
pub fn chip_padding() -> Vec2 {
    match pointer() {
        Pointer::Mouse => Vec2::new(6.0, 1.0),
        Pointer::Touch => Vec2::new(8.0, 2.0),
    }
}

/// 详情里的停站表 / 经由表行高。
pub fn detail_row_height() -> f32 {
    match pointer() {
        Pointer::Mouse => DETAIL_ROW_HEIGHT_MOUSE,
        Pointer::Touch => DETAIL_ROW_HEIGHT_TOUCH,
    }
}

/// 详情表格表头高度。
pub fn detail_header_height() -> f32 {
    match pointer() {
        Pointer::Mouse => DETAIL_HEADER_HEIGHT_MOUSE,
        Pointer::Touch => DETAIL_HEADER_HEIGHT_TOUCH,
    }
}

/// 开行计划日历的格子边长（下限）。
pub fn plan_cell() -> f32 {
    match pointer() {
        Pointer::Mouse => 22.0,
        Pointer::Touch => 32.0,
    }
}

/// 一行七格的边长：按可用宽度摊开，最小取 [`plan_cell`]，最大 48 点。
///
/// 手机上整行铺满、桌面上不至于把格子拉成大方块；两侧各留 6 点余量给行间距。
pub fn plan_cell_size(available: f32) -> f32 {
    ((available - 12.0) / 7.0).clamp(plan_cell(), 48.0)
}

/// 滚动条宽度：触摸时拉宽，不然拖不住。
pub fn scroll_bar_width() -> f32 {
    match pointer() {
        Pointer::Mouse => 5.0,
        Pointer::Touch => 6.0,
    }
}

/// 可点文字（站名、车次号）的命中区在字形外再扩多少。
///
/// 触摸时按手指放宽：上下各 8 点仍在行高之内，不会串到相邻行。
pub fn link_hot_pad() -> Vec2 {
    match pointer() {
        Pointer::Mouse => Vec2::new(2.0, 4.0),
        Pointer::Touch => Vec2::new(6.0, 8.0),
    }
}

// ---------------------------------------------------------------- 布局阈值
//
// 同一套界面按宽度换布局。阈值集中在这里，免得散成一堆魔法数、各表各写一套。

/// 详情从「底部面板」变成「右侧面板」的宽度；也是自动聚焦的门槛。
pub const WIDE_MIN_WIDTH: f32 = 820.0;
/// 更窄时详情改为竖排：信息在上、停站表在下。
pub const STACKED_DETAIL_MAX_WIDTH: f32 = 580.0;
/// 查询日期与标签页同一行的宽度。
pub const INLINE_DATE_MIN_WIDTH: f32 = 640.0;
/// 车次表开始显示「数据 / 开行日期」列的宽度。
pub const TRAIN_COLUMNS_MIN_WIDTH: f32 = 760.0;
/// 车站表开始显示「本站里程 / 类型」列的宽度。
pub const STATION_COLUMNS_MIN_WIDTH: f32 = 880.0;
/// 站站查询开始显示「里程 / 区段」列的宽度。
pub const OD_COLUMNS_MIN_WIDTH: f32 = 840.0;

/// 触摸屏上「状态 车次 出发站 出发时间 到达站 到达时间」这套六列表的最小宽度：
/// 宽度不够时不再压缩列，而是允许横向溢出、用横向滚动看全。
pub const OD_TOUCH_SCROLL_WIDTH: f32 = 640.0;

/// 触摸屏上「横屏布局」的判定：高度比这矮、宽度比这宽，就左右分栏
/// （手机横屏 851×393 命中；平板 1024×768 不命中，保持竖排 + 底栏）。
pub const LANDSCAPE_MAX_HEIGHT: f32 = 520.0;
pub const LANDSCAPE_MIN_WIDTH: f32 = 640.0;

/// 常用间距，全部取 4 的倍数。
pub const GAP_XS: f32 = 4.0;
pub const GAP_S: f32 = 8.0;
pub const GAP_M: f32 = 12.0;

/// 面板内边距（左/右, 上/下）。触摸时上下收一点，把高度让给内容。
pub const PANEL_MARGIN_X: i8 = 12;
pub const PANEL_MARGIN_Y: i8 = 8;
const PANEL_MARGIN_TOUCH_Y: i8 = 6;
/// 详情面板内边距。
pub const DETAIL_MARGIN_X: i8 = 12;
pub const DETAIL_MARGIN_Y: i8 = 10;

// ---------------------------------------------------------------- 颜色

/// 强调色：选中、开行状态、经由里的办客站共用。
pub fn accent(ui: &egui::Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

/// 辅助文字颜色。
pub fn meta_color(ui: &egui::Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

/// 顶部 / 中央 / 侧边面板统一的内边距与底色。
pub fn panel_frame(ui: &egui::Ui) -> egui::Frame {
    let margin_y = match pointer() {
        Pointer::Mouse => PANEL_MARGIN_Y,
        Pointer::Touch => PANEL_MARGIN_TOUCH_Y,
    };
    egui::Frame::new()
        .inner_margin(Margin::symmetric(PANEL_MARGIN_X, margin_y))
        .fill(ui.visuals().panel_fill)
}

/// 底部栏的内边距与底色：比面板更薄，只留够放下一行控件的高度。
pub fn bar_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(PANEL_MARGIN_X, 4))
        .fill(ui.visuals().panel_fill)
}

/// 详情面板的内边距与底色。
pub fn detail_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(DETAIL_MARGIN_X, DETAIL_MARGIN_Y))
        .fill(ui.visuals().panel_fill)
}

/// 主按钮底色。
pub fn primary_fill(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x2b, 0x55, 0x9b)
    } else {
        Color32::from_rgb(0x1e, 0x5c, 0xc7)
    }
}

/// 主按钮文字色。
pub fn primary_text() -> Color32 {
    Color32::from_rgb(0xf7, 0xf9, 0xfc)
}

/// 浅色 / 深色两套配色，其余字段沿用 egui 默认值。
pub fn install(ctx: &egui::Context, pointer: Pointer) {
    let _ = POINTER.set(pointer);
    // 触摸屏上没有「拖选文字」这回事，而 selectable 的 Label 自带 click sense：
    // 它会盖在表格行上面，把「点行打开详情」的手势吃掉——手指点在车次号、站名、
    // 始发终到这些字上就没反应，只能点到字缝里。关掉它，整行都能点。
    let touch = pointer == Pointer::Touch;
    ctx.style_mut_of(Theme::Light, |style| {
        style.interaction.selectable_labels = !touch;
    });
    ctx.style_mut_of(Theme::Dark, |style| {
        style.interaction.selectable_labels = !touch;
    });
    ctx.style_mut_of(Theme::Light, |style| {
        apply_text_styles(style);
        apply_spacing(style);
        apply_transitions(style);
        style.visuals = light_visuals();
        // 表格、网格统一使用斑马纹；斑马纹只用来区分行，不做装饰。
        style.visuals.striped = true;
    });
    ctx.style_mut_of(Theme::Dark, |style| {
        apply_text_styles(style);
        apply_spacing(style);
        apply_transitions(style);
        style.visuals = dark_visuals();
        style.visuals.striped = true;
    });
}

/// egui 内建的过渡（弹窗淡入、滚动条淡入淡出、折叠箭头）统一时长。
///
/// 默认 0.2s 在密集表格里显得发滞；0.13s 仍能看出过渡，又不拖住眼睛。
fn apply_transitions(style: &mut egui::Style) {
    style.animation_time = 0.13;
}

fn apply_text_styles(style: &mut egui::Style) {
    let proportional = FontFamily::Proportional;
    // 等宽字体只用于时刻列；中文仍回退到正文字体链。
    let monospace = FontFamily::Monospace;
    for (text_style, font_id) in [
        (
            TextStyle::Heading,
            FontId::new(FONT_TITLE, proportional.clone()),
        ),
        (
            TextStyle::Body,
            FontId::new(FONT_BODY, proportional.clone()),
        ),
        (
            TextStyle::Button,
            FontId::new(FONT_BODY, proportional.clone()),
        ),
        (TextStyle::Small, FontId::new(FONT_META, proportional)),
        (TextStyle::Monospace, FontId::new(FONT_MONO, monospace)),
    ] {
        style.text_styles.insert(text_style, font_id);
    }
}

fn apply_spacing(style: &mut egui::Style) {
    let spacing = &mut style.spacing;
    // 行高在触摸屏上大一号，行距同比放大，免得表格看起来贴在一起。
    let gap_y = match pointer() {
        Pointer::Mouse => 6.0,
        Pointer::Touch => 8.0,
    };
    spacing.item_spacing = Vec2::new(GAP_S, gap_y);
    spacing.button_padding = Vec2::new(10.0, 5.0);
    // 输入框、按钮、日期选择器都以此为最小高度，保证同一行高度一致。
    spacing.interact_size = Vec2::new(40.0, control_height());
    spacing.icon_width = 14.0;
    spacing.icon_width_inner = 8.0;
    spacing.indent = 16.0;
    spacing.window_margin = Margin::same(12);
    spacing.menu_margin = Margin::same(8);

    // 所有表格都用浮动细滚动条：平时不占位、几乎看不见，滚动/拖动时才浮出来。
    // 粗滚动条在手机上一次要吃掉十几点宽度，而表格本来就紧。
    spacing.scroll = egui::style::ScrollStyle::floating();
    let scroll = &mut spacing.scroll;
    scroll.bar_width = scroll_bar_width();
    scroll.floating_width = scroll_bar_width();
    scroll.floating_allocated_width = scroll_bar_width() + 2.0;
}

struct WidgetPalette {
    text: Color32,
    bg: Color32,
    control: Color32,
    control_hover: Color32,
    control_active: Color32,
    line: Color32,
    line_strong: Color32,
}

fn base_widgets(p: WidgetPalette) -> egui::style::Widgets {
    let WidgetPalette {
        text,
        bg,
        control,
        control_hover,
        control_active,
        line,
        line_strong,
    } = p;
    let mut widgets = egui::style::Widgets::light();
    widgets.noninteractive.bg_fill = bg;
    widgets.noninteractive.weak_bg_fill = bg;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, line);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);

    widgets.inactive.bg_fill = bg;
    widgets.inactive.weak_bg_fill = control;
    widgets.inactive.bg_stroke = Stroke::new(1.0, line);
    widgets.inactive.fg_stroke = Stroke::new(1.0, text);

    widgets.hovered.bg_fill = control_hover;
    widgets.hovered.weak_bg_fill = control_hover;
    widgets.hovered.bg_stroke = Stroke::new(1.0, line_strong);
    widgets.hovered.fg_stroke = Stroke::new(1.0, text);

    widgets.active.bg_fill = control_active;
    widgets.active.weak_bg_fill = control_active;
    widgets.active.bg_stroke = Stroke::new(1.0, line_strong);
    widgets.active.fg_stroke = Stroke::new(1.0, text);

    for visuals in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        visuals.corner_radius = CornerRadius::same(6);
        visuals.expansion = 0.0;
    }
    widgets.open = widgets.active;
    widgets
}

fn light_visuals() -> egui::Visuals {
    let text = Color32::from_rgb(0x20, 0x24, 0x2a);
    let weak = Color32::from_rgb(0x67, 0x70, 0x7e);
    let line = Color32::from_rgb(0xdf, 0xe3, 0xe9);
    let line_strong = Color32::from_rgb(0xc3, 0xca, 0xd5);

    let mut visuals = egui::Visuals::light();
    visuals.override_text_color = Some(text);
    visuals.weak_text_color = Some(weak);
    visuals.panel_fill = Color32::from_rgb(0xf6, 0xf7, 0xf9);
    visuals.window_fill = Color32::from_rgb(0xff, 0xff, 0xff);
    visuals.extreme_bg_color = Color32::from_rgb(0xff, 0xff, 0xff);
    visuals.text_edit_bg_color = Some(Color32::from_rgb(0xff, 0xff, 0xff));
    visuals.faint_bg_color = Color32::from_rgb(0xf1, 0xf3, 0xf6);
    visuals.code_bg_color = Color32::from_rgb(0xf1, 0xf3, 0xf6);
    visuals.window_stroke = Stroke::new(1.0, line);
    // 链接色与选中行文字/聚焦描边用同一支深蓝，同一界面不出现两种蓝。
    visuals.hyperlink_color = Color32::from_rgb(0x1b, 0x45, 0x7d);
    visuals.warn_fg_color = Color32::from_rgb(0xa8, 0x5a, 0x0a);
    visuals.error_fg_color = Color32::from_rgb(0xb4, 0x23, 0x18);
    visuals.selection.bg_fill = Color32::from_rgb(0xdd, 0xe8, 0xf9);
    // 选中行文字用深蓝而不是强调色本身，浅蓝底上仍然清晰；
    // 输入框聚焦描边也用这支颜色。
    visuals.selection.stroke = Stroke::new(1.5, Color32::from_rgb(0x1b, 0x45, 0x7d));
    visuals.widgets = base_widgets(WidgetPalette {
        text,
        bg: Color32::from_rgb(0xff, 0xff, 0xff),
        control: Color32::from_rgb(0xee, 0xf1, 0xf5),
        control_hover: Color32::from_rgb(0xe5, 0xea, 0xf1),
        control_active: Color32::from_rgb(0xdc, 0xe2, 0xea),
        line,
        line_strong,
    });
    visuals
}

fn dark_visuals() -> egui::Visuals {
    let text = Color32::from_rgb(0xe4, 0xe7, 0xec);
    let weak = Color32::from_rgb(0x9a, 0xa3, 0xb1);
    let line = Color32::from_rgb(0x33, 0x38, 0x40);
    let line_strong = Color32::from_rgb(0x47, 0x4e, 0x59);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(text);
    visuals.weak_text_color = Some(weak);
    visuals.panel_fill = Color32::from_rgb(0x1b, 0x1d, 0x21);
    visuals.window_fill = Color32::from_rgb(0x21, 0x24, 0x28);
    visuals.extreme_bg_color = Color32::from_rgb(0x16, 0x18, 0x1b);
    visuals.text_edit_bg_color = Some(Color32::from_rgb(0x16, 0x18, 0x1b));
    visuals.faint_bg_color = Color32::from_rgb(0x21, 0x24, 0x29);
    visuals.code_bg_color = Color32::from_rgb(0x21, 0x24, 0x29);
    visuals.window_stroke = Stroke::new(1.0, line);
    visuals.hyperlink_color = Color32::from_rgb(0x7d, 0xa7, 0xe8);
    visuals.warn_fg_color = Color32::from_rgb(0xe0, 0xa0, 0x5c);
    visuals.error_fg_color = Color32::from_rgb(0xef, 0x8b, 0x80);
    visuals.selection.bg_fill = Color32::from_rgb(0x28, 0x3d, 0x5c);
    visuals.selection.stroke = Stroke::new(1.5, Color32::from_rgb(0xdb, 0xe7, 0xf7));
    visuals.widgets = base_widgets(WidgetPalette {
        text,
        bg: Color32::from_rgb(0x21, 0x24, 0x28),
        control: Color32::from_rgb(0x25, 0x29, 0x2f),
        control_hover: Color32::from_rgb(0x2e, 0x34, 0x3b),
        control_active: Color32::from_rgb(0x37, 0x3e, 0x49),
        line,
        line_strong,
    });
    visuals
}
