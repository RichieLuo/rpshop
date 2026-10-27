//! Shop dialog geometry. Keep these metrics consistent across the four apps.
use eframe::egui::{self, Color32, RichText};

pub(super) fn frame(ctx: &egui::Context) -> egui::Frame {
    let dark = ctx.style().visuals.dark_mode;
    egui::Frame::new()
        .fill(if dark {
            Color32::from_rgb(32, 34, 38)
        } else {
            Color32::WHITE
        })
        .stroke(egui::Stroke::new(
            1.0,
            if dark {
                Color32::from_white_alpha(28)
            } else {
                Color32::from_black_alpha(24)
            },
        ))
        .corner_radius(12)
        .inner_margin(24)
        .shadow(egui::epaint::Shadow {
            offset: [0, 12],
            blur: 36,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 72 } else { 28 }),
        })
}

pub(super) fn modal(ctx: &egui::Context, id: &str) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id))
        .frame(frame(ctx))
        .backdrop_color(Color32::from_black_alpha(64))
}

pub(super) fn title(ui: &mut egui::Ui, title: &str) {
    let style = ui.style_mut();
    style.spacing.item_spacing = egui::vec2(8.0, 10.0);
    style.spacing.button_padding = egui::vec2(16.0, 8.0);
    style.spacing.interact_size.y = 36.0;
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = 6.into();
    }
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
    ui.label(
        RichText::new(title)
            .size(20.0)
            .strong()
            .color(ui.visuals().text_color()),
    );
    ui.add_space(6.0);
}

pub(super) fn actions<R>(
    ui: &mut egui::Ui,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 36.0),
        egui::Layout::right_to_left(egui::Align::Center),
        content,
    )
}

pub(super) fn primary(label: &str, color: Color32) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).color(Color32::WHITE))
        .fill(color)
        .stroke(egui::Stroke::NONE)
        .corner_radius(6)
        .min_size(egui::vec2(80.0, 36.0))
}
