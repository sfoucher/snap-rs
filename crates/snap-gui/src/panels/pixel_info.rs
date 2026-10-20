use crate::app::AppState;
use snap_core::{DataType, GeoPos, ProductId, TileKey, TILE};

pub struct PixelInfo {
    pub image_x: u32,
    pub image_y: u32,
    pub geo: Option<GeoPos>,
    pub crs_xy: Option<(f64, f64)>,
    pub crs_name: Option<String>,
    /// `None` = tile not cached yet (never triggers a read).
    pub bands: Vec<(String, Option<f64>)>,
    pub masks: Vec<(String, bool)>,
    // Private, parallel to `bands`: unit, and whether the band does not cover this pixel.
    units: Vec<Option<String>>,
    out_of_band: Vec<bool>,
}

fn peek_value(state: &AppState, key: TileKey, ix: u32, iy: u32) -> Option<(DataType, f64)> {
    let t = state.cache.peek(&key)?;
    let (r, c) = ((iy % TILE) as usize, (ix % TILE) as usize);
    Some((t.data_type(), t.get_f64(r, c)))
}

pub fn pixel_info_at(state: &AppState, product: ProductId, x: f64, y: f64) -> Option<PixelInfo> {
    let p = state.product(product)?.read().unwrap();
    if !(x >= 0.0 && y >= 0.0 && x < p.width as f64 && y < p.height as f64) {
        return None;
    }
    let (ix, iy) = (x.floor() as u32, y.floor() as u32);
    // Geo is evaluated at the pixel centre (R27).
    let (cx, cy) = (ix as f64 + 0.5, iy as f64 + 0.5);
    let key = |band: &str| TileKey {
        product,
        band: band.into(),
        level: 0,
        tx: ix / TILE,
        ty: iy / TILE,
    };
    let out_of_band: Vec<bool> = p
        .bands
        .iter()
        .map(|b| ix >= b.width || iy >= b.height)
        .collect();
    let units = p.bands.iter().map(|b| b.unit.clone()).collect();
    let bands = p
        .bands
        .iter()
        .map(|b| {
            let v = if ix >= b.width || iy >= b.height {
                None
            } else {
                peek_value(state, key(&b.name), ix, iy).map(|(dt, raw)| {
                    let nd = b.no_data.is_some_and(|nd| match dt {
                        DataType::F32 => raw as f32 == nd as f32,
                        _ => raw == nd,
                    });
                    if nd {
                        return f64::NAN;
                    }
                    let v = raw * b.scaling.factor + b.scaling.offset;
                    if b.scaling.log10 {
                        10f64.powf(v)
                    } else {
                        v
                    }
                })
            };
            (b.name.clone(), v)
        })
        .collect();
    let masks = p
        .masks
        .iter()
        .map(|m| {
            let on = peek_value(state, key(&m.name), ix, iy).is_some_and(|(_, v)| v == 1.0);
            (m.name.clone(), on)
        })
        .collect();
    Some(PixelInfo {
        image_x: ix,
        image_y: iy,
        geo: p.geocoding.pixel_to_geo(cx, cy),
        crs_xy: p.geocoding.pixel_to_crs(cx, cy),
        crs_name: p
            .geocoding
            .crs()
            .map(|c| c.name.clone())
            .filter(|n| !n.is_empty()),
        bands,
        masks,
        units,
        out_of_band,
    })
}

/// 4 significant digits; scientific outside 1e-4..1e7.
fn sig4(v: f64) -> String {
    if !v.is_finite() {
        return if v.is_nan() {
            "NaN".into()
        } else {
            format!("{v}")
        };
    }
    if v == 0.0 {
        return "0".into();
    }
    let sci = format!("{v:.3e}");
    let exp: i32 = sci
        .split('e')
        .nth(1)
        .and_then(|e| e.parse().ok())
        .unwrap_or(0);
    if (-4..7).contains(&exp) {
        format!("{v:.*}", (3 - exp).max(0) as usize)
    } else {
        sci
    }
}

pub fn pixel_info_ui(ui: &mut egui::Ui, info: Option<&PixelInfo>) {
    let Some(i) = info else {
        ui.label("No pixel");
        return;
    };
    egui::Grid::new("pixel_info").striped(true).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(k);
            ui.label(v);
            ui.end_row();
        };
        let ll = |f: fn(&GeoPos) -> f64| {
            i.geo
                .as_ref()
                .map_or("—".into(), |g| format!("{:.6}", f(g)))
        };
        row("Image-X", i.image_x.to_string());
        row("Image-Y", i.image_y.to_string());
        row("Lat", ll(|g| g.lat));
        row("Lon", ll(|g| g.lon));
        if let Some((x, y)) = i.crs_xy {
            row("CRS X", format!("{x:.3}"));
            row("CRS Y", format!("{y:.3}"));
        }
        if let Some(n) = &i.crs_name {
            row("CRS", n.clone());
        }
    });
    ui.separator();
    egui::Grid::new("pixel_bands").striped(true).show(ui, |ui| {
        for (k, (n, v)) in i.bands.iter().enumerate() {
            ui.label(n);
            let text = match (v, i.out_of_band[k]) {
                (_, true) => "—".to_string(),
                (None, _) => "loading…".to_string(),
                (Some(v), _) => match &i.units[k] {
                    Some(u) => format!("{} {u}", sig4(*v)),
                    None => sig4(*v),
                },
            };
            ui.label(text);
            ui.end_row();
        }
        for (n, on) in &i.masks {
            ui.label(n);
            ui.label(if *on { "true" } else { "false" });
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fixtures;
    use snap_core::DataType;

    fn open(
        name: &str,
        w: u32,
        h: u32,
        epsg: Option<u32>,
    ) -> (AppState, ProductId, std::path::PathBuf) {
        snap_io_gdal::init_error_handler();
        let dir = fixtures::temp_dir(name);
        let fx = fixtures::make_fixture(&dir, "a.tif", w, h, DataType::U16, 2, false, None, epsg);
        let mut st = AppState::new();
        let id = st.open_path(&fx.path).unwrap();
        (st, id, dir)
    }

    fn prefetch(st: &AppState, id: ProductId, band: usize, tx: u32, ty: u32) {
        let (src, name) = {
            let p = st.product(id).unwrap().read().unwrap();
            (p.bands[band].source.clone(), p.bands[band].name.clone())
        };
        let key = TileKey {
            product: id,
            band: name,
            level: 0,
            tx,
            ty,
        };
        st.cache.get_blocking(&key, &src).unwrap();
    }

    #[test]
    fn pixel_info_reads_cached_value() {
        let (st, id, dir) = open("pi-read", 1000, 700, Some(32618));
        prefetch(&st, id, 0, 1, 1);
        st.product(id).unwrap().write().unwrap().bands[0].unit = Some("m".into());
        let i = pixel_info_at(&st, id, 600.0, 600.0).unwrap();
        assert_eq!(i.units[0].as_deref(), Some("m"));
        assert_eq!((i.image_x, i.image_y), (600, 600));
        assert_eq!(i.bands[0].1, Some(((600 * 1000 + 600) % 1000) as f64));
        assert_eq!(i.bands[1].1, None, "band 2 tile not cached");
        assert!(i.geo.is_some());
        assert_eq!(
            i.crs_xy,
            Some((700000.0 + 600.5 * 10.0, 5030000.0 - 600.5 * 10.0))
        );
        assert!(i.crs_name.is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn pixel_info_outside_image_is_none() {
        let (st, id, dir) = open("pi-out", 64, 32, Some(32618));
        assert!(pixel_info_at(&st, id, -1.0, 0.0).is_none());
        assert!(pixel_info_at(&st, id, 0.0, -0.5).is_none());
        assert!(pixel_info_at(&st, id, 64.0, 0.0).is_none());
        assert!(pixel_info_at(&st, id, 0.0, 32.0).is_none());
        assert!(pixel_info_at(&st, id, 63.9, 31.9).is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn pixel_info_without_georef_has_no_geo() {
        let (st, id, dir) = open("pi-nogeo", 64, 32, None);
        let i = pixel_info_at(&st, id, 3.0, 4.0).unwrap();
        assert!(i.geo.is_none() && i.crs_xy.is_none() && i.crs_name.is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn pixel_info_masks_cached_and_uncached() {
        use snap_core::{Mask, MemorySource, RasterSource};
        use std::sync::Arc;
        let (st, id, dir) = open("pi-mask", 64, 32, Some(32618));
        let mut data = ndarray::Array2::<f32>::zeros((32, 64));
        data[(4, 3)] = 1.0;
        let src: Arc<dyn RasterSource> = Arc::new(MemorySource::new(data));
        for name in ["m_cached", "m_cold"] {
            st.product(id).unwrap().write().unwrap().masks.push(Mask {
                name: name.into(),
                expr: String::new(),
                color: [255, 0, 0, 255],
                opacity: 0.5,
                source: src.clone(),
            });
        }
        let key = TileKey {
            product: id,
            band: "m_cached".into(),
            level: 0,
            tx: 0,
            ty: 0,
        };
        st.cache.get_blocking(&key, &src).unwrap();
        let i = pixel_info_at(&st, id, 3.0, 4.0).unwrap();
        assert_eq!(
            i.masks,
            vec![
                ("m_cached".to_string(), true),
                ("m_cold".to_string(), false)
            ]
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sig4_four_significant_digits() {
        assert_eq!(sig4(0.123456), "0.1235");
        assert_eq!(sig4(1234.6), "1235");
        assert_eq!(sig4(12.0), "12.00");
        assert_eq!(sig4(9.99996), "10.00");
        assert_eq!(sig4(0.0), "0");
        assert_eq!(sig4(-1.23456), "-1.235");
        assert_eq!(sig4(1.5e9), "1.500e9");
        assert_eq!(sig4(f64::NAN), "NaN");
    }
}
