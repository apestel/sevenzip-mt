use std::io::{self, Write};

/// Wraps a writer and tracks the total number of bytes written.
pub struct CountingWriter<W: Write> {
    inner: W,
    bytes_written: u64,
}

impl<W: Write> CountingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            bytes_written: 0,
        }
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counting_writer() {
        let mut cw = CountingWriter::new(Vec::new());
        cw.write_all(b"hello").unwrap();
        assert_eq!(cw.bytes_written(), 5);
        cw.write_all(b" world").unwrap();
        assert_eq!(cw.bytes_written(), 11);
        assert_eq!(cw.into_inner(), b"hello world");
    }
}
