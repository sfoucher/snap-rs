use crate::errors::{clear_gdal_error, init_error_handler, last_gdal_error, open_dataset};
use crate::metadata::metadata_tree;
use crate::source::{map_type, open_err, SharedDs};
use crate::GdalSource;
use gdal::raster::ColorInterpretation;
use gdal::{Dataset, Metadata};
use snap_core::{
    Band, ColorTable, Crs, CrsGeoCoding, DataType, GeoCoding, NoGeoCoding, Product, Pyramid,
    Scaling,
};
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
    let dimap = crate::dimap_names::read(path);
    let shared = SharedDs::default();
    for i in 1..=ds.raster_count() {
        let b = ds.rasterband(i).map_err(|e| open_err(path, e))?;
        let bt = b.band_type();
        let data_type = map_type(bt).ok_or_else(|| IoError::Open {
            path: path.to_path_buf(),
            msg: format!("unsupported data type {}", bt.name()),
        })?;
        let color_table = class_colors(&b, data_type);
        let mut desc = b.description().unwrap_or_default();
        let mut name = desc.clone();
        if let Some((n, d)) = dimap.get(&(i - 1)).filter(|_| name.is_empty()) {
            name = n.clone();
            desc = d.clone().unwrap_or_default();
        }
        let unit = b.unit();
        let (bw, bh) = b.size();
        let cache_dir = dirs::cache_dir().map(|d| {
            d.join("snap-rs")
                .join("pyramids")
                .join(&key)
                // class levels are not means, so they must not share sidecars with a mean pyramid
                .join(format!(
                    "band_{i}{}",
                    if color_table.is_some() { "_nn" } else { "" }
                ))
        });
        let mut pyramid = Pyramid::new(
            Arc::new(GdalSource::open_shared(path, i, shared.clone())?),
            cache_dir,
            b.no_data_value(),
        );
        if color_table.is_some() {
            pyramid = pyramid.nearest();
        }
        let source = Arc::new(pyramid);
        p.add_band(Band {
            name: if name.is_empty() {
                format!("band_{i}")
            } else {
                name
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
            color_table,
        })?;
    }
    p.metadata = metadata_tree(ds);
    p.geocoding = geocoding_from(ds)?;
    Ok(p)
}

/// Class colours of a paletted Byte band; other types are not rendered through a table.
fn class_colors(b: &gdal::raster::RasterBand, data_type: DataType) -> Option<ColorTable> {
    if data_type != DataType::U8 || b.color_interpretation() != ColorInterpretation::PaletteIndex {
        return None;
    }
    let t = b.color_table()?;
    let entries = (0..t.entry_count().min(256))
        .map(|i| {
            t.entry_as_rgb(i).map_or([0, 0, 0, 255], |e| {
                [e.r as u8, e.g as u8, e.b as u8, e.a as u8]
            })
        })
        .collect();
    Some(ColorTable { entries })
}

pub(crate) fn geocoding_from(ds: &Dataset) -> Result<Arc<dyn GeoCoding>, IoError> {
    let wkt = ds.projection();
    if wkt.is_empty() {
        return Ok(Arc::new(NoGeoCoding));
    }
    clear_gdal_error();
    let gt = match ds.geo_transform() {
        Ok(gt) => gt,
        Err(e) => {
            let m = last_gdal_error();
            log::warn!(
                "no geotransform, using NoGeoCoding: {}",
                if m.is_empty() { e.to_string() } else { m }
            );
            return Ok(Arc::new(NoGeoCoding));
        }
    };
    let srs = ds.spatial_ref().ok();
    let epsg = srs.as_ref().and_then(|s| {
        (s.auth_name().ok()?.eq_ignore_ascii_case("EPSG"))
            .then(|| s.auth_code().ok().and_then(|c| u32::try_from(c).ok()))
            .flatten()
    });
    let name = srs.and_then(|s| s.name().ok()).unwrap_or_default();
    // A CRS PROJ can't transform (or a singular transform) must not block opening (R18).
    match CrsGeoCoding::new(gt, Crs { wkt, epsg, name }) {
        Ok(g) => Ok(Arc::new(g)),
        Err(e) => {
            log::warn!("unusable geocoding, using NoGeoCoding: {e}");
            Ok(Arc::new(NoGeoCoding))
        }
    }
}
