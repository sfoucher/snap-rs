use ndarray::Array2;

use crate::DataType;

pub const TILE: u32 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone)]
pub enum Tile {
    U8(Array2<u8>),
    I8(Array2<i8>),
    U16(Array2<u16>),
    I16(Array2<i16>),
    U32(Array2<u32>),
    I32(Array2<i32>),
    F32(Array2<f32>),
    F64(Array2<f64>),
}

macro_rules! each {
    ($t:expr, $a:ident => $e:expr) => {
        match $t {
            Tile::U8($a) => $e,
            Tile::I8($a) => $e,
            Tile::U16($a) => $e,
            Tile::I16($a) => $e,
            Tile::U32($a) => $e,
            Tile::I32($a) => $e,
            Tile::F32($a) => $e,
            Tile::F64($a) => $e,
        }
    };
}

#[allow(clippy::unnecessary_cast)] // the macro casts f64/f32 arms to themselves
impl Tile {
    pub fn data_type(&self) -> DataType {
        match self {
            Tile::U8(_) => DataType::U8,
            Tile::I8(_) => DataType::I8,
            Tile::U16(_) => DataType::U16,
            Tile::I16(_) => DataType::I16,
            Tile::U32(_) => DataType::U32,
            Tile::I32(_) => DataType::I32,
            Tile::F32(_) => DataType::F32,
            Tile::F64(_) => DataType::F64,
        }
    }

    /// (rows, cols)
    pub fn shape(&self) -> (usize, usize) {
        each!(self, a => a.dim())
    }

    /// True when pixel value `v` (read via get_f64) is `no_data`, compared in the
    /// tile's own type (R15): F32 pixels against `nd as f32`, others in f64.
    pub(crate) fn is_no_data(&self, v: f64, no_data: Option<f64>) -> bool {
        match (no_data, self) {
            (Some(nd), Tile::F32(_)) => v as f32 == nd as f32,
            (Some(nd), _) => v == nd,
            (None, _) => false,
        }
    }

    pub fn get_f64(&self, row: usize, col: usize) -> f64 {
        each!(self, a => a[(row, col)] as f64)
    }

    pub fn to_f32(&self) -> Array2<f32> {
        each!(self, a => a.mapv(|v| v as f32))
    }

    pub fn byte_len(&self) -> usize {
        let (r, c) = self.shape();
        r * c * self.data_type().byte_size()
    }
}

/// Size of pyramid `level`: ceil(w / 2^level).
pub fn level_size(w: u32, h: u32, level: u8) -> (u32, u32) {
    let d = 1u64 << level;
    ((w as u64).div_ceil(d) as u32, (h as u64).div_ceil(d) as u32)
}

pub fn tile_count(w: u32, h: u32) -> (u32, u32) {
    (w.div_ceil(TILE), h.div_ceil(TILE))
}

pub fn tile_rect(w: u32, h: u32, tx: u32, ty: u32) -> Option<Rect> {
    let (nx, ny) = tile_count(w, h);
    if tx >= nx || ty >= ny {
        return None;
    }
    let (x, y) = (tx * TILE, ty * TILE);
    Some(Rect {
        x,
        y,
        w: TILE.min(w - x),
        h: TILE.min(h - y),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreError, MemorySource, RasterSource};
    use ndarray::Array2;

    #[test]
    fn tile_rect_clips_edges() {
        assert_eq!(
            tile_rect(1000, 700, 1, 1),
            Some(Rect {
                x: 512,
                y: 512,
                w: 488,
                h: 188
            })
        );
        assert_eq!(tile_rect(1000, 700, 2, 0), None);
        assert_eq!(tile_count(1000, 700), (2, 2));
        assert_eq!(level_size(1000, 700, 1), (500, 350));
        assert_eq!(level_size(1001, 700, 1), (501, 350));
    }

    #[test]
    fn memory_source_reads_clipped_tile() {
        let src = MemorySource::new(Array2::from_shape_fn((700, 1000), |(r, c)| {
            (r * 1000 + c) as f32
        }));
        let t = src.read_tile(0, 1, 1).unwrap();
        assert_eq!(t.shape(), (188, 488));
        assert_eq!(t.get_f64(0, 0), (512 * 1000 + 512) as f64);
        assert!(matches!(
            src.read_tile(0, 2, 0),
            Err(CoreError::OutOfRange { .. })
        ));
    }
}
