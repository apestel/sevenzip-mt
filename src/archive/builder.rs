use crate::archive::header::{
    unix_to_filetime, ArchiveHeader, FileEntry, FolderInfo,
};
use crate::archive::writer::{write_signature_header, SIGNATURE_HEADER_SIZE};
use crate::compression::lzma2::{encode_properties_byte, Lzma2Config, LZMA2_END_MARKER};
use crate::error::{Result, SevenZipError};
use crate::compression::block::{CompressedBlock, RawBlock};
use crate::progress::{self, ProgressCallback, ProgressStage};
use crate::threading::scheduler::build_thread_pool;
use crate::threading::worker::compress_raw_block;
use rayon::prelude::*;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Metadata for a non-empty file, separated from its raw data so the data
/// can be moved into RawBlocks without cloning.
struct FileMeta {
    name: String,
    mtime: Option<u64>,
    uncompressed_size: u64,
    crc: u32,
}

/// Input entry queued for inclusion in the archive.
enum PendingEntry {
    File {
        disk_path: std::path::PathBuf,
        archive_name: String,
    },
    Bytes {
        archive_name: String,
        data: Vec<u8>,
    },
}

/// State for a file currently being read block-by-block.
struct CurrentFileState {
    reader: std::fs::File,
    archive_name: String,
    mtime: Option<u64>,
    hasher: crc32fast::Hasher,
    file_size: u64,
    remaining: u64,
}

/// Lazily produces batches of `RawBlock` from pending entries, reading from
/// disk incrementally so that at most `batch_size` raw blocks are in memory
/// at any time.
struct BlockProducer {
    entries: std::vec::IntoIter<PendingEntry>,
    block_size: usize,
    current_file: Option<CurrentFileState>,
    /// Bytes entries that have been taken from the iterator but not yet
    /// fully consumed (for entries larger than the remaining batch capacity).
    pending_bytes: Option<PendingBytesState>,
    // Accumulated results (grow incrementally)
    file_metas: Vec<FileMeta>,
    empty_files: Vec<(String, Option<u64>)>,
    block_file_map: Vec<(usize, bool)>,
    next_block_index: usize,
}

/// State for an in-memory entry being split across batches.
struct PendingBytesState {
    archive_name: String,
    data: Vec<u8>,
    crc: u32,
    offset: usize,
}

impl BlockProducer {
    fn new(entries: Vec<PendingEntry>, block_size: usize) -> Self {
        Self {
            entries: entries.into_iter(),
            block_size,
            current_file: None,
            pending_bytes: None,
            file_metas: Vec::new(),
            empty_files: Vec::new(),
            block_file_map: Vec::new(),
            next_block_index: 0,
        }
    }

    /// Produces up to `batch_size` raw blocks. Returns an empty vec when all
    /// entries are exhausted.
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<RawBlock>> {
        let mut batch = Vec::with_capacity(batch_size);

        while batch.len() < batch_size {
            // 1. Continue reading from a partially-consumed file
            if let Some(ref mut state) = self.current_file {
                while batch.len() < batch_size && state.remaining > 0 {
                    let chunk_len = self.block_size.min(state.remaining as usize);
                    let mut buf = vec![0u8; chunk_len];
                    state.reader.read_exact(&mut buf)?;
                    state.hasher.update(&buf);

                    let block_index = self.next_block_index;
                    self.next_block_index += 1;


                    batch.push(RawBlock {
                        data: buf,
                        block_index,
                    });
                    state.remaining -= chunk_len as u64;

                    // Eagerly record the file map entry so that blocks can be
                    // written before the file is fully consumed.
                    let file_index = self.file_metas.len();
                    let is_last = state.remaining == 0;
                    debug_assert_eq!(self.block_file_map.len(), block_index);
                    self.block_file_map.push((file_index, is_last));
                }

                // If file fully read, finalize its metadata
                if state.remaining == 0 {
                    let state = self.current_file.take().unwrap();

                    self.file_metas.push(FileMeta {
                        name: state.archive_name,
                        mtime: state.mtime,
                        uncompressed_size: state.file_size,
                        crc: state.hasher.finalize(),
                    });
                }

                if batch.len() >= batch_size {
                    break;
                }
                continue;
            }

            // 2. Continue splitting a partially-consumed bytes entry
            if let Some(ref mut pbs) = self.pending_bytes {
                while batch.len() < batch_size && pbs.offset < pbs.data.len() {
                    let end = (pbs.offset + self.block_size).min(pbs.data.len());
                    let chunk = pbs.data[pbs.offset..end].to_vec();
                    pbs.offset = end;

                    let block_index = self.next_block_index;
                    self.next_block_index += 1;


                    batch.push(RawBlock {
                        data: chunk,
                        block_index,
                    });

                    // Eagerly record the file map entry
                    let file_index = self.file_metas.len();
                    let is_last = pbs.offset >= pbs.data.len();
                    debug_assert_eq!(self.block_file_map.len(), block_index);
                    self.block_file_map.push((file_index, is_last));
                }

                // If bytes entry fully consumed, finalize
                if pbs.offset >= pbs.data.len() {
                    let pbs = self.pending_bytes.take().unwrap();

                    self.file_metas.push(FileMeta {
                        name: pbs.archive_name,
                        mtime: None,
                        uncompressed_size: pbs.data.len() as u64,
                        crc: pbs.crc,
                    });
                }

                if batch.len() >= batch_size {
                    break;
                }
                continue;
            }

            // 3. Get next entry from iterator
            let entry = match self.entries.next() {
                Some(e) => e,
                None => break,
            };

            match entry {
                PendingEntry::File {
                    disk_path,
                    archive_name,
                } => {
                    let metadata = std::fs::metadata(&disk_path)?;
                    let mtime = metadata
                        .modified()
                        .ok()
                        .and_then(|t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .ok()
                                .map(|d| unix_to_filetime(d.as_secs()))
                        });
                    let file_size = metadata.len();

                    if file_size == 0 {
                        self.empty_files.push((archive_name, mtime));
                        continue;
                    }

                    let file = std::fs::File::open(&disk_path)?;
                    self.current_file = Some(CurrentFileState {
                        reader: file,
                        archive_name,
                        mtime,
                        hasher: crc32fast::Hasher::new(),
                        file_size,
                        remaining: file_size,
                    });
                    // Loop back to read blocks from this file
                }
                PendingEntry::Bytes {
                    archive_name,
                    data,
                } => {
                    if data.is_empty() {
                        self.empty_files.push((archive_name, None));
                        continue;
                    }

                    let crc = crc32fast::hash(&data);

                    // For single-block data, emit directly without going through
                    // PendingBytesState to allow zero-copy move.
                    if data.len() <= self.block_size {
                        let block_index = self.next_block_index;
                        self.next_block_index += 1;

                        let file_index = self.file_metas.len();
                        self.block_file_map.push((file_index, true));

                        self.file_metas.push(FileMeta {
                            name: archive_name,
                            mtime: None,
                            uncompressed_size: data.len() as u64,
                            crc,
                        });

                        batch.push(RawBlock {
                            data,
                            block_index,
                        });
                    } else {
                        self.pending_bytes = Some(PendingBytesState {
                            archive_name,
                            data,
                            crc,
                            offset: 0,
                        });
                        // Loop back to split bytes
                    }
                }
            }
        }

        Ok(batch)
    }
}

/// Estimates the total number of blocks across all entries by stat-ing files
/// without reading any data. Returns `(total_blocks, total_bytes)`.
fn estimate_block_count(entries: &[PendingEntry], block_size: usize) -> Result<(usize, u64)> {
    let mut total_blocks = 0usize;
    let mut total_bytes = 0u64;

    for entry in entries {
        let size = match entry {
            PendingEntry::File { disk_path, .. } => {
                let metadata = std::fs::metadata(disk_path)?;
                metadata.len()
            }
            PendingEntry::Bytes { data, .. } => data.len() as u64,
        };

        if size > 0 {
            total_blocks += ((size as usize) + block_size - 1) / block_size;
            total_bytes += size;
        }
    }

    Ok((total_blocks, total_bytes))
}

/// Creates valid 7z archives with LZMA2 compression and multi-threaded block compression.
///
/// # Example
/// ```no_run
/// use sevenzip_mt::SevenZipWriter;
///
/// let file = std::fs::File::create("output.7z").unwrap();
/// let mut archive = SevenZipWriter::new(file).unwrap();
/// archive.add_file("local/path.txt", "archive/path.txt").unwrap();
/// archive.add_bytes("data.bin", &[1, 2, 3]).unwrap();
/// archive.finish().unwrap();
/// ```
pub struct SevenZipWriter<W: Write + Seek> {
    writer: W,
    entries: Vec<PendingEntry>,
    config: Lzma2Config,
    num_threads: Option<usize>,
}

impl<W: Write + Seek> SevenZipWriter<W> {
    /// Creates a new archive writer. Writes a 32-byte placeholder for the SignatureHeader.
    pub fn new(mut writer: W) -> Result<Self> {
        // Write 32 zero bytes as placeholder for the SignatureHeader
        writer.write_all(&[0u8; 32])?;

        Ok(Self {
            writer,
            entries: Vec::new(),
            config: Lzma2Config::default(),
            num_threads: None,
        })
    }

    /// Sets the LZMA2 compression configuration.
    pub fn set_config(&mut self, config: Lzma2Config) {
        self.config = config;
    }

    /// Sets the number of threads for parallel compression.
    /// If `None` (the default), uses the number of available logical CPUs.
    pub fn set_num_threads(&mut self, num_threads: Option<usize>) {
        self.num_threads = num_threads;
    }

    /// Queues a file from disk for inclusion in the archive.
    pub fn add_file(&mut self, disk_path: &str, archive_name: &str) -> Result<()> {
        let path = std::path::Path::new(disk_path);
        if !path.exists() {
            return Err(SevenZipError::FileNotFound(disk_path.to_string()));
        }
        self.entries.push(PendingEntry::File {
            disk_path: path.to_path_buf(),
            archive_name: archive_name.to_string(),
        });
        Ok(())
    }

    /// Queues in-memory data for inclusion in the archive.
    pub fn add_bytes(&mut self, archive_name: &str, data: &[u8]) -> Result<()> {
        self.entries.push(PendingEntry::Bytes {
            archive_name: archive_name.to_string(),
            data: data.to_vec(),
        });
        Ok(())
    }

    /// Finalizes the archive: compresses data, writes it, builds and writes the header,
    /// then seeks back to write the real SignatureHeader. Consumes self.
    pub fn finish(self) -> Result<W> {
        self.finish_internal(None)
    }

    /// Finalizes the archive like [`finish`](Self::finish), but calls `callback`
    /// periodically with progress information.
    ///
    /// # Example
    /// ```no_run
    /// use sevenzip_mt::SevenZipWriter;
    ///
    /// let file = std::fs::File::create("output.7z").unwrap();
    /// let mut archive = SevenZipWriter::new(file).unwrap();
    /// archive.add_bytes("data.bin", &[1, 2, 3]).unwrap();
    /// archive.finish_with_progress(|info| {
    ///     println!("{:.0}%", info.percentage * 100.0);
    /// }).unwrap();
    /// ```
    pub fn finish_with_progress<F>(self, callback: F) -> Result<W>
    where
        F: Fn(&crate::progress::ProgressInfo) + Send + Sync + 'static,
    {
        self.finish_internal(Some(Arc::new(callback)))
    }

    fn finish_internal(mut self, progress: Option<ProgressCallback>) -> Result<W> {
        let block_size = self.config.effective_block_size();

        // Lightweight stat-only pass to estimate total blocks for progress.
        let (total_blocks, _) = estimate_block_count(&self.entries, block_size)?;

        // 1. Reading stage (0–5%)
        progress::report(&progress, ProgressStage::Reading, 0.0, 0, total_blocks);

        let mut producer = BlockProducer::new(self.entries, block_size);
        // Clear self.entries since it has been moved into the producer.
        self.entries = Vec::new();

        let properties_byte = encode_properties_byte(self.config.effective_dict_size());

        // Determine batch size: use num_threads or default to available CPUs.
        let batch_size = self.num_threads.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });

        let pool = build_thread_pool(self.num_threads)?;
        let completed = AtomicUsize::new(0);
        let mut per_file_compressed: Vec<u64> = Vec::new();

        // 2. Pipelined read → compress → write loop (5–95%)
        progress::report(&progress, ProgressStage::Reading, 0.05, 0, total_blocks);

        loop {
            let chunk = producer.next_batch(batch_size)?;
            if chunk.is_empty() {
                break;
            }

            progress::report(
                &progress,
                ProgressStage::CompressingAndWriting,
                0.05 + 0.90 * (completed.load(Ordering::Relaxed) as f64 / total_blocks.max(1) as f64),
                completed.load(Ordering::Relaxed),
                total_blocks,
            );

            let config = &self.config;
            let mut batch: Vec<CompressedBlock> = pool.install(|| {
                chunk
                    .into_par_iter()
                    .map(|block| {
                        let result = compress_raw_block(block, config);
                        if result.is_ok() {
                            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                            let pct = 0.05 + 0.90 * (done as f64 / total_blocks.max(1) as f64);
                            progress::report(&progress, ProgressStage::CompressingAndWriting, pct, done, total_blocks);
                        }
                        result
                    })
                    .collect::<Result<Vec<_>>>()
            })?;

            batch.sort_by_key(|b| b.block_index);

            // Grow per_file_compressed to cover all file indices referenced by
            // block_file_map. This may exceed file_metas.len() when a file's
            // blocks are emitted before the file is fully read (and its
            // FileMeta entry pushed).
            if let Some(&(max_file_idx, _)) = producer.block_file_map.last() {
                while per_file_compressed.len() <= max_file_idx {
                    per_file_compressed.push(0);
                }
            }

            for block in &batch {
                Self::write_single_block(
                    &mut self.writer,
                    block,
                    &producer.block_file_map,
                    &mut per_file_compressed,
                )?;
            }
            // batch dropped here → compressed data freed
        }

        // Ensure per_file_compressed covers all files (in case last file had no blocks yet accounted)
        while per_file_compressed.len() < producer.file_metas.len() {
            per_file_compressed.push(0);
        }

        // 3. Header stage (95–100%)
        progress::report(&progress, ProgressStage::Finalizing, 0.95, total_blocks, total_blocks);

        let pack_position = 0u64;
        let mut folders = Vec::new();
        let mut file_entries = Vec::new();

        for (i, meta) in producer.file_metas.iter().enumerate() {
            let compressed_size = per_file_compressed[i];
            folders.push(FolderInfo {
                compressed_size,
                uncompressed_size: meta.uncompressed_size,
                uncompressed_crc: meta.crc,
                lzma2_properties_byte: properties_byte,
            });
            file_entries.push(FileEntry {
                name: meta.name.clone(),
                uncompressed_size: meta.uncompressed_size,
                compressed_size,
                crc: meta.crc,
                has_data: true,
                modified_time: meta.mtime,
            });
        }

        // Add empty file entries (no folder for these)
        for (name, mtime) in &producer.empty_files {
            file_entries.push(FileEntry {
                name: name.clone(),
                uncompressed_size: 0,
                compressed_size: 0,
                crc: 0,
                has_data: false,
                modified_time: *mtime,
            });
        }

        // Build and serialize the header
        let header = ArchiveHeader {
            folders,
            files: file_entries,
            pack_position,
        };
        let header_bytes = header.serialize()?;
        let header_crc = crc32fast::hash(&header_bytes);

        // Write the header
        let header_offset_from_sig_end = self.writer.stream_position()? - SIGNATURE_HEADER_SIZE;
        self.writer.write_all(&header_bytes)?;

        // Seek back and write the real SignatureHeader
        self.writer.seek(SeekFrom::Start(0))?;
        write_signature_header(
            &mut self.writer,
            header_offset_from_sig_end,
            header_bytes.len() as u64,
            header_crc,
        )?;

        // Seek to end so the writer is in a clean state
        self.writer.seek(SeekFrom::End(0))?;

        progress::report(&progress, ProgressStage::Finalizing, 1.0, total_blocks, total_blocks);

        Ok(self.writer)
    }

    /// Writes a single compressed block, stripping the LZMA2 end marker for
    /// non-last blocks of a file. Accumulates compressed size in `per_file_compressed`.
    fn write_single_block(
        writer: &mut W,
        block: &CompressedBlock,
        block_file_map: &[(usize, bool)],
        per_file_compressed: &mut [u64],
    ) -> Result<()> {
        let (file_idx, is_last) = block_file_map[block.block_index];

        if is_last {
            // Last (or only) block of this file: write as-is
            writer.write_all(&block.compressed_data)?;
            per_file_compressed[file_idx] += block.compressed_data.len() as u64;
        } else {
            // Intermediate block: strip the trailing LZMA2 end marker
            let data = &block.compressed_data;
            if data.last() != Some(&LZMA2_END_MARKER) {
                return Err(SevenZipError::Compression(
                    "invalid LZMA2 stream: missing end-of-stream marker".to_string(),
                ));
            }
            let payload = &data[..data.len() - 1];
            writer.write_all(payload)?;
            per_file_compressed[file_idx] += payload.len() as u64;
        }

        Ok(())
    }
}
