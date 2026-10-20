mod app;
mod explorer;
mod palette_cpd;
mod panels;
mod view;

use app::SnapApp;

fn main() -> eframe::Result<()> {
    env_logger::init();
    snap_io_gdal::init_error_handler();
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    eframe::run_native(
        "snap-rs",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            ..Default::default()
        },
        Box::new(move |cc| {
            let mut app = SnapApp::new(cc);
            for f in &files {
                app.open(f);
            }
            Ok(Box::new(app))
        }),
    )
}
