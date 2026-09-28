//! S8-01/S9-01: exported `sqlite3_*` symbols called through declarations
//! written from upstream SQLite 3.53.1 `sqlite3.h`, not from RedlineDB's
//! own header or Rust signatures. A caller compiled against upstream must
//! get upstream behaviour, so these declarations bind by symbol name only.

use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int, c_uint};
use std::ptr;

// Force the rlib (and its #[no_mangle] exports) into the test binary.
use redlinedb as _;

const SQLITE_OK: c_int = 0;
const SQLITE_ERROR: c_int = 1;
const SQLITE_ROW: c_int = 100;
const SQLITE_PREPARE_PERSISTENT: c_uint = 0x01;
const SQLITE_PREPARE_NORMALIZE: c_uint = 0x02;
const SQLITE_PREPARE_NO_VTAB: c_uint = 0x04;
const SQLITE_PREPARE_DONT_LOG: c_uint = 0x10;
const SQLITE_PREPARE_FROM_DDL: c_uint = 0x20;

type Db = c_void;
type Stmt = c_void;

unsafe extern "C" {
    #[link_name = "sqlite3_open"]
    fn up_open(filename: *const c_char, db: *mut *mut Db) -> c_int;
    #[link_name = "sqlite3_close"]
    fn up_close(db: *mut Db) -> c_int;
    #[link_name = "sqlite3_errmsg"]
    fn up_errmsg(db: *mut Db) -> *const c_char;
    /// Upstream: `int sqlite3_prepare_v3(sqlite3*, const char*, int,
    /// unsigned int prepFlags, sqlite3_stmt**, const char**)`.
    #[link_name = "sqlite3_prepare_v3"]
    fn up_prepare_v3(
        db: *mut Db,
        sql: *const c_char,
        nbytes: c_int,
        prep_flags: c_uint,
        stmt: *mut *mut Stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    #[link_name = "sqlite3_step"]
    fn up_step(stmt: *mut Stmt) -> c_int;
    #[link_name = "sqlite3_column_int64"]
    fn up_column_int64(stmt: *mut Stmt, index: c_int) -> i64;
    #[link_name = "sqlite3_finalize"]
    fn up_finalize(stmt: *mut Stmt) -> c_int;
}

fn open() -> (tempfile::TempDir, *mut Db) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = std::ffi::CString::new(dir.path().join("abi.redline").to_str().expect("utf8"))
        .expect("cstring");
    let mut db: *mut Db = ptr::null_mut();
    // SAFETY: valid NUL-terminated path and writable out pointer.
    assert_eq!(unsafe { up_open(path.as_ptr(), &mut db) }, SQLITE_OK);
    assert!(!db.is_null());
    (dir, db)
}

fn sentinel() -> *mut Stmt {
    ptr::dangling_mut::<u8>().cast()
}

#[test]
fn prepare_v3_upstream_order_flags_0_1_2() {
    let (_dir, db) = open();
    let sql = c"SELECT 7; SELECT 9";
    for flags in [
        0,
        SQLITE_PREPARE_PERSISTENT,
        SQLITE_PREPARE_NORMALIZE,
        SQLITE_PREPARE_PERSISTENT | SQLITE_PREPARE_NORMALIZE,
    ] {
        let mut stmt = sentinel();
        let mut tail: *const c_char = ptr::null();
        // SAFETY: upstream argument order; every pointer is valid.
        let rc = unsafe { up_prepare_v3(db, sql.as_ptr(), -1, flags, &mut stmt, &mut tail) };
        assert_eq!(rc, SQLITE_OK, "flags {flags:#x}");
        assert!(!stmt.is_null() && stmt != sentinel(), "flags {flags:#x}");
        assert_eq!(tail, sql.as_ptr().wrapping_add(9), "flags {flags:#x}");
        // SAFETY: stmt was just prepared and is finalized below.
        unsafe {
            assert_eq!(up_step(stmt), SQLITE_ROW);
            assert_eq!(up_column_int64(stmt, 0), 7);
            assert_eq!(up_finalize(stmt), SQLITE_OK);
        }
    }
    // SAFETY: no statements remain open.
    assert_eq!(unsafe { up_close(db) }, SQLITE_OK);
}

#[test]
fn prepare_v3_unsupported_flag_nulls_stmt() {
    let (_dir, db) = open();
    let sql = c"SELECT 7";
    for flags in [
        SQLITE_PREPARE_NO_VTAB,
        SQLITE_PREPARE_DONT_LOG,
        SQLITE_PREPARE_FROM_DDL,
        0x80,
        0x8000_0000,
        SQLITE_PREPARE_PERSISTENT | SQLITE_PREPARE_NO_VTAB,
    ] {
        let mut stmt = sentinel();
        let mut tail: *const c_char = ptr::null();
        // SAFETY: upstream argument order; every pointer is valid.
        let rc = unsafe { up_prepare_v3(db, sql.as_ptr(), -1, flags, &mut stmt, &mut tail) };
        assert_eq!(rc, SQLITE_ERROR, "flags {flags:#x}");
        assert!(stmt.is_null(), "flags {flags:#x} must leave *ppStmt NULL");
        // SAFETY: errmsg returns a NUL-terminated string owned by db.
        let message = unsafe { CStr::from_ptr(up_errmsg(db)) };
        assert_eq!(
            message.to_str().expect("utf8"),
            "unsupported sqlite3_prepare_v3 flags"
        );
    }
    // SAFETY: no statements remain open.
    assert_eq!(unsafe { up_close(db) }, SQLITE_OK);
}
