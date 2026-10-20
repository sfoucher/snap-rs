use crate::IoError;
use snap_core::{Product, Rect, TileCache};
use std::path::Path;

/// Higher wins; any `Yes` beats any `Maybe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    No,
    Maybe(u8),
    Yes(u8),
}

pub trait ProductReader: Send + Sync {
    fn name(&self) -> &'static str;
    fn can_read(&self, path: &Path) -> Probe;
    fn read(&self, path: &Path) -> Result<Product, IoError>;
}

/// Empty `bands` means all bands.
#[derive(Debug, Clone, PartialEq)]
pub struct SubsetRegion {
    pub rect: Rect,
    pub bands: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    #[default]
    None,
    Deflate,
    Lzw,
    Zstd,
}

#[derive(Debug, Clone, Default)]
pub struct WriteOpts {
    pub subset: Option<SubsetRegion>,
    pub cog: bool,
    pub compression: Compression,
}

pub trait ProductWriter: Send + Sync {
    fn name(&self) -> &'static str;
    fn write(
        &self,
        product: &Product,
        path: &Path,
        opts: &WriteOpts,
        cache: &TileCache,
    ) -> Result<(), IoError>;
}
