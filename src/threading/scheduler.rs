use crate::compression::block::{CompressedBlock, RawBlock};
use crate::compression::lzma2::Lzma2Config;
use crate::error::Result;
use rayon::prelude::*;

/// Compresses multiple blocks in parallel using rayon, returning them sorted by block_index.
pub fn compress_blocks_parallel(
    blocks: Vec<RawBlock>,
    config: &Lzma2Config,
) -> Result<Vec<CompressedBlock>> {
    let mut results: Vec<CompressedBlock> = blocks
        .into_par_iter()
        .map(|block| crate::threading::worker::compress_raw_block(block, config))
        .collect::<Result<Vec<_>>>()?;

    results.sort_by_key(|b| b.block_index);
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_parallel_ordering() {
        let blocks: Vec<RawBlock> = (0..4)
            .map(|i| RawBlock {
                data: format!("block {i} data with some content").into_bytes(),
                block_index: i,
            })
            .collect();

        let config = Lzma2Config::default();
        let results = compress_blocks_parallel(blocks, &config).unwrap();

        assert_eq!(results.len(), 4);
        for (i, block) in results.iter().enumerate() {
            assert_eq!(block.block_index, i);
        }
    }
}
