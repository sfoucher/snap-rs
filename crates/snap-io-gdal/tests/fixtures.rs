#![allow(dead_code)] // each test binary uses a subset
use gdal::raster::{Buffer, RasterCreationOptions};
use gdal::spatial_ref::SpatialRef;
use gdal::DriverManager;
use snap_core::DataType;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub struct Fixture {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub gt: [f64; 6],
    pub epsg: u32,
    pub dtype: DataType,
}

/// Unique scratch dir under the system temp dir; caller removes it.
pub fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "snap-rs-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Tiled GTiff. Pixel value of band k (1-based) = (row*w + col + (k-1)*100) % 1000.
/// U8/I8 values wrap when cast (values reach 999).
/// Deviation from the plan text ("k*1000 before the modulo"), which is a no-op and
/// would make all bands identical (ruling R17); band 1 is unchanged.
#[allow(clippy::too_many_arguments)]
pub fn make_fixture(
    dir: &Path,
    name: &str,
    w: u32,
    h: u32,
    dtype: DataType,
    bands: usize,
    overviews: bool,
    nodata: Option<f64>,
    epsg: Option<u32>,
) -> Fixture {
    let path = dir.join(name);
    let mut opts = RasterCreationOptions::new();
    for (k, v) in [
        ("TILED", "YES"),
        ("BLOCKXSIZE", "256"),
        ("BLOCKYSIZE", "256"),
    ] {
        opts.set_name_value(k, v).unwrap();
    }
    let drv = DriverManager::get_driver_by_name("GTiff").unwrap();
    let (wu, hu) = (w as usize, h as usize);

    macro_rules! build {
        ($t:ty) => {{
            let mut ds = drv
                .create_with_band_type_with_options::<$t, _>(&path, wu, hu, bands, &opts)
                .expect("create");
            for k in 1..=bands {
                let data: Vec<$t> = (0..wu * hu)
                    .map(|i| ((i + (k - 1) * 100) % 1000) as $t)
                    .collect();
                let mut b = ds.rasterband(k).unwrap();
                b.write((0, 0), (wu, hu), &mut Buffer::new((wu, hu), data))
                    .unwrap();
                if nodata.is_some() {
                    b.set_no_data_value(nodata).unwrap();
                }
            }
            let gt = match epsg {
                Some(e) => {
                    let gt = [700000.0, 10.0, 0.0, 5030000.0, 0.0, -10.0];
                    ds.set_geo_transform(&gt).unwrap();
                    ds.set_projection(&SpatialRef::from_epsg(e).unwrap().to_wkt().unwrap())
                        .unwrap();
                    gt
                }
                None => [0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            };
            if overviews {
                ds.build_overviews("NEAREST", &[2], &[]).unwrap();
            }
            gt
        }};
    }
    let gt = match dtype {
        DataType::U8 => build!(u8),
        DataType::I8 => build!(i8),
        DataType::U16 => build!(u16),
        DataType::I16 => build!(i16),
        DataType::U32 => build!(u32),
        DataType::I32 => build!(i32),
        DataType::F32 => build!(f32),
        DataType::F64 => build!(f64),
    };
    Fixture {
        path,
        width: w,
        height: h,
        gt,
        epsg: epsg.unwrap_or(0),
        dtype,
    }
}
