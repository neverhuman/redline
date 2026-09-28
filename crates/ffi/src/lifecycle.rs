//! Database open/close FFI surface.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::Ordering;

use crate::types::*;
use crate::util::{api, flatten_code, open_handle, reclaim_box};

/// # Safety
///
/// - `path` must be NULL or point to a NUL-terminated string.
/// - `out_db` must be NULL or valid for writing one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_open(path: *const c_char, out_db: *mut *mut rldb) -> c_int {
    flatten_code(api(|| {
        if path.is_null() || out_db.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: `path` non-null (checked); per redlinedb.h:85 it is a
        // NUL-terminated C string; open_handle copies it into owned PathBuf.
        let handle = open_handle(unsafe { CStr::from_ptr(path) }, None, true, false)?;
        // SAFETY: `out_db` non-null (checked); per redlinedb.h:85 it is a
        // writable rldb**; open_handle returned a Box::into_raw pointer
        // whose ownership transfers to the C caller (paired with rldb_close).
        unsafe {
            *out_db = handle;
        }
        Ok(RLDB_OK)
    }))
}

/// # Safety
///
/// - `path` must be NULL or point to a NUL-terminated string.
/// - `config` must be NULL or point to a readable, initialised `rldb_config`.
/// - `out_db` must be NULL or valid for writing one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_open_v2(
    path: *const c_char,
    config: *const rldb_config,
    out_db: *mut *mut rldb,
) -> c_int {
    flatten_code(api(|| {
        if path.is_null() || out_db.is_null() {
            return Err(RLDB_MISUSE);
        }
        let config = if config.is_null() {
            None
        } else {
            // SAFETY: `config` non-null (just checked); per redlinedb.h:86
            // caller owns rldb_config for this call; borrow consumed inside
            // open_handle which copies fields into owned DbOptions.
            Some(unsafe { &*config })
        };
        // SAFETY: `path` non-null (checked); per redlinedb.h:86 it is a
        // NUL-terminated C string; open_handle copies it into owned PathBuf.
        let handle = open_handle(unsafe { CStr::from_ptr(path) }, config, true, false)?;
        // SAFETY: `out_db` non-null (checked); per redlinedb.h:86 it is a
        // writable rldb**; open_handle returned a Box::into_raw pointer
        // whose ownership transfers to the C caller (paired with rldb_close).
        unsafe {
            *out_db = handle;
        }
        Ok(RLDB_OK)
    }))
}

/// # Safety
///
/// `db` must be NULL or a live database handle. After a successful close the
/// handle is dangling and must not be passed to any function again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_close(db: *mut rldb) -> c_int {
    flatten_code(api(|| {
        if db.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: `db` non-null (just checked); per redlinedb.h:87 from
        // rldb_open not yet closed; only reads active_statements count.
        let db_ref = unsafe { &*db };
        if db_ref.active_statements.load(Ordering::Relaxed) != 0 {
            return Err(RLDB_BUSY);
        }
        // SAFETY: matching constructor/destructor pair — `db` originates from
        // Box::into_raw(handle) at open_handle (crates/ffi/src/util.rs:133);
        // ownership invariant: only rldb_close / rldb_close_v2 consume it
        // (caller never frees directly per redlinedb.h:87); exclusive access
        // upheld by the active_statements==0 check above; double-close guarded
        // by the null check above (caller must NULL the handle after close);
        // ledgered at .jankurai/unsafe-ledger.toml (file=crates/ffi/src/lifecycle.rs,
        // line=94, detector=rust.unsafe.raw-parts); proof:
        // crates/ffi/tests/safety_invariants.rs::double_close_via_null_after_close_is_safe.
        unsafe {
            // SAFETY: reclaim and drop the leaked Box; matching destructor for
            // the Box::into_raw at open_handle (see invariant above).
            drop(reclaim_box(db));
        }
        Ok(RLDB_OK)
    }))
}

/// # Safety
///
/// `db` must be NULL or a live database handle. After a successful close the
/// handle is dangling and must not be passed to any function again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_close_v2(db: *mut rldb) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_close(db) }
}
