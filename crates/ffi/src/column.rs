//! Column accessor FFI surface.

use std::os::raw::{c_char, c_int, c_uchar, c_void};
use std::ptr;

use redlinedb_sql::value::SqlValue;

use crate::types::*;
use crate::util::{api, flatten_code, record_status_with_message};

#[unsafe(no_mangle)]
pub extern "C" fn rldb_column_count(stmt: *mut rldb_stmt) -> c_int {
    if stmt.is_null() {
        return RLDB_MISUSE;
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:114 from
    // rldb_prepare_v2 not yet finalized; reads Copy integer only.
    unsafe { (*stmt).stmt.column_count() as c_int }
}

#[unsafe(no_mangle)]
pub extern "C" fn rldb_column_name(stmt: *mut rldb_stmt, index: c_int) -> *const c_char {
    if stmt.is_null() {
        return ptr::null();
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:115 from
    // rldb_prepare_v2; returned pointer is into rldb_stmt.column_names,
    // valid until rldb_step/reset/finalize.
    unsafe {
        let stmt = &*stmt;
        stmt.column_names
            .get(index as usize)
            .map(|value| value.as_ptr())
            .unwrap_or(ptr::null())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rldb_column_type(stmt: *mut rldb_stmt, index: c_int) -> c_int {
    flatten_code(api(|| {
        if stmt.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:116 from
        // rldb_prepare_v2 not yet finalized; single-thread ownership.
        let stmt = unsafe { &*stmt };
        // The class of the stored value, never of a conversion: a REAL is
        // REAL even though column_i64 would accept it.
        Ok(match current_value(stmt, index) {
            None | Some(SqlValue::Null) => RLDB_NULL,
            Some(SqlValue::Integer(_)) => RLDB_INTEGER,
            Some(SqlValue::Real(_)) => RLDB_REAL,
            Some(SqlValue::Text(_)) => RLDB_TEXT,
            Some(SqlValue::Blob(_)) => RLDB_BLOB,
        })
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn rldb_column_int64(stmt: *mut rldb_stmt, index: c_int) -> i64 {
    if stmt.is_null() {
        return 0;
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:117 from
    // rldb_prepare_v2; reads Copy integer return value only.
    unsafe { (*stmt).stmt.column_i64(index as usize).unwrap_or(0) }
}

#[unsafe(no_mangle)]
pub extern "C" fn rldb_column_double(stmt: *mut rldb_stmt, index: c_int) -> f64 {
    if stmt.is_null() {
        return 0.0;
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:118 from
    // rldb_prepare_v2; reads Copy f64 return value only.
    unsafe { (*stmt).stmt.column_f64(index as usize).unwrap_or(0.0) }
}

#[unsafe(no_mangle)]
pub extern "C" fn rldb_column_text(stmt: *mut rldb_stmt, index: c_int) -> *const c_uchar {
    if stmt.is_null() {
        return ptr::null();
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h from
    // rldb_prepare_v2 not yet finalized; single-thread ownership. The
    // returned pointer is into rldb_stmt.text_cache, valid until
    // rldb_step/reset/finalize.
    let stmt = unsafe { &mut *stmt };
    cached_text(stmt, index)
        .map(|bytes| bytes.as_ptr() as *const c_uchar)
        .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn rldb_column_blob(stmt: *mut rldb_stmt, index: c_int) -> *const c_void {
    if stmt.is_null() {
        return ptr::null();
    }
    // SAFETY: `stmt` non-null (checked); per redlinedb.h:120 from
    // rldb_prepare_v2; blob pointer owned by column slot, valid until
    // next rldb_step/reset/finalize per C ABI contract.
    unsafe {
        match (*stmt).stmt.column_blob(index as usize) {
            Ok(blob) => blob.as_ptr() as *const c_void,
            Err(_) => ptr::null(),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rldb_column_bytes(stmt: *mut rldb_stmt, index: c_int) -> c_int {
    flatten_code(api(|| {
        if stmt.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:121 from
        // rldb_prepare_v2 not yet finalized; single-thread ownership.
        let stmt = unsafe { &mut *stmt };
        let len = match current_value(stmt, index) {
            None | Some(SqlValue::Null) => 0,
            Some(SqlValue::Text(text)) => text.len(),
            Some(SqlValue::Blob(blob)) => blob.len(),
            // Numbers: the length of the text column_text returns.
            Some(SqlValue::Integer(_) | SqlValue::Real(_)) => {
                cached_text(stmt, index).map_or(0, |bytes| bytes.len() - 1)
            }
        };
        Ok(c_int::try_from(len).unwrap_or(c_int::MAX))
    }))
}

/// The current row's value in column `index`. An index outside the result
/// columns records SQLITE_RANGE on the connection, as upstream does; with no
/// current row (before the first step or after DONE) there is no value.
fn current_value(stmt: &rldb_stmt, index: c_int) -> Option<&SqlValue> {
    let column = usize::try_from(index)
        .ok()
        .filter(|&column| column < stmt.stmt.column_count());
    let Some(column) = column else {
        record_status_with_message(stmt.db, RLDB_RANGE, "column index out of range");
        return None;
    };
    stmt.stmt.column_value(column).ok()
}

/// The `sqlite3_column_text` form of a value, with a trailing NUL. INTEGER
/// and REAL use the text `CAST(x AS TEXT)` produces; TEXT and BLOB are their
/// bytes unchanged, interior NULs included; NULL has no text.
fn text_form(value: &SqlValue) -> Option<Box<[u8]>> {
    let mut bytes = match value {
        SqlValue::Null => return None,
        SqlValue::Integer(v) => v.to_string().into_bytes(),
        SqlValue::Real(v) => redlinedb_sql::format_real_sqlite(*v).into_bytes(),
        SqlValue::Text(text) => text.as_bytes().to_vec(),
        SqlValue::Blob(blob) => blob.to_vec(),
    };
    bytes.push(0);
    Some(bytes.into_boxed_slice())
}

/// The column's text form (bytes plus NUL), converted on first use and kept
/// until the next step or reset so repeated calls return the same pointer.
fn cached_text(stmt: &mut rldb_stmt, index: c_int) -> Option<&[u8]> {
    let slot = usize::try_from(index).ok();
    let cached = slot
        .and_then(|slot| stmt.text_cache.get(slot))
        .is_some_and(Option::is_some);
    if !cached {
        let form = text_form(current_value(stmt, index)?)?;
        let slot = index as usize;
        if stmt.text_cache.len() <= slot {
            stmt.text_cache.resize_with(slot + 1, || None);
        }
        stmt.text_cache[slot] = Some(form);
    }
    stmt.text_cache[index as usize].as_deref()
}
