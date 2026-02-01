use crate::compression::block::{CompressedBlock, RawBlock};
use crate::compression::lzma2::Lzma2Config;
use crate::error::{Result, SevenZipError};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;

/// Compresses multiple blocks in parallel using a dedicated rayon thread pool,
/// returning them sorted by block_index.
///
/// If `num_threads` is `None`, uses the number of available logical CPUs.
pub fn compress_blocks_parallel(
    blocks: Vec<RawBlock>,
    config: &Lzma2Config,
    num_threads: Option<usize>,
) -> Result<Vec<CompressedBlock>> {
    let mut builder = ThreadPoolBuilder::new();
    if let Some(n) = num_threads {
        builder = builder.num_threads(n);
    }
    let pool = builder.build().map_err(|e| {
        SevenZipError::Threading(format!("failed to build thread pool: {e}"))
    })?;

    let mut results: Vec<CompressedBlock> = pool.install(|| {
        blocks
            .into_par_iter()
            .map(|block| crate::threading::worker::compress_raw_block(block, config))
            .collect::<Result<Vec<_>>>()
    })?;

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
        let results = compress_blocks_parallel(blocks, &config, None).unwrap();

        assert_eq!(results.len(), 4);
        for (i, block) in results.iter().enumerate() {
            assert_eq!(block.block_index, i);
        }
    }

    #[test]
    fn test_compress_parallel_with_explicit_threads() {
        let blocks: Vec<RawBlock> = (0..4)
            .map(|i| RawBlock {
                data: format!("block {i} content").into_bytes(),
                block_index: i,
            })
            .collect();

        let config = Lzma2Config::default();
        let results = compress_blocks_parallel(blocks, &config, Some(2)).unwrap();

        assert_eq!(results.len(), 4);
        for (i, block) in results.iter().enumerate() {
            assert_eq!(block.block_index, i);
        }
    }
}
