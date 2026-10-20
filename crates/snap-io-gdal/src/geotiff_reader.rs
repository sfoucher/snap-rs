use crate::errors::{init_error_handler, open_dataset};
use crate::metadata::metadata_tree;
use crate::source::{map_type, open_err};
use crate::GdalSource;
use gdal::{Dataset, Metadata};
use snap_core::{Band, Crs, CrsGeoCoding, GeoCoding, NoGeoCoding, Product, Pyramid, Scaling};
use snap_io::{IoError, Probe, ProductReader};
use std::path::Path;
use std::sync::{Arc, OnceLock};

pub struct GeoTiffReader;

impl ProductReader for GeoTiffReader {
    fn name(&self) -> &'static str {
        "GeoTIFF"
    }

    fn can_read(&self, path: &Path) -> Probe {
        let tif = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff"));
        if !tif {
            return Probe::No;
        }
        init_error_handler();
        match open_dataset(path) {
            Ok(ds) if matches!(ds.driver().short_name().as_str(), "GTiff" | "COG") => {
                Probe::Yes(100)
            }
            _ => Probe::No,
        }
    }

    fn read(&self, path: &Path) -> Result<Product, IoError> {
        init_error_handler();
        let ds = open_dataset(path).map_err(|e| open_err(path, e))?;
        build_product(&ds, path, "GeoTIFF")
    }
}

pub(crate) fn build_product(
    ds: &Dataset,
    path: &Path,
    product_type: &str,
) -> Result<Product, IoError> {
    let name = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let (w, h) = ds.raster_size();
    let mut p = Product::new(&name, product_type, w as u32, h as u32);
    p.file_path = Some(path.to_path_buf());
    let key = Pyramid::cache_key(path)?;
    for i in 1..=ds.raster_count() {
        let b = ds.rasterband(i).map_err(|e| open_err(path, e))?;
        let bt = b.band_type();
        let data_type = map_type(bt).ok_or_else(|| IoError::Open {
            path: path.to_path_buf(),
            msg: format!("unsupported data type {}", bt.name()),
        })?;
        let desc = b.description().unwrap_or_default();
        let unit = b.unit();
        let (bw, bh) = b.size();
        let cache_dir = dirs::cache_dir().map(|d| {
            d.join("snap-rs")
                .join("pyramids")
                .join(&key)
                .join(format!("band_{i}"))
        });
        let source = Arc::new(Pyramid::new(
            Arc::new(GdalSource::open(path, i)?),
            cache_dir,
        ));
        p.add_band(Band {
            name: if desc.is_empty() {
                format!("band_{i}")
            } else {
                desc.clone()
            },
            data_type,
            width: bw as u32,
            height: bh as u32,
            no_data: b.no_data_value(),
            scaling: Scaling {
                factor: b.scale().unwrap_or(1.0),
                offset: b.offset().unwrap_or(0.0),
                log10: false,
            },
            unit: (!unit.is_empty()).then_some(unit),
            valid_pixel_expr: None,
            description: (!desc.is_empty()).then_some(desc),
            source,
            stats: OnceLock::new(),
        })?;
    }
    p.metadata = metadata_tree(ds);
    p.geocoding = geocoding_from(ds)?;
    Ok(p)
}

pub(crate) fn geocoding_from(ds: &Dataset) -> Result<Arc<dyn GeoCoding>, IoError> {
    let wkt = ds.projection();
    if wkt.is_empty() {
        return Ok(Arc::new(NoGeoCoding));
    }
    let gt = ds.geo_transform().map_err(|e| IoError::Open {
        path: ds.description().unwrap_or_default().into(),
        msg: e.to_string(),
    })?;
    let srs = ds.spatial_ref().ok();
    let epsg = srs.as_ref().and_then(|s| {
        (s.auth_name().ok()?.eq_ignore_ascii_case("EPSG"))
            .then(|| s.auth_code().ok().and_then(|c| u32::try_from(c).ok()))
            .flatten()
    });
    let name = srs.and_then(|s| s.name().ok()).unwrap_or_default();
    Ok(Arc::new(CrsGeoCoding::new(gt, Crs { wkt, epsg, name })?))
}
