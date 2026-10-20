use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use ndarray::Array2;
use sha2::{Digest, Sha256};

use crate::{level_size, tile_rect, CoreError, DataType, RasterSource, Tile, TILE};

pub struct Pyramid {
    inner: Arc<dyn RasterSource>,
    levels: u8,
    cache_dir: Option<PathBuf>,
}

impl Pyramid {
    /// Number of levels: smallest n whose level fits in one tile, plus one for level 0.
    pub fn needed_levels(w: u32, h: u32) -> u8 {
        let mut n = 0u8;
        loop {
            let (lw, lh) = level_size(w, h, n);
            if lw <= TILE && lh <= TILE {
                return n + 1;
            }
            n += 1;
        }
    }

    pub fn new(inner: Arc<dyn RasterSource>, cache_dir: Option<PathBuf>) -> Self {
        let (w, h) = inner.size();
        let levels = Self::needed_levels(w, h).max(inner.levels());
        Pyramid {
            inner,
            levels,
            cache_dir,
        }
    }

    pub fn cache_key(path: &Path) -> Result<String, CoreError> {
        let canon = path.canonicalize()?;
        let meta = std::fs::metadata(&canon)?;
        let mtime = meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut h = Sha256::new();
        h.update(canon.as_os_str().to_string_lossy().as_bytes());
        h.update(mtime.to_le_bytes());
        h.update(meta.len().to_le_bytes());
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    fn compute(
        &self,
        level: u8,
        tx: u32,
        ty: u32,
        rows: usize,
        cols: usize,
    ) -> Result<Tile, CoreError> {
        let mut out = Array2::<f64>::zeros((rows, cols));
        let half = (TILE / 2) as usize;
        for dy in 0..2u32 {
            for dx in 0..2u32 {
                let child = match self.read_tile(level - 1, 2 * tx + dx, 2 * ty + dy) {
                    Ok(t) => downsample2(&t, false),
                    Err(CoreError::OutOfRange { .. }) => continue,
                    Err(e) => return Err(e),
                };
                let (r, c) = child.shape();
                let (y0, x0) = (dy as usize * half, dx as usize * half);
                for i in 0..r {
                    for j in 0..c {
                        out[(y0 + i, x0 + j)] = child.get_f64(i, j);
                    }
                }
            }
        }
        Ok(from_f64(self.inner.data_type(), &out))
    }

    fn sidecar_path(&self, level: u8, tx: u32, ty: u32) -> Option<PathBuf> {
        let dir = self.cache_dir.as_ref()?;
        Some(
            dir.join(format!("level{level}"))
                .join(format!("{tx}_{ty}.bin")),
        )
    }
}

impl RasterSource for Pyramid {
    fn data_type(&self) -> DataType {
        self.inner.data_type()
    }

    fn size(&self) -> (u32, u32) {
        self.inner.size()
    }

    fn levels(&self) -> u8 {
        self.levels
    }

    fn read_tile(&self, level: u8, tx: u32, ty: u32) -> Result<Tile, CoreError> {
        if level < self.inner.levels() {
            return self.inner.read_tile(level, tx, ty);
        }
        let oor = CoreError::OutOfRange { level, tx, ty };
        if level >= self.levels {
            return Err(oor);
        }
        let (w, h) = self.inner.size();
        let (lw, lh) = level_size(w, h, level);
        let Some(r) = tile_rect(lw, lh, tx, ty) else {
            return Err(oor);
        };
        let (rows, cols) = (r.h as usize, r.w as usize);
        let path = self.sidecar_path(level, tx, ty);
        if let Some(t) = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| decode(&b, self.inner.data_type(), rows, cols))
        {
            return Ok(t);
        }
        let tile = self.compute(level, tx, ty, rows, cols)?;
        if let Some(p) = path {
            let _ = write_atomic(&p, &encode(&tile)); // cache is best-effort
        }
        Ok(tile)
    }
}

static TMP: AtomicU32 = AtomicU32::new(0);

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().expect("sidecar path has a parent");
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".tmp{}_{}",
        std::process::id(),
        TMP.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

const HEADER: usize = 9;

fn dtype_code(dt: DataType) -> u8 {
    match dt {
        DataType::U8 => 0,
        DataType::I8 => 1,
        DataType::U16 => 2,
        DataType::I16 => 3,
        DataType::U32 => 4,
        DataType::I32 => 5,
        DataType::F32 => 6,
        DataType::F64 => 7,
    }
}

fn encode(tile: &Tile) -> Vec<u8> {
    let (rows, cols) = tile.shape();
    let mut b = Vec::with_capacity(HEADER + tile.byte_len());
    b.push(dtype_code(tile.data_type()));
    b.extend((rows as u32).to_le_bytes());
    b.extend((cols as u32).to_le_bytes());
    macro_rules! px {
        ($a:expr) => {
            for v in $a.iter() {
                b.extend(v.to_le_bytes());
            }
        };
    }
    match tile {
        Tile::U8(a) => px!(a),
        Tile::I8(a) => px!(a),
        Tile::U16(a) => px!(a),
        Tile::I16(a) => px!(a),
        Tile::U32(a) => px!(a),
        Tile::I32(a) => px!(a),
        Tile::F32(a) => px!(a),
        Tile::F64(a) => px!(a),
    }
    b
}

/// None (a cache miss) unless dtype, shape and length all match what is expected.
fn decode(b: &[u8], dt: DataType, rows: usize, cols: usize) -> Option<Tile> {
    let size = dt.byte_size();
    if b.len() != HEADER + rows * cols * size
        || b[0] != dtype_code(dt)
        || u32::from_le_bytes(b[1..5].try_into().ok()?) as usize != rows
        || u32::from_le_bytes(b[5..9].try_into().ok()?) as usize != cols
    {
        return None;
    }
    macro_rules! px {
        ($variant:ident, $t:ty) => {{
            let v = b[HEADER..]
                .chunks_exact(size)
                .map(|c| <$t>::from_le_bytes(c.try_into().unwrap()))
                .collect();
            Tile::$variant(Array2::from_shape_vec((rows, cols), v).ok()?)
        }};
    }
    Some(match dt {
        DataType::U8 => px!(U8, u8),
        DataType::I8 => px!(I8, i8),
        DataType::U16 => px!(U16, u16),
        DataType::I16 => px!(I16, i16),
        DataType::U32 => px!(U32, u32),
        DataType::I32 => px!(I32, i32),
        DataType::F32 => px!(F32, f32),
        DataType::F64 => px!(F64, f64),
    })
}

/// Every supported pixel type is exactly representable in f64.
fn from_f64(dt: DataType, a: &Array2<f64>) -> Tile {
    match dt {
        DataType::U8 => Tile::U8(a.mapv(|v| v as u8)),
        DataType::I8 => Tile::I8(a.mapv(|v| v as i8)),
        DataType::U16 => Tile::U16(a.mapv(|v| v as u16)),
        DataType::I16 => Tile::I16(a.mapv(|v| v as i16)),
        DataType::U32 => Tile::U32(a.mapv(|v| v as u32)),
        DataType::I32 => Tile::I32(a.mapv(|v| v as i32)),
        DataType::F32 => Tile::F32(a.mapv(|v| v as f32)),
        DataType::F64 => Tile::F64(a.clone()),
    }
}

/// Halve a tile: mean of each 2x2 block ignoring NaN, or the top-left pixel when `is_flag`.
pub fn downsample2(tile: &Tile, is_flag: bool) -> Tile {
    let (rows, cols) = tile.shape();
    let (or, oc) = (rows.div_ceil(2), cols.div_ceil(2));
    let out = Array2::from_shape_fn((or, oc), |(i, j)| {
        if is_flag {
            return tile.get_f64(2 * i, 2 * j);
        }
        let (mut sum, mut n) = (0.0, 0u32);
        for r in 2 * i..(2 * i + 2).min(rows) {
            for c in 2 * j..(2 * j + 2).min(cols) {
                let v = tile.get_f64(r, c);
                if !v.is_nan() {
                    sum += v;
                    n += 1;
                }
            }
        }
        if n == 0 {
            f64::NAN
        } else if tile.data_type().is_float() {
            sum / n as f64
        } else {
            (sum / n as f64).round()
        }
    });
    from_f64(tile.data_type(), &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySource;
    use ndarray::Array2;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "snap-rs-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ramp(n: usize) -> Arc<MemorySource> {
        Arc::new(MemorySource::new(Array2::from_shape_fn(
            (n, n),
            |(r, c)| (r * n + c) as f32,
        )))
    }

    #[test]
    fn needed_levels() {
        assert_eq!(Pyramid::needed_levels(512, 512), 1);
        assert_eq!(Pyramid::needed_levels(513, 10), 2);
        assert_eq!(Pyramid::needed_levels(40000, 40000), 8);
    }

    #[test]
    fn downsample_mean_and_shape() {
        let t = Tile::F32(Array2::from_shape_fn((4, 4), |(r, c)| (r * 4 + c) as f32));
        let d = downsample2(&t, false);
        assert_eq!(d.shape(), (2, 2));
        assert_eq!(d.get_f64(0, 0), 2.5);
        assert_eq!(d.get_f64(1, 1), 12.5);
        let t5 = Tile::F32(Array2::from_shape_fn((5, 5), |(r, c)| (r * 5 + c) as f32));
        let d5 = downsample2(&t5, false);
        assert_eq!(d5.shape(), (3, 3));
        assert_eq!(d5.get_f64(2, 2), 24.0); // lone corner pixel
        assert_eq!(d5.get_f64(0, 2), 6.5); // edge block of 2 pixels
                                           // integers: mean rounded, same type
        let ti = Tile::U8(Array2::from_shape_vec((2, 2), vec![1, 2, 2, 2]).unwrap());
        let di = downsample2(&ti, false);
        assert_eq!(di.data_type(), DataType::U8);
        assert_eq!(di.get_f64(0, 0), 2.0);
        // NaN ignored; all-NaN stays NaN
        let tn = Tile::F32(
            Array2::from_shape_vec(
                (2, 4),
                vec![
                    f32::NAN,
                    2.0,
                    f32::NAN,
                    f32::NAN,
                    4.0,
                    4.0,
                    f32::NAN,
                    f32::NAN,
                ],
            )
            .unwrap(),
        );
        let dn = downsample2(&tn, false);
        assert_eq!(dn.get_f64(0, 0), (10.0f64 / 3.0) as f32 as f64);
        assert!(dn.get_f64(0, 1).is_nan());
        // flag: top-left of each block
        let df = downsample2(&t, true);
        assert_eq!(df.get_f64(0, 0), 0.0);
        assert_eq!(df.get_f64(1, 1), 10.0);
    }

    #[test]
    fn level1_from_memory_source() {
        let p = Pyramid::new(ramp(1024), None);
        let t = p.read_tile(1, 0, 0).unwrap();
        assert_eq!(t.shape(), (512, 512));
        let inner = p.read_tile(0, 0, 0).unwrap();
        let m =
            (inner.get_f64(0, 0) + inner.get_f64(0, 1) + inner.get_f64(1, 0) + inner.get_f64(1, 1))
                / 4.0;
        assert_eq!(t.get_f64(0, 0), m);
        assert!(matches!(
            p.read_tile(1, 1, 0),
            Err(CoreError::OutOfRange { .. })
        ));
        assert!(matches!(
            p.read_tile(5, 0, 0),
            Err(CoreError::OutOfRange { .. })
        ));
    }

    #[test]
    fn sidecar_roundtrip_and_corruption() {
        let dir = temp_dir("sidecar");
        let p = Pyramid::new(ramp(1024), Some(dir.clone()));
        let a = p.read_tile(1, 0, 0).unwrap();
        let f = dir.join("level1").join("0_0.bin");
        assert_eq!(std::fs::metadata(&f).unwrap().len(), 9 + 512 * 512 * 4);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&f)
            .unwrap()
            .set_len(3)
            .unwrap();
        let b = p.read_tile(1, 0, 0).unwrap();
        assert_eq!(a.to_f32(), b.to_f32());
        assert_eq!(std::fs::metadata(&f).unwrap().len(), 9 + 512 * 512 * 4);
        let c = p.read_tile(1, 0, 0).unwrap(); // clean hit
        assert_eq!(a.to_f32(), c.to_f32());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cache_key_tracks_file() {
        let dir = temp_dir("key");
        let f = dir.join("a.bin");
        std::fs::write(&f, b"abc").unwrap();
        let k1 = Pyramid::cache_key(&f).unwrap();
        assert_eq!(k1, Pyramid::cache_key(&f).unwrap());
        assert_eq!(k1.len(), 64);
        std::fs::write(&f, b"abcdef").unwrap();
        assert_ne!(k1, Pyramid::cache_key(&f).unwrap());
        assert!(Pyramid::cache_key(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
