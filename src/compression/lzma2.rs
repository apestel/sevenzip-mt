use crate::error::{Result, SevenZipError};
use lzma_rust2::{Lzma2Options, Lzma2Writer};
use std::io::Write;

/// Configuration for LZMA2 compression.
#[derive(Debug, Clone)]
pub struct Lzma2Config {
    /// Compression preset level (0-9). Higher = better compression, slower.
    pub preset: u32,
    /// Dictionary size in bytes. If `None`, uses the default for the preset.
    pub dict_size: Option<u32>,
}

impl Default for Lzma2Config {
    fn default() -> Self {
        Self {
            preset: 6,
            dict_size: None,
        }
    }
}

impl Lzma2Config {
    fn to_lzma2_options(&self) -> Lzma2Options {
        let mut opts = Lzma2Options::with_preset(self.preset);
        if let Some(ds) = self.dict_size {
            opts.lzma_options.dict_size = ds;
        }
        opts
    }

    /// Returns the effective dictionary size for this config.
    pub fn effective_dict_size(&self) -> u32 {
        let opts = self.to_lzma2_options();
        opts.lzma_options.dict_size
    }
}

/// Encodes a dictionary size into the LZMA2 properties byte used in the 7z header.
///
/// The byte uses a 1-bit mantissa / 5-bit exponent scheme:
/// - Even values: 2^(prop/2 + 12)   (but prop=0 -> 2^12 = 4KiB... actually prop=0 -> 2^11+1 wait)
///
/// Corrected table:
/// - prop 0 -> 2^12 = 4096 (but really the formula is 2 << (prop/2 + 11))
///
/// The standard encoding:
///   prop=0: dict_size =  (2 | 0) << (0/2 + 11) = 2 << 11 = 4096
///   prop=1: dict_size =  (2 | 1) << (1/2 + 11) = 3 << 11 = 6144
///   prop=2: dict_size =  (2 | 0) << (2/2 + 11) = 2 << 12 = 8192
///   prop=3: dict_size =  (2 | 1) << (3/2 + 11) = 3 << 12 = 12288
///   ...
///   prop=40: dict_size = (2 | 0) << (40/2 + 11) = 2 << 31 = 4 GiB (clamped)
pub fn encode_properties_byte(dict_size: u32) -> u8 {
    if dict_size <= 4096 {
        return 0;
    }

    for prop in 1u8..=40 {
        let decoded = decode_dict_size(prop);
        if decoded >= dict_size {
            return prop;
        }
    }
    40
}

fn decode_dict_size(prop: u8) -> u32 {
    if prop > 40 {
        return u32::MAX;
    }
    let mantissa = 2u64 | ((prop as u64) & 1);
    let exponent = (prop as u32) / 2 + 11;
    let size = mantissa << exponent;
    if size > u32::MAX as u64 {
        u32::MAX
    } else {
        size as u32
    }
}

/// Compresses a data block using LZMA2.
pub fn compress_block(data: &[u8], config: &Lzma2Config) -> Result<Vec<u8>> {
    let options = config.to_lzma2_options();
    let output = Vec::new();
    let mut writer = Lzma2Writer::new(output, options);
    writer
        .write_all(data)
        .map_err(|e| SevenZipError::Compression(format!("LZMA2 write failed: {e}")))?;
    let compressed = writer
        .finish()
        .map_err(|e| SevenZipError::Compression(format!("LZMA2 finish failed: {e}")))?;
    Ok(compressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_properties_byte_small() {
        // prop=0 -> 4096
        assert_eq!(encode_properties_byte(4096), 0);
        assert_eq!(encode_properties_byte(1024), 0);
    }

    #[test]
    fn test_encode_properties_byte_known() {
        // prop=2 -> 8192
        assert_eq!(decode_dict_size(2), 8192);
        assert_eq!(encode_properties_byte(8192), 2);

        // prop=24 -> 2 << (12+11) = 2 << 23 = 16 MiB
        assert_eq!(decode_dict_size(24), 16_777_216);

        // Default dict_size is 8 MiB = 8_388_608
        // prop=22: (2|0) << (22/2+11) = 2 << 22 = 8_388_608 (exact match)
        assert_eq!(decode_dict_size(22), 8_388_608);
        assert_eq!(encode_properties_byte(8_388_608), 22);
    }

    #[test]
    fn test_decode_dict_size_roundtrip() {
        for prop in 0..=40u8 {
            let size = decode_dict_size(prop);
            let encoded = encode_properties_byte(size);
            assert_eq!(encoded, prop, "roundtrip failed for prop={prop}, size={size}");
        }
    }

    #[test]
    fn test_compress_block_basic() {
        let data = b"Hello, World! This is a test of LZMA2 compression.";
        let config = Lzma2Config::default();
        let compressed = compress_block(data, &config).unwrap();
        assert!(!compressed.is_empty());
    }

    #[test]
    fn test_compress_block_empty() {
        let data = b"";
        let config = Lzma2Config::default();
        let compressed = compress_block(data, &config).unwrap();
        assert!(!compressed.is_empty()); // LZMA2 stream end marker
    }
}
