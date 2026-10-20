use std::sync::Arc;

use crate::{level_size, tile_count, CoreError, RasterSource};

const BINS: usize = 512;

#[derive(Debug, Clone)]
pub struct BandStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub histogram: Vec<u32>,
    pub bin_width: f64,
    pub sample_count: u64,
    pub level: u8,
}

impl BandStats {
    pub fn percentile(&self, p: f64) -> f64 {
        if self.sample_count == 0 || self.min == self.max {
            return self.min;
        }
        let target = p.clamp(0.0, 100.0) / 100.0 * self.sample_count as f64;
        let mut cum = 0.0;
        for (i, &c) in self.histogram.iter().enumerate() {
            let c = c as f64;
            if c > 0.0 && cum + c >= target {
                let v = self.min + (i as f64 + (target - cum) / c) * self.bin_width;
                return v.clamp(self.min, self.max);
            }
            cum += c;
        }
        self.max
    }
}

/// Visit every valid (non-NaN, != no_data) sample of `level`.
fn for_valid(
    source: &Arc<dyn RasterSource>,
    no_data: Option<f64>,
    level: u8,
    mut f: impl FnMut(f64),
) -> Result<(), CoreError> {
    let (w, h) = source.size();
    let (lw, lh) = level_size(w, h, level);
    let (nx, ny) = tile_count(lw, lh);
    for ty in 0..ny {
        for tx in 0..nx {
            let tile = source.read_tile(level, tx, ty)?;
            let (rows, cols) = tile.shape();
            for r in 0..rows {
                for c in 0..cols {
                    let v = tile.get_f64(r, c);
                    if !v.is_nan() && no_data != Some(v) {
                        f(v);
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn compute_stats(
    source: &Arc<dyn RasterSource>,
    no_data: Option<f64>,
    level: u8,
) -> Result<BandStats, CoreError> {
    let (mut min, mut max, mut sum, mut n) = (f64::INFINITY, f64::NEG_INFINITY, 0.0, 0u64);
    for_valid(source, no_data, level, |v| {
        min = min.min(v);
        max = max.max(v);
        sum += v;
        n += 1;
    })?;
    let mut histogram = vec![0u32; BINS];
    if n == 0 {
        return Ok(BandStats {
            min: f64::NAN,
            max: f64::NAN,
            mean: f64::NAN,
            histogram,
            bin_width: 1.0,
            sample_count: 0,
            level,
        });
    }
    let bin_width = if min == max {
        1.0
    } else {
        (max - min) / BINS as f64
    };
    // ponytail: second read of the level; fine since the coarsest level is <= one tile
    for_valid(source, no_data, level, |v| {
        let b = if min == max {
            0
        } else {
            (((v - min) / bin_width) as usize).min(BINS - 1)
        };
        histogram[b] += 1;
    })?;
    Ok(BandStats {
        min,
        max,
        mean: sum / n as f64,
        histogram,
        bin_width,
        sample_count: n,
        level,
    })
}

pub fn compute_stats_coarsest(
    source: &Arc<dyn RasterSource>,
    no_data: Option<f64>,
) -> Result<BandStats, CoreError> {
    compute_stats(source, no_data, source.levels() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataType, MemorySource, Pyramid, Tile};
    use ndarray::Array2;
    use std::sync::Mutex;

    fn mem(a: Array2<f32>) -> Arc<dyn RasterSource> {
        Arc::new(MemorySource::new(a))
    }

    #[test]
    fn percentiles_of_ramp() {
        let s = mem(Array2::from_shape_fn((1, 1000), |(_, x)| x as f32));
        let st = compute_stats(&s, None, 0).unwrap();
        assert_eq!((st.min, st.max), (0.0, 999.0));
        assert_eq!(st.histogram.len(), 512);
        assert!((st.percentile(2.0) - 20.0).abs() <= 2.0);
        assert!((st.percentile(98.0) - 980.0).abs() <= 2.0);
    }

    #[test]
    fn stats_of_constant_band() {
        let st = compute_stats(&mem(Array2::from_elem((10, 10), 7.0)), None, 0).unwrap();
        assert_eq!((st.min, st.max, st.percentile(50.0)), (7.0, 7.0, 7.0));
        assert_eq!((st.histogram[0], st.bin_width), (100, 1.0));
    }

    #[test]
    fn stats_exclude_nodata_and_nan() {
        let a = Array2::from_shape_vec((1, 4), vec![5.0, f32::NAN, -9999.0, 5.0]).unwrap();
        let st = compute_stats(&mem(a), Some(-9999.0), 0).unwrap();
        assert_eq!((st.min, st.max, st.sample_count), (5.0, 5.0, 2));
    }

    #[test]
    fn stats_with_no_valid_samples() {
        let st = compute_stats(&mem(Array2::from_elem((2, 2), f32::NAN)), None, 0).unwrap();
        assert!(st.min.is_nan() && st.max.is_nan() && st.mean.is_nan());
        assert_eq!(st.sample_count, 0);
        assert!(st.histogram.iter().all(|&c| c == 0));
        assert_eq!(st.bin_width, 1.0);
        assert!(st.percentile(50.0).is_nan());
    }

    struct CountingSource {
        inner: Arc<dyn RasterSource>,
        levels: Mutex<Vec<u8>>,
    }

    impl RasterSource for CountingSource {
        fn data_type(&self) -> DataType {
            self.inner.data_type()
        }
        fn size(&self) -> (u32, u32) {
            self.inner.size()
        }
        fn levels(&self) -> u8 {
            self.inner.levels()
        }
        fn read_tile(&self, level: u8, tx: u32, ty: u32) -> Result<Tile, CoreError> {
            self.levels.lock().unwrap().push(level);
            self.inner.read_tile(level, tx, ty)
        }
    }

    #[test]
    fn stats_sample_from_coarsest_level() {
        let ramp = Array2::from_shape_fn((4096, 4096), |(y, x)| (x + y) as f32);
        let pyr: Arc<dyn RasterSource> = Arc::new(Pyramid::new(mem(ramp), None));
        let cs = Arc::new(CountingSource {
            inner: pyr,
            levels: Mutex::new(vec![]),
        });
        let dynsrc: Arc<dyn RasterSource> = cs.clone();
        let st = compute_stats_coarsest(&dynsrc, None).unwrap();
        assert_eq!(st.level, 3);
        let seen = cs.levels.lock().unwrap();
        assert!(!seen.is_empty() && seen.iter().all(|&l| l == 3));
    }
}
