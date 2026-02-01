use crate::archive::header::{
    unix_to_filetime, ArchiveHeader, FileEntry, FolderInfo,
};
use crate::archive::writer::{write_signature_header, SIGNATURE_HEADER_SIZE};
use crate::compression::lzma2::{encode_properties_byte, Lzma2Config};
use crate::error::{Result, SevenZipError};
use crate::compression::block::RawBlock;
use crate::threading::scheduler::compress_blocks_parallel;
use std::io::{Seek, SeekFrom, Write};

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
        // 1. Read all pending entries into (name, data, mtime) tuples
        let mut file_inputs: Vec<(String, Vec<u8>, Option<u64>)> = Vec::new();
        for entry in &self.entries {
            match entry {
                PendingEntry::File {
                    disk_path,
                    archive_name,
                } => {
                    let data = std::fs::read(disk_path)?;
                    let mtime = std::fs::metadata(disk_path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .ok()
                                .map(|d| unix_to_filetime(d.as_secs()))
                        });
                    file_inputs.push((archive_name.clone(), data, mtime));
                }
                PendingEntry::Bytes {
                    archive_name,
                    data,
                } => {
                    file_inputs.push((archive_name.clone(), data.clone(), None));
                }
            }
        }

        // 2. Separate files with data from empty files
        let mut files_with_data: Vec<(String, Vec<u8>, Option<u64>)> = Vec::new();
        let mut empty_files: Vec<(String, Option<u64>)> = Vec::new();

        for (name, data, mtime) in file_inputs {
            if data.is_empty() {
                empty_files.push((name, mtime));
            } else {
                files_with_data.push((name, data, mtime));
            }
        }

        // 3. Compress each file with data in parallel (one folder per file)
        //    Each file is treated as a single block (no intra-file splitting for simplicity).
        let raw_blocks: Vec<RawBlock> = files_with_data
            .iter()
            .enumerate()
            .map(|(i, (_name, data, _mtime))| RawBlock {
                data: data.clone(),
                block_index: i,
            })
            .collect();

        let compressed_blocks = if raw_blocks.is_empty() {
            Vec::new()
        } else {
            compress_blocks_parallel(raw_blocks, &self.config)?
        };

        // 4. Write compressed data sequentially
        let pack_position = 0u64; // compressed data starts right after SignatureHeader
        let mut folders = Vec::new();
        let mut file_entries = Vec::new();
        let properties_byte = encode_properties_byte(self.config.effective_dict_size());

        for (i, block) in compressed_blocks.iter().enumerate() {
            self.writer.write_all(&block.compressed_data)?;

            let (name, _data, mtime) = &files_with_data[i];
            folders.push(FolderInfo {
                compressed_size: block.compressed_size,
                uncompressed_size: block.uncompressed_size,
                uncompressed_crc: block.uncompressed_crc,
                lzma2_properties_byte: properties_byte,
            });
            file_entries.push(FileEntry {
                name: name.clone(),
                uncompressed_size: block.uncompressed_size,
                compressed_size: block.compressed_size,
                crc: block.uncompressed_crc,
                has_data: true,
                modified_time: *mtime,
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
