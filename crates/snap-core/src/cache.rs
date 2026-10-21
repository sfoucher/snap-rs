use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use lru::LruCache;

use crate::{CoreError, ProductId, RasterSource, Tile};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TileKey {
    pub product: ProductId,
    pub band: String,
    pub level: u8,
    pub tx: u32,
    pub ty: u32,
}

pub enum TileState {
    Ready(Arc<Tile>),
    Pending,
    Failed(String),
}

const MAX_FAILURES: usize = 1024;

pub struct TileCache {
    inner: Arc<Inner>,
    pool: rayon::ThreadPool,
}

struct Inner {
    state: Mutex<State>,
    budget_bytes: usize,
    tx: Sender<TileKey>,
}

struct State {
    tiles: LruCache<TileKey, Arc<Tile>>,
    used_bytes: usize,
    /// In-flight loads, by generation: a completion is kept only if its
    /// generation is still the pending one (not evicted and re-requested).
    pending: HashMap<TileKey, u64>,
    next_gen: u64,
    failures: HashMap<TileKey, String>,
}

impl Inner {
    fn insert(&self, key: TileKey, tile: Arc<Tile>) {
        let mut st = self.state.lock().unwrap();
        self.insert_locked(&mut st, key, tile);
    }

    fn insert_locked(&self, st: &mut State, key: TileKey, tile: Arc<Tile>) {
        st.used_bytes += tile.byte_len();
        if let Some(old) = st.tiles.put(key, tile) {
            st.used_bytes -= old.byte_len();
        }
        while st.used_bytes > self.budget_bytes {
            match st.tiles.pop_lru() {
                Some((_, t)) => st.used_bytes -= t.byte_len(),
                None => break,
            }
        }
    }
}

impl TileCache {
    pub fn new(budget_bytes: usize) -> (Arc<Self>, Receiver<TileKey>) {
        let (tx, rx) = channel();
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                tiles: LruCache::unbounded(),
                used_bytes: 0,
                pending: HashMap::new(),
                next_gen: 0,
                failures: HashMap::new(),
            }),
            budget_bytes,
            tx,
        });
        let pool = rayon::ThreadPoolBuilder::new()
            .build()
            .expect("tile cache thread pool");
        (Arc::new(TileCache { inner, pool }), rx)
    }

    pub fn get(&self, key: &TileKey, source: &Arc<dyn RasterSource>) -> TileState {
        let gen = {
            let mut st = self.inner.state.lock().unwrap();
            if let Some(t) = st.tiles.get(key) {
                return TileState::Ready(t.clone());
            }
            if let Some(msg) = st.failures.get(key) {
                return TileState::Failed(msg.clone());
            }
            if st.pending.contains_key(key) {
                return TileState::Pending;
            }
            st.next_gen += 1;
            let gen = st.next_gen;
            st.pending.insert(key.clone(), gen);
            gen
        };
        let (inner, source, key) = (self.inner.clone(), source.clone(), key.clone());
        self.pool.spawn(move || {
            // A panicking reader must not abort the process (rayon aborts on worker panics).
            let result = catch_unwind(AssertUnwindSafe(|| {
                source.read_tile(key.level, key.tx, key.ty)
            }))
            .unwrap_or_else(|p| {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                Err(CoreError::Other(format!("internal error: {msg}")))
            });
            let mut st = inner.state.lock().unwrap();
            // Evicted (and maybe re-requested) while loading: the result is stale, drop it.
            if st.pending.get(&key) == Some(&gen) {
                st.pending.remove(&key);
                match result {
                    Ok(tile) => inner.insert_locked(&mut st, key.clone(), Arc::new(tile)),
                    Err(e) => {
                        // ponytail: wholesale clear when full; an LRU would keep recent failures.
                        if st.failures.len() >= MAX_FAILURES {
                            st.failures.clear();
                        }
                        st.failures.insert(key.clone(), e.to_string());
                    }
                }
            }
            drop(st);
            let _ = inner.tx.send(key);
        });
        TileState::Pending
    }

    pub fn get_blocking(
        &self,
        key: &TileKey,
        source: &Arc<dyn RasterSource>,
    ) -> Result<Arc<Tile>, CoreError> {
        if let Some(t) = self.inner.state.lock().unwrap().tiles.get(key) {
            return Ok(t.clone());
        }
        let tile = Arc::new(source.read_tile(key.level, key.tx, key.ty)?);
        self.inner.insert(key.clone(), tile.clone());
        Ok(tile)
    }

    pub fn peek(&self, key: &TileKey) -> Option<Arc<Tile>> {
        self.inner.state.lock().unwrap().tiles.peek(key).cloned()
    }

    pub fn evict_product(&self, id: ProductId) {
        self.evict_if(|k| k.product == id);
    }

    /// Drops every cached tile (and recorded failure) of one band or mask.
    pub fn evict_band(&self, id: ProductId, band: &str) {
        self.evict_if(|k| k.product == id && k.band == band);
    }

    fn evict_if(&self, doomed: impl Fn(&TileKey) -> bool) {
        let mut st = self.inner.state.lock().unwrap();
        let keys: Vec<TileKey> = st
            .tiles
            .iter()
            .filter(|(k, _)| doomed(k))
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            if let Some(t) = st.tiles.pop(&k) {
                st.used_bytes -= t.byte_len();
            }
        }
        st.failures.retain(|k, _| !doomed(k));
        st.pending.retain(|k, _| !doomed(k));
    }

    pub fn used_bytes(&self) -> usize {
        self.inner.state.lock().unwrap().used_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataType, MemorySource};
    use ndarray::Array2;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn key(tx: u32) -> TileKey {
        TileKey {
            product: ProductId(1),
            band: "b".into(),
            level: 0,
            tx,
            ty: 0,
        }
    }

    fn mem(w: usize, h: usize) -> Arc<dyn RasterSource> {
        Arc::new(MemorySource::new(Array2::zeros((h, w))))
    }

    #[test]
    fn evict_band_drops_only_that_band() {
        let (cache, rx) = TileCache::new(1 << 30);
        let src = mem(8, 8);
        let other = TileKey {
            band: "c".into(),
            ..key(0)
        };
        for k in [key(0), other.clone()] {
            cache.get(&k, &src);
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        let bytes = cache.peek(&key(0)).unwrap().byte_len();
        let before = cache.used_bytes();
        cache
            .inner
            .state
            .lock()
            .unwrap()
            .failures
            .insert(key(1), "x".into());
        cache.evict_band(ProductId(1), "b");
        assert!(cache.peek(&key(0)).is_none());
        assert!(cache.peek(&other).is_some());
        assert_eq!(cache.used_bytes(), before - bytes);
        assert!(cache.inner.state.lock().unwrap().failures.is_empty());
    }

    #[test]
    fn miss_is_pending_then_ready() {
        let (cache, rx) = TileCache::new(1 << 30);
        let src = mem(512, 512);
        assert!(matches!(cache.get(&key(0), &src), TileState::Pending));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), key(0));
        assert!(matches!(cache.get(&key(0), &src), TileState::Ready(_)));
    }

    #[test]
    fn eviction_respects_budget() {
        let budget = 3 * 512 * 512 * 4;
        let (cache, _rx) = TileCache::new(budget);
        let src = mem(512 * 5, 512);
        for tx in 0..5 {
            cache.get_blocking(&key(tx), &src).unwrap();
        }
        assert!(cache.used_bytes() <= budget);
        assert!(cache.peek(&key(0)).is_none());
    }

    struct Failing(AtomicUsize);

    impl RasterSource for Failing {
        fn data_type(&self) -> DataType {
            DataType::F32
        }
        fn size(&self) -> (u32, u32) {
            (512, 512)
        }
        fn levels(&self) -> u8 {
            1
        }
        fn read_tile(&self, _: u8, _: u32, _: u32) -> Result<Tile, CoreError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(CoreError::Other("boom".into()))
        }
    }

    #[test]
    fn failure_is_reported_once() {
        let (cache, rx) = TileCache::new(1 << 30);
        let failing = Arc::new(Failing(AtomicUsize::new(0)));
        let src: Arc<dyn RasterSource> = failing.clone();
        assert!(matches!(cache.get(&key(0), &src), TileState::Pending));
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..3 {
            assert!(matches!(cache.get(&key(0), &src), TileState::Failed(_)));
        }
        assert_eq!(failing.0.load(Ordering::SeqCst), 1);
    }

    struct Panicking;

    impl RasterSource for Panicking {
        fn data_type(&self) -> DataType {
            DataType::F32
        }
        fn size(&self) -> (u32, u32) {
            (512, 512)
        }
        fn levels(&self) -> u8 {
            1
        }
        fn read_tile(&self, _: u8, _: u32, _: u32) -> Result<Tile, CoreError> {
            panic!("reader blew up")
        }
    }

    #[test]
    fn panicking_read_becomes_failure() {
        let (cache, rx) = TileCache::new(1 << 30);
        let src: Arc<dyn RasterSource> = Arc::new(Panicking);
        assert!(matches!(cache.get(&key(0), &src), TileState::Pending));
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        match cache.get(&key(0), &src) {
            TileState::Failed(m) => assert!(m.contains("internal error"), "{m}"),
            _ => panic!("expected Failed"),
        }
        assert!(cache.inner.state.lock().unwrap().pending.is_empty());
    }

    /// Call i blocks until gate i is released, then returns a 1x1 tile of value i.
    struct Gated {
        calls: AtomicUsize,
        gates: [Mutex<Receiver<()>>; 2],
    }

    impl RasterSource for Gated {
        fn data_type(&self) -> DataType {
            DataType::F32
        }
        fn size(&self) -> (u32, u32) {
            (1, 1)
        }
        fn levels(&self) -> u8 {
            1
        }
        fn read_tile(&self, _: u8, _: u32, _: u32) -> Result<Tile, CoreError> {
            let i = self.calls.fetch_add(1, Ordering::SeqCst);
            self.gates[i].lock().unwrap().recv().unwrap();
            Ok(Tile::F32(Array2::from_elem((1, 1), i as f32)))
        }
    }

    #[test]
    fn stale_load_after_evict_and_rerequest_is_dropped() {
        let (cache, rx) = TileCache::new(1 << 30);
        let (g0, r0) = channel();
        let (g1, r1) = channel();
        let gated = Arc::new(Gated {
            calls: AtomicUsize::new(0),
            gates: [Mutex::new(r0), Mutex::new(r1)],
        });
        let src: Arc<dyn RasterSource> = gated.clone();
        cache.get(&key(0), &src);
        let t0 = std::time::Instant::now();
        while gated.calls.load(Ordering::SeqCst) == 0 {
            assert!(t0.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        cache.evict_band(ProductId(1), "b");
        assert!(matches!(cache.get(&key(0), &src), TileState::Pending));
        g0.send(()).unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(cache.peek(&key(0)).is_none(), "stale load was cached");
        g1.send(()).unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let t = cache.peek(&key(0)).expect("new load cached");
        assert_eq!(t.get_f64(0, 0), 1.0);
    }
}
