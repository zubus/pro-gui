use super::*;

pub(super) const CONTENT_MAX_WIDTH: f32 = 1100.0;

pub(super) const SIDEBAR_WIDTH: f32 = 320.0;

pub(super) const CANVAS: Color32 = Color32::from_rgb(12, 15, 20);
pub(super) const SIDEBAR_FILL: Color32 = Color32::from_rgb(19, 22, 27);
pub(super) const HAIRLINE: Color32 = Color32::from_rgb(34, 39, 47);
pub(super) const CARD_FILL: Color32 = Color32::from_rgb(21, 25, 31);
pub(super) const CARD_STROKE: Color32 = Color32::from_rgb(41, 47, 56);
pub(super) const ACCENT: Color32 = Color32::from_rgb(38, 105, 176);
pub(super) const TEXT_STRONG: Color32 = Color32::from_rgb(232, 238, 244);
pub(super) const TEXT_BODY: Color32 = Color32::from_rgb(210, 218, 226);
pub(super) const TEXT_MUTED: Color32 = Color32::from_rgb(146, 157, 168);
pub(super) const TEXT_FAINT: Color32 = Color32::from_rgb(108, 118, 130);
pub(super) const KEY_STORED: Color32 = Color32::from_rgb(134, 225, 174);
pub(super) const KEY_MISSING: Color32 = Color32::from_rgb(255, 151, 143);

pub(super) fn column_width(panel_width: f32) -> f32 {
    (panel_width - 48.0).clamp(300.0, CONTENT_MAX_WIDTH)
}

pub(super) fn configure_visuals(ctx: &Context) {
    let mut visuals = Visuals::dark();
    visuals.window_fill = Color32::from_rgb(20, 24, 30);
    visuals.window_stroke = Stroke::new(1.0_f32, CARD_STROKE);
    visuals.panel_fill = CANVAS;
    visuals.extreme_bg_color = Color32::from_rgb(22, 25, 31);
    visuals.override_text_color = Some(Color32::from_rgb(223, 229, 235));
    visuals.selection.bg_fill = Color32::from_rgb(26, 85, 144);
    visuals.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(223, 229, 235));
    visuals.widgets.active.bg_fill = Color32::from_rgb(26, 85, 144);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(43, 49, 58);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(32, 37, 43);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(223, 229, 235));
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(22, 25, 31);
    visuals.widgets.noninteractive.fg_stroke =
        Stroke::new(1.0_f32, Color32::from_rgb(188, 197, 206));
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, HAIRLINE);
    visuals.window_corner_radius = egui::CornerRadius::same(12);
    ctx.set_visuals(visuals);
}

pub(super) fn apply_font_size(ctx: &Context, base: f32) {
    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(
        egui::TextStyle::Body,
        FontId::new(base, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        FontId::new(base, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        FontId::new((base - 2.0).max(9.0), FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        FontId::new(base, FontFamily::Monospace),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        FontId::new(base + 7.0, FontFamily::Proportional),
    );
    ctx.set_style(style);
}

pub(super) fn pill(ui: &mut egui::Ui, text: RichText, fill: Color32) -> egui::Response {
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let padding = egui::vec2(7.0, 2.0);
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + 2.0 * padding, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), fill);
        ui.painter().galley(rect.min + padding, galley, TEXT_BODY);
    }
    response
}

pub(super) fn status_pill(ui: &mut egui::Ui, status: &str) -> egui::Response {
    let (fill, text) = match status {
        "completed" => (
            Color32::from_rgb(18, 54, 39),
            Color32::from_rgb(134, 225, 174),
        ),
        "queued" => (
            Color32::from_rgb(72, 58, 16),
            Color32::from_rgb(247, 209, 92),
        ),
        "in_progress" => (
            Color32::from_rgb(15, 47, 74),
            Color32::from_rgb(122, 196, 255),
        ),
        "cancelled" => (
            Color32::from_rgb(70, 43, 31),
            Color32::from_rgb(232, 162, 126),
        ),
        "incomplete" | "requires_action" => (
            Color32::from_rgb(74, 50, 14),
            Color32::from_rgb(246, 180, 92),
        ),
        "failed" => (
            Color32::from_rgb(82, 23, 23),
            Color32::from_rgb(255, 151, 143),
        ),
        _ => (
            Color32::from_rgb(49, 53, 60),
            Color32::from_rgb(195, 199, 204),
        ),
    };

    pill(
        ui,
        RichText::new(status.replace('_', " ")).small().color(text),
        fill,
    )
}

pub(super) fn render_setting_chip(ui: &mut egui::Ui, key: &str, value: &str) {
    pill(
        ui,
        RichText::new(format!("{key}={value}"))
            .small()
            .family(FontFamily::Monospace)
            .color(Color32::from_rgb(176, 204, 230)),
        Color32::from_rgb(27, 33, 41),
    );
}

pub(super) fn render_usage_chip(ui: &mut egui::Ui, key: &str, value: i64) {
    pill(
        ui,
        RichText::new(format!("{key}={value}"))
            .small()
            .family(FontFamily::Monospace)
            .color(Color32::from_rgb(148, 219, 178)),
        Color32::from_rgb(18, 38, 30),
    );
}

pub(super) fn status_color(status: &str) -> Color32 {
    match status {
        "completed" => Color32::from_rgb(103, 201, 146),
        "queued" => Color32::from_rgb(247, 209, 92),
        "in_progress" => Color32::from_rgb(105, 190, 255),
        "failed" => Color32::from_rgb(255, 120, 114),
        "cancelled" => Color32::from_rgb(214, 151, 120),
        "incomplete" | "requires_action" => Color32::from_rgb(246, 180, 92),
        _ => Color32::from_rgb(130, 138, 146),
    }
}

pub(super) fn filter_chip(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).small().color(if active {
            Color32::WHITE
        } else {
            TEXT_BODY
        }))
        .fill(if active {
            Color32::from_rgb(27, 61, 99)
        } else {
            Color32::from_rgb(28, 33, 40)
        })
        .corner_radius(egui::CornerRadius::same(10)),
    )
}

pub(super) fn key_status_dot(ui: &mut egui::Ui, name: &str, stored: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::click());
    ui.painter().circle_filled(
        rect.center(),
        4.0,
        if stored { KEY_STORED } else { KEY_MISSING },
    );
    response.on_hover_text(format!(
        "{name} API key {}",
        if stored {
            "stored"
        } else {
            "missing, click to add it"
        }
    ))
}

pub(super) fn body_size(ui: &egui::Ui) -> f32 {
    ui.style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .map_or(14.0, |font| font.size)
}
