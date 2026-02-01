use crate::archive::header::{
    unix_to_filetime, ArchiveHeader, FileEntry, FolderInfo,
};
use crate::archive::writer::{write_signature_header, SIGNATURE_HEADER_SIZE};
use crate::compression::lzma2::{encode_properties_byte, Lzma2Config, LZMA2_END_MARKER};
use crate::error::{Result, SevenZipError};
use crate::compression::block::RawBlock;
use crate::threading::scheduler::compress_blocks_parallel;
use std::io::{Read, Seek, SeekFrom, Write};

/// Metadata for a non-empty file, separated from its raw data so the data
/// can be moved into RawBlocks without cloning.
struct FileMeta {
    name: String,
    mtime: Option<u64>,
    uncompressed_size: u64,
    crc: u32,
    /// Number of compressed blocks belonging to this file.
    block_count: usize,
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
    pub fn finish(mut self) -> Result<W> {
        let block_size = self.config.effective_block_size();
        let mut file_metas: Vec<FileMeta> = Vec::new();
        let mut raw_blocks: Vec<RawBlock> = Vec::new();
        let mut empty_files: Vec<(String, Option<u64>)> = Vec::new();

        // 1. Build RawBlocks from all entries.
        //    - Disk files: read by chunks directly into RawBlocks (never hold
        //      the full file as a single Vec), compute CRC incrementally.
        //    - Memory entries: move or split data (zero-copy for single block).
        for entry in self.entries {
            match entry {
                PendingEntry::File {
                    disk_path,
                    archive_name,
                } => {
                    Self::read_file_into_blocks(
                        &disk_path,
                        archive_name,
                        block_size,
                        &mut file_metas,
                        &mut raw_blocks,
                        &mut empty_files,
                    )?;
                }
                PendingEntry::Bytes {
                    archive_name,
                    data,
                } => {
                    Self::split_bytes_into_blocks(
                        archive_name,
                        data,
                        block_size,
                        &mut file_metas,
                        &mut raw_blocks,
                        &mut empty_files,
                    );
                }
            }
        }

        // 2. Compress all blocks in parallel using a dedicated thread pool.
        let compressed_blocks = if raw_blocks.is_empty() {
            Vec::new()
        } else {
            compress_blocks_parallel(raw_blocks, &self.config, self.num_threads)?
        };

        // 3. Write compressed data directly to the output, one file at a time.
        //    Each compressed block is written and immediately dropped (freed).
        //    For multi-block files, intermediate LZMA2 end markers are stripped
        //    inline — no concatenation buffer is allocated.
        let pack_position = 0u64;
        let mut folders = Vec::new();
        let mut file_entries = Vec::new();
        let properties_byte = encode_properties_byte(self.config.effective_dict_size());

        let mut block_iter = compressed_blocks.into_iter();

        for meta in &file_metas {
            let compressed_size = Self::write_file_blocks(
                &mut self.writer,
                &mut block_iter,
                meta.block_count,
            )?;

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

        // 4. Add empty file entries (no folder for these)
        for (name, mtime) in &empty_files {
            file_entries.push(FileEntry {
                name: name.clone(),
                uncompressed_size: 0,
                compressed_size: 0,
                crc: 0,
                has_data: false,
                modified_time: *mtime,
            });
        }

        // 5. Build and serialize the header
        let header = ArchiveHeader {
            folders,
            files: file_entries,
            pack_position,
        };
        let header_bytes = header.serialize()?;
        let header_crc = crc32fast::hash(&header_bytes);

        // 6. Write the header
        let header_offset_from_sig_end = self.writer.stream_position()? - SIGNATURE_HEADER_SIZE;
        self.writer.write_all(&header_bytes)?;

        // 7. Seek back and write the real SignatureHeader
        self.writer.seek(SeekFrom::Start(0))?;
        write_signature_header(
            &mut self.writer,
            header_offset_from_sig_end,
            header_bytes.len() as u64,
            header_crc,
        )?;

        // 8. Seek to end so the writer is in a clean state
        self.writer.seek(SeekFrom::End(0))?;

        Ok(self.writer)
    }

    /// Reads a disk file by chunks directly into RawBlocks, computing CRC
    /// incrementally. The full file is never loaded as a single allocation.
    fn read_file_into_blocks(
        disk_path: &std::path::Path,
        archive_name: String,
        block_size: usize,
        file_metas: &mut Vec<FileMeta>,
        raw_blocks: &mut Vec<RawBlock>,
        empty_files: &mut Vec<(String, Option<u64>)>,
    ) -> Result<()> {
        let metadata = std::fs::metadata(disk_path)?;
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
            empty_files.push((archive_name, mtime));
            return Ok(());
        }

        let mut file = std::fs::File::open(disk_path)?;
        let mut hasher = crc32fast::Hasher::new();
        let first_block = raw_blocks.len();
        let mut remaining = file_size;

        while remaining > 0 {
            let chunk_len = block_size.min(remaining as usize);
            let mut buf = vec![0u8; chunk_len];
            file.read_exact(&mut buf)?;
            hasher.update(&buf);
            raw_blocks.push(RawBlock {
                data: buf,
                block_index: raw_blocks.len(),
            });
            remaining -= chunk_len as u64;
        }

        file_metas.push(FileMeta {
            name: archive_name,
            mtime,
            uncompressed_size: file_size,
            crc: hasher.finalize(),
            block_count: raw_blocks.len() - first_block,
        });

        Ok(())
    }

    /// Splits in-memory data into RawBlocks. Single-block data is moved
    /// directly (zero copy); larger data is split into chunks.
    fn split_bytes_into_blocks(
        archive_name: String,
        data: Vec<u8>,
        block_size: usize,
        file_metas: &mut Vec<FileMeta>,
        raw_blocks: &mut Vec<RawBlock>,
        empty_files: &mut Vec<(String, Option<u64>)>,
    ) {
        if data.is_empty() {
            empty_files.push((archive_name, None));
            return;
        }

        let uncompressed_size = data.len() as u64;
        let crc = crc32fast::hash(&data);
        let first_block = raw_blocks.len();

        if data.len() <= block_size {
            raw_blocks.push(RawBlock {
                data,
                block_index: first_block,
            });
        } else {
            for chunk in data.chunks(block_size) {
                raw_blocks.push(RawBlock {
                    data: chunk.to_vec(),
                    block_index: raw_blocks.len(),
                });
            }
        }

        file_metas.push(FileMeta {
            name: archive_name,
            mtime: None,
            uncompressed_size,
            crc,
            block_count: raw_blocks.len() - first_block,
        });
    }

    /// Writes a file's compressed blocks directly to the output, stripping
    /// intermediate LZMA2 end markers inline. Each block is dropped (freed)
    /// immediately after writing. Returns total bytes written.
    fn write_file_blocks(
        writer: &mut W,
        block_iter: &mut impl Iterator<Item = crate::compression::block::CompressedBlock>,
        block_count: usize,
    ) -> Result<u64> {
        let mut compressed_size = 0u64;
        let last_index = block_count - 1;

        for i in 0..block_count {
            let block = block_iter.next().ok_or_else(|| {
                SevenZipError::Compression("unexpected end of compressed blocks".to_string())
            })?;

            if i < last_index {
                // Intermediate block: strip the trailing LZMA2 end marker
                let data = &block.compressed_data;
                if data.last() != Some(&LZMA2_END_MARKER) {
                    return Err(SevenZipError::Compression(
                        "invalid LZMA2 stream: missing end-of-stream marker".to_string(),
                    ));
                }
                let payload = &data[..data.len() - 1];
                writer.write_all(payload)?;
                compressed_size += payload.len() as u64;
            } else {
                // Last (or only) block: write as-is
                writer.write_all(&block.compressed_data)?;
                compressed_size += block.compressed_data.len() as u64;
            }
            // `block` is dropped here — compressed_data freed immediately
        }

        Ok(compressed_size)
    }
}
