use crate::errors::{init_error_handler, last_gdal_error};
use gdal::raster::{GdalDataType, GdalType};
use gdal::Dataset;
use ndarray::Array2;
use snap_core::{tile_rect, CoreError, DataType, RasterSource, Tile};
use snap_io::IoError;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct GdalSource {
    path: PathBuf,
    band: usize,
    data_type: DataType,
    size: (u32, u32),
    overviews: u8,
    ds: Mutex<Option<Dataset>>,
}

fn open_err(path: &Path, e: impl std::fmt::Display) -> IoError {
    let mut msg = last_gdal_error();
    if msg.is_empty() {
        msg = e.to_string();
    }
    IoError::Open {
        path: path.to_path_buf(),
        msg,
    }
}

fn map_type(t: GdalDataType) -> Option<DataType> {
    Some(match t {
        GdalDataType::UInt8 => DataType::U8,
        GdalDataType::Int8 => DataType::I8,
        GdalDataType::UInt16 => DataType::U16,
        GdalDataType::Int16 => DataType::I16,
        GdalDataType::UInt32 => DataType::U32,
        GdalDataType::Int32 => DataType::I32,
        GdalDataType::Float32 => DataType::F32,
        GdalDataType::Float64 => DataType::F64,
        _ => return None,
    })
}

fn read<T: Copy + GdalType>(
    b: &gdal::raster::RasterBand,
    off: (isize, isize),
    wh: (usize, usize),
) -> Result<Array2<T>, CoreError> {
    let buf = b
        .read_as::<T>(off, wh, wh, None)
        .map_err(|e| CoreError::Other(gdal_msg(e)))?;
    let (_, v) = buf.into_shape_and_vec();
    Array2::from_shape_vec((wh.1, wh.0), v).map_err(|e| CoreError::Other(e.to_string()))
}

fn gdal_msg(e: gdal::errors::GdalError) -> String {
    let m = last_gdal_error();
    if m.is_empty() {
        e.to_string()
    } else {
        m
    }
}

impl GdalSource {
    /// `band` is 1-based. Reads metadata only; the dataset is reopened lazily by `read_tile`.
    pub fn open(path: &Path, band: usize) -> Result<Self, IoError> {
        init_error_handler();
        let ds = Dataset::open(path).map_err(|e| open_err(path, e))?;
        if band == 0 || band > ds.raster_count() {
            return Err(IoError::Open {
                path: path.to_path_buf(),
                msg: format!("band {band} out of range (1..={})", ds.raster_count()),
            });
        }
        let b = ds.rasterband(band).map_err(|e| open_err(path, e))?;
        let gt = b.band_type();
        let data_type = map_type(gt).ok_or_else(|| IoError::Open {
            path: path.to_path_buf(),
            msg: format!("unsupported data type {}", gt.name()),
        })?;
        let (w, h) = b.size();
        let overviews = b.overview_count().map_err(|e| open_err(path, e))?.max(0) as u8;
        Ok(Self {
            path: path.to_path_buf(),
            band,
            data_type,
            size: (w as u32, h as u32),
            overviews,
            ds: Mutex::new(None),
        })
    }
}

impl RasterSource for GdalSource {
    fn data_type(&self) -> DataType {
        self.data_type
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn levels(&self) -> u8 {
        1 + self.overviews
    }

    fn read_tile(&self, level: u8, tx: u32, ty: u32) -> Result<Tile, CoreError> {
        let oor = CoreError::OutOfRange { level, tx, ty };
        if level >= self.levels() {
            return Err(oor);
        }
        let mut guard = self.ds.lock().unwrap_or_else(|p| {
            let mut g = p.into_inner();
            *g = None;
            g
        });
        if guard.is_none() {
            init_error_handler();
            let ds = Dataset::open(&self.path).map_err(|e| CoreError::Other(gdal_msg(e)))?;
            *guard = Some(ds);
        }
        let ds = guard.as_ref().expect("opened above");
        let band = ds
            .rasterband(self.band)
            .map_err(|e| CoreError::Other(gdal_msg(e)))?;
        let band = if level == 0 {
            band
        } else {
            band.overview(level as usize - 1)
                .map_err(|e| CoreError::Other(gdal_msg(e)))?
        };
        let (w, h) = band.size();
        let r = tile_rect(w as u32, h as u32, tx, ty).ok_or(oor)?;
        let off = (r.x as isize, r.y as isize);
        let wh = (r.w as usize, r.h as usize);
        Ok(match self.data_type {
            DataType::U8 => Tile::U8(read(&band, off, wh)?),
            DataType::I8 => Tile::I8(read(&band, off, wh)?),
            DataType::U16 => Tile::U16(read(&band, off, wh)?),
            DataType::I16 => Tile::I16(read(&band, off, wh)?),
            DataType::U32 => Tile::U32(read(&band, off, wh)?),
            DataType::I32 => Tile::I32(read(&band, off, wh)?),
            DataType::F32 => Tile::F32(read(&band, off, wh)?),
            DataType::F64 => Tile::F64(read(&band, off, wh)?),
        })
    }
}

#[cfg(test)]
#[path = "../tests/fixtures.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::fixtures;
    use super::*;

    #[test]
    fn open_reads_no_tiles() {
        let dir = fixtures::temp_dir("lazy");
        let f = fixtures::make_fixture(&dir, "a.tif", 600, 600, DataType::U8, 1, false, None, None);
        let s = GdalSource::open(&f.path, 1).unwrap();
        assert!(s.ds.lock().unwrap().is_none());
        s.read_tile(0, 0, 0).unwrap();
        assert!(s.ds.lock().unwrap().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
