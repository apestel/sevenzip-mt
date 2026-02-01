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
    /// Block size for intra-file parallel compression.
    /// Files larger than this are split into blocks compressed in parallel.
    /// If `None`, defaults to `2 × dict_size` (minimum 1 MiB).
    pub block_size: Option<usize>,
}

impl Default for Lzma2Config {
    fn default() -> Self {
        Self {
            preset: 6,
            dict_size: None,
            block_size: None,
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

    /// Returns the effective block size for intra-file splitting.
    /// Defaults to `2 × dict_size`, minimum 1 MiB.
    pub fn effective_block_size(&self) -> usize {
        self.block_size
            .unwrap_or_else(|| (2 * self.effective_dict_size() as usize).max(1 << 20))
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

/// LZMA2 end-of-stream marker byte.
const LZMA2_END_MARKER: u8 = 0x00;

/// Concatenates multiple independently-compressed LZMA2 streams into a single
/// valid LZMA2 stream by stripping intermediate end-of-stream markers.
///
/// Each input stream must be a valid LZMA2 stream ending with the `0x00`
/// end-of-stream marker. The result decompresses to the concatenation of all
/// input streams' decompressed data.
pub fn concatenate_lzma2_streams(streams: Vec<Vec<u8>>) -> Result<Vec<u8>> {
    if streams.is_empty() {
        return Ok(vec![LZMA2_END_MARKER]);
    }

    for stream in &streams {
        if stream.last() != Some(&LZMA2_END_MARKER) {
            return Err(SevenZipError::Compression(
                "invalid LZMA2 stream: missing end-of-stream marker".to_string(),
            ));
        }
    }

    if streams.len() == 1 {
        let mut streams = streams;
        return Ok(streams.swap_remove(0));
    }

    let last_index = streams.len() - 1;
    let total_size: usize = streams.iter().map(|s| s.len()).sum::<usize>() - last_index;
    let mut result = Vec::with_capacity(total_size);

    for (i, stream) in streams.iter().enumerate() {
        if i < last_index {
            // Strip the trailing 0x00 end marker from intermediate streams
            result.extend_from_slice(&stream[..stream.len() - 1]);
        } else {
            // Keep the last stream complete (with its terminator)
            result.extend_from_slice(stream);
        }
    }

    Ok(result)
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

    #[test]
    fn test_concatenate_single_stream() {
        let config = Lzma2Config::default();
        let stream = compress_block(b"hello", &config).unwrap();
        let original = stream.clone();
        let result = concatenate_lzma2_streams(vec![stream]).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn test_concatenate_multiple_streams() {
        let config = Lzma2Config::default();
        let s1 = compress_block(b"hello ", &config).unwrap();
        let s2 = compress_block(b"world", &config).unwrap();

        // Each stream ends with 0x00
        assert_eq!(*s1.last().unwrap(), 0x00);
        assert_eq!(*s2.last().unwrap(), 0x00);

        let concatenated = concatenate_lzma2_streams(vec![s1.clone(), s2.clone()]).unwrap();

        // Result should be s1 without terminator + s2 with terminator
        let expected_len = s1.len() - 1 + s2.len();
        assert_eq!(concatenated.len(), expected_len);
        assert_eq!(*concatenated.last().unwrap(), 0x00);
    }

    #[test]
    fn test_concatenate_empty_input() {
        let result = concatenate_lzma2_streams(vec![]).unwrap();
        assert_eq!(result, vec![0x00]);
    }

    #[test]
    fn test_concatenate_invalid_stream() {
        let result = concatenate_lzma2_streams(vec![vec![0xFF]]);
        assert!(result.is_err());
    }

    #[test]
    fn test_effective_block_size_default() {
        let config = Lzma2Config::default();
        let dict = config.effective_dict_size() as usize;
        assert_eq!(config.effective_block_size(), 2 * dict);
    }

    #[test]
    fn test_effective_block_size_custom() {
        let config = Lzma2Config {
            preset: 6,
            dict_size: None,
            block_size: Some(4096),
        };
        assert_eq!(config.effective_block_size(), 4096);
    }

    #[test]
    fn test_effective_block_size_minimum() {
        // Low preset with tiny dict: block_size should be at least 1 MiB
        let config = Lzma2Config {
            preset: 0,
            dict_size: Some(4096),
            block_size: None,
        };
        assert!(config.effective_block_size() >= 1 << 20);
    }
}
