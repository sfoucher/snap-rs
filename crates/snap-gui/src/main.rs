mod app;
mod explorer;
mod view;

use app::SnapApp;

fn main() -> eframe::Result<()> {
    env_logger::init();
    snap_io_gdal::init_error_handler();
    eframe::run_native(
        "snap-rs",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(SnapApp::new(cc)))),
    )
}
