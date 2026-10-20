use ndarray::{s, Array2};

use crate::{tile_rect, CoreError, DataType, Tile};

pub trait RasterSource: Send + Sync {
    fn data_type(&self) -> DataType;
    fn size(&self) -> (u32, u32);
    fn levels(&self) -> u8;
    fn read_tile(&self, level: u8, tx: u32, ty: u32) -> Result<Tile, CoreError>;
}

pub struct MemorySource {
    data: Array2<f32>,
}

impl MemorySource {
    pub fn new(data: Array2<f32>) -> Self {
        MemorySource { data }
    }
}

impl RasterSource for MemorySource {
    fn data_type(&self) -> DataType {
        DataType::F32
    }

    fn size(&self) -> (u32, u32) {
        (self.data.ncols() as u32, self.data.nrows() as u32)
    }

    fn levels(&self) -> u8 {
        1
    }

    fn read_tile(&self, level: u8, tx: u32, ty: u32) -> Result<Tile, CoreError> {
        let (w, h) = self.size();
        let r = match tile_rect(w, h, tx, ty) {
            Some(r) if level == 0 => r,
            _ => return Err(CoreError::OutOfRange { level, tx, ty }),
        };
        let (x, y, w, h) = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
        Ok(Tile::F32(
            self.data.slice(s![y..y + h, x..x + w]).to_owned(),
        ))
    }
}
