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
    /// Make the entries of directory `path` durable: names created,
    /// renamed or removed in it survive power loss once this returns.
    /// A file's own `sync_data` does not cover its directory entry.
    fn sync_dir(&self, path: &Path) -> Result<()>;
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

    /// On unix this opens the directory and calls `fsync` on it.
    ///
    /// On other targets it returns `Ok(())` without doing anything: there
    /// is no portable way to fsync a directory there, so directory-entry
    /// durability is only claimed on unix.
    fn sync_dir(&self, path: &Path) -> Result<()> {
        #[cfg(unix)]
        File::open(path)?.sync_all()?;
        #[cfg(not(unix))]
        let _ = path;
        #[cfg(test)]
        SYNCED_DIRS.with(|dirs| dirs.borrow_mut().push(path.to_path_buf()));
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static SYNCED_DIRS: std::cell::RefCell<Vec<std::path::PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Directories this thread synced through [`StdFileSystem::sync_dir`], in
/// order, since the last call. Per thread so parallel tests do not mix.
#[cfg(test)]
pub(crate) fn take_synced_dirs() -> Vec<std::path::PathBuf> {
    SYNCED_DIRS.with(|dirs| std::mem::take(&mut *dirs.borrow_mut()))
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

/// Create `path` and any missing ancestors, then make every new name
/// durable by syncing the parent of each directory this call created,
/// outermost first.
///
/// A directory that already existed is left alone: whoever created it was
/// responsible for its name. Which directories are missing is checked on
/// the real file system. A relative path's outermost new directory is
/// synced through `.`.
pub fn create_dir_all_durable<Fs: FileSystem + ?Sized>(fs: &Fs, path: &Path) -> Result<()> {
    let mut created = Vec::new();
    let mut cursor = Some(path);
    while let Some(dir) = cursor {
        if dir.as_os_str().is_empty() || dir.try_exists()? {
            break;
        }
        created.push(dir);
        cursor = dir.parent();
    }
    fs.create_dir_all(path)?;
    for dir in created.iter().rev() {
        match dir.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => fs.sync_dir(parent)?,
            _ => fs.sync_dir(Path::new("."))?,
        }
    }
    Ok(())
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
