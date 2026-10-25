use crate::{branding, ui};

pub fn run() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if crate::cli::run(&args) {
        return Ok(());
    }
    let screenshot = args
        .windows(2)
        .find(|w| w[0] == "--screenshot")
        .map(|w| std::path::PathBuf::from(&w[1]));
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("RPShop · 原型工作室")
        .with_inner_size([1500.0, 950.0])
        .with_min_inner_size([1100.0, 720.0]);
    viewport = viewport.with_icon(branding::icon_data());
    let options = eframe::NativeOptions {
        viewport,
        persist_window: screenshot.is_none(),
        persistence_path: screenshot
            .as_ref()
            .map(|_| std::env::temp_dir().join(format!("rpshop-qa-{}.ron", std::process::id()))),
        ..Default::default()
    };
    eframe::run_native(
        "RPShop",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, screenshot)))),
    )
}
