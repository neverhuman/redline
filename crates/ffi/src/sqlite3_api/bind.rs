use std::os::raw::{c_char, c_int, c_void};

use redlinedb_sql::value::SqlValue;

use crate::types::{RLDB_MISUSE, RLDB_TOOBIG, sqlite3_stmt};
use crate::util::record_status;
use crate::{
    rldb_bind_blob, rldb_bind_double, rldb_bind_int64, rldb_bind_null, rldb_bind_parameter_index,
    rldb_bind_text, rldb_parameter_count,
};

use super::value::RldbValue;

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_null(stmt: *mut sqlite3_stmt, index: c_int) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_bind_null(stmt, index) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:157 from
        // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_int64(
    stmt: *mut sqlite3_stmt,
    index: c_int,
    value: i64,
) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_bind_int64(stmt, index, value) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:158 from
        // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_double(
    stmt: *mut sqlite3_stmt,
    index: c_int,
    value: f64,
) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_bind_double(stmt, index, value) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:159 from
        // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}

/// # Safety
///
/// - `stmt` must be NULL or a live statement.
/// - `value` must be NULL, or NUL-terminated when `nbytes` is negative, or
///   readable for `nbytes` bytes otherwise. The bytes are copied before the
///   call returns. `_destructor` is never called.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_text(
    stmt: *mut sqlite3_stmt,
    index: c_int,
    value: *const c_char,
    nbytes: c_int,
    _destructor: Option<unsafe extern "C" fn(*mut c_void)>,
) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_bind_text(stmt, index, value, nbytes) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:160 from
        // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}

/// # Safety
///
/// - `stmt` must be NULL or a live statement.
/// - `value` must be NULL or readable for `nbytes` bytes, and `nbytes` must not
///   be negative. The bytes are copied before the call returns. `_destructor`
///   is never called.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_blob(
    stmt: *mut sqlite3_stmt,
    index: c_int,
    value: *const c_void,
    nbytes: c_int,
    _destructor: Option<unsafe extern "C" fn(*mut c_void)>,
) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_bind_blob(stmt, index, value, nbytes) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:161 from
        // sqlite3_prepare_v2 not yet finalized; reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_zeroblob(
    stmt: *mut sqlite3_stmt,
    index: c_int,
    nbytes: c_int,
) -> c_int {
    if nbytes < 0 {
        return RLDB_MISUSE;
    }
    let bytes = vec![0; nbytes as usize];
    // SAFETY: `stmt` is covered by this function's contract; `bytes` is a live
    // buffer of exactly `nbytes` bytes.
    let rc = unsafe { rldb_bind_blob(stmt, index, bytes.as_ptr() as *const c_void, nbytes) };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_zeroblob64(
    stmt: *mut sqlite3_stmt,
    index: c_int,
    nbytes: u64,
) -> c_int {
    if nbytes > c_int::MAX as u64 {
        return RLDB_TOOBIG;
    }
    // SAFETY: `stmt` is covered by this function's contract; `nbytes` was
    // checked to fit in c_int.
    unsafe { sqlite3_bind_zeroblob(stmt, index, nbytes as c_int) }
}

/// # Safety
/// `value` must be NULL or a valid `*mut RldbValue` from this crate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_value(
    stmt: *mut sqlite3_stmt,
    index: c_int,
    value: *mut RldbValue,
) -> c_int {
    if value.is_null() {
        // SAFETY: `stmt` is covered by this function's contract.
        return unsafe { sqlite3_bind_null(stmt, index) };
    }
    // SAFETY: caller obligation; non-null checked above.
    let value = unsafe { &*value };
    let sql_value = value.to_sql();
    // SAFETY: `stmt` is covered by this function's contract; the text and
    // blob pointers borrow `sql_value`, which outlives each call, and their
    // lengths are the exact byte counts.
    let rc = unsafe {
        match sql_value {
            SqlValue::Null => rldb_bind_null(stmt, index),
            SqlValue::Integer(i) => rldb_bind_int64(stmt, index, i),
            SqlValue::Real(f) => rldb_bind_double(stmt, index, f),
            SqlValue::Text(text) => {
                let bytes = text.as_bytes();
                rldb_bind_text(
                    stmt,
                    index,
                    bytes.as_ptr() as *const c_char,
                    bytes.len() as c_int,
                )
            }
            SqlValue::Blob(blob) => rldb_bind_blob(
                stmt,
                index,
                blob.as_ptr() as *const c_void,
                blob.len() as c_int,
            ),
        }
    };
    if !stmt.is_null() {
        // SAFETY: `stmt` non-null (checked); reads Copy db field only.
        let db = unsafe { (*stmt).db };
        record_status(db, rc);
    }
    rc
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_parameter_count(stmt: *mut sqlite3_stmt) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_parameter_count(stmt) }
}

/// # Safety
///
/// - `stmt` must be NULL or a live statement.
/// - `name` must be NULL or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_bind_parameter_index(
    stmt: *mut sqlite3_stmt,
    name: *const c_char,
) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_bind_parameter_index(stmt, name) }
}
