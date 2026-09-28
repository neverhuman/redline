//! S9-04 through the C API: `sqlite3_prepare_v2` of a savepoint command
//! changes nothing until `sqlite3_step`, and a finalize without a step
//! leaves the transaction as it was.

use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;

use std::os::raw::c_int;

use redlinedb::sqlite3_api::*;
use redlinedb::types::{sqlite3, sqlite3_stmt};

// Exported by the library as a C symbol but not re-exported for Rust.
unsafe extern "C" {
    fn sqlite3_get_autocommit(db: *mut sqlite3) -> c_int;
}

// The SQLite result codes this test checks (sqlite3.h).
const RLDB_OK: c_int = 0;
const RLDB_DONE: c_int = 101;

fn open(name: &str) -> *mut sqlite3 {
    let dir = std::env::temp_dir().join(format!(
        "redlinedb-ffi-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = CString::new(dir.join("sp.redline").to_str().expect("utf8")).expect("path");
    let mut db: *mut sqlite3 = ptr::null_mut();
    // SAFETY: `path` is a NUL-terminated CString; `db` is a local out slot.
    assert_eq!(unsafe { sqlite3_open(path.as_ptr(), &mut db) }, RLDB_OK);
    db
}

fn exec(db: *mut sqlite3, sql: &str) -> i32 {
    let sql = CString::new(sql).expect("sql");
    // SAFETY: `db` is live; `sql` is NUL-terminated; no callback or errmsg slot.
    unsafe { sqlite3_exec(db, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) }
}

fn prepare(db: *mut sqlite3, sql: &str) -> *mut sqlite3_stmt {
    let sql = CString::new(sql).expect("sql");
    let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
    assert_eq!(
        // SAFETY: `db` is live; `sql` is NUL-terminated; `stmt` is a local slot.
        unsafe { sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut()) },
        RLDB_OK
    );
    assert!(!stmt.is_null());
    stmt
}

extern "C" fn count_rows(
    ctx: *mut c_void,
    _n: std::os::raw::c_int,
    _argv: *mut *mut std::os::raw::c_char,
    _names: *mut *mut std::os::raw::c_char,
) -> std::os::raw::c_int {
    // SAFETY: `ctx` is the `&mut usize` passed by `rows` below.
    unsafe { *(ctx as *mut usize) += 1 };
    0
}

fn rows(db: *mut sqlite3, sql: &str) -> usize {
    let sql = CString::new(sql).expect("sql");
    let mut counter = 0usize;
    assert_eq!(
        // SAFETY: `db` is live; `sql` is NUL-terminated; `counter` outlives the call.
        unsafe {
            sqlite3_exec(
                db,
                sql.as_ptr(),
                Some(count_rows),
                (&mut counter) as *mut usize as *mut c_void,
                ptr::null_mut(),
            )
        },
        RLDB_OK
    );
    counter
}

#[test]
fn prepare_and_finalize_of_a_savepoint_command_changes_nothing() {
    let db = open("sp-prepare");
    assert_eq!(exec(db, "CREATE TABLE t(id INTEGER PRIMARY KEY)"), RLDB_OK);

    // SAVEPOINT prepared and finalized: no transaction opens.
    let savepoint = prepare(db, "SAVEPOINT sp");
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_stmt_readonly(savepoint) }, 1);
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_finalize(savepoint) }, RLDB_OK);
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_get_autocommit(db) }, 1);

    // RELEASE of the savepoint that opened the transaction, prepared and
    // finalized: nothing commits, and ROLLBACK still discards the row.
    assert_eq!(exec(db, "SAVEPOINT sp; INSERT INTO t VALUES (1)"), RLDB_OK);
    let release = prepare(db, "RELEASE sp");
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_finalize(release) }, RLDB_OK);
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_get_autocommit(db) }, 0);
    assert_eq!(exec(db, "ROLLBACK"), RLDB_OK);
    assert_eq!(rows(db, "SELECT id FROM t"), 0);

    // ROLLBACK TO prepared and finalized keeps the rows; stepped, it rewinds.
    assert_eq!(
        exec(
            db,
            "BEGIN; INSERT INTO t VALUES (1); SAVEPOINT sp; INSERT INTO t VALUES (2)"
        ),
        RLDB_OK
    );
    let rollback_to = prepare(db, "ROLLBACK TO sp");
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_finalize(rollback_to) }, RLDB_OK);
    assert_eq!(rows(db, "SELECT id FROM t"), 2);
    let rollback_to = prepare(db, "ROLLBACK TO sp");
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_step(rollback_to) }, RLDB_DONE);
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_finalize(rollback_to) }, RLDB_OK);
    assert_eq!(rows(db, "SELECT id FROM t"), 1);
    assert_eq!(exec(db, "COMMIT"), RLDB_OK);
    assert_eq!(rows(db, "SELECT id FROM t"), 1);
    // SAFETY: `db` and the statement are live until their close/finalize below.
    assert_eq!(unsafe { sqlite3_close(db) }, RLDB_OK);
}
