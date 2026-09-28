//! Copy the redlinedb data directory to a destination path for snapshot use.

use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::Database;
use crate::error::Result;
use crate::options::{BackupOptions, BackupStats};

#[cfg(test)]
#[path = "backup_hold_tests.rs"]
mod backup_hold_tests;

pub(crate) fn backup_to_path(
    src: &Database,
    dst: impl AsRef<Path>,
    _options: BackupOptions,
) -> Result<BackupStats> {
    let start = Instant::now();
    let dst = dst.as_ref();
    if dst.exists() {
        fs::remove_dir_all(dst)?;
    }
    fs::create_dir_all(dst)?;
    // No checkpoint may run while the files are copied: a pressure
    // checkpoint between the control files and the page file would leave a
    // copy whose control generation and pages disagree.
    let (_checkpoint, _hold) = src.inner.db.checkpoint_and_hold()?;
    run_before_copy_hook();
    copy_dir(src.path(), dst)?;
    Ok(BackupStats {
        tables_copied: 0,
        rows_copied: 0,
        indexes_created: 0,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "owner.lock" {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir_all(&dst_path)?;
            copy_dir(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static BEFORE_COPY: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test hook: runs on the copying thread once a backup holds checkpoints
/// off, before it reads any file.
#[cfg(test)]
pub(crate) fn set_before_copy_hook(hook: Option<Box<dyn FnMut()>>) {
    BEFORE_COPY.with(|slot| *slot.borrow_mut() = hook);
}

pub(crate) fn run_before_copy_hook() {
    #[cfg(test)]
    BEFORE_COPY.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook();
        }
    });
}
