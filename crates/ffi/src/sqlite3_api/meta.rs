use std::ffi::{CStr, c_char};
use std::os::raw::{c_int, c_void};
use std::ptr;

use crate::types::{RLDB_MISUSE, RLDB_OK, sqlite3, sqlite3_stmt};
use crate::util::record_status;
use crate::{
    rldb_busy_timeout, rldb_changes, rldb_checkpoint, rldb_errcode, rldb_errmsg, rldb_free,
    rldb_interrupt, rldb_last_insert_rowid, rldb_stats_json, rldb_vacuum,
};

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_errcode(db: *mut sqlite3) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_errcode(db) }
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_errmsg(db) }
}

/// # Safety
///
/// `ptr` must be NULL or a pointer this library handed to the caller to free
/// (an `rldb_exec`/`sqlite3_exec` error message or `rldb_stats_json` output)
/// that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_free(ptr: *mut c_void) {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_free(ptr) }
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_interrupt(db: *mut sqlite3) {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_interrupt(db) }
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_is_interrupted(db: *mut sqlite3) -> c_int {
    use std::sync::atomic::Ordering;

    if db.is_null() {
        return 0;
    }
    // SAFETY: `db` non-null (checked); reads atomic interrupt flag only.
    unsafe { (*db).interrupted.load(Ordering::Relaxed) as c_int }
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_busy_timeout(db: *mut sqlite3, milliseconds: c_int) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_busy_timeout(db, milliseconds) };
    record_status(db, rc);
    rc
}

/// # Safety
///
/// The pointer arguments are not read; the function is `unsafe` only because
/// every pointer-taking export is (see the crate docs).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_extended_result_codes(_db: *mut sqlite3, _onoff: c_int) -> c_int {
    RLDB_OK
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_changes(db: *mut sqlite3) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_changes(db) }
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_changes64(db: *mut sqlite3) -> i64 {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_changes(db) as i64 }
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_total_changes(db: *mut sqlite3) -> c_int {
    use crate::util::with_db;
    with_db(db, |db| db.conn.total_changes() as c_int).unwrap_or(RLDB_MISUSE)
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_total_changes64(db: *mut sqlite3) -> i64 {
    use crate::util::with_db;
    with_db(db, |db| db.conn.total_changes() as i64).unwrap_or(-1)
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_get_autocommit(db: *mut sqlite3) -> c_int {
    use crate::util::with_db;
    with_db(db, |db| (!db.conn.in_transaction()) as c_int).unwrap_or(1)
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_last_insert_rowid(db: *mut sqlite3) -> i64 {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_last_insert_rowid(db) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_db_handle(stmt: *mut sqlite3_stmt) -> *mut sqlite3 {
    if stmt.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:189 from
    // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
    unsafe { (*stmt).db }
}

/// # Safety
///
/// - `db` must be NULL or a live database handle.
/// - `name` must be NULL or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_db_filename(
    db: *mut sqlite3,
    name: *const c_char,
) -> *const c_char {
    if db.is_null() || name.is_null() {
        return ptr::null();
    }
    // SAFETY: both `db` and `name` non-null (checked); per redlinedb.h:190
    // `db` from sqlite3_open not yet closed, `name` NUL-terminated C
    // string; returned pointer into db.path_text, valid for connection life.
    unsafe {
        if CStr::from_ptr(name).to_bytes() == b"main" {
            (*db).path_text.as_ptr()
        } else {
            ptr::null()
        }
    }
}

/// # Safety
///
/// The pointer arguments are not read; the function is `unsafe` only because
/// every pointer-taking export is (see the crate docs).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_db_readonly(_db: *mut sqlite3, _name: *const c_char) -> c_int {
    0
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_checkpoint(db: *mut sqlite3) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_checkpoint(db) };
    record_status(db, rc);
    rc
}

/// # Safety
///
/// `db` must be NULL or a live database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_vacuum(db: *mut sqlite3) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_vacuum(db) };
    record_status(db, rc);
    rc
}

/// # Safety
///
/// - `db` must be NULL or a live database handle.
/// - `out_json` must be NULL or valid for writing one pointer; the string
///   written there is owned by the caller and freed with `sqlite3_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_stats_json(db: *mut sqlite3, out_json: *mut *mut c_char) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_stats_json(db, out_json) };
    record_status(db, rc);
    rc
}
