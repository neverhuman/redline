//! S8-03/S9-02: `rldb_prepare_v2` must honour the caller's byte bound.
//!
//! SQLite reads at most `nbytes` bytes when `nbytes >= 0` and stops at the
//! first NUL, so a caller may pass SQL that ends exactly at the end of a
//! mapping with no terminator. The input here is placed against a
//! `PROT_NONE` guard page: an implementation that runs `strlen` first, or
//! forms a slice over the whole bound, faults instead of failing an assert.
//! Every failure path must also leave `*out_stmt` NULL.

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::ptr;

use redlinedb::{
    rldb, rldb_close, rldb_column_int64, rldb_finalize, rldb_open, rldb_prepare_v2, rldb_step,
    rldb_stmt,
};

const RLDB_OK: c_int = 0;
const RLDB_ERROR: c_int = 1;
const RLDB_MISMATCH: c_int = 20;
const RLDB_MISUSE: c_int = 21;
const RLDB_ROW: c_int = 100;

/// Two anonymous pages; the second is `PROT_NONE`. Unmapped on drop.
struct GuardPage {
    base: *mut u8,
    page: usize,
}

impl GuardPage {
    fn new() -> Self {
        // SAFETY: sysconf has no memory preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        assert!(page > 0, "page size");
        let page = page as usize;
        // SAFETY: anonymous private mapping with no address hint; the result
        // is checked against MAP_FAILED before use.
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_ne!(base, libc::MAP_FAILED, "mmap");
        // SAFETY: `base + page` is the second page of the mapping just made.
        let rc =
            unsafe { libc::mprotect((base as *mut u8).add(page).cast(), page, libc::PROT_NONE) };
        assert_eq!(rc, 0, "mprotect");
        Self {
            base: base.cast(),
            page,
        }
    }

    /// First byte of the inaccessible page.
    fn edge(&self) -> *mut u8 {
        // SAFETY: stays within (one past the readable part of) the mapping.
        unsafe { self.base.add(self.page) }
    }

    /// Copy `bytes` so they end exactly at the guard page; return their start.
    fn place_at_edge(&self, bytes: &[u8]) -> *const c_char {
        assert!(bytes.len() <= self.page);
        // SAFETY: the destination lies in the readable first page.
        unsafe {
            let start = self.edge().sub(bytes.len());
            ptr::copy_nonoverlapping(bytes.as_ptr(), start, bytes.len());
            start as *const c_char
        }
    }
}

impl Drop for GuardPage {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly the mapping created in `new`.
        unsafe { libc::munmap(self.base.cast(), self.page * 2) };
    }
}

fn open_db() -> (tempfile::TempDir, *mut rldb) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path =
        CString::new(dir.path().join("bounded.redline").to_str().expect("utf8")).expect("cstring");
    let mut db: *mut rldb = ptr::null_mut();
    assert_eq!(unsafe { rldb_open(path.as_ptr(), &mut db) }, RLDB_OK);
    (dir, db)
}

fn sentinel() -> *mut rldb_stmt {
    ptr::dangling_mut::<rldb_stmt>()
}

#[test]
fn bounded_sql_ending_at_guard_page_prepares() {
    let (_dir, db) = open_db();
    let guard = GuardPage::new();
    let sql = guard.place_at_edge(b"SELECT 1");
    let mut stmt = sentinel();
    let mut tail: *const c_char = ptr::null();
    let rc = unsafe { rldb_prepare_v2(db, sql, 8, &mut stmt, &mut tail) };
    assert_eq!(rc, RLDB_OK);
    assert!(!stmt.is_null() && stmt != sentinel());
    assert_eq!(
        tail as *const u8,
        guard.edge() as *const u8,
        "tail ends at the bound"
    );
    assert_eq!(unsafe { rldb_step(stmt) }, RLDB_ROW);
    assert_eq!(unsafe { rldb_column_int64(stmt, 0) }, 1);
    assert_eq!(unsafe { rldb_finalize(stmt) }, RLDB_OK);
    assert_eq!(unsafe { rldb_close(db) }, RLDB_OK);
}

#[test]
fn zero_length_at_guard_page_reads_nothing() {
    let (_dir, db) = open_db();
    let guard = GuardPage::new();
    let sql = guard.edge() as *const c_char;
    let mut stmt = sentinel();
    let mut tail: *const c_char = ptr::null();
    let rc = unsafe { rldb_prepare_v2(db, sql, 0, &mut stmt, &mut tail) };
    assert_eq!(rc, RLDB_OK);
    assert!(stmt.is_null(), "empty input prepares no statement");
    assert_eq!(tail, sql);
    assert_eq!(unsafe { rldb_close(db) }, RLDB_OK);
}

#[test]
fn embedded_nul_stops_scan() {
    let (_dir, db) = open_db();
    let input: &[u8; 16] = b"SELECT 1\0garbage";
    let sql = input.as_ptr() as *const c_char;
    let mut stmt = sentinel();
    let mut tail: *const c_char = ptr::null();
    let rc = unsafe { rldb_prepare_v2(db, sql, input.len() as c_int, &mut stmt, &mut tail) };
    assert_eq!(rc, RLDB_OK);
    assert!(!stmt.is_null() && stmt != sentinel());
    assert_eq!(tail, sql.wrapping_add(8), "tail stops at the NUL");
    assert_eq!(unsafe { rldb_step(stmt) }, RLDB_ROW);
    assert_eq!(unsafe { rldb_column_int64(stmt, 0) }, 1);
    assert_eq!(unsafe { rldb_finalize(stmt) }, RLDB_OK);
    assert_eq!(unsafe { rldb_close(db) }, RLDB_OK);
}

#[test]
fn failed_prepare_nulls_out_stmt() {
    let (_dir, db) = open_db();
    let bad = CString::new("SELEC").expect("cstring");
    let mut stmt = sentinel();
    let rc = unsafe { rldb_prepare_v2(db, bad.as_ptr(), -1, &mut stmt, ptr::null_mut()) };
    assert_eq!(rc, RLDB_ERROR);
    assert!(stmt.is_null(), "parse failure leaves *out_stmt NULL");

    let good = CString::new("SELECT 1").expect("cstring");
    let mut stmt = sentinel();
    let rc = unsafe {
        rldb_prepare_v2(
            ptr::null_mut(),
            good.as_ptr(),
            -1,
            &mut stmt,
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, RLDB_MISUSE);
    assert!(stmt.is_null(), "NULL db leaves *out_stmt NULL");

    let mut stmt = sentinel();
    let rc = unsafe { rldb_prepare_v2(db, ptr::null(), -1, &mut stmt, ptr::null_mut()) };
    assert_eq!(rc, RLDB_MISUSE);
    assert!(stmt.is_null(), "NULL sql leaves *out_stmt NULL");
    assert_eq!(unsafe { rldb_close(db) }, RLDB_OK);
}

#[test]
fn invalid_utf8_bounded_returns_mismatch() {
    let (_dir, db) = open_db();
    let guard = GuardPage::new();
    let input: &[u8] = b"SELECT '\xff'";
    let sql = guard.place_at_edge(input);
    let mut stmt = sentinel();
    let rc = unsafe { rldb_prepare_v2(db, sql, input.len() as c_int, &mut stmt, ptr::null_mut()) };
    assert_eq!(rc, RLDB_MISMATCH);
    assert!(stmt.is_null(), "encoding failure leaves *out_stmt NULL");
    assert_eq!(unsafe { rldb_close(db) }, RLDB_OK);
}
