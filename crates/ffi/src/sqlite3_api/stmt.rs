use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::atomic::Ordering;

use crate::types::{RLDB_DONE, RLDB_OK, RLDB_ROW, sqlite3, sqlite3_stmt};
use crate::util::record_status;
use crate::{rldb_clear_bindings, rldb_finalize, rldb_prepare_v2, rldb_reset, rldb_step};

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_stmt_readonly(stmt: *mut sqlite3_stmt) -> c_int {
    if stmt.is_null() {
        return 0;
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:101 from
    // sqlite3_prepare_v2 not yet finalized; reads Copy bool only.
    unsafe { (*stmt).stmt.is_readonly() as c_int }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_stmt_busy(stmt: *mut sqlite3_stmt) -> c_int {
    if stmt.is_null() {
        return 0;
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:102 from
    // sqlite3_prepare_v2 not yet finalized; reads Copy bool only.
    unsafe { (*stmt).stmt.is_busy() as c_int }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_sql(stmt: *mut sqlite3_stmt) -> *const c_char {
    if stmt.is_null() {
        return ptr::null();
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:103 from
    // sqlite3_prepare_v2; returned pointer is into rldb_stmt.sql_text,
    // valid until sqlite3_finalize.
    unsafe { (*stmt).sql_text.as_ptr() }
}

/// # Safety
///
/// - `db` must be NULL or a live database handle.
/// - `sql` must be NULL or readable up to its first NUL byte when `nbytes` is
///   negative; when `nbytes >= 0` only the bytes before the first NUL or the
///   bound, whichever comes first, need to be readable (`nbytes == 0` reads
///   nothing).
/// - `out_stmt` must be NULL or valid for writing one pointer.
/// - `tail` must be NULL or valid for writing one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_prepare_v2(
    db: *mut sqlite3,
    sql: *const c_char,
    nbytes: c_int,
    out_stmt: *mut *mut sqlite3_stmt,
    tail: *mut *const c_char,
) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_prepare_v2(db, sql, nbytes, out_stmt, tail) };
    // Only overwrite last_message on success — on failure, rldb_prepare_v2
    // has already stashed the enriched parser/binder error string, so
    // replacing it with the generic "ok"/"error" status text would drop
    // the actionable detail (`sqlite3_errmsg` consumers rely on it).
    if rc == RLDB_OK {
        record_status(db, rc);
    } else if !db.is_null() {
        // SAFETY: `db` non-null (checked); per redlinedb.h:151 from
        // sqlite3_open not yet closed; touches atomic last_code only.
        unsafe {
            (*db).last_code.store(rc, Ordering::Relaxed);
        }
    }
    rc
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_step(stmt) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:152 from
        // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
        let db = unsafe { (*stmt).db };
        // rldb_step already recorded an enriched error message on failure;
        // only update the generic "ok" message on success/row/done.
        if rc == RLDB_OK || rc == RLDB_ROW || rc == RLDB_DONE {
            record_status(db, rc);
        } else if !db.is_null() {
            // SAFETY: `db` non-null (checked); recorded at prepare time and
            // lives at least as long as the statement (active_statements
            // gates close); touches atomic last_code only.
            unsafe {
                (*db).last_code.store(rc, Ordering::Relaxed);
            }
        }
    }
    rc
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_reset(stmt: *mut sqlite3_stmt) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_reset(stmt) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:153 from
        // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}

/// # Safety
///
/// `stmt` must be NULL or a live statement. After a successful finalize it is
/// dangling and must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_finalize(stmt) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_clear_bindings(stmt: *mut sqlite3_stmt) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_clear_bindings(stmt) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:155 from
        // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}
