use crate::{IoError, Probe, ProductReader, ProductWriter};
use snap_core::Product;
use std::path::Path;

#[derive(Default)]
pub struct Registry {
    readers: Vec<Box<dyn ProductReader>>,
    writers: Vec<Box<dyn ProductWriter>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_reader(&mut self, r: Box<dyn ProductReader>) {
        self.readers.push(r);
    }

    pub fn add_writer(&mut self, w: Box<dyn ProductWriter>) {
        self.writers.push(w);
    }

    /// Highest `Yes` wins; else highest `Maybe`; ties go to the first registered.
    pub fn open(&self, path: &Path) -> Result<Product, IoError> {
        let mut best: Option<(&dyn ProductReader, (u8, u8))> = None;
        for r in &self.readers {
            let rank = match r.can_read(path) {
                Probe::No => continue,
                Probe::Maybe(p) => (0, p),
                Probe::Yes(p) => (1, p),
            };
            if best.is_none_or(|(_, b)| rank > b) {
                best = Some((r.as_ref(), rank));
            }
        }
        best.ok_or_else(|| IoError::NoReader(path.to_path_buf()))?
            .0
            .read(path)
    }

    pub fn writer(&self, name: &str) -> Option<&dyn ProductWriter> {
        self.writers
            .iter()
            .map(|w| w.as_ref())
            .find(|w| w.name() == name)
    }

    pub fn readers(&self) -> &[Box<dyn ProductReader>] {
        &self.readers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IoError, Probe, ProductReader};
    use snap_core::Product;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct Fake {
        probe: Probe,
        called: Arc<AtomicBool>,
    }
    impl ProductReader for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn can_read(&self, _: &Path) -> Probe {
            self.probe
        }
        fn read(&self, _: &Path) -> Result<Product, IoError> {
            self.called.store(true, Ordering::SeqCst);
            Ok(Product::new("p", "t", 1, 1))
        }
    }

    fn fake(probe: Probe) -> (Box<Fake>, Arc<AtomicBool>) {
        let called = Arc::new(AtomicBool::new(false));
        (
            Box::new(Fake {
                probe,
                called: called.clone(),
            }),
            called,
        )
    }

    #[test]
    fn registry_prefers_highest_yes() {
        let mut r = Registry::new();
        let (a, ca) = fake(Probe::Yes(10));
        let (b, cb) = fake(Probe::Yes(50));
        r.add_reader(a);
        r.add_reader(b);
        r.open(Path::new("x")).unwrap();
        assert!(cb.load(Ordering::SeqCst));
        assert!(!ca.load(Ordering::SeqCst));
    }

    #[test]
    fn yes_beats_higher_maybe_and_tie_first_wins() {
        let mut r = Registry::new();
        let (a, ca) = fake(Probe::Maybe(200));
        let (b, cb) = fake(Probe::Yes(1));
        let (c, cc) = fake(Probe::Yes(1));
        r.add_reader(a);
        r.add_reader(b);
        r.add_reader(c);
        r.open(Path::new("x")).unwrap();
        assert!(
            cb.load(Ordering::SeqCst) && !ca.load(Ordering::SeqCst) && !cc.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn all_no_is_no_reader() {
        let mut r = Registry::new();
        r.add_reader(fake(Probe::No).0);
        r.add_reader(fake(Probe::No).0);
        assert!(matches!(r.open(Path::new("x")), Err(IoError::NoReader(_))));
    }

    #[test]
    fn lone_maybe_is_chosen() {
        let mut r = Registry::new();
        let (a, ca) = fake(Probe::Maybe(5));
        r.add_reader(a);
        r.open(Path::new("x")).unwrap();
        assert!(ca.load(Ordering::SeqCst));
    }
}
