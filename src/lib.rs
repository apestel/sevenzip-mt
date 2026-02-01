#![forbid(unsafe_code)]

pub mod archive;
pub mod compression;
pub mod error;
pub mod io;
pub mod threading;

pub use archive::builder::SevenZipWriter;
pub use compression::lzma2::Lzma2Config;
pub use error::SevenZipError;
