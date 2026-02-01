use crate::error::{Result, SevenZipError};
use crate::io::writer::{
    write_bool_vector, write_number, write_u32_le, write_u64_le, write_utf16le_string,
};
use std::io::Write;

// 7z property IDs
pub const K_END: u8 = 0x00;
pub const K_HEADER: u8 = 0x01;
pub const K_MAIN_STREAMS_INFO: u8 = 0x04;
pub const K_FILES_INFO: u8 = 0x05;
pub const K_PACK_INFO: u8 = 0x06;
pub const K_UNPACK_INFO: u8 = 0x07;
pub const K_SUB_STREAMS_INFO: u8 = 0x08;
pub const K_SIZE: u8 = 0x09;
pub const K_CRC: u8 = 0x0A;
pub const K_FOLDER: u8 = 0x0B;
pub const K_CODERS_UNPACK_SIZE: u8 = 0x0C;
pub const K_NUM_UNPACK_STREAM: u8 = 0x0D;
pub const K_EMPTY_STREAM: u8 = 0x0E;
pub const K_EMPTY_FILE: u8 = 0x0F;
pub const K_NAME: u8 = 0x11;
pub const K_M_TIME: u8 = 0x14;
pub const K_ATTRIBUTES: u8 = 0x15;

/// 7z file signature bytes.
pub const SIGNATURE: [u8; 6] = [b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C];

/// LZMA2 coder ID in 7z format.
pub const LZMA2_CODER_ID: u8 = 0x21;

/// Metadata for a file entry in the archive.
pub struct FileEntry {
    pub name: String,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub crc: u32,
    pub has_data: bool,
    pub modified_time: Option<u64>, // Windows FILETIME
}

/// Metadata for a folder (one per file-with-data in non-solid mode).
pub struct FolderInfo {
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub uncompressed_crc: u32,
    pub lzma2_properties_byte: u8,
}

/// The archive header, built after all compressed data is written.
pub struct ArchiveHeader {
    pub folders: Vec<FolderInfo>,
    pub files: Vec<FileEntry>,
    /// Offset from end of SignatureHeader to start of packed data (always 0 in our case).
    pub pack_position: u64,
}

impl ArchiveHeader {
    /// Serializes the complete header to bytes.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();

        // kHeader
        buf.write_all(&[K_HEADER])
            .map_err(|e| SevenZipError::HeaderError(format!("write header tag: {e}")))?;

        // MainStreamsInfo (only if we have folders)
        if !self.folders.is_empty() {
            self.write_main_streams_info(&mut buf)?;
        }

        // FilesInfo
        if !self.files.is_empty() {
            self.write_files_info(&mut buf)?;
        }

        // kEnd (Header)
        buf.write_all(&[K_END])
            .map_err(|e| SevenZipError::HeaderError(format!("write end tag: {e}")))?;

        Ok(buf)
    }

    fn write_main_streams_info(&self, w: &mut Vec<u8>) -> Result<()> {
        w.write_all(&[K_MAIN_STREAMS_INFO])
            .map_err(|e| SevenZipError::HeaderError(e.to_string()))?;

        self.write_pack_info(w)?;
        self.write_coders_info(w)?;
        self.write_sub_streams_info(w)?;

        w.write_all(&[K_END])
            .map_err(|e| SevenZipError::HeaderError(e.to_string()))?;

        Ok(())
    }

    fn write_pack_info(&self, w: &mut Vec<u8>) -> Result<()> {
        let map_err = |e: std::io::Error| SevenZipError::HeaderError(e.to_string());

        // kPackInfo
        w.write_all(&[K_PACK_INFO]).map_err(map_err)?;

        // PackPos (offset from end of signature header)
        write_number(w, self.pack_position).map_err(map_err)?;

        // NumPackStreams (one per folder)
        write_number(w, self.folders.len() as u64).map_err(map_err)?;

        // kSize
        w.write_all(&[K_SIZE]).map_err(map_err)?;
        for folder in &self.folders {
            write_number(w, folder.compressed_size).map_err(map_err)?;
        }

        // kEnd (PackInfo)
        w.write_all(&[K_END]).map_err(map_err)?;

        Ok(())
    }

    fn write_coders_info(&self, w: &mut Vec<u8>) -> Result<()> {
        let map_err = |e: std::io::Error| SevenZipError::HeaderError(e.to_string());

        // kUnPackInfo
        w.write_all(&[K_UNPACK_INFO]).map_err(map_err)?;

        // kFolder
        w.write_all(&[K_FOLDER]).map_err(map_err)?;
        write_number(w, self.folders.len() as u64).map_err(map_err)?;

        // External = 0 (not external)
        w.write_all(&[0x00]).map_err(map_err)?;

        // For each folder: write the coder info
        for folder in &self.folders {
            // NumCoders (NUMBER) = 1
            write_number(w, 1).map_err(map_err)?;

            // Coder record:
            //   Flag byte: bits 0-3 = CodecIdSize, bit 4 = IsComplexCoder, bit 5 = HasAttributes
            //   CodecId bytes
            //   NumInStreams, NumOutStreams (if complex, omitted for simple)
            //   PropertiesSize (if has attributes)
            //   Properties bytes

            // Flag: id_size=1 (bits 0-3), not complex (bit 4=0), has attributes (bit 5=1)
            // = 0b0010_0001 = 0x21
            let flag: u8 = (1 & 0x0F) | (1 << 5); // id_size=1, has_attributes=true
            w.write_all(&[flag]).map_err(map_err)?;

            // CodecId: LZMA2 = 0x21
            w.write_all(&[LZMA2_CODER_ID]).map_err(map_err)?;

            // PropertiesSize (NUMBER)
            write_number(w, 1).map_err(map_err)?;

            // Properties: LZMA2 dict size byte
            w.write_all(&[folder.lzma2_properties_byte]).map_err(map_err)?;
        }

        // kCodersUnPackSize: uncompressed sizes for each folder's output stream
        w.write_all(&[K_CODERS_UNPACK_SIZE]).map_err(map_err)?;
        for folder in &self.folders {
            write_number(w, folder.uncompressed_size).map_err(map_err)?;
        }

        // kEnd (UnPackInfo) -- CRC is in SubStreamsInfo instead
        w.write_all(&[K_END]).map_err(map_err)?;

        Ok(())
    }

    fn write_sub_streams_info(&self, w: &mut Vec<u8>) -> Result<()> {
        let map_err = |e: std::io::Error| SevenZipError::HeaderError(e.to_string());

        // kSubStreamsInfo
        w.write_all(&[K_SUB_STREAMS_INFO]).map_err(map_err)?;

        // NumUnPackStream per folder: default is 1, so we omit it.

        // kCRC for each stream
        w.write_all(&[K_CRC]).map_err(map_err)?;

        // AllAreDefined = 1 (all streams have CRC)
        w.write_all(&[0x01]).map_err(map_err)?;

        // CRC32 values (u32 LE, NOT u64)
        for folder in &self.folders {
            write_u32_le(w, folder.uncompressed_crc).map_err(map_err)?;
        }

        // kEnd (SubStreamsInfo)
        w.write_all(&[K_END]).map_err(map_err)?;

        Ok(())
    }

    fn write_files_info(&self, w: &mut Vec<u8>) -> Result<()> {
        let map_err = |e: std::io::Error| SevenZipError::HeaderError(e.to_string());

        // kFilesInfo
        w.write_all(&[K_FILES_INFO]).map_err(map_err)?;

        // NumFiles
        write_number(w, self.files.len() as u64).map_err(map_err)?;

        // --- Property: Names ---
        self.write_names_property(w)?;

        // --- Property: EmptyStream (if any files have no data) ---
        let empty_stream: Vec<bool> = self.files.iter().map(|f| !f.has_data).collect();
        if empty_stream.iter().any(|&b| b) {
            self.write_empty_stream_property(w, &empty_stream)?;

            // EmptyFile: among empty-stream entries, which are files (vs directories)
            // For now, mark all empty-stream entries as empty files
            let empty_file: Vec<bool> = self
                .files
                .iter()
                .filter(|f| !f.has_data)
                .map(|_| true)
                .collect();
            self.write_empty_file_property(w, &empty_file)?;
        }

        // --- Property: MTime (if any files have modification times) ---
        let has_any_mtime = self.files.iter().any(|f| f.modified_time.is_some());
        if has_any_mtime {
            self.write_mtime_property(w)?;
        }

        // kEnd (FilesInfo)
        w.write_all(&[K_END]).map_err(map_err)?;

        Ok(())
    }

    fn write_names_property(&self, w: &mut Vec<u8>) -> Result<()> {
        let map_err = |e: std::io::Error| SevenZipError::HeaderError(e.to_string());

        w.write_all(&[K_NAME]).map_err(map_err)?;

        // Compute the size of the names data: External byte + UTF-16LE names with null terminators
        let mut names_buf = Vec::new();
        // External = 0
        names_buf.write_all(&[0x00]).map_err(map_err)?;
        for file in &self.files {
            // Use forward slashes in archive paths
            let name = file.name.replace('\\', "/");
            write_utf16le_string(&mut names_buf, &name).map_err(map_err)?;
        }

        // PropertySize
        write_number(w, names_buf.len() as u64).map_err(map_err)?;
        w.write_all(&names_buf).map_err(map_err)?;

        Ok(())
    }

    fn write_empty_stream_property(&self, w: &mut Vec<u8>, empty_stream: &[bool]) -> Result<()> {
        let map_err = |e: std::io::Error| SevenZipError::HeaderError(e.to_string());

        w.write_all(&[K_EMPTY_STREAM]).map_err(map_err)?;

        let mut data = Vec::new();
        write_bool_vector(&mut data, empty_stream).map_err(map_err)?;

        write_number(w, data.len() as u64).map_err(map_err)?;
        w.write_all(&data).map_err(map_err)?;

        Ok(())
    }

    fn write_empty_file_property(&self, w: &mut Vec<u8>, empty_file: &[bool]) -> Result<()> {
        let map_err = |e: std::io::Error| SevenZipError::HeaderError(e.to_string());

        w.write_all(&[K_EMPTY_FILE]).map_err(map_err)?;

        let mut data = Vec::new();
        write_bool_vector(&mut data, empty_file).map_err(map_err)?;

        write_number(w, data.len() as u64).map_err(map_err)?;
        w.write_all(&data).map_err(map_err)?;

        Ok(())
    }

    fn write_mtime_property(&self, w: &mut Vec<u8>) -> Result<()> {
        let map_err = |e: std::io::Error| SevenZipError::HeaderError(e.to_string());

        w.write_all(&[K_M_TIME]).map_err(map_err)?;

        let mut data = Vec::new();

        // Defined vector: which files have mtime defined
        let defined: Vec<bool> = self.files.iter().map(|f| f.modified_time.is_some()).collect();
        let all_defined = defined.iter().all(|&b| b);

        if all_defined {
            // AllAreDefined = 1
            data.write_all(&[0x01]).map_err(map_err)?;
        } else {
            // AllAreDefined = 0, then write defined vector
            data.write_all(&[0x00]).map_err(map_err)?;
            write_bool_vector(&mut data, &defined).map_err(map_err)?;
        }

        // External = 0
        data.write_all(&[0x00]).map_err(map_err)?;

        // Write FILETIME values for defined entries
        for file in &self.files {
            if let Some(ft) = file.modified_time {
                write_u64_le(&mut data, ft).map_err(map_err)?;
            }
        }

        write_number(w, data.len() as u64).map_err(map_err)?;
        w.write_all(&data).map_err(map_err)?;

        Ok(())
    }
}

/// Converts a Unix timestamp (seconds since epoch) to a Windows FILETIME.
pub fn unix_to_filetime(unix_secs: u64) -> u64 {
    (unix_secs + 11_644_473_600) * 10_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unix_to_filetime() {
        // Unix epoch = Jan 1 1970 -> FILETIME for that
        let ft = unix_to_filetime(0);
        assert_eq!(ft, 116_444_736_000_000_000);
    }

    #[test]
    fn test_serialize_empty_archive() {
        let header = ArchiveHeader {
            folders: vec![],
            files: vec![],
            pack_position: 0,
        };
        let data = header.serialize().unwrap();
        // kHeader + kEnd
        assert_eq!(data, vec![K_HEADER, K_END]);
    }

    #[test]
    fn test_serialize_header_with_one_file() {
        let header = ArchiveHeader {
            folders: vec![FolderInfo {
                compressed_size: 100,
                uncompressed_size: 200,
                uncompressed_crc: 0x12345678,
                lzma2_properties_byte: 23,
            }],
            files: vec![FileEntry {
                name: "test.txt".to_string(),
                uncompressed_size: 200,
                compressed_size: 100,
                crc: 0x12345678,
                has_data: true,
                modified_time: None,
            }],
            pack_position: 0,
        };
        let data = header.serialize().unwrap();
        // Should start with kHeader and contain pack info, coders info, files info
        assert_eq!(data[0], K_HEADER);
        assert_eq!(data[1], K_MAIN_STREAMS_INFO);
        // Should end with kEnd
        assert_eq!(*data.last().unwrap(), K_END);
    }
}
