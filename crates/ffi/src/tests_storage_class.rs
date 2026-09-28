//! S9-03: column storage classes and text/byte conversions.
//!
//! The SQLite surface reports upstream storage classes (NULL is 5); the
//! native `rldb_column_type` keeps RLDB_NULL = 0. `sqlite3_column_text`
//! returns the text form of INTEGER and REAL values (the same text as
//! `CAST(x AS TEXT)`), a NULL pointer for SQL NULL, and the raw bytes of
//! TEXT and BLOB values, including interior NULs.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;

use tempfile::TempDir;

use crate::sqlite3_api::*;
use crate::types::*;
use crate::{rldb_column_bytes, rldb_column_text, rldb_column_type};

const SQLITE_NULL_TAG: c_int = 5;

fn open() -> (TempDir, *mut sqlite3) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path =
        CString::new(dir.path().join("tags.redline").to_str().expect("utf8")).expect("cstring");
    let mut db: *mut sqlite3 = ptr::null_mut();
    assert_eq!(sqlite3_open(path.as_ptr(), &mut db), RLDB_OK);
    (dir, db)
}

fn prepare(db: *mut sqlite3, sql: &str) -> *mut sqlite3_stmt {
    let sql = CString::new(sql).expect("cstring");
    let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
    assert_eq!(
        sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut()),
        RLDB_OK
    );
    assert!(!stmt.is_null());
    stmt
}

/// Copy `len` bytes starting at `ptr`, one byte at a time.
fn read_bytes(ptr: *const u8, len: usize) -> Vec<u8> {
    (0..len)
        // SAFETY: callers pass a pointer the library documents as valid for
        // `len` bytes until the next step/reset/finalize (or value free).
        .map(|offset| unsafe { *ptr.add(offset) })
        .collect()
}

fn column_bytes_view(stmt: *mut sqlite3_stmt, index: c_int) -> Option<Vec<u8>> {
    let text = sqlite3_column_text(stmt, index);
    let len = sqlite3_column_bytes(stmt, index);
    if text.is_null() {
        assert_eq!(len, 0, "NULL text has zero bytes");
        return None;
    }
    // column_text is `len` bytes followed by a NUL terminator.
    let mut bytes = read_bytes(text, len as usize + 1);
    assert_eq!(bytes.pop(), Some(0), "text is NUL-terminated");
    Some(bytes)
}

#[test]
fn sqlite3_column_type_reports_sqlite_storage_classes() {
    let (_dir, db) = open();
    let stmt = prepare(db, "SELECT NULL, 1.5, 7, 'x', x'01', -0.5, '', x''");
    let sqlite_tags = [SQLITE_NULL_TAG, 2, 1, 3, 4, 2, 3, 4];
    let native_tags = [RLDB_NULL, 2, 1, 3, 4, 2, 3, 4];
    for round in 0..2 {
        assert_eq!(sqlite3_step(stmt), RLDB_ROW, "round {round}");
        for (index, (&sqlite, &native)) in sqlite_tags.iter().zip(&native_tags).enumerate() {
            let index = index as c_int;
            // Accessors that convert must not change the reported class.
            let _ = sqlite3_column_int64(stmt, index);
            let _ = sqlite3_column_text(stmt, index);
            let _ = sqlite3_column_bytes(stmt, index);
            assert_eq!(sqlite3_column_type(stmt, index), sqlite, "column {index}");
            assert_eq!(rldb_column_type(stmt, index), native, "column {index}");
            // SAFETY: column_value returns a statement-owned value or NULL.
            let value_tag = unsafe { sqlite3_value_type(sqlite3_column_value(stmt, index)) };
            assert_eq!(value_tag, sqlite, "column {index} via sqlite3_value_type");
        }
        assert_eq!(sqlite3_reset(stmt), RLDB_OK);
    }
    assert_eq!(sqlite3_finalize(stmt), RLDB_OK);
    assert_eq!(sqlite3_close(db), RLDB_OK);
}

#[test]
fn sqlite3_column_text_converts_numbers() {
    let (_dir, db) = open();
    let stmt = prepare(
        db,
        "SELECT 7, 1.5, NULL, char(97, 0, 98), x'610062', 1.0, \
         -9223372036854775807, CAST(1.5 AS TEXT), '', x'', 1e20, CAST(1e20 AS TEXT)",
    );
    assert_eq!(
        sqlite3_step(stmt),
        RLDB_ROW,
        "interior NUL must not fail step"
    );
    let expected: [Option<&[u8]>; 10] = [
        Some(b"7"),
        Some(b"1.5"),
        None,
        Some(b"a\0b"),
        Some(b"a\0b"),
        Some(b"1.0"),
        Some(b"-9223372036854775807"),
        Some(b"1.5"),
        Some(b""),
        Some(b""),
    ];
    // A REAL's text is exactly what CAST(x AS TEXT) produces in this engine.
    assert_eq!(column_bytes_view(stmt, 10), column_bytes_view(stmt, 11));
    for (index, want) in expected.iter().enumerate() {
        let index = index as c_int;
        assert_eq!(
            column_bytes_view(stmt, index).as_deref(),
            *want,
            "column {index}"
        );
        // The pointer is stable across repeated calls until the next step.
        assert_eq!(
            sqlite3_column_text(stmt, index),
            sqlite3_column_text(stmt, index),
            "column {index}"
        );
        // sqlite3_value_text over the same column agrees with column_text.
        let value = sqlite3_column_value(stmt, index);
        // SAFETY: statement-owned value, valid until the next step.
        let text = unsafe { sqlite3_value_text(value) };
        let value_bytes = (!text.is_null()).then(|| {
            // SAFETY: statement-owned value, valid until the next step.
            let len = unsafe { sqlite3_value_bytes(value) } as usize;
            read_bytes(text, len)
        });
        assert_eq!(value_bytes.as_deref(), *want, "value column {index}");
    }
    // Byte counts before any text call use the same conversion.
    assert_eq!(sqlite3_reset(stmt), RLDB_OK);
    assert_eq!(sqlite3_step(stmt), RLDB_ROW);
    let counts: Vec<c_int> = (0..4).map(|i| sqlite3_column_bytes(stmt, i)).collect();
    assert_eq!(counts, [1, 3, 0, 3]);
    assert_eq!(rldb_column_bytes(stmt, 2), 0);
    assert!(rldb_column_text(stmt, 2).is_null());
    assert_eq!(sqlite3_finalize(stmt), RLDB_OK);
    assert_eq!(sqlite3_close(db), RLDB_OK);
}

#[test]
fn sqlite3_column_accessors_out_of_range_report_null_and_range() {
    let (_dir, db) = open();
    let stmt = prepare(db, "SELECT 1");
    // No current row yet: NULL class, NULL text, zero bytes.
    assert_eq!(sqlite3_column_type(stmt, 0), SQLITE_NULL_TAG);
    assert!(sqlite3_column_text(stmt, 0).is_null());
    assert_eq!(sqlite3_column_bytes(stmt, 0), 0);
    assert_eq!(sqlite3_step(stmt), RLDB_ROW);
    for index in [-1, 1, 99] {
        assert_eq!(sqlite3_column_type(stmt, index), SQLITE_NULL_TAG);
        assert_eq!(sqlite3_errcode(db), RLDB_RANGE, "index {index}");
        // SAFETY: errmsg pointer is owned by the open connection.
        let message = unsafe { CStr::from_ptr(sqlite3_errmsg(db) as *const c_char) };
        assert_eq!(message.to_str().expect("utf8"), "column index out of range");
        assert!(sqlite3_column_text(stmt, index).is_null());
        assert_eq!(sqlite3_column_bytes(stmt, index), 0);
    }
    assert_eq!(sqlite3_column_type(ptr::null_mut(), 0), SQLITE_NULL_TAG);
    assert_eq!(sqlite3_finalize(stmt), RLDB_OK);
    assert_eq!(sqlite3_close(db), RLDB_OK);
}
