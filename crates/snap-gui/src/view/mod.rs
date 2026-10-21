mod canvas;
mod gpu;

pub use canvas::canvas_ui;
pub use gpu::GpuState;

use snap_core::{level_size, tile_rect, ColorTable, DataType, ProductId, Rect, TILE};

#[derive(Clone, Debug, PartialEq)]
pub struct Stretch {
    pub min: f32,
    pub max: f32,
    pub gamma: f32,
}

impl Stretch {
    /// Full range of the data type (floats: 0..1) until band statistics are available.
    pub fn for_type(dt: DataType) -> Self {
        let (min, max) = match dt {
            DataType::U8 => (0.0, u8::MAX as f32),
            DataType::I8 => (i8::MIN as f32, i8::MAX as f32),
            DataType::U16 => (0.0, u16::MAX as f32),
            DataType::I16 => (i16::MIN as f32, i16::MAX as f32),
            DataType::U32 => (0.0, u32::MAX as f32),
            DataType::I32 => (i32::MIN as f32, i32::MAX as f32),
            DataType::F32 | DataType::F64 => (0.0, 1.0),
        };
        Stretch {
            min,
            max,
            gamma: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Palette {
    pub name: String,
    pub stops: Vec<(f32, [u8; 3])>,
    /// Class colours by pixel value, for paletted rasters: `to_lut` returns these as they are.
    pub exact: Option<Box<[[u8; 4]; 256]>>,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            name: "Grey".into(),
            stops: vec![(0.0, [0, 0, 0]), (1.0, [255, 255, 255])],
            exact: None,
        }
    }
}

impl Palette {
    /// One colour per pixel value; values past the table are opaque black. Render it with the
    /// stretch 0..255, gamma 1, so that value i reads LUT entry i.
    pub fn categorical(table: &ColorTable) -> Self {
        let mut lut = [[0, 0, 0, 255]; 256];
        for (out, c) in lut.iter_mut().zip(&table.entries) {
            *out = *c;
        }
        Palette {
            name: "Classes".into(),
            exact: Some(Box::new(lut)),
            ..Palette::default()
        }
    }

    /// (value, colour) of the classes worth listing: visible and not black.
    /// ponytail: opaque black is read as "unused", GDAL pads GeoTIFF tables with it; a real
    /// black class is not listed.
    pub fn classes(&self) -> Vec<(usize, [u8; 3])> {
        self.exact.as_ref().map_or_else(Vec::new, |lut| {
            (0..256)
                .filter(|&i| lut[i][3] > 0 && lut[i][..3] != [0, 0, 0])
                .map(|i| (i, [lut[i][0], lut[i][1], lut[i][2]]))
                .collect()
        })
    }

    pub fn to_lut(&self) -> [[u8; 4]; 256] {
        if let Some(lut) = &self.exact {
            return **lut;
        }
        let mut lut = [[0, 0, 0, 255]; 256];
        for (i, out) in lut.iter_mut().enumerate() {
            let t = i as f32 / 255.0;
            let hi = self
                .stops
                .iter()
                .position(|s| s.0 >= t)
                .unwrap_or(self.stops.len() - 1);
            let (t1, c1) = self.stops[hi];
            let (t0, c0) = self.stops[hi.saturating_sub(1)];
            let f = if t1 > t0 {
                ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            for k in 0..3 {
                out[k] = (c0[k] as f32 + (c1[k] as f32 - c0[k] as f32) * f).round() as u8;
            }
        }
        lut
    }
}

pub enum ViewSources {
    Single(String),
    Rgb([String; 3]),
}

pub struct ViewState {
    pub product: ProductId,
    pub sources: ViewSources,
    pub stretch: [Stretch; 3],
    pub palette: Palette,
    /// Screen points per level-0 pixel; <= 0 means "fit on first draw".
    pub zoom: f64,
    /// Level-0 pixel at the canvas centre.
    pub center: (f64, f64),
    pub show_masks: bool,
    pub title: String,
}

impl ViewState {
    pub fn single(
        product: ProductId,
        band: &str,
        dt: DataType,
        size: (u32, u32),
        title: String,
    ) -> Self {
        let s = Stretch::for_type(dt);
        ViewState {
            product,
            sources: ViewSources::Single(band.into()),
            stretch: [s.clone(), s.clone(), s],
            palette: Palette::default(),
            zoom: 0.0,
            center: (size.0 as f64 / 2.0, size.1 as f64 / 2.0),
            show_masks: true,
            title,
        }
    }
}

/// Largest n with 2^-n >= zoom, clamped to max_level; zoom >= 1 → 0.
pub fn level_for_zoom(zoom: f64, max_level: u8) -> u8 {
    let mut n = 0;
    while n < max_level && 0.5f64.powi(n as i32 + 1) >= zoom {
        n += 1;
    }
    n
}

pub fn visible_tiles(
    view: &ViewState,
    viewport: egui::Rect,
    img_w: u32,
    img_h: u32,
    level: u8,
) -> Vec<(u32, u32, Rect)> {
    let (lw, lh) = level_size(img_w, img_h, level);
    let scale = view.zoom * (1u64 << level) as f64;
    let c = viewport.center();
    // Viewport edges in level pixels, clamped to the level's extent.
    let to_level = |screen: f32, sc: f32, vc: f64, len: u32| {
        (vc / (1u64 << level) as f64 + (screen - sc) as f64 / scale).clamp(0.0, len as f64)
    };
    let (x0, x1) = (
        to_level(viewport.min.x, c.x, view.center.0, lw),
        to_level(viewport.max.x, c.x, view.center.0, lw),
    );
    let (y0, y1) = (
        to_level(viewport.min.y, c.y, view.center.1, lh),
        to_level(viewport.max.y, c.y, view.center.1, lh),
    );
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let t = TILE as f64;
    let mut out = Vec::new();
    for ty in (y0 / t) as u32..(y1 / t).ceil() as u32 {
        for tx in (x0 / t) as u32..(x1 / t).ceil() as u32 {
            out.extend(tile_rect(lw, lh, tx, ty).map(|r| (tx, ty, r)));
        }
    }
    out
}

#[cfg(test)]
pub(crate) fn test_view(zoom: f64, center: (f64, f64)) -> ViewState {
    ViewState {
        zoom,
        center,
        ..ViewState::single(ProductId(0), "b", DataType::U8, (0, 0), "t".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Rect as ERect};

    #[test]
    fn level_for_zoom_table() {
        assert_eq!(level_for_zoom(1.0, 8), 0);
        assert_eq!(level_for_zoom(0.5, 8), 1);
        assert_eq!(level_for_zoom(0.3, 8), 1);
        assert_eq!(level_for_zoom(0.25, 8), 2);
        assert_eq!(level_for_zoom(0.001, 3), 3);
    }

    #[test]
    fn visible_tiles_includes_clipped_edge() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        // 1000x700 at zoom 0.5 is 500x350 screen px; the viewport covers it all.
        let v = test_view(0.5, (500.0, 350.0));
        let vp = ERect::from_min_max(pos2(0.0, 0.0), pos2(500.0, 350.0));
        assert_eq!(
            visible_tiles(&v, vp, 1000, 700, 1),
            vec![(0, 0, r(0, 0, 500, 350))]
        );
        // zoom 1, a 200x100 viewport centred over the bottom-right corner.
        let v = test_view(1.0, (900.0, 650.0));
        let vp = ERect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 100.0));
        let tiles = visible_tiles(&v, vp, 1000, 700, 0);
        assert!(tiles.contains(&(1, 1, r(512, 512, 488, 188))), "{tiles:?}");
    }

    #[test]
    fn class_palette_maps_each_value_to_its_colour() {
        let ct = ColorTable {
            entries: vec![
                [0, 0, 0, 0],
                [255, 255, 0, 255],
                [0, 100, 0, 255],
                [0, 0, 0, 255],
            ],
        };
        let p = Palette::categorical(&ct);
        let lut = p.to_lut();
        assert_eq!(lut[1], [255, 255, 0, 255]);
        assert_eq!(lut[2], [0, 100, 0, 255]);
        assert_eq!(lut[200], [0, 0, 0, 255], "past the table");
        // nodata (alpha 0) and black padding are not listed
        assert_eq!(p.classes(), vec![(1, [255, 255, 0]), (2, [0, 100, 0])]);
        assert!(Palette::default().classes().is_empty());
    }

    #[test]
    fn palette_lut_endpoints() {
        let lut = Palette::default().to_lut();
        assert_eq!(lut[0], [0, 0, 0, 255]);
        assert_eq!(lut[255], [255, 255, 255, 255]);
    }
}
