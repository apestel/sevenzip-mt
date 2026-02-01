use thiserror::Error;

#[derive(Debug, Error)]
pub enum SevenZipError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("file not found: {0}")]
    FileNotFound(String),

    #[error("compression error: {0}")]
    Compression(String),

    #[error("invalid state: {0}")]
    InvalidState(String),

    #[error("header error: {0}")]
    HeaderError(String),

    #[error("archive already finalized")]
    AlreadyFinalized,
}

pub type Result<T> = std::result::Result<T, SevenZipError>;
