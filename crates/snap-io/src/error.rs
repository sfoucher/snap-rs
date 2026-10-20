use snap_core::{CoreError, GeoError};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("no reader can open {}", .0.display())]
    NoReader(PathBuf),
    #[error("cannot open {}: {msg}", path.display())]
    Open { path: PathBuf, msg: String },
    #[error("read error: {0}")]
    Read(String),
    #[error("write error: {0}")]
    Write(String),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Geo(#[from] GeoError),
}
