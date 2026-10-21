use crate::errors::{clear_gdal_error, init_error_handler, last_gdal_error};
use gdal::raster::{Buffer, GdalType, RasterCreationOptions};
use gdal::{Dataset, DriverManager, Metadata};
use snap_core::{
    tile_count, tile_rect, Band, DataType, Product, Pyramid, Rect, TileCache, TileKey,
};
use snap_io::{Compression, IoError, ProductWriter, WriteOpts};
use std::path::{Path, PathBuf};

pub struct GeoTiffWriter;

fn gerr(e: gdal::errors::GdalError) -> IoError {
    let m = last_gdal_error();
    IoError::Write(if m.is_empty() { e.to_string() } else { m })
}

/// R11: the common type if all equal, else F64 if any F64, else F32.
fn output_type(bands: &[&Band]) -> DataType {
    let first = bands[0].data_type;
    if bands.iter().all(|b| b.data_type == first) {
        first
    } else if bands.iter().any(|b| b.data_type == DataType::F64) {
        DataType::F64
    } else {
        DataType::F32
    }
}

fn compress_name(c: Compression) -> &'static str {
    match c {
        Compression::None => "NONE",
        Compression::Deflate => "DEFLATE",
        Compression::Lzw => "LZW",
        Compression::Zstd => "ZSTD",
    }
}

fn options(pairs: &[(&str, &str)]) -> Result<RasterCreationOptions, IoError> {
    let mut o = RasterCreationOptions::new();
    for (k, v) in pairs {
        o.set_name_value(k, v).map_err(gerr)?;
    }
    Ok(o)
}

fn geo_transform(p: &Product, x0: u32, y0: u32) -> Option<[f64; 6]> {
    let (x, y) = (f64::from(x0), f64::from(y0));
    let o = p.geocoding.pixel_to_crs(x, y)?;
    let r = p.geocoding.pixel_to_crs(x + 1.0, y)?;
    let d = p.geocoding.pixel_to_crs(x, y + 1.0)?;
    Some([o.0, r.0 - o.0, d.0 - o.0, o.1, r.1 - o.1, d.1 - o.1])
}

fn inside(r: Rect, w: u32, h: u32) -> bool {
    r.w > 0
        && r.h > 0
        && u64::from(r.x) + u64::from(r.w) <= u64::from(w)
        && u64::from(r.y) + u64::from(r.h) <= u64::from(h)
}

struct Plan<'a> {
    product: &'a Product,
    bands: Vec<&'a Band>,
    rect: Rect,
    cache: &'a TileCache,
}

/// Create the dataset as type `T` and fill every band, converting through f64.
fn write_pixels<T: GdalType + Copy>(
    plan: &Plan,
    path: &Path,
    opts: &RasterCreationOptions,
    conv: fn(f64) -> T,
) -> Result<Dataset, IoError> {
    let (rw, rh) = (plan.rect.w as usize, plan.rect.h as usize);
    let drv = DriverManager::get_driver_by_name("GTiff").map_err(gerr)?;
    let mut ds = drv
        .create_with_band_type_with_options::<T, _>(path, rw, rh, plan.bands.len(), opts)
        .map_err(gerr)?;
    if let Some(gt) = geo_transform(plan.product, plan.rect.x, plan.rect.y) {
        ds.set_geo_transform(&gt).map_err(gerr)?;
        if let Some(crs) = plan.product.geocoding.crs() {
            ds.set_projection(&crs.wkt).map_err(gerr)?;
        }
    }
    let r = plan.rect;
    for (i, band) in plan.bands.iter().enumerate() {
        let mut out = ds.rasterband(i + 1).map_err(gerr)?;
        if band.no_data.is_some() {
            out.set_no_data_value(band.no_data).map_err(gerr)?;
        }
        let s = band.scaling;
        if s.factor != 1.0 || s.offset != 0.0 {
            out.set_scale(s.factor).map_err(gerr)?;
            out.set_offset(s.offset).map_err(gerr)?;
        }
        out.set_description(&band.name).map_err(gerr)?;
        let (tcx, tcy) = tile_count(band.width, band.height);
        for ty in 0..tcy {
            for tx in 0..tcx {
                let Some(t) = tile_rect(band.width, band.height, tx, ty) else {
                    continue;
                };
                let (x0, y0) = (t.x.max(r.x), t.y.max(r.y));
                let (x1, y1) = ((t.x + t.w).min(r.x + r.w), (t.y + t.h).min(r.y + r.h));
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                let key = TileKey {
                    product: plan.product.id,
                    band: band.name.clone(),
                    level: 0,
                    tx,
                    ty,
                };
                let tile = plan.cache.get_blocking(&key, &band.source)?;
                let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
                let mut data = Vec::with_capacity(w * h);
                for y in y0..y1 {
                    for x in x0..x1 {
                        data.push(conv(tile.get_f64((y - t.y) as usize, (x - t.x) as usize)));
                    }
                }
                clear_gdal_error();
                out.write(
                    ((x0 - r.x) as isize, (y0 - r.y) as isize),
                    (w, h),
                    &mut Buffer::new((w, h), data),
                )
                .map_err(gerr)?;
            }
        }
    }
    Ok(ds)
}

impl ProductWriter for GeoTiffWriter {
    fn name(&self) -> &'static str {
        "GeoTIFF"
    }

    fn write(
        &self,
        product: &Product,
        path: &Path,
        opts: &WriteOpts,
        cache: &TileCache,
    ) -> Result<(), IoError> {
        init_error_handler();
        clear_gdal_error();
        let full = Rect {
            x: 0,
            y: 0,
            w: product.width,
            h: product.height,
        };
        let (rect, names) = opts
            .subset
            .as_ref()
            .map_or((full, &[][..]), |s| (s.rect, &s.bands[..]));
        if !inside(rect, product.width, product.height) {
            return Err(IoError::Write(format!(
                "subset {rect:?} is empty or outside the {}x{} product",
                product.width, product.height
            )));
        }
        let bands: Vec<&Band> = if names.is_empty() {
            product.bands.iter().collect()
        } else {
            names
                .iter()
                .map(|n| {
                    product
                        .bands
                        .iter()
                        .find(|b| &b.name == n)
                        .ok_or_else(|| IoError::Write(format!("unknown band '{n}'")))
                })
                .collect::<Result<_, _>>()?
        };
        if let Some(b) = bands
            .iter()
            .find(|b| (b.width, b.height) != (product.width, product.height))
        {
            return Err(IoError::Write(format!(
                "band '{}' is {}x{}, product is {}x{}",
                b.name, b.width, b.height, product.width, product.height
            )));
        }
        if bands.is_empty() {
            return Err(IoError::Write("no bands to write".into()));
        }
        // GTiff stores one nodata for all bands; NaN-safe comparison via bits.
        let nd = bands[0].no_data.map(f64::to_bits);
        if bands.iter().any(|b| b.no_data.map(f64::to_bits) != nd) {
            return Err(IoError::Write(
                "bands have different no-data values; export them separately".into(),
            ));
        }
        let plan = Plan {
            product,
            bands,
            rect,
            cache,
        };
        let comp = compress_name(opts.compression);
        // Write beside the target and rename on success, so a failed export never
        // clobbers an existing file. COG is copied from the plain tiled temp file
        // (the COG driver builds its own overviews) into a second temp.
        let sibling = |suffix: &str| -> PathBuf {
            let mut s = path.as_os_str().to_owned();
            s.push(suffix);
            s.into()
        };
        let tmp = sibling(".tmp.tif");
        let cog_tmp = sibling(".cog.tmp.tif");
        let co = options(&[
            ("TILED", "YES"),
            ("BLOCKXSIZE", "512"),
            ("BLOCKYSIZE", "512"),
            ("COMPRESS", comp),
            ("BIGTIFF", "IF_SAFER"),
        ])?;
        let result = (|| -> Result<(), IoError> {
            clear_gdal_error();
            let mut ds = match output_type(&plan.bands) {
                DataType::U8 => write_pixels(&plan, &tmp, &co, |v| v as u8)?,
                DataType::I8 => write_pixels(&plan, &tmp, &co, |v| v as i8)?,
                DataType::U16 => write_pixels(&plan, &tmp, &co, |v| v as u16)?,
                DataType::I16 => write_pixels(&plan, &tmp, &co, |v| v as i16)?,
                DataType::U32 => write_pixels(&plan, &tmp, &co, |v| v as u32)?,
                DataType::I32 => write_pixels(&plan, &tmp, &co, |v| v as i32)?,
                DataType::F32 => write_pixels(&plan, &tmp, &co, |v| v as f32)?,
                DataType::F64 => write_pixels(&plan, &tmp, &co, |v| v)?,
            };
            if opts.cog {
                let cog_opts = options(&[
                    ("COMPRESS", comp),
                    ("BIGTIFF", "IF_SAFER"),
                    ("RESAMPLING", "AVERAGE"),
                ])?;
                let drv = DriverManager::get_driver_by_name("COG").map_err(gerr)?;
                clear_gdal_error();
                drop(ds.create_copy(&drv, &cog_tmp, &cog_opts).map_err(gerr)?);
                drop(ds);
                std::fs::rename(&cog_tmp, path).map_err(|e| IoError::Write(e.to_string()))?;
            } else {
                let n = Pyramid::needed_levels(rect.w, rect.h);
                let factors: Vec<i32> = (1..n).map(|l| 1 << l).collect();
                if !factors.is_empty() {
                    clear_gdal_error();
                    ds.build_overviews("AVERAGE", &factors, &[]).map_err(gerr)?;
                }
                drop(ds); // flush and close before the rename
                std::fs::rename(&tmp, path).map_err(|e| IoError::Write(e.to_string()))?;
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&cog_tmp);
        result
    }
}
