//! SQLite-compatible C ABI surface.
//!
//! These entry points preserve the standard `sqlite3_*` symbol names so
//! callers linked against libsqlite3 can swap in libredlinedb at runtime.
//! They delegate to the corresponding `rldb_*` implementation, layering on
//! the status-recording semantics expected by the SQLite ABI.

use std::os::raw::{c_char, c_int};
use std::ptr;

mod bind;
pub mod blob;
pub mod collation;
mod column;
pub mod context;
mod core;
mod exec;
pub mod hooks;
pub mod hooks_fire;
mod meta;
pub mod result;
mod stmt;
pub mod udf;
mod user_data;
pub mod value;

pub use bind::*;
pub use blob::*;
pub use collation::*;
pub use column::*;
pub use context::*;
pub use core::*;
pub use exec::*;
pub use hooks::*;
pub use hooks_fire::*;
#[allow(unused_imports)]
pub(crate) use meta::*;
pub use result::*;
pub use stmt::*;
pub use udf::*;
pub use value::*;

use crate::types::{sqlite3, sqlite3_backup};
use crate::util::{record_status, with_db};
use crate::{
    rldb_backup_close, rldb_backup_finish, rldb_backup_init, rldb_backup_pagecount,
    rldb_backup_remaining, rldb_backup_step,
};

/// # Safety
///
/// `dst` and `src` must each be NULL or a live database handle. The name
/// arguments are not read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_backup_init(
    dst: *mut sqlite3,
    _dst_name: *const c_char,
    src: *mut sqlite3,
    _src_name: *const c_char,
) -> *mut sqlite3_backup {
    if dst.is_null() || src.is_null() {
        return ptr::null_mut();
    }
    let dst_path = match with_db(dst, |db| db.path_text.as_ptr()) {
        Ok(path) => path,
        Err(code) => {
            record_status(dst, code);
            return ptr::null_mut();
        }
    };
    let mut backup = ptr::null_mut();
    // SAFETY: `src` is live per this function's contract; `dst_path` points
    // into the live `dst` handle's NUL-terminated path; the config is NULL and
    // `backup` is a local slot.
    let rc = unsafe { rldb_backup_init(src, dst_path, ptr::null(), &mut backup) };
    record_status(dst, rc);
    if rc == 0 { backup } else { ptr::null_mut() }
}

/// # Safety
///
/// `backup` must be NULL or a live backup handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_backup_step(backup: *mut sqlite3_backup, pages: c_int) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_backup_step(backup, pages) }
}

/// # Safety
///
/// `backup` must be NULL or a live backup handle. After a successful finish it
/// is dangling and must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_backup_finish(backup: *mut sqlite3_backup) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_backup_finish(backup) };
    if rc != 0 {
        return rc;
    }
    // SAFETY: `backup` is live per this function's contract and the finish
    // above did not close it; it is closed exactly once here.
    unsafe { rldb_backup_close(backup) }
}

/// # Safety
///
/// `backup` must be NULL or a live backup handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_backup_remaining(backup: *mut sqlite3_backup) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_backup_remaining(backup) }
}

/// # Safety
///
/// `backup` must be NULL or a live backup handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_backup_pagecount(backup: *mut sqlite3_backup) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_backup_pagecount(backup) }
}
