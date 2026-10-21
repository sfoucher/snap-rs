use std::sync::Arc;

use ndarray::Array2;
use snap_core::{
    level_size, tile_rect, CoreError, DataType, GeoCoding, Product, Pyramid, RasterSource, Scaling,
    Tile,
};

use crate::{compile, parse, Compiled, ExprError, PixelCtx};

struct Input {
    source: Arc<dyn RasterSource>,
    no_data: Option<f64>,
    scaling: Scaling,
}

/// A band-maths expression over a product's bands, served as an ordinary tiled raster.
pub struct ExprSource {
    compiled: Compiled,
    inputs: Vec<Input>,
    size: (u32, u32),
    levels: u8,
    geocoding: Arc<dyn GeoCoding>,
}

impl ExprSource {
    pub fn new(expr_src: &str, product: &Product) -> Result<Self, ExprError> {
        let expr = parse(expr_src)?;
        let names: Vec<String> = product.bands.iter().map(|b| b.name.clone()).collect();
        let compiled = compile(&expr, &names)?;
        let inputs: Vec<Input> = compiled
            .bands
            .iter()
            .filter_map(|n| product.band(n))
            .map(|b| Input {
                source: b.source.clone(),
                no_data: b.no_data,
                scaling: b.scaling,
            })
            .collect();
        let size = inputs
            .first()
            .map_or((product.width, product.height), |i| i.source.size());
        if inputs.iter().any(|i| i.source.size() != size) {
            return Err(ExprError::TypeMismatch {
                msg: "referenced bands differ in size".into(),
                span: expr.span(),
            });
        }
        let levels = inputs
            .iter()
            .map(|i| i.source.levels())
            .min()
            .unwrap_or_else(|| Pyramid::needed_levels(size.0, size.1));
        Ok(ExprSource {
            compiled,
            inputs,
            size,
            levels,
            geocoding: product.geocoding.clone(),
        })
    }
}

/// no_data (compared on the raw value, in the band's own type) → NaN, then scaling.
fn geophysical(t: &Tile, no_data: Option<f64>, s: Scaling) -> Array2<f32> {
    let is_f32 = matches!(t, Tile::F32(_));
    Array2::from_shape_fn(t.shape(), |(r, c)| {
        let raw = t.get_f64(r, c);
        let hit = match no_data {
            // A no_data like -3.4e38 isn't exact in f32; widening the pixel wouldn't match it.
            Some(nd) if is_f32 => raw as f32 == nd as f32,
            Some(nd) => raw == nd,
            None => false,
        };
        if hit {
            return f32::NAN;
        }
        let v = raw * s.factor + s.offset;
        (if s.log10 { 10f64.powf(v) } else { v }) as f32
    })
}

impl RasterSource for ExprSource {
    fn data_type(&self) -> DataType {
        if self.compiled.is_boolean {
            DataType::U8
        } else {
            DataType::F32
        }
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn levels(&self) -> u8 {
        self.levels
    }

    fn read_tile(&self, level: u8, tx: u32, ty: u32) -> Result<Tile, CoreError> {
        let (lw, lh) = level_size(self.size.0, self.size.1, level);
        let rect = tile_rect(lw, lh, tx, ty)
            .filter(|_| level < self.levels)
            .ok_or(CoreError::OutOfRange { level, tx, ty })?;
        let arrays = self
            .inputs
            .iter()
            .map(|i| {
                let t = i.source.read_tile(level, tx, ty)?;
                Ok(geophysical(&t, i.no_data, i.scaling))
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        let ctx = PixelCtx {
            origin_x: rect.x,
            origin_y: rect.y,
            level,
            geocoding: Some(self.geocoding.as_ref()),
        };
        let shape = (rect.h as usize, rect.w as usize);
        Ok(self.compiled.eval_arrays(&arrays, shape, &ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::{arr2, Array2};
    use snap_core::{Band, Crs, CrsGeoCoding, GeoCoding, MemorySource, Scaling};
    use std::sync::{Arc, OnceLock};

    // Copied from snap-core's geocoding tests (test helpers can't be shared across crates).
    const UTM18N_WKT: &str = r#"PROJCS["WGS 84 / UTM zone 18N",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0,AUTHORITY["EPSG","8901"]],UNIT["degree",0.0174532925199433,AUTHORITY["EPSG","9122"]],AUTHORITY["EPSG","4326"]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",-75],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],PARAMETER["false_northing",0],UNIT["metre",1,AUTHORITY["EPSG","9001"]],AXIS["Easting",EAST],AXIS["Northing",NORTH],AUTHORITY["EPSG","32618"]]"#;

    fn band(name: &str, data: Array2<f32>, no_data: Option<f64>, scaling: Scaling) -> Band {
        Band {
            name: name.into(),
            data_type: DataType::F32,
            width: data.ncols() as u32,
            height: data.nrows() as u32,
            no_data,
            scaling,
            unit: None,
            valid_pixel_expr: None,
            description: None,
            source: Arc::new(MemorySource::new(data)),
            stats: OnceLock::new(),
            color_table: None,
        }
    }

    #[test]
    fn nodata_becomes_nan() {
        let mut p = Product::new("p", "T", 2, 1);
        p.add_band(band(
            "B1",
            arr2(&[[1.0, -9999.0]]),
            Some(-9999.0),
            Scaling::default(),
        ))
        .unwrap();
        let s = ExprSource::new("B1 * 1", &p).unwrap();
        assert_eq!(s.data_type(), DataType::F32);
        let Tile::F32(a) = s.read_tile(0, 0, 0).unwrap() else {
            panic!("expected F32")
        };
        assert_eq!(a[(0, 0)], 1.0);
        assert!(a[(0, 1)].is_nan());
    }

    #[test]
    fn f32_nodata_compared_in_f32() {
        // -3.4e38 is not exactly representable in f32; the comparison must happen in f32.
        let mut p = Product::new("p", "T", 2, 1);
        p.add_band(band(
            "B1",
            arr2(&[[1.0, -3.4e38_f32]]),
            Some(-3.4e38),
            Scaling::default(),
        ))
        .unwrap();
        let Tile::F32(a) = ExprSource::new("B1", &p)
            .unwrap()
            .read_tile(0, 0, 0)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(a[(0, 0)], 1.0);
        assert!(a[(0, 1)].is_nan());
    }

    #[test]
    fn scaling_applied_before_eval() {
        let mut p = Product::new("p", "T", 1, 1);
        let sc = Scaling {
            factor: 2.0,
            offset: 1.0,
            log10: false,
        };
        p.add_band(band("B1", arr2(&[[3.0]]), None, sc)).unwrap();
        let Tile::F32(a) = ExprSource::new("B1", &p)
            .unwrap()
            .read_tile(0, 0, 0)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(a[(0, 0)], 7.0);
    }

    #[test]
    fn lat_lon_pseudo_bands() {
        let gc = CrsGeoCoding::new(
            [700_000.0, 10.0, 0.0, 5_030_000.0, 0.0, -10.0],
            Crs {
                wkt: UTM18N_WKT.into(),
                epsg: Some(32618),
                name: "WGS 84 / UTM zone 18N".into(),
            },
        )
        .unwrap();
        let want = gc.pixel_to_geo(0.5, 0.5).unwrap();
        let mut p = Product::new("p", "T", 4, 4);
        p.geocoding = Arc::new(gc);
        for (e, w) in [("LAT", want.lat), ("LON", want.lon)] {
            let s = ExprSource::new(e, &p).unwrap();
            assert_eq!(s.size(), (4, 4));
            let Tile::F32(a) = s.read_tile(0, 0, 0).unwrap() else {
                panic!()
            };
            assert_eq!(a.dim(), (4, 4));
            assert!(
                (a[(0, 0)] - w as f32).abs() < 1e-5,
                "{e}: {} vs {w}",
                a[(0, 0)]
            );
        }
    }

    #[test]
    fn boolean_source_is_u8_and_out_of_range_errors() {
        let mut p = Product::new("p", "T", 2, 1);
        p.add_band(band("B1", arr2(&[[1.0, 5.0]]), None, Scaling::default()))
            .unwrap();
        let s = ExprSource::new("B1 > 2", &p).unwrap();
        assert_eq!(s.data_type(), DataType::U8);
        let Tile::U8(a) = s.read_tile(0, 0, 0).unwrap() else {
            panic!()
        };
        assert_eq!(a, arr2(&[[0u8, 1]]));
        assert!(matches!(
            s.read_tile(0, 1, 0),
            Err(CoreError::OutOfRange { .. })
        ));
    }

    #[test]
    fn mismatched_band_sizes_rejected() {
        let mut p = Product::new("p", "T", 2, 1);
        p.add_band(band("A", arr2(&[[1.0, 2.0]]), None, Scaling::default()))
            .unwrap();
        p.add_band(band("B", arr2(&[[1.0]]), None, Scaling::default()))
            .unwrap();
        assert!(matches!(
            ExprSource::new("A + B", &p),
            Err(ExprError::TypeMismatch { .. })
        ));
    }
}
