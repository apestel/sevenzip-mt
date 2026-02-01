use crate::archive::header::{
    unix_to_filetime, ArchiveHeader, FileEntry, FolderInfo,
};
use crate::archive::writer::{write_signature_header, SIGNATURE_HEADER_SIZE};
use crate::compression::lzma2::{concatenate_lzma2_streams, encode_properties_byte, Lzma2Config};
use crate::error::{Result, SevenZipError};
use crate::compression::block::RawBlock;
use crate::threading::scheduler::compress_blocks_parallel;
use std::io::{Seek, SeekFrom, Write};

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
        })
    }

    /// Sets the LZMA2 compression configuration.
    pub fn set_config(&mut self, config: Lzma2Config) {
        self.config = config;
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
        // 1. Read all pending entries into (name, data, mtime) tuples.
        //    finish() takes self by value, so we move data out of entries — no clones.
        let mut file_inputs: Vec<(String, Vec<u8>, Option<u64>)> = Vec::new();
        for entry in self.entries {
            match entry {
                PendingEntry::File {
                    disk_path,
                    archive_name,
                } => {
                    let data = std::fs::read(&disk_path)?;
                    let mtime = std::fs::metadata(&disk_path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .ok()
                                .map(|d| unix_to_filetime(d.as_secs()))
                        });
                    file_inputs.push((archive_name, data, mtime));
                }
                PendingEntry::Bytes {
                    archive_name,
                    data,
                } => {
                    file_inputs.push((archive_name, data, None));
                }
            }
        }

        // 2. Split non-empty files into blocks for parallel compression.
        //    Compute CRC over the full file data before splitting so we have
        //    the correct whole-file CRC for the header.
        let block_size = self.config.effective_block_size();
        let mut file_metas: Vec<FileMeta> = Vec::new();
        let mut raw_blocks: Vec<RawBlock> = Vec::new();
        let mut empty_files: Vec<(String, Option<u64>)> = Vec::new();

        for (name, data, mtime) in file_inputs {
            if data.is_empty() {
                empty_files.push((name, mtime));
                continue;
            }

            let uncompressed_size = data.len() as u64;
            let crc = crc32fast::hash(&data);
            let first_block = raw_blocks.len();

            if data.len() <= block_size {
                // Single block — move data directly (zero copy)
                raw_blocks.push(RawBlock {
                    data,
                    block_index: first_block,
                });
            } else {
                // Multiple blocks — split into chunks
                for chunk in data.chunks(block_size) {
                    raw_blocks.push(RawBlock {
                        data: chunk.to_vec(),
                        block_index: raw_blocks.len(),
                    });
                }
            }

            file_metas.push(FileMeta {
                name,
                mtime,
                uncompressed_size,
                crc,
                block_count: raw_blocks.len() - first_block,
            });
        }

        // 3. Compress all blocks in parallel
        let compressed_blocks = if raw_blocks.is_empty() {
            Vec::new()
        } else {
            compress_blocks_parallel(raw_blocks, &self.config)?
        };

        // 4. For each file, concatenate its compressed LZMA2 streams into a
        //    single stream, then write it as one folder.
        let pack_position = 0u64; // compressed data starts right after SignatureHeader
        let mut folders = Vec::new();
        let mut file_entries = Vec::new();
        let properties_byte = encode_properties_byte(self.config.effective_dict_size());

        let mut block_iter = compressed_blocks.into_iter();

        for meta in &file_metas {
            // Collect this file's compressed streams (moved, not cloned)
            let mut streams: Vec<Vec<u8>> = Vec::with_capacity(meta.block_count);
            for _ in 0..meta.block_count {
                let block = block_iter.next().ok_or_else(|| {
                    SevenZipError::Compression("unexpected end of compressed blocks".to_string())
                })?;
                streams.push(block.compressed_data);
            }

            let concatenated = concatenate_lzma2_streams(streams)?;
            let compressed_size = concatenated.len() as u64;

            self.writer.write_all(&concatenated)?;

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
}
