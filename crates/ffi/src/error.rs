//! Error reporting and ancillary FFI surface (`errcode`, `errmsg`,
//! `interrupt`, `free`).

use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::Ordering;

use crate::types::*;
use crate::util::with_db;

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_errcode(db: *mut rldb) -> c_int {
    with_db(db, |db| db.last_code.load(Ordering::Relaxed)).unwrap_or(RLDB_MISUSE)
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_errmsg(db: *mut rldb) -> *const c_char {
    with_db(db, |db| {
        db.last_message
            .lock()
            .map(|value| value.as_ptr())
            .unwrap_or(ptr::null())
    })
    .unwrap_or(ptr::null())
}

/// # Safety
///
/// `ptr` must be NULL or a pointer this library handed to the caller to free
/// (an `rldb_exec`/`sqlite3_exec` error message or `rldb_stats_json` output)
/// that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_free(ptr: *mut c_void) {
    if !ptr.is_null() {
        // SAFETY: matching constructor/destructor pair — every pointer handed to
        // rldb_free originates from CString::new(...).into_raw() in
        // errmsg_to_c_string (crates/ffi/src/util.rs:379), routed through
        // set_errmsg (crates/ffi/src/util.rs:397); ownership invariant: this
        // library has exclusive access as the sole producer of such pointers per
        // redlinedb.h:128; double-free guarded by the caller's obligation to
        // NULL the handle after rldb_free (redlinedb.h:128); ledgered at
        // .jankurai/unsafe-ledger.toml (file=crates/ffi/src/error.rs, line=52,
        // detector=rust.unsafe.raw-parts); proof:
        // crates/ffi/tests/safety_invariants.rs::rldb_free_null_is_noop and
        // ::exec_callback_failure_round_trips_errmsg_ownership.
        unsafe {
            // SAFETY: reclaim and free the leaked CString; matching destructor
            // for the documented CString::into_raw above.
            crate::util::reclaim_cstring(ptr as *mut c_char);
        }
    }
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_interrupt(db: *mut rldb) {
    let _ = with_db(db, |db| db.interrupted.store(true, Ordering::Relaxed));
}
