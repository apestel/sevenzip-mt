# sevenzip-mt

Pure-Rust, multi-threaded 7z archive creator with LZMA2 compression.

Archives produced by this crate are compatible with the official [7-Zip](https://www.7-zip.org/) tool.

## Features

- **100% safe Rust** -- `#![forbid(unsafe_code)]`, no FFI, no C/C++ dependencies
- **LZMA2 compression** with configurable preset (0-9)
- **Multi-threaded** block compression via a dedicated rayon thread pool
- **Intra-file parallelism** -- large files are split into blocks and compressed in parallel
- **Memory-efficient** -- disk files are read in chunks, compressed blocks are freed immediately after writing
- **Compatible** with official 7-Zip (verified by integration tests)

## What this library does NOT do

- Decompression / extraction
- Encryption (AES)
- Solid compression (multi-file streams)
- BCJ / Delta filters
- Streaming input

## Library usage

Add to your `Cargo.toml`:

```toml
[dependencies]
sevenzip-mt = "0.1.0"
```

### Basic example

```rust
use sevenzip_mt::SevenZipWriter;

let file = std::fs::File::create("archive.7z")?;
let mut archive = SevenZipWriter::new(file)?;

archive.add_file("path/on/disk.txt", "name-in-archive.txt")?;
archive.add_bytes("hello.txt", b"Hello, world!")?;

archive.finish()?;
```

### Configuration

```rust
use sevenzip_mt::{SevenZipWriter, Lzma2Config};

let file = std::fs::File::create("archive.7z")?;
let mut archive = SevenZipWriter::new(file)?;

archive.set_config(Lzma2Config {
    preset: 9,              // 0-9, default 6
    dict_size: None,        // override dictionary size (bytes), or None for preset default
    block_size: Some(1 << 22), // intra-file block size (bytes), or None for 2x dict_size
});

archive.set_num_threads(Some(4)); // or None for auto-detect

archive.add_bytes("data.bin", &data)?;
archive.finish()?;
```

### Public API

| Type | Description |
|---|---|
| `SevenZipWriter<W>` | Archive builder. `W: Write + Seek`. |
| `Lzma2Config` | Compression configuration (preset, dict size, block size). |
| `SevenZipError` | Error enum covering I/O, compression, header, threading. |

**`SevenZipWriter` methods:**

| Method | Description |
|---|---|
| `new(writer)` | Create a new archive writer. |
| `set_config(config)` | Set LZMA2 compression configuration. |
| `set_num_threads(n)` | Set thread count (`None` = auto). |
| `add_file(disk_path, archive_name)` | Queue a file from disk. |
| `add_bytes(archive_name, data)` | Queue in-memory data. |
| `finish()` | Compress, write, and finalize the archive. Consumes `self`. |

## CLI

The crate also ships a binary:

```
sevenzip-mt <OUTPUT> <FILES>... [OPTIONS]

Arguments:
  <OUTPUT>    Path to the output .7z archive
  <FILES>...  Files to add to the archive

Options:
  -l, --level <LEVEL>      Compression level 0-9 [default: 6]
  -t, --threads <THREADS>  Number of threads [default: logical CPUs]
  -h, --help               Print help
  -V, --version            Print version
```

Example:

```bash
sevenzip-mt archive.7z file1.txt file2.txt --level 9 --threads 4
```

## How it works

1. Files are split into blocks (default size: 2x LZMA2 dictionary size, minimum 1 MiB).
2. All blocks are compressed in parallel on a dedicated rayon thread pool.
3. Compressed LZMA2 streams belonging to the same file are concatenated (intermediate end-of-stream markers stripped).
4. Compressed data is written sequentially; each block is freed immediately after writing.
5. The 7z header is built from collected metadata and written at the end of the file.
6. The signature header is written back at the start of the file.

Disk files are read in chunks directly into blocks -- the full file is never held as a single allocation.

## Testing

```bash
cargo test
```

The test suite includes:

- **Unit tests** -- binary serialization, CRC, LZMA2 block compression, stream concatenation, thread pool configuration
- **Integration tests** -- archive creation, extraction with official `7z`, SHA-256 verification of extracted files

Integration tests require the `7z` command-line tool to be installed.

## Dependencies

| Crate | Purpose |
|---|---|
| `lzma-rust2` | LZMA2 compression (pure Rust) |
| `rayon` | Parallel block compression |
| `crc32fast` | CRC-32 checksums |
| `byteorder` | Binary serialization |
| `thiserror` | Error types |
| `clap` | CLI argument parsing |

## License

See `Cargo.toml` for details.
