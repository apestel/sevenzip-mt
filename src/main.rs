#![forbid(unsafe_code)]

use clap::Parser;
use sevenzip_mt::{Lzma2Config, SevenZipWriter};
use std::path::PathBuf;
use std::process::ExitCode;

/// Create 7z archives with LZMA2 multi-threaded compression
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path to the output .7z archive
    output: PathBuf,

    /// Files to add to the archive
    #[arg(required = true)]
    files: Vec<PathBuf>,

    /// Compression level 0-9
    #[arg(short, long, default_value_t = 6)]
    level: u32,
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if cli.level > 9 {
        return Err(format!("compression level must be 0-9, got {}", cli.level).into());
    }

    for path in &cli.files {
        if !path.exists() {
            return Err(format!("file not found: {}", path.display()).into());
        }
    }

    let output_file = std::fs::File::create(&cli.output)?;
    let mut archive = SevenZipWriter::new(output_file)?;

    archive.set_config(Lzma2Config {
        preset: cli.level,
        dict_size: None,
        block_size: None,
    });

    for path in &cli.files {
        let archive_name = path
            .file_name()
            .ok_or_else(|| format!("cannot determine file name for {}", path.display()))?
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 file name: {}", path.display()))?;

        archive.add_file(&path.to_string_lossy(), archive_name)?;
    }

    archive.finish()?;

    eprintln!(
        "Created {} with {} file(s)",
        cli.output.display(),
        cli.files.len()
    );

    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
