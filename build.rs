fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/branding/rpshop-logo.png");

    #[cfg(target_os = "windows")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon();
    }
}

#[cfg(target_os = "windows")]
fn embed_windows_icon() {
    use image::codecs::ico::{IcoEncoder, IcoFrame};

    let source = image::open("assets/branding/rpshop-logo.png")
        .expect("read the RPShop application logo")
        .into_rgba8();
    let frames: Vec<_> = [16, 24, 32, 48, 64, 128, 256]
        .into_iter()
        .map(|size| {
            let resized =
                image::imageops::resize(&source, size, size, image::imageops::FilterType::Lanczos3);
            IcoFrame::as_png(
                resized.as_raw(),
                size,
                size,
                image::ExtendedColorType::Rgba8,
            )
            .expect("encode an RPShop Windows icon frame")
        })
        .collect();
    let icon_path =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"))
            .join("rpshop.ico");
    let file = std::fs::File::create(&icon_path).expect("create the RPShop Windows icon");
    IcoEncoder::new(file)
        .encode_images(&frames)
        .expect("write the RPShop Windows icon");

    winres::WindowsResource::new()
        .set_icon(icon_path.to_str().expect("UTF-8 application icon path"))
        .set("ProductName", "RPShop")
        .set("FileDescription", "RPShop — Prototyping Studio")
        .set("OriginalFilename", "rpshop.exe")
        .compile()
        .expect("embed the RPShop application icon in the executable");
}
