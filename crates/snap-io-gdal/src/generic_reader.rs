use crate::errors::{init_error_handler, open_dataset};
use crate::geotiff_reader::build_product;
use crate::source::open_err;
use snap_core::Product;
use snap_io::{IoError, Probe, ProductReader};
use std::path::Path;

pub struct GdalGenericReader;

impl ProductReader for GdalGenericReader {
    fn name(&self) -> &'static str {
        "GDAL"
    }

    fn can_read(&self, path: &Path) -> Probe {
        init_error_handler();
        match open_dataset(path) {
            Ok(_) => Probe::Maybe(10),
            Err(_) => Probe::No,
        }
    }

    fn read(&self, path: &Path) -> Result<Product, IoError> {
        init_error_handler();
        let ds = open_dataset(path).map_err(|e| open_err(path, e))?;
        let t = format!("GDAL:{}", ds.driver().short_name());
        build_product(&ds, path, &t)
    }
}
