use crate::archive::header::SIGNATURE;
use byteorder::{LittleEndian, WriteBytesExt};
use std::io::Write;

/// 7z format version: major 0, minor 4.
const VERSION_MAJOR: u8 = 0;
const VERSION_MINOR: u8 = 4;

/// Size of the SignatureHeader in bytes.
pub const SIGNATURE_HEADER_SIZE: u64 = 32;

/// Writes the 32-byte SignatureHeader to the writer.
///
/// Layout (32 bytes):
///   [0..6]   Signature: '7' 'z' 0xBC 0xAF 0x27 0x1C
///   [6..7]   ArchiveVersion.Major
///   [7..8]   ArchiveVersion.Minor
///   [8..12]  StartHeaderCRC (CRC32 of bytes 12..32)
///   [12..20] NextHeaderOffset (u64 LE)
///   [20..28] NextHeaderSize (u64 LE)
///   [28..32] NextHeaderCRC (u32 LE)
pub fn write_signature_header<W: Write>(
    w: &mut W,
    next_header_offset: u64,
    next_header_size: u64,
    next_header_crc: u32,
) -> std::io::Result<()> {
    // Build the 20 bytes that StartHeaderCRC covers (bytes 12..32)
    let mut start_header_data = Vec::with_capacity(20);
    start_header_data.write_u64::<LittleEndian>(next_header_offset)?;
    start_header_data.write_u64::<LittleEndian>(next_header_size)?;
    start_header_data.write_u32::<LittleEndian>(next_header_crc)?;

    let start_header_crc = crc32fast::hash(&start_header_data);

    // Write the full 32-byte header
    w.write_all(&SIGNATURE)?;
    w.write_u8(VERSION_MAJOR)?;
    w.write_u8(VERSION_MINOR)?;
    w.write_u32::<LittleEndian>(start_header_crc)?;
    w.write_all(&start_header_data)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_header_size() {
        let mut buf = Vec::new();
        write_signature_header(&mut buf, 0, 0, 0).unwrap();
        assert_eq!(buf.len(), 32);
    }

    #[test]
    fn test_signature_header_starts_with_signature() {
        let mut buf = Vec::new();
        write_signature_header(&mut buf, 100, 50, 0xDEADBEEF).unwrap();
        assert_eq!(&buf[0..6], &SIGNATURE);
        assert_eq!(buf[6], 0); // major
        assert_eq!(buf[7], 4); // minor
    }

    #[test]
    fn test_signature_header_crc_covers_20_bytes() {
        let mut buf = Vec::new();
        write_signature_header(&mut buf, 100, 50, 0xAABBCCDD).unwrap();

        // Verify StartHeaderCRC (bytes 8..12) matches CRC of bytes 12..32
        let start_header_crc = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let computed_crc = crc32fast::hash(&buf[12..32]);
        assert_eq!(start_header_crc, computed_crc);
    }
}
