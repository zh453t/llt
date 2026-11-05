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

/// 输入框、按钮、日期选择器统一高度。
pub const CONTROL_HEIGHT: f32 = 30.0;
/// 查询结果列表行高。
pub const LIST_ROW_HEIGHT: f32 = 28.0;
/// 列表表头高度。
pub const LIST_HEADER_HEIGHT: f32 = 30.0;
/// 详情里的停站表 / 经由表行高。
pub const DETAIL_ROW_HEIGHT: f32 = 26.0;
/// 详情表格表头高度。
pub const DETAIL_HEADER_HEIGHT: f32 = 26.0;

/// 常用间距，全部取 4 的倍数。
pub const GAP_XS: f32 = 4.0;
pub const GAP_S: f32 = 8.0;
pub const GAP_M: f32 = 12.0;

/// 面板内边距（左/右, 上/下）。
pub const PANEL_MARGIN_X: i8 = 12;
pub const PANEL_MARGIN_Y: i8 = 8;
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
    egui::Frame::new()
        .inner_margin(Margin::symmetric(PANEL_MARGIN_X, PANEL_MARGIN_Y))
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
pub fn install(ctx: &egui::Context) {
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
    spacing.item_spacing = Vec2::new(GAP_S, 6.0);
    spacing.button_padding = Vec2::new(10.0, 5.0);
    // 输入框、按钮、日期选择器都以此为最小高度，保证同一行高度一致。
    spacing.interact_size = Vec2::new(40.0, CONTROL_HEIGHT);
    spacing.icon_width = 14.0;
    spacing.icon_width_inner = 8.0;
    spacing.indent = 16.0;
    spacing.window_margin = Margin::same(12);
    spacing.menu_margin = Margin::same(8);
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
    visuals.hyperlink_color = Color32::from_rgb(0x1e, 0x5c, 0xc7);
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
