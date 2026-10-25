pub fn icon_image() -> image::RgbaImage {
    image::load_from_memory(include_bytes!("../assets/branding/rpshop-logo.png"))
        .expect("the embedded RPShop application icon must be a valid PNG")
        .into_rgba8()
}
