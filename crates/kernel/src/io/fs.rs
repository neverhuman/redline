use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::FileExt;

use crate::Result;

pub trait FileHandle {
    fn len(&self) -> Result<u64>;
    fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
    fn write_all_at(&mut self, offset: u64, buf: &[u8]) -> Result<()>;
    fn sync_data(&self) -> Result<()>;
    fn set_len(&self, len: u64) -> Result<()>;
}

pub trait FileSystem {
    type File: FileHandle;

    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn read_dir_names(&self, path: &Path) -> Result<Vec<String>>;
    fn open_rw_create(&self, path: &Path) -> Result<Self::File>;
    fn open_rw_existing(&self, path: &Path) -> Result<Self::File>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StdFileSystem;

impl FileSystem for StdFileSystem {
    type File = StdFileHandle;

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path)?;
        Ok(())
    }

    fn read_dir_names(&self, path: &Path) -> Result<Vec<String>> {
        let mut names = Vec::new();
        if !path.exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    fn open_rw_create(&self, path: &Path) -> Result<Self::File> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        Ok(StdFileHandle(file))
    }

    fn open_rw_existing(&self, path: &Path) -> Result<Self::File> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(StdFileHandle(file))
    }
}

#[derive(Debug)]
pub struct StdFileHandle(File);

impl FileHandle for StdFileHandle {
    fn len(&self) -> Result<u64> {
        Ok(self.0.metadata()?.len())
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        read_exact_at(&self.0, offset, buf)
    }

    fn write_all_at(&mut self, offset: u64, buf: &[u8]) -> Result<()> {
        write_all_at(&self.0, offset, buf)
    }

    fn sync_data(&self) -> Result<()> {
        self.0.sync_data()?;
        Ok(())
    }

    fn set_len(&self, len: u64) -> Result<()> {
        self.0.set_len(len)?;
        Ok(())
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = file.read_at(&mut buf[filled..], offset + filled as u64)?;
        if n == 0 {
            return Err(
                io::Error::new(ErrorKind::UnexpectedEof, "failed to fill whole buffer").into(),
            );
        }
        filled += n;
    }
    Ok(())
}

#[cfg(unix)]
fn write_all_at(file: &File, offset: u64, buf: &[u8]) -> Result<()> {
    let mut written = 0;
    while written < buf.len() {
        let n = file.write_at(&buf[written..], offset + written as u64)?;
        if n == 0 {
            return Err(
                io::Error::new(ErrorKind::WriteZero, "failed to write whole buffer").into(),
            );
        }
        written += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_reads_do_not_depend_on_write_order() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut file = StdFileSystem
            .open_rw_create(&dir.path().join("bytes"))
            .expect("open");
        file.write_all_at(8, b"xyz").expect("tail");
        file.write_all_at(0, b"abcd").expect("head");
        let mut head = [0_u8; 4];
        let mut tail = [0_u8; 3];
        file.read_exact_at(0, &mut head).expect("read head");
        file.read_exact_at(8, &mut tail).expect("read tail");
        assert_eq!(&head, b"abcd");
        assert_eq!(&tail, b"xyz");
    }
}
