#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("tile out of range: level {level}, tx {tx}, ty {ty}")]
    OutOfRange { level: u8, tx: u32, ty: u32 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}
