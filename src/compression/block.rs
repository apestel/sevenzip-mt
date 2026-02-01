/// A raw (uncompressed) block of data with its index.
pub struct RawBlock {
    pub data: Vec<u8>,
    pub block_index: usize,
}

/// A compressed block with metadata.
pub struct CompressedBlock {
    pub compressed_data: Vec<u8>,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub uncompressed_crc: u32,
    pub block_index: usize,
}

/// Splits data into blocks of at most `block_size` bytes.
pub fn split_into_blocks(data: &[u8], block_size: usize) -> Vec<RawBlock> {
    data.chunks(block_size)
        .enumerate()
        .map(|(i, chunk)| RawBlock {
            data: chunk.to_vec(),
            block_index: i,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_exact() {
        let data = vec![1, 2, 3, 4, 5, 6];
        let blocks = split_into_blocks(&data, 3);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].data, vec![1, 2, 3]);
        assert_eq!(blocks[0].block_index, 0);
        assert_eq!(blocks[1].data, vec![4, 5, 6]);
        assert_eq!(blocks[1].block_index, 1);
    }

    #[test]
    fn test_split_remainder() {
        let data = vec![1, 2, 3, 4, 5];
        let blocks = split_into_blocks(&data, 3);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].data, vec![4, 5]);
    }

    #[test]
    fn test_split_single_block() {
        let data = vec![1, 2, 3];
        let blocks = split_into_blocks(&data, 10);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].data, vec![1, 2, 3]);
    }

    #[test]
    fn test_split_empty() {
        let data: Vec<u8> = vec![];
        let blocks = split_into_blocks(&data, 10);
        assert_eq!(blocks.len(), 0);
    }
}
