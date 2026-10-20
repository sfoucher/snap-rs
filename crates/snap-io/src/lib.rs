mod error;
mod reader;
mod registry;

pub use error::IoError;
pub use reader::{Compression, Probe, ProductReader, ProductWriter, SubsetRegion, WriteOpts};
pub use registry::Registry;
