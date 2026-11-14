mod app;
mod bigtext;
pub mod context;
mod db;
mod docedit;
pub mod models;
pub mod providers;

use app::ProGuiApp;

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 920.0])
            .with_min_inner_size([1100.0, 720.0])
            .with_maximized(true)
            .with_title("Pro GUI"),
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        "Pro GUI",
        options,
        Box::new(|cc| {
            let app = ProGuiApp::new(cc).map_err(|err| {
                Box::new(std::io::Error::other(err.to_string()))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
            Ok(Box::new(app))
        }),
    )
}
