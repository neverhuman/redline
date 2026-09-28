//! `sqlite3_exec` / `rldb_exec` callback arguments.
//!
//! The callback receives each value as `sqlite3_column_text` would return
//! it: a REAL keeps its fraction ("1.5", not "1"), SQL NULL is a NULL
//! pointer, a BLOB is its own bytes (not hex), and TEXT with an interior NUL
//! is passed through so C sees the text up to the first NUL, instead of
//! failing the whole exec. Column names with an interior NUL are cut at the
//! NUL, as `sqlite3_column_name` cuts them.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use redlinedb::sqlite3_api::{sqlite3_close, sqlite3_exec, sqlite3_open};
use redlinedb::types::sqlite3;
use redlinedb::{rldb, rldb_close, rldb_exec, rldb_open};

const SQLITE_OK: c_int = 0;

/// One callback row: each argument (NULL pointer as `None`, otherwise the
/// bytes up to the terminating NUL) and each column name.
#[derive(Debug, Default, PartialEq)]
struct Rows {
    rows: Vec<Vec<Option<Vec<u8>>>>,
    names: Vec<Vec<String>>,
}

extern "C" fn collect(
    context: *mut c_void,
    argc: c_int,
    argv: *mut *mut c_char,
    names: *mut *mut c_char,
) -> c_int {
    // SAFETY: `context` is the `&mut Rows` passed to exec below, and the
    // argument arrays hold `argc` entries valid for this call.
    let rows = unsafe { &mut *(context as *mut Rows) };
    let mut row = Vec::new();
    let mut row_names = Vec::new();
    for index in 0..argc as usize {
        // SAFETY: `index < argc`; see above.
        let (value, name) = unsafe { (*argv.add(index), *names.add(index)) };
        // SAFETY: non-null entries are NUL-terminated strings owned by the
        // library for the duration of the call.
        row.push((!value.is_null()).then(|| unsafe { CStr::from_ptr(value) }.to_bytes().to_vec()));
        assert!(!name.is_null(), "column {index} has a name");
        // SAFETY: as above.
        row_names.push(
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    rows.rows.push(row);
    rows.names.push(row_names);
    0
}

/// Values whose conversion differs from a plain `to_string`: a REAL with a
/// fraction, NULL, a BLOB and an integral REAL.
const QUERY: &str = "SELECT 1.5 AS r, NULL AS n, x'41' AS b, 7 AS i, 2.0 AS w";

fn expected_row() -> Vec<Option<Vec<u8>>> {
    vec![
        Some(b"1.5".to_vec()),
        None,
        Some(b"A".to_vec()),
        Some(b"7".to_vec()),
        Some(b"2.0".to_vec()),
    ]
}

fn expected_names() -> Vec<String> {
    ["r", "n", "b", "i", "w"].map(String::from).to_vec()
}

#[test]
fn sqlite3_exec_passes_values_as_column_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path =
        CString::new(dir.path().join("exec.redline").to_str().expect("utf8")).expect("cstring");
    let mut db: *mut sqlite3 = ptr::null_mut();
    // SAFETY: `path` is NUL-terminated and `db` is a local out slot.
    assert_eq!(unsafe { sqlite3_open(path.as_ptr(), &mut db) }, SQLITE_OK);
    let sql = CString::new(QUERY).expect("cstring");
    let mut rows = Rows::default();
    let mut errmsg: *mut c_char = ptr::null_mut();
    // SAFETY: `db` is live, `sql` is NUL-terminated, and `rows` outlives the
    // call; the callback matches the documented signature.
    let rc = unsafe {
        sqlite3_exec(
            db,
            sql.as_ptr(),
            Some(collect),
            &mut rows as *mut Rows as *mut c_void,
            &mut errmsg,
        )
    };
    assert_eq!(rc, SQLITE_OK);
    assert!(errmsg.is_null());
    assert_eq!(rows.rows, vec![expected_row()]);
    assert_eq!(rows.names, vec![expected_names()]);
    // SAFETY: `db` is live and closed exactly once.
    assert_eq!(unsafe { sqlite3_close(db) }, SQLITE_OK);
}

#[test]
fn rldb_exec_passes_values_as_column_text_and_cuts_names_at_nul() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path =
        CString::new(dir.path().join("exec.redline").to_str().expect("utf8")).expect("cstring");
    let mut db: *mut rldb = ptr::null_mut();
    // SAFETY: `path` is NUL-terminated and `db` is a local out slot.
    assert_eq!(unsafe { rldb_open(path.as_ptr(), &mut db) }, SQLITE_OK);
    // The engine names an unaliased folded constant after its value, so
    // this column's name has an interior NUL.
    let sql = CString::new(format!(
        "{QUERY}, char(97, 0, 98) AS t, char(99, 0, 100); SELECT 2.25"
    ))
    .expect("cstring");
    let mut rows = Rows::default();
    let mut errmsg: *mut c_char = ptr::null_mut();
    // SAFETY: as in the test above.
    let rc = unsafe {
        rldb_exec(
            db,
            sql.as_ptr(),
            Some(collect),
            &mut rows as *mut Rows as *mut c_void,
            &mut errmsg,
        )
    };
    assert_eq!(rc, SQLITE_OK, "an interior NUL must not fail the exec");
    assert!(errmsg.is_null());
    let mut first = expected_row();
    first.push(Some(b"a".to_vec()));
    first.push(Some(b"c".to_vec()));
    assert_eq!(rows.rows, vec![first, vec![Some(b"2.25".to_vec())]]);
    assert_eq!(rows.names[0][5], "t");
    // The engine names the column after the folded literal, 'c\0d', so the
    // name C can see ends at that NUL.
    assert_eq!(rows.names[0][6], "'c", "the name is cut at its NUL");
    assert_eq!(rows.names[1], ["2.25"]);
    // SAFETY: `db` is live and closed exactly once.
    assert_eq!(unsafe { rldb_close(db) }, SQLITE_OK);
}
