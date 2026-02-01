use sevenzip_mt::Lzma2Config;
use sha2::{Digest, Sha256};
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[test]
fn test_create_and_extract_single_file() {
    let dir = TempDir::new().unwrap();
    let archive_path = dir.path().join("test.7z");
    let extract_dir = dir.path().join("extracted");
    fs::create_dir_all(&extract_dir).unwrap();

    let content = b"Hello, 7-Zip! This is a test file with some content for compression.";
    let content_hash = sha256_hex(content);

    // Create archive
    let file = fs::File::create(&archive_path).unwrap();
    let mut archive = sevenzip_mt::SevenZipWriter::new(file).unwrap();
    archive.add_bytes("hello.txt", content).unwrap();
    archive.finish().unwrap();

    // Verify with 7z t (integrity test)
    let output = Command::new("7z")
        .args(["t", archive_path.to_str().unwrap()])
        .output()
        .expect("failed to run 7z");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "7z t failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Extract with 7z x
    let output = Command::new("7z")
        .args([
            "x",
            archive_path.to_str().unwrap(),
            &format!("-o{}", extract_dir.to_str().unwrap()),
            "-y",
        ])
        .output()
        .expect("failed to run 7z");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "7z x failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Verify extracted content matches
    let extracted = fs::read(extract_dir.join("hello.txt")).unwrap();
    assert_eq!(sha256_hex(&extracted), content_hash);
    assert_eq!(extracted, content);
}

#[test]
fn test_create_and_extract_multiple_files() {
    let dir = TempDir::new().unwrap();
    let archive_path = dir.path().join("multi.7z");
    let extract_dir = dir.path().join("extracted");
    fs::create_dir_all(&extract_dir).unwrap();

    let files: Vec<(&str, Vec<u8>)> = vec![
        ("file1.txt", b"First file content".to_vec()),
        ("file2.bin", vec![0u8; 1024]),
        (
            "subdir/file3.txt",
            "Third file with unicode: \u{00e9}\u{00e8}\u{00ea}".into(),
        ),
    ];

    let hashes: Vec<String> = files.iter().map(|(_, data)| sha256_hex(data)).collect();

    // Create archive
    let file = fs::File::create(&archive_path).unwrap();
    let mut archive = sevenzip_mt::SevenZipWriter::new(file).unwrap();
    for (name, data) in &files {
        archive.add_bytes(name, data).unwrap();
    }
    archive.finish().unwrap();

    // Verify integrity
    let output = Command::new("7z")
        .args(["t", archive_path.to_str().unwrap()])
        .output()
        .expect("failed to run 7z");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "7z t failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Extract
    let output = Command::new("7z")
        .args([
            "x",
            archive_path.to_str().unwrap(),
            &format!("-o{}", extract_dir.to_str().unwrap()),
            "-y",
        ])
        .output()
        .expect("failed to run 7z");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "7z x failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Verify each extracted file
    for (i, (name, original_data)) in files.iter().enumerate() {
        let extracted = fs::read(extract_dir.join(name)).unwrap();
        assert_eq!(
            sha256_hex(&extracted),
            hashes[i],
            "hash mismatch for {name}"
        );
        assert_eq!(extracted, *original_data, "content mismatch for {name}");
    }
}

#[test]
fn test_add_file_from_disk() {
    let dir = TempDir::new().unwrap();
    let archive_path = dir.path().join("disk.7z");
    let extract_dir = dir.path().join("extracted");
    let source_file = dir.path().join("source.txt");
    fs::create_dir_all(&extract_dir).unwrap();

    let content = b"Content from a real file on disk.";
    fs::write(&source_file, content).unwrap();
    let content_hash = sha256_hex(content);

    // Create archive
    let file = fs::File::create(&archive_path).unwrap();
    let mut archive = sevenzip_mt::SevenZipWriter::new(file).unwrap();
    archive
        .add_file(source_file.to_str().unwrap(), "source.txt")
        .unwrap();
    archive.finish().unwrap();

    // Verify and extract
    let output = Command::new("7z")
        .args(["t", archive_path.to_str().unwrap()])
        .output()
        .expect("failed to run 7z");
    assert!(
        output.status.success(),
        "7z t failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let output = Command::new("7z")
        .args([
            "x",
            archive_path.to_str().unwrap(),
            &format!("-o{}", extract_dir.to_str().unwrap()),
            "-y",
        ])
        .output()
        .expect("failed to run 7z");
    assert!(
        output.status.success(),
        "7z x failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let extracted = fs::read(extract_dir.join("source.txt")).unwrap();
    assert_eq!(sha256_hex(&extracted), content_hash);
}

#[test]
fn test_empty_file_in_archive() {
    let dir = TempDir::new().unwrap();
    let archive_path = dir.path().join("empty.7z");
    let extract_dir = dir.path().join("extracted");
    fs::create_dir_all(&extract_dir).unwrap();

    // Create archive with an empty file and a non-empty file
    let file = fs::File::create(&archive_path).unwrap();
    let mut archive = sevenzip_mt::SevenZipWriter::new(file).unwrap();
    archive.add_bytes("nonempty.txt", b"some data").unwrap();
    archive.add_bytes("empty.txt", b"").unwrap();
    archive.finish().unwrap();

    // Verify integrity
    let output = Command::new("7z")
        .args(["t", archive_path.to_str().unwrap()])
        .output()
        .expect("failed to run 7z");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "7z t failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Extract
    let output = Command::new("7z")
        .args([
            "x",
            archive_path.to_str().unwrap(),
            &format!("-o{}", extract_dir.to_str().unwrap()),
            "-y",
        ])
        .output()
        .expect("failed to run 7z");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "7z x failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Verify
    let nonempty = fs::read(extract_dir.join("nonempty.txt")).unwrap();
    assert_eq!(nonempty, b"some data");

    let empty = fs::read(extract_dir.join("empty.txt")).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn test_large_file_compression() {
    let dir = TempDir::new().unwrap();
    let archive_path = dir.path().join("large.7z");
    let extract_dir = dir.path().join("extracted");
    fs::create_dir_all(&extract_dir).unwrap();

    // Create a 1MB file with repetitive data (compresses well)
    let content: Vec<u8> = (0..1_048_576).map(|i| (i % 251) as u8).collect();
    let content_hash = sha256_hex(&content);

    let file = fs::File::create(&archive_path).unwrap();
    let mut archive = sevenzip_mt::SevenZipWriter::new(file).unwrap();
    archive.add_bytes("large.bin", &content).unwrap();
    archive.finish().unwrap();

    // Verify compressed archive is smaller than original
    let archive_size = fs::metadata(&archive_path).unwrap().len();
    assert!(
        archive_size < content.len() as u64,
        "archive ({archive_size}) should be smaller than original ({})",
        content.len()
    );

    // Verify and extract
    let output = Command::new("7z")
        .args(["t", archive_path.to_str().unwrap()])
        .output()
        .expect("failed to run 7z");
    assert!(
        output.status.success(),
        "7z t failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let output = Command::new("7z")
        .args([
            "x",
            archive_path.to_str().unwrap(),
            &format!("-o{}", extract_dir.to_str().unwrap()),
            "-y",
        ])
        .output()
        .expect("failed to run 7z");
    assert!(
        output.status.success(),
        "7z x failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let extracted = fs::read(extract_dir.join("large.bin")).unwrap();
    assert_eq!(sha256_hex(&extracted), content_hash);
    assert_eq!(extracted.len(), content.len());
}

#[test]
fn test_intra_file_block_splitting() {
    let dir = TempDir::new().unwrap();
    let archive_path = dir.path().join("split.7z");
    let extract_dir = dir.path().join("extracted");
    fs::create_dir_all(&extract_dir).unwrap();

    // 100 KiB of data with a small block_size (16 KiB) to force splitting
    // into ~7 blocks, exercising parallel intra-file compression.
    let content: Vec<u8> = (0..102_400).map(|i| (i % 251) as u8).collect();
    let content_hash = sha256_hex(&content);

    let file = fs::File::create(&archive_path).unwrap();
    let mut archive = sevenzip_mt::SevenZipWriter::new(file).unwrap();
    archive.set_config(Lzma2Config {
        preset: 1,
        dict_size: None,
        block_size: Some(16_384), // 16 KiB blocks
    });
    archive.add_bytes("split.bin", &content).unwrap();
    archive.finish().unwrap();

    // Verify integrity with 7z
    let output = Command::new("7z")
        .args(["t", archive_path.to_str().unwrap()])
        .output()
        .expect("failed to run 7z");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "7z t failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Extract and verify content
    let output = Command::new("7z")
        .args([
            "x",
            archive_path.to_str().unwrap(),
            &format!("-o{}", extract_dir.to_str().unwrap()),
            "-y",
        ])
        .output()
        .expect("failed to run 7z");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "7z x failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let extracted = fs::read(extract_dir.join("split.bin")).unwrap();
    assert_eq!(sha256_hex(&extracted), content_hash);
    assert_eq!(extracted.len(), content.len());
}
