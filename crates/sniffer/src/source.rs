use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

/// Random-access byte source. Reads are bounds-checked by the caller.
pub trait Source {
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fill `buf` from `offset`. Fails if the range runs past the end.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;

    /// Read `len` bytes from `offset`, or fail if the range is out of bounds.
    fn read_vec(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let end = offset
            .checked_add(len as u64)
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        if end > self.len() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let mut buf = vec![0u8; len];
        self.read_at(offset, &mut buf)?;
        Ok(buf)
    }
}

/// A file opened for sniffing. Its length is fixed when opened.
pub struct FileSource {
    file: File,
    len: u64,
}

impl FileSource {
    pub fn new(file: File) -> io::Result<Self> {
        let len = file.metadata()?.len();
        Ok(FileSource { file, len })
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buf)
    }
}

/// In-memory bytes.
pub struct SliceSource<'a> {
    bytes: &'a [u8],
}

impl<'a> SliceSource<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        SliceSource { bytes }
    }
}

impl Source for SliceSource<'_> {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let end = start
            .checked_add(buf.len())
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        let src = self
            .bytes
            .get(start..end)
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}
