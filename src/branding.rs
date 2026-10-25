use eframe::egui::IconData;

pub fn icon_data() -> IconData {
    let image = image::load_from_memory(include_bytes!("../assets/branding/rpshop-logo.png"))
        .expect("the embedded RPShop application icon must be a valid PNG")
        .resize(256, 256, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }
}
