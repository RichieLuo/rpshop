use super::*;

pub(super) fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let cjk_font = "noto-cjk-sc";
    fonts.font_data.insert(
        cjk_font.into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/NotoSansCJKsc-Regular.otf"
        ))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, cjk_font.into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, cjk_font.into());
    let mut bold_names = Vec::new();
    for (name, path) in [
        ("canvas-arial-bold", "C:/Windows/Fonts/arialbd.ttf"),
        ("canvas-cjk-bold", "C:/Windows/Fonts/msyhbd.ttc"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                name.into(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            bold_names.push(name.to_owned());
        }
    }
    if !bold_names.is_empty() {
        bold_names.extend(fonts.families[&egui::FontFamily::Proportional].clone());
        fonts
            .families
            .insert(egui::FontFamily::Name("canvas-bold".into()), bold_names);
    }
    ctx.set_fonts(fonts);
    let mut style = egui::Style {
        visuals: egui::Visuals::light(),
        ..Default::default()
    };
    style.visuals.override_text_color = Some(INK);
    style.visuals.panel_fill = Color32::WHITE;
    style.visuals.selection.bg_fill = Color32::from_rgb(239, 233, 255);
    style.visuals.selection.stroke = Stroke::new(1.0, PURPLE);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(247, 247, 251);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(247, 247, 251);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(240, 235, 255);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(240, 235, 255);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, PURPLE);
    style.spacing.item_spacing = vec2(7.0, 7.0);
    style.spacing.button_padding = vec2(10.0, 6.0);
    style.spacing.interact_size = vec2(30.0, 29.0);
    style.spacing.text_edit_width = 205.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(11.0));
    ctx.set_theme(egui::Theme::Light);
    ctx.set_style_of(egui::Theme::Light, style);
}
