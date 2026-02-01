use byteorder::{LittleEndian, WriteBytesExt};
use std::io::Write;

/// Writes a 7z variable-length encoded integer (NUMBER).
///
/// Encoding: the first byte has a leading sequence of 1-bits that indicates
/// how many additional bytes follow. The remaining bits of the first byte
/// are the most significant bits of the value.
///
/// - 0xxxxxxx                             -> 1 byte,  value 0..127
/// - 10xxxxxx yyyyyyyy                    -> 2 bytes, value 0..16383
/// - 110xxxxx + 2 bytes                   -> 3 bytes
/// - ...up to...
/// - 11111111 + 8 bytes                   -> 9 bytes, full u64
pub fn write_number<W: Write>(w: &mut W, value: u64) -> std::io::Result<()> {
    if value < 0x80 {
        w.write_u8(value as u8)?;
        return Ok(());
    }

    // Find the minimum number of value bytes needed
    let mut byte_count = 1u8;

    // Determine how many bytes the value part needs
    while byte_count < 8 {
        let bits_available = 8 - (byte_count + 1); // bits available in first byte
        let max_first_byte = (1u64 << bits_available) - 1;
        let max_value = (max_first_byte << (byte_count * 8))
            | ((1u64 << (byte_count * 8)) - 1);
        if value <= max_value {
            break;
        }
        byte_count += 1;
    }

    if byte_count >= 8 {
        // 9-byte encoding: first byte is 0xFF, then 8 bytes LE
        w.write_u8(0xFF)?;
        w.write_u64::<LittleEndian>(value)?;
        return Ok(());
    }

    // Build the first byte: byte_count leading 1-bits, then value bits
    let mask: u8 = !((0xFFu16 >> byte_count) as u8); // leading 1-bits
    let shift = byte_count * 8;
    let first_byte_value = (value >> shift) as u8;
    let first_byte = mask | first_byte_value;

    w.write_u8(first_byte)?;

    // Write the remaining bytes in little-endian order
    for i in 0..byte_count {
        w.write_u8((value >> (i * 8)) as u8)?;
    }

    Ok(())
}

pub fn write_u32_le<W: Write>(w: &mut W, value: u32) -> std::io::Result<()> {
    w.write_u32::<LittleEndian>(value)
}

pub fn write_u64_le<W: Write>(w: &mut W, value: u64) -> std::io::Result<()> {
    w.write_u64::<LittleEndian>(value)
}

/// Writes a UTF-16LE encoded string with null terminator.
pub fn write_utf16le_string<W: Write>(w: &mut W, s: &str) -> std::io::Result<()> {
    for code_unit in s.encode_utf16() {
        w.write_u16::<LittleEndian>(code_unit)?;
    }
    // Null terminator
    w.write_u16::<LittleEndian>(0)?;
    Ok(())
}

/// Writes a bit vector. Each bool maps to one bit, packed into bytes MSB-first.
/// Padding bits in the last byte are 0.
pub fn write_bool_vector<W: Write>(w: &mut W, bools: &[bool]) -> std::io::Result<()> {
    let mut current_byte: u8 = 0;
    let mut bit_index: u8 = 0;

    for &b in bools {
        if b {
            current_byte |= 1 << (7 - bit_index);
        }
        bit_index += 1;
        if bit_index == 8 {
            w.write_u8(current_byte)?;
            current_byte = 0;
            bit_index = 0;
        }
    }

    if bit_index > 0 {
        w.write_u8(current_byte)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_number(value: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        write_number(&mut buf, value).unwrap();
        buf
    }

    #[test]
    fn test_number_single_byte() {
        assert_eq!(encode_number(0), vec![0x00]);
        assert_eq!(encode_number(1), vec![0x01]);
        assert_eq!(encode_number(0x7F), vec![0x7F]);
    }

    #[test]
    fn test_number_two_bytes() {
        // 128 = 0x80 -> 2-byte encoding
        // First byte: 10_000000 | (128 >> 8) = 0x80 | 0 = 0x80
        // Second byte: 128 & 0xFF = 0x80
        assert_eq!(encode_number(128), vec![0x80, 0x80]);
        // 16383 = 0x3FFF
        assert_eq!(encode_number(0x3FFF), vec![0xBF, 0xFF]);
    }

    #[test]
    fn test_number_three_bytes() {
        // 16384 = 0x4000 -> 3-byte encoding
        assert_eq!(encode_number(0x4000), vec![0xC0, 0x00, 0x40]);
    }

    #[test]
    fn test_number_nine_bytes() {
        let val = u64::MAX;
        let result = encode_number(val);
        assert_eq!(result.len(), 9);
        assert_eq!(result[0], 0xFF);
    }

    #[test]
    fn test_utf16le_string() {
        let mut buf = Vec::new();
        write_utf16le_string(&mut buf, "a").unwrap();
        assert_eq!(buf, vec![0x61, 0x00, 0x00, 0x00]); // 'a' + null
    }

    #[test]
    fn test_bool_vector() {
        let mut buf = Vec::new();
        write_bool_vector(&mut buf, &[true, false, true, false, false, false, false, false]).unwrap();
        assert_eq!(buf, vec![0b10100000]);

        let mut buf = Vec::new();
        write_bool_vector(&mut buf, &[true, true]).unwrap();
        assert_eq!(buf, vec![0b11000000]);
    }
}
