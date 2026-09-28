//! C-visible types shared across the FFI surface.
//!
//! These types form part of the cdylib's ABI: their layout, names, and
//! exported visibility must remain stable across refactors. Helper Rust
//! types (handles like `rldb`, `rldb_stmt`) are owned through raw
//! pointers by C callers, so their fields are intentionally `pub(crate)`
//! — only the opaque pointer crosses the boundary.

use std::ffi::CString;
use std::os::raw::c_int;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize};
use std::sync::{Arc, Mutex};

use crate::sqlite3_api::hooks::HookSlots;

// ---- Result codes (C-visible) -----------------------------------------------

pub(crate) const RLDB_OK: c_int = 0;
pub(crate) const RLDB_ERROR: c_int = 1;
pub(crate) const RLDB_INTERNAL: c_int = 2;
pub(crate) const RLDB_BUSY: c_int = 5;
pub(crate) const RLDB_LOCKED: c_int = 6;
pub(crate) const RLDB_NOMEM: c_int = 7;
pub(crate) const RLDB_INTERRUPT: c_int = 9;
pub(crate) const RLDB_IOERR: c_int = 10;
pub(crate) const RLDB_READONLY: c_int = 8;
pub(crate) const RLDB_CANTOPEN: c_int = 14;
pub(crate) const RLDB_SCHEMA: c_int = 17;
pub(crate) const RLDB_TOOBIG: c_int = 18;
pub(crate) const RLDB_CONSTRAINT: c_int = 19;
pub(crate) const RLDB_MISMATCH: c_int = 20;
pub(crate) const RLDB_MISUSE: c_int = 21;
pub(crate) const RLDB_AUTH: c_int = 23;
pub(crate) const RLDB_RANGE: c_int = 25;
pub(crate) const RLDB_NOTADB: c_int = 26;
pub(crate) const RLDB_ROW: c_int = 100;
pub(crate) const RLDB_DONE: c_int = 101;

// Native storage classes (`rldb_column_type`). RLDB_NULL stays 0.
pub(crate) const RLDB_NULL: c_int = 0;
pub(crate) const RLDB_INTEGER: c_int = 1;
pub(crate) const RLDB_REAL: c_int = 2;
pub(crate) const RLDB_TEXT: c_int = 3;
pub(crate) const RLDB_BLOB: c_int = 4;

/// SQLite's NULL storage class. INTEGER..BLOB share the native values.
pub(crate) const SQLITE_NULL: c_int = 5;

/// Map a native storage class to the SQLite one. Anything that is not a
/// non-NULL class (RLDB_NULL, or an error code from a native accessor)
/// reports SQLITE_NULL, as upstream does for a missing row or column.
pub(crate) fn sqlite_tag(native: c_int) -> c_int {
    match native {
        RLDB_INTEGER | RLDB_REAL | RLDB_TEXT | RLDB_BLOB => native,
        _ => SQLITE_NULL,
    }
}

pub(crate) const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
pub(crate) const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
pub(crate) const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
pub(crate) const SQLITE_OPEN_URI: c_int = 0x0000_0040;
pub(crate) const SQLITE_OPEN_MEMORY: c_int = 0x0000_0080;

/// `sqlite3_prepare_v3` flags RedlineDB accepts: PERSISTENT is an allocation
/// hint and NORMALIZE is a no-op upstream. Any other flag asks for behaviour
/// this implementation does not provide and is refused.
pub(crate) const SQLITE_PREPARE_SUPPORTED: std::os::raw::c_uint = 0x01 | 0x02;

// ---- C-visible structs ------------------------------------------------------

#[repr(C)]
pub struct rldb_config {
    pub struct_size: u32,
    pub flags: u32,
    pub durability: u32,
    pub cache_bytes: u64,
    pub work_mem_bytes: u64,
    pub max_spill_bytes: u64,
    pub statement_cache_capacity: u32,
    pub busy_timeout_ms: u32,
}

#[allow(non_camel_case_types)]
pub struct rldb {
    pub(crate) db: Arc<redlinedb_sql::Database>,
    pub(crate) conn: Arc<redlinedb_sql::Connection>,
    pub(crate) path: PathBuf,
    pub(crate) path_text: CString,
    pub(crate) last_code: AtomicI32,
    pub(crate) last_message: Mutex<CString>,
    pub(crate) interrupted: AtomicBool,
    pub(crate) active_statements: AtomicUsize,
    pub(crate) hooks: HookSlots,
}

#[allow(non_camel_case_types)]
pub struct rldb_stmt {
    pub(crate) db: *mut rldb,
    pub(crate) stmt: redlinedb_sql::Statement,
    pub(crate) sql_text: CString,
    pub(crate) column_names: Vec<CString>,
    /// Lazily filled `column_text` form per column: the bytes plus a
    /// trailing NUL. Cleared on every step and reset.
    pub(crate) text_cache: Vec<Option<Box<[u8]>>>,
    pub(crate) value_cache: Vec<Option<Box<crate::sqlite3_api::value::RldbValue>>>,
}

#[allow(non_camel_case_types)]
pub struct rldb_backup {
    pub(crate) src_path: PathBuf,
    pub(crate) dst_path: PathBuf,
    pub(crate) done: bool,
    pub(crate) remaining: i64,
    pub(crate) pagecount: i64,
}

// ---- SQLite-compatible type aliases ----------------------------------------

#[allow(non_camel_case_types)]
pub type sqlite3 = rldb;

#[allow(non_camel_case_types)]
pub type sqlite3_stmt = rldb_stmt;

#[allow(non_camel_case_types)]
pub type sqlite3_backup = rldb_backup;
