use crate::compression::block::{CompressedBlock, RawBlock};
use crate::compression::lzma2::Lzma2Config;
use crate::error::{Result, SevenZipError};
use crate::progress::{self, ProgressCallback, ProgressStage};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Builds a rayon thread pool with the given number of threads.
/// If `num_threads` is `None`, uses the number of available logical CPUs.
pub fn build_thread_pool(num_threads: Option<usize>) -> Result<rayon::ThreadPool> {
    let mut builder = ThreadPoolBuilder::new();
    if let Some(n) = num_threads {
        builder = builder.num_threads(n);
    }
    builder.build().map_err(|e| {
        SevenZipError::Threading(format!("failed to build thread pool: {e}"))
    })
}

/// Compresses multiple blocks in parallel using a dedicated rayon thread pool,
/// returning them sorted by block_index.
///
/// If `num_threads` is `None`, uses the number of available logical CPUs.
/// If a `progress` callback is provided, it is called after each block completes
/// with a percentage mapped to the 5–95% range.
pub fn compress_blocks_parallel(
    blocks: Vec<RawBlock>,
    config: &Lzma2Config,
    num_threads: Option<usize>,
    progress: &Option<ProgressCallback>,
    total_blocks: usize,
) -> Result<Vec<CompressedBlock>> {
    let mut builder = ThreadPoolBuilder::new();
    if let Some(n) = num_threads {
        builder = builder.num_threads(n);
    }
    let pool = builder.build().map_err(|e| {
        SevenZipError::Threading(format!("failed to build thread pool: {e}"))
    })?;

    let completed = AtomicUsize::new(0);

    let mut results: Vec<CompressedBlock> = pool.install(|| {
        blocks
            .into_par_iter()
            .map(|block| {
                let result = crate::threading::worker::compress_raw_block(block, config);
                if result.is_ok() {
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    let pct = 0.05 + 0.90 * (done as f64 / total_blocks as f64);
                    progress::report(progress, ProgressStage::CompressingAndWriting, pct, done, total_blocks);
                }
                result
            })
            .collect::<Result<Vec<_>>>()
    })?;

    results.sort_by_key(|b| b.block_index);
    Ok(results)
}

/// Compresses blocks in parallel using batches of `batch_size`, calling `on_batch`
/// with each sorted batch of compressed blocks before moving to the next.
///
/// This caps peak memory at `batch_size × block_size` instead of loading all
/// compressed data into memory at once.
pub fn compress_blocks_parallel_streaming<F>(
    blocks: Vec<RawBlock>,
    config: &Lzma2Config,
    num_threads: Option<usize>,
    progress: &Option<ProgressCallback>,
    total_blocks: usize,
    batch_size: usize,
    mut on_batch: F,
) -> Result<()>
where
    F: FnMut(Vec<CompressedBlock>) -> Result<()>,
{
    let mut builder = ThreadPoolBuilder::new();
    if let Some(n) = num_threads {
        builder = builder.num_threads(n);
    }
    let pool = builder.build().map_err(|e| {
        SevenZipError::Threading(format!("failed to build thread pool: {e}"))
    })?;

    let completed = AtomicUsize::new(0);

    for chunk in chunks_vec(blocks, batch_size) {
        let mut batch: Vec<CompressedBlock> = pool.install(|| {
            chunk
                .into_par_iter()
                .map(|block| {
                    let result = crate::threading::worker::compress_raw_block(block, config);
                    if result.is_ok() {
                        let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        let pct = 0.05 + 0.90 * (done as f64 / total_blocks as f64);
                        progress::report(progress, ProgressStage::CompressingAndWriting, pct, done, total_blocks);
                    }
                    result
                })
                .collect::<Result<Vec<_>>>()
        })?;

        batch.sort_by_key(|b| b.block_index);
        on_batch(batch)?;
    }

    Ok(())
}

/// Splits a `Vec<T>` into chunks, consuming the vector and moving elements
/// without cloning. Returns an iterator of `Vec<T>`.
fn chunks_vec<T>(mut vec: Vec<T>, chunk_size: usize) -> impl Iterator<Item = Vec<T>> {
    let mut result = Vec::new();
    while !vec.is_empty() {
        let at = chunk_size.min(vec.len());
        let rest = vec.split_off(at);
        result.push(vec);
        vec = rest;
    }
    result.into_iter()
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
        let results = compress_blocks_parallel(blocks, &config, None, &None, 4).unwrap();

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
        let results = compress_blocks_parallel(blocks, &config, Some(2), &None, 4).unwrap();

        assert_eq!(results.len(), 4);
        for (i, block) in results.iter().enumerate() {
            assert_eq!(block.block_index, i);
        }
    }

    #[test]
    fn test_compress_streaming_batched() {
        let blocks: Vec<RawBlock> = (0..6)
            .map(|i| RawBlock {
                data: format!("block {i} data with some content").into_bytes(),
                block_index: i,
            })
            .collect();

        let config = Lzma2Config::default();
        let mut all_blocks = Vec::new();
        let mut batch_count = 0usize;

        compress_blocks_parallel_streaming(
            blocks,
            &config,
            Some(2),
            &None,
            6,
            2, // batch_size = 2, so 3 batches for 6 blocks
            |batch| {
                batch_count += 1;
                assert!(batch.len() <= 2);
                all_blocks.extend(batch);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(batch_count, 3);
        assert_eq!(all_blocks.len(), 6);
        // Verify ordering within and across batches
        for (i, block) in all_blocks.iter().enumerate() {
            assert_eq!(block.block_index, i);
        }
    }

    #[test]
    fn test_chunks_vec() {
        let v = vec![0, 1, 2, 3, 4];
        let chunks: Vec<Vec<i32>> = chunks_vec(v, 2).collect();
        assert_eq!(chunks, vec![vec![0, 1], vec![2, 3], vec![4]]);

        let empty: Vec<i32> = vec![];
        let chunks: Vec<Vec<i32>> = chunks_vec(empty, 2).collect();
        assert!(chunks.is_empty());
    }
}
