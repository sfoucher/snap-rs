use crate::{
    BandStats, CoreError, DataType, GeoCoding, MetadataElement, NoGeoCoding, RasterSource, Scaling,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Copy, Clone, Eq, Hash, PartialEq)]
pub struct ProductId(pub u64);

#[derive(Clone)]
pub struct Band {
    pub name: String,
    pub data_type: DataType,
    pub width: u32,
    pub height: u32,
    pub no_data: Option<f64>,
    pub scaling: Scaling,
    pub unit: Option<String>,
    pub valid_pixel_expr: Option<String>,
    pub description: Option<String>,
    pub source: Arc<dyn RasterSource>,
    pub stats: OnceLock<BandStats>,
}

#[derive(Clone)]
pub struct Mask {
    pub name: String,
    pub expr: String,
    pub color: [u8; 4],
    pub opacity: f32,
    pub source: Arc<dyn RasterSource>,
}

#[derive(Clone)]
pub struct Product {
    pub id: ProductId,
    pub name: String,
    pub product_type: String,
    pub width: u32,
    pub height: u32,
    pub file_path: Option<PathBuf>,
    pub bands: Vec<Band>,
    pub masks: Vec<Mask>,
    pub metadata: MetadataElement,
    pub geocoding: Arc<dyn GeoCoding>,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl Product {
    pub fn new(name: &str, product_type: &str, width: u32, height: u32) -> Self {
        Self {
            id: ProductId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            name: name.into(),
            product_type: product_type.into(),
            width,
            height,
            file_path: None,
            bands: Vec::new(),
            masks: Vec::new(),
            metadata: MetadataElement::new("metadata"),
            geocoding: Arc::new(NoGeoCoding),
            start_time: None,
            end_time: None,
        }
    }

    pub fn band(&self, name: &str) -> Option<&Band> {
        self.bands.iter().find(|b| b.name == name)
    }

    pub fn band_index(&self, name: &str) -> Option<usize> {
        self.bands.iter().position(|b| b.name == name)
    }

    pub fn add_band(&mut self, band: Band) -> Result<(), CoreError> {
        if self.band(&band.name).is_some() {
            return Err(CoreError::Other(format!(
                "duplicate band name: {}",
                band.name
            )));
        }
        self.bands.push(band);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySource;

    fn band(name: &str) -> Band {
        Band {
            name: name.into(),
            data_type: DataType::F32,
            width: 1,
            height: 1,
            no_data: None,
            scaling: Scaling::default(),
            unit: None,
            valid_pixel_expr: None,
            description: None,
            source: Arc::new(MemorySource::new(ndarray::Array2::<f32>::zeros((1, 1)))),
            stats: OnceLock::new(),
        }
    }

    #[test]
    fn duplicate_band_name_rejected() {
        let mut p = Product::new("p", "T", 1, 1);
        assert!(p.add_band(band("B1")).is_ok());
        assert!(p.add_band(band("B1")).is_err());
        assert_eq!(p.bands.len(), 1);
        assert_eq!(p.band_index("B1"), Some(0));
        assert!(p.band("B1").is_some());
    }

    #[test]
    fn product_ids_are_unique() {
        assert_ne!(
            Product::new("a", "T", 1, 1).id,
            Product::new("b", "T", 1, 1).id
        );
    }
}
