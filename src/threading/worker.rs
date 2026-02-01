use crate::compression::block::{CompressedBlock, RawBlock};
use crate::compression::lzma2::{compress_block, Lzma2Config};
use crate::error::Result;

/// Compresses a single raw block with LZMA2 and computes its CRC32.
pub fn compress_raw_block(block: RawBlock, config: &Lzma2Config) -> Result<CompressedBlock> {
    let uncompressed_size = block.data.len() as u64;
    let uncompressed_crc = crc32fast::hash(&block.data);
    let compressed_data = compress_block(&block.data, config)?;
    let compressed_size = compressed_data.len() as u64;

    Ok(CompressedBlock {
        compressed_data,
        uncompressed_size,
        compressed_size,
        uncompressed_crc,
        block_index: block.block_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_raw_block() {
        let block = RawBlock {
            data: b"Hello, World!".to_vec(),
            block_index: 0,
        };
        let config = Lzma2Config::default();
        let result = compress_raw_block(block, &config).unwrap();
        assert_eq!(result.uncompressed_size, 13);
        assert_eq!(result.block_index, 0);
        assert_eq!(result.compressed_size, result.compressed_data.len() as u64);
        assert_eq!(result.uncompressed_crc, crc32fast::hash(b"Hello, World!"));
    }
}
