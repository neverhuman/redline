use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_uint};
use std::ptr;

use crate::types::{
    RLDB_CANTOPEN, RLDB_ERROR, RLDB_MISUSE, RLDB_OK, RLDB_READONLY, SQLITE_OPEN_CREATE,
    SQLITE_OPEN_MEMORY, SQLITE_OPEN_READONLY, SQLITE_OPEN_READWRITE, SQLITE_OPEN_URI,
    SQLITE_PREPARE_SUPPORTED, sqlite3, sqlite3_stmt,
};
use crate::util::{
    api, flatten_code, open_handle, record_status, record_status_with_message, sqlite_errstr,
    sqlite_sourceid_cstr, sqlite_version_cstr,
};
use crate::{rldb_close, rldb_close_v2, rldb_open};

use super::stmt::sqlite3_prepare_v2;

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_open(path: *const c_char, out_db: *mut *mut sqlite3) -> c_int {
    rldb_open(path, out_db)
}

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_libversion() -> *const c_char {
    sqlite_version_cstr().as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_libversion_number() -> c_int {
    let version = env!("CARGO_PKG_VERSION");
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|part| part.parse::<i32>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|part| part.parse::<i32>().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|part| part.parse::<i32>().ok())
        .unwrap_or(0);
    major * 1_000_000 + minor * 1_000 + patch
}

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_sourceid() -> *const c_char {
    sqlite_sourceid_cstr().as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_threadsafe() -> c_int {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_errstr(code: c_int) -> *const c_char {
    sqlite_errstr(code).as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_open_v2(
    path: *const c_char,
    out_db: *mut *mut sqlite3,
    flags: c_int,
    _vfs: *const c_char,
) -> c_int {
    flatten_code(api(|| {
        if path.is_null() || out_db.is_null() {
            return Err(RLDB_MISUSE);
        }
        if flags & SQLITE_OPEN_READONLY != 0 {
            return Err(RLDB_READONLY);
        }
        if flags & SQLITE_OPEN_READWRITE == 0 {
            return Err(RLDB_MISUSE);
        }
        let create_if_missing = flags & SQLITE_OPEN_CREATE != 0;
        // SAFETY: `path` non-null (checked); per redlinedb.h:147 it is a
        // NUL-terminated C string; open_handle copies it into owned PathBuf.
        let path = unsafe { CStr::from_ptr(path) };
        // URI filenames are not interpreted. Refuse a "file:" name the
        // caller asked to be read as a URI instead of creating a database
        // at that literal path; without SQLITE_OPEN_URI the name is a plain
        // path, as in upstream builds with URI handling off by default.
        if flags & SQLITE_OPEN_URI != 0 && path.to_bytes().starts_with(b"file:") {
            return Err(RLDB_CANTOPEN);
        }
        let in_memory = flags & SQLITE_OPEN_MEMORY != 0;
        let handle = open_handle(path, None, create_if_missing, in_memory)?;
        // SAFETY: `out_db` non-null (checked); per redlinedb.h:147 it is a
        // writable sqlite3**; open_handle returned a Box::into_raw pointer
        // whose ownership transfers to the C caller (paired with sqlite3_close).
        unsafe {
            *out_db = handle;
        }
        record_status(handle, RLDB_OK);
        Ok(RLDB_OK)
    }))
}

/// Upstream argument order: `(db, sql, nbytes, prepFlags, ppStmt, pzTail)`.
/// PERSISTENT and NORMALIZE are accepted (an allocation hint and a no-op
/// upstream). Any other flag asks for behaviour RedlineDB does not provide,
/// so it fails with SQLITE_ERROR and a NULL statement.
#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_prepare_v3(
    db: *mut sqlite3,
    sql: *const c_char,
    nbytes: c_int,
    flags: c_uint,
    out_stmt: *mut *mut sqlite3_stmt,
    tail: *mut *const c_char,
) -> c_int {
    if !out_stmt.is_null() {
        // SAFETY: `out_stmt` non-null (checked); per the prepare contract it
        // is a writable sqlite3_stmt** owned by the caller for this call.
        unsafe { *out_stmt = ptr::null_mut() };
    }
    if flags & !SQLITE_PREPARE_SUPPORTED != 0 {
        if db.is_null() {
            return RLDB_MISUSE;
        }
        record_status_with_message(db, RLDB_ERROR, "unsupported sqlite3_prepare_v3 flags");
        return RLDB_ERROR;
    }
    sqlite3_prepare_v2(db, sql, nbytes, out_stmt, tail)
}

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_close(db: *mut sqlite3) -> c_int {
    rldb_close(db)
}

#[unsafe(no_mangle)]
pub extern "C" fn sqlite3_close_v2(db: *mut sqlite3) -> c_int {
    rldb_close_v2(db)
}
