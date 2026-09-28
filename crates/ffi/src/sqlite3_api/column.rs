use std::os::raw::{c_char, c_int, c_uchar, c_void};
use std::ptr;

use crate::types::{sqlite_tag, sqlite3_stmt};
use crate::{
    rldb_column_blob, rldb_column_bytes, rldb_column_count, rldb_column_double, rldb_column_int64,
    rldb_column_name, rldb_column_text, rldb_column_type,
};

use super::value::RldbValue;

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_count(stmt: *mut sqlite3_stmt) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_column_count(stmt) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_name(
    stmt: *mut sqlite3_stmt,
    index: c_int,
) -> *const c_char {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_column_name(stmt, index) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_type(stmt: *mut sqlite3_stmt, index: c_int) -> c_int {
    // Upstream storage classes: NULL is SQLITE_NULL (5), not RLDB_NULL (0).
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    sqlite_tag(unsafe { rldb_column_type(stmt, index) })
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_int64(stmt: *mut sqlite3_stmt, index: c_int) -> i64 {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_column_int64(stmt, index) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_double(stmt: *mut sqlite3_stmt, index: c_int) -> f64 {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_column_double(stmt, index) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_text(
    stmt: *mut sqlite3_stmt,
    index: c_int,
) -> *const c_uchar {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_column_text(stmt, index) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_blob(
    stmt: *mut sqlite3_stmt,
    index: c_int,
) -> *const c_void {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_column_blob(stmt, index) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_bytes(stmt: *mut sqlite3_stmt, index: c_int) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    unsafe { rldb_column_bytes(stmt, index) }
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_column_value(
    stmt: *mut sqlite3_stmt,
    index: c_int,
) -> *mut RldbValue {
    if stmt.is_null() || index < 0 {
        return ptr::null_mut();
    }
    // SAFETY: `stmt` non-null (checked); per sqlite3_prepare_v2, the
    // statement is valid for exclusive statement-owned cache mutation for
    // this call.
    let stmt = unsafe { &mut *stmt };
    let index = index as usize;
    if index >= stmt.stmt.column_count() {
        return ptr::null_mut();
    }
    if stmt.value_cache.len() < stmt.stmt.column_count() {
        stmt.value_cache
            .resize_with(stmt.stmt.column_count(), || None);
    }
    if stmt.value_cache[index].is_none() {
        let Ok(value) = stmt.stmt.column_value(index) else {
            return ptr::null_mut();
        };
        stmt.value_cache[index] = Some(Box::new(RldbValue::from_sql(value)));
    }
    stmt.value_cache[index]
        .as_mut()
        .map(|value| value.as_mut() as *mut RldbValue)
        .unwrap_or(ptr::null_mut())
}
