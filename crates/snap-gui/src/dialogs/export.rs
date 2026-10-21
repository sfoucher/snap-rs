use crate::app::AppState;
use crate::view::ViewState;
use snap_core::{ProductId, Rect};
use snap_io::{Compression, IoError, ProductWriter, SubsetRegion, WriteOpts};
use snap_io_gdal::GeoTiffWriter;
use std::path::PathBuf;

pub struct ExportRequest {
    pub product: ProductId,
    pub path: PathBuf,
    pub opts: WriteOpts,
}

/// Image pixels covered by `viewport` (the inverse of the canvas transform), rounded
/// outwards and clamped to the image; empty when the view shows none of it.
pub fn view_region(view: &ViewState, viewport: egui::Rect, w: u32, h: u32) -> Rect {
    let c = viewport.center();
    let span = |lo: f32, hi: f32, centre: f64, mid: f32, len: u32| {
        let at = |s: f32| centre + (s - mid) as f64 / view.zoom;
        let clamp = |v: f64| v.clamp(0.0, len as f64) as u32;
        let (a, b) = (clamp(at(lo).floor()), clamp(at(hi).ceil()));
        (a, b - a)
    };
    let (x, rw) = span(viewport.min.x, viewport.max.x, view.center.0, c.x, w);
    let (y, rh) = span(viewport.min.y, viewport.max.y, view.center.1, c.y, h);
    Rect { x, y, w: rw, h: rh }
}

/// True when `out` is the product's own file (canonicalised when both exist).
fn is_source(out: &std::path::Path, source: Option<&std::path::Path>) -> bool {
    let Some(src) = source else { return false };
    match (out.canonicalize(), src.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => out == src,
    }
}

/// Overwriting is allowed when the target is new or the tick was given for this very path.
fn overwrite_ok(
    exists: bool,
    ticked_for: Option<&std::path::Path>,
    target: &std::path::Path,
) -> bool {
    !exists || ticked_for == Some(target)
}

#[derive(Clone, Copy, PartialEq)]
enum Region {
    Full,
    View,
    Custom,
}

#[derive(Clone)]
struct Form {
    region: Region,
    custom: Rect,
    bands: Vec<(String, bool)>,
    cog: bool,
    compression: Compression,
    path: String,
    /// Target the overwrite tick was given for; it authorises that path only.
    overwrite_for: Option<PathBuf>,
    /// (path, is source, exists), refreshed only when the path changes.
    status: Option<(PathBuf, bool, bool)>,
}

const COMPRESSIONS: [(Compression, &str); 4] = [
    (Compression::None, "None"),
    (Compression::Deflate, "Deflate"),
    (Compression::Lzw, "LZW"),
    (Compression::Zstd, "Zstd"),
];

fn default_path(file: Option<&std::path::Path>, name: &str) -> String {
    match file {
        Some(f) => {
            let stem = f.file_stem().map_or(name.into(), |s| s.to_string_lossy());
            f.with_file_name(format!("{stem}_subset.tif"))
        }
        None => PathBuf::from(format!("{name}_subset.tif")),
    }
    .display()
    .to_string()
}

/// Window with region / band / format options; Some(request) when OK is pressed.
pub fn export_dialog(
    ctx: &egui::Context,
    state: &mut AppState,
    product: ProductId,
    open: &mut bool,
) -> Option<ExportRequest> {
    let (w, h, names, file, name) = {
        let p = state.product(product)?.read().unwrap();
        let names: Vec<String> = p.bands.iter().map(|b| b.name.clone()).collect();
        (
            p.width,
            p.height,
            names,
            p.file_path.clone(),
            p.name.clone(),
        )
    };
    let view_rect = state
        .active_view
        .into_iter()
        .chain(0..state.views.len())
        .filter(|&i| state.views.get(i).is_some_and(|v| v.product == product))
        .find_map(|i| Some((state.views.get(i)?, state.canvas_rects.get(&i)?)))
        .map(|(v, r)| view_region(v, *r, w, h));
    let id = egui::Id::new(("export-dialog", product.0));
    let mut f: Form = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| Form {
        region: Region::Full,
        custom: Rect { x: 0, y: 0, w, h },
        bands: names.iter().map(|n| (n.clone(), true)).collect(),
        cog: false,
        compression: Compression::Deflate,
        path: default_path(file.as_deref(), &name),
        overwrite_for: None,
        status: None,
    });
    if f.region == Region::View && view_rect.is_none() {
        f.region = Region::Full;
    }
    let rect = match f.region {
        Region::Full => Rect { x: 0, y: 0, w, h },
        Region::View => view_rect.unwrap_or(Rect { x: 0, y: 0, w, h }),
        Region::Custom => f.custom,
    };
    let out = PathBuf::from(f.path.trim());
    if f.status.as_ref().map(|s| &s.0) != Some(&out) {
        let is_src = is_source(&out, file.as_deref());
        f.status = Some((out.clone(), is_src, !is_src && out.exists()));
    }
    let (_, is_src, exists) = f.status.clone().unwrap();
    let valid = rect.w > 0
        && rect.h > 0
        && !f.path.trim().is_empty()
        && f.bands.iter().any(|b| b.1)
        && !is_src
        && overwrite_ok(exists, f.overwrite_for.as_deref(), &out);
    let (mut ok, mut cancel) = (false, false);
    egui::Window::new("Export Subset")
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label("Region");
            ui.radio_value(&mut f.region, Region::Full, "Full image");
            ui.add_enabled_ui(view_rect.is_some(), |ui| {
                ui.radio_value(&mut f.region, Region::View, "Current view");
            });
            ui.radio_value(&mut f.region, Region::Custom, "Custom");
            if f.region == Region::Custom {
                let c = &mut f.custom;
                ui.horizontal(|ui| {
                    ui.label("x");
                    ui.add(egui::DragValue::new(&mut c.x).range(0..=w.saturating_sub(1)));
                    ui.label("y");
                    ui.add(egui::DragValue::new(&mut c.y).range(0..=h.saturating_sub(1)));
                    ui.label("w");
                    ui.add(egui::DragValue::new(&mut c.w).range(1..=w.saturating_sub(c.x).max(1)));
                    ui.label("h");
                    ui.add(egui::DragValue::new(&mut c.h).range(1..=h.saturating_sub(c.y).max(1)));
                });
            }
            ui.label(format!(
                "{} x {} px at ({}, {})",
                rect.w, rect.h, rect.x, rect.y
            ));
            ui.separator();
            ui.label("Bands");
            for (n, on) in &mut f.bands {
                ui.checkbox(on, n.as_str());
            }
            ui.separator();
            ui.checkbox(&mut f.cog, "Cloud-optimised GeoTIFF");
            egui::ComboBox::from_label("Compression")
                .selected_text(
                    COMPRESSIONS
                        .iter()
                        .find(|c| c.0 == f.compression)
                        .map_or("", |c| c.1),
                )
                .show_ui(ui, |ui| {
                    for (c, label) in COMPRESSIONS {
                        ui.selectable_value(&mut f.compression, c, label);
                    }
                });
            ui.horizontal(|ui| {
                ui.label("Output");
                ui.text_edit_singleline(&mut f.path);
                if ui.button("Browse…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("GeoTIFF", &["tif"])
                        .save_file()
                    {
                        f.path = p.display().to_string();
                    }
                }
            });
            if is_src {
                ui.colored_label(egui::Color32::RED, "cannot overwrite the source file");
            } else if exists {
                let mut on = f.overwrite_for.as_deref() == Some(out.as_path());
                ui.checkbox(&mut on, "Overwrite existing file");
                f.overwrite_for = on.then(|| out.clone());
            }
            ui.horizontal(|ui| {
                ok = ui.add_enabled(valid, egui::Button::new("OK")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
    if ok || cancel {
        *open = false;
        ctx.data_mut(|d| d.remove::<Form>(id));
        return (ok && valid).then(|| ExportRequest {
            product,
            path: PathBuf::from(f.path.trim()),
            opts: WriteOpts {
                subset: Some(SubsetRegion {
                    rect,
                    bands: f.bands.into_iter().filter(|b| b.1).map(|b| b.0).collect(),
                }),
                cog: f.cog,
                compression: f.compression,
            },
        });
    }
    ctx.data_mut(|d| d.insert_temp(id, f));
    None
}

/// Writes on a worker thread from a snapshot of the product (same id and band sources,
/// so cached tiles are shared) taken under a brief read lock.
pub fn run_export(
    state: &AppState,
    req: ExportRequest,
) -> std::thread::JoinHandle<Result<(), IoError>> {
    let snapshot = state
        .product(req.product)
        .map(|p| p.read().unwrap().clone());
    let cache = state.cache.clone();
    std::thread::spawn(move || {
        let p = snapshot.ok_or_else(|| IoError::Write("product was closed".into()))?;
        if is_source(&req.path, p.file_path.as_deref()) {
            return Err(IoError::Write("cannot overwrite the source file".into()));
        }
        GeoTiffWriter.write(&p, &req.path, &req.opts, &cache)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fixtures;
    use crate::view::test_view;
    use egui::{pos2, Rect as ERect};
    use snap_core::DataType;

    #[test]
    fn export_current_view_region_is_clipped_to_image() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        let vp = ERect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        // viewport overhangs the top-left corner of a 200x100 image
        let v = test_view(1.0, (10.0, 10.0));
        assert_eq!(view_region(&v, vp, 200, 100), r(0, 0, 60, 60));
        // and the bottom-right corner
        let v = test_view(1.0, (190.0, 90.0));
        assert_eq!(view_region(&v, vp, 200, 100), r(140, 40, 60, 60));
        // fully inside: 100x80 viewport at zoom 2
        let vp = ERect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 80.0));
        let v = test_view(2.0, (50.0, 40.0));
        assert_eq!(view_region(&v, vp, 200, 100), r(25, 20, 50, 40));
        // fractional edges grow outwards
        let v = test_view(2.0, (50.25, 40.0));
        assert_eq!(view_region(&v, vp, 200, 100), r(25, 20, 51, 40));
        // fully outside: empty at the clamped corner
        let v = test_view(1.0, (-500.0, 50.0));
        let g = view_region(&v, vp, 200, 100);
        assert_eq!(g, r(0, 10, 0, 80));
    }

    #[test]
    fn run_export_writes_subset_that_reopens() {
        snap_io_gdal::init_error_handler();
        let dir = fixtures::temp_dir("gui-export");
        let fx = fixtures::make_fixture(
            &dir,
            "a.tif",
            64,
            64,
            DataType::U16,
            2,
            false,
            None,
            Some(32633),
        );
        let mut st = AppState::new();
        let id = st.open_path(&fx.path).unwrap();
        let band = st.product(id).unwrap().read().unwrap().bands[1]
            .name
            .clone();
        let out = dir.join("out.tif");
        let req = ExportRequest {
            product: id,
            path: out.clone(),
            opts: WriteOpts {
                subset: Some(SubsetRegion {
                    rect: Rect {
                        x: 8,
                        y: 4,
                        w: 20,
                        h: 10,
                    },
                    bands: vec![band],
                }),
                ..Default::default()
            },
        };
        run_export(&st, req).join().unwrap().unwrap();
        let p = st.registry.open(&out).unwrap();
        assert_eq!((p.width, p.height, p.bands.len()), (20, 10, 1));
        drop(p);
        drop(st);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn run_export_refuses_the_source_file() {
        snap_io_gdal::init_error_handler();
        let dir = fixtures::temp_dir("gui-export-src");
        let fx = fixtures::make_fixture(&dir, "a.tif", 8, 8, DataType::U8, 1, false, None, None);
        let mut st = AppState::new();
        let id = st.open_path(&fx.path).unwrap();
        assert!(is_source(&fx.path, Some(&fx.path)));
        assert!(is_source(&dir.join(".").join("a.tif"), Some(&fx.path)));
        assert!(!is_source(&dir.join("b.tif"), Some(&fx.path)));
        assert!(!is_source(&fx.path, None));
        let req = ExportRequest {
            product: id,
            path: fx.path.clone(),
            opts: WriteOpts::default(),
        };
        assert!(run_export(&st, req).join().unwrap().is_err());
        drop(st);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn overwrite_tick_authorises_only_its_own_target() {
        use std::path::Path;
        let (a, b) = (Path::new("a.tif"), Path::new("b.tif"));
        assert!(overwrite_ok(true, Some(a), a));
        assert!(!overwrite_ok(true, Some(a), b));
        assert!(!overwrite_ok(true, None, b));
        assert!(overwrite_ok(false, None, b));
    }
}
