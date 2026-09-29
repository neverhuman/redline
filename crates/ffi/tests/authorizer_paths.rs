//! Authorizer coverage for the paths `hooks.rs` does not drive:
//!
//! - statements run with `rldb_prepare_v2` + `rldb_step` rather than
//!   `rldb_exec`, including a CTE body, which the engine reads while the
//!   statement is prepared;
//! - `SQLITE_UPDATE`, which upstream asks once per assigned column with the
//!   column name in `arg4` (never NULL), and whose `SQLITE_IGNORE` leaves
//!   just that column unchanged.
//!
//! Each test keeps its own authorizer state in `user_data`, so the tests
//! can run on parallel threads.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::Mutex;

use redlinedb::types::{rldb, rldb_stmt};
use redlinedb::{
    rldb_close, rldb_column_text, rldb_errmsg, rldb_exec, rldb_finalize, rldb_free, rldb_open,
    rldb_prepare_v2, rldb_step, sqlite3_set_authorizer,
};
use tempfile::TempDir;

const SQLITE_OK: c_int = 0;
const SQLITE_ERROR: c_int = 1;
const SQLITE_AUTH: c_int = 23;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_DENY: c_int = 1;
const SQLITE_IGNORE: c_int = 2;
const SQLITE_SELECT: c_int = 21;
const SQLITE_UPDATE: c_int = 23;

fn open_db() -> (TempDir, *mut rldb) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = CString::new(dir.path().join("a.redline").to_str().unwrap()).unwrap();
    let mut db: *mut rldb = ptr::null_mut();
    // SAFETY: `path` is NUL-terminated and `db` is a local out slot.
    assert_eq!(unsafe { rldb_open(path.as_ptr(), &mut db) }, SQLITE_OK);
    (dir, db)
}

fn exec(db: *mut rldb, sql: &str) -> (c_int, String) {
    let sql = CString::new(sql).unwrap();
    let mut errmsg: *mut c_char = ptr::null_mut();
    // SAFETY: `db` is live, `sql` is NUL-terminated, `errmsg` is a local slot.
    let rc = unsafe { rldb_exec(db, sql.as_ptr(), None, ptr::null_mut(), &mut errmsg) };
    let message = if errmsg.is_null() {
        String::new()
    } else {
        // SAFETY: a non-NULL errmsg is a NUL-terminated string this library
        // allocated; it is freed exactly once here.
        let message = unsafe { CStr::from_ptr(errmsg) }
            .to_string_lossy()
            .into_owned();
        unsafe { rldb_free(errmsg as *mut c_void) };
        message
    };
    (rc, message)
}

fn errmsg(db: *mut rldb) -> String {
    // SAFETY: `db` is live; the message is owned by the connection.
    unsafe { CStr::from_ptr(rldb_errmsg(db)) }
        .to_string_lossy()
        .into_owned()
}

/// Prepare and step `sql` to completion. Returns the first failing code
/// (from prepare or step) or SQLITE_DONE, and every text value it read.
fn prepare_and_step(db: *mut rldb, sql: &str) -> (c_int, Vec<String>) {
    let sql = CString::new(sql).unwrap();
    let mut stmt: *mut rldb_stmt = ptr::null_mut();
    // SAFETY: `db` is live, `sql` is NUL-terminated, `stmt` is a local slot.
    let rc = unsafe { rldb_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut()) };
    if rc != SQLITE_OK {
        assert!(stmt.is_null(), "a failed prepare leaves no statement");
        return (rc, Vec::new());
    }
    let mut seen = Vec::new();
    let rc = loop {
        // SAFETY: `stmt` is live until the finalize below.
        match unsafe { rldb_step(stmt) } {
            SQLITE_ROW => {
                // SAFETY: `stmt` is on a row; the text is copied at once.
                let text = unsafe { rldb_column_text(stmt, 0) };
                if !text.is_null() {
                    seen.push(
                        unsafe { CStr::from_ptr(text.cast()) }
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
            other => break other,
        }
    };
    // SAFETY: `stmt` is live and finalized exactly once.
    unsafe { rldb_finalize(stmt) };
    (rc, seen)
}

fn text(p: *const c_char) -> Option<String> {
    // SAFETY: the library passes NULL or a NUL-terminated string valid for
    // the duration of the callback.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

// ---- SELECT through prepare/step ----------------------------------------

unsafe extern "C" fn deny_select_sensitive(
    _: *mut c_void,
    action: c_int,
    arg3: *const c_char,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
) -> c_int {
    if action == SQLITE_SELECT && text(arg3).as_deref() == Some("sensitive") {
        SQLITE_DENY
    } else {
        SQLITE_OK
    }
}

#[test]
fn prepare_step_denies_a_table_read_through_a_cte_or_subquery() {
    let (_dir, db) = open_db();
    let (rc, _) = exec(
        db,
        "CREATE TABLE sensitive(secret TEXT); INSERT INTO sensitive VALUES ('shh'); \
         CREATE TABLE open_data(v TEXT); INSERT INTO open_data VALUES ('hello')",
    );
    assert_eq!(rc, SQLITE_OK);
    // SAFETY: `db` is live; the callback keeps no state.
    unsafe { sqlite3_set_authorizer(db, Some(deny_select_sensitive), ptr::null_mut()) };
    for sql in [
        "SELECT secret FROM sensitive",
        "WITH s AS (SELECT secret FROM sensitive) SELECT * FROM s",
        "WITH s AS MATERIALIZED (SELECT secret FROM sensitive) SELECT secret FROM s",
        "WITH RECURSIVE r(x) AS (SELECT secret FROM sensitive UNION ALL \
         SELECT x FROM r WHERE 0) SELECT x FROM r",
        "SELECT * FROM (SELECT secret FROM sensitive)",
        "SELECT v FROM open_data UNION ALL SELECT secret FROM sensitive",
    ] {
        let (rc, seen) = prepare_and_step(db, sql);
        assert_eq!(rc, SQLITE_AUTH, "{sql}: rows {seen:?}");
        assert!(!seen.contains(&"shh".to_owned()), "{sql} leaked the secret");
        assert_eq!(errmsg(db), "not authorized", "{sql}");
    }
    // The allowed table still reads, through a CTE too.
    let (rc, seen) = prepare_and_step(db, "WITH o AS (SELECT v FROM open_data) SELECT v FROM o");
    assert_eq!((rc, seen), (SQLITE_DONE, vec!["hello".to_owned()]));
    // SAFETY: `db` is live and closed exactly once.
    unsafe { rldb_close(db) };
}

unsafe extern "C" fn authorizer_malfunction(
    _: *mut c_void,
    _: c_int,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
) -> c_int {
    7 // not SQLITE_OK, SQLITE_DENY or SQLITE_IGNORE
}

unsafe extern "C" fn deny_all(
    _: *mut c_void,
    _: c_int,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
) -> c_int {
    SQLITE_DENY
}

#[test]
fn prepare_step_reports_an_authorizer_malfunction() {
    let (_dir, db) = open_db();
    assert_eq!(
        exec(db, "CREATE TABLE t(x); INSERT INTO t VALUES (1)").0,
        SQLITE_OK
    );
    // SAFETY: `db` is live; the callbacks keep no state.
    unsafe { sqlite3_set_authorizer(db, Some(authorizer_malfunction), ptr::null_mut()) };
    for sql in [
        "SELECT x FROM t",
        "INSERT INTO t VALUES (2)",
        "WITH c AS (SELECT x FROM t) SELECT x FROM c",
    ] {
        let (rc, _) = prepare_and_step(db, sql);
        assert_eq!(rc, SQLITE_ERROR, "{sql}");
        assert_eq!(errmsg(db), "authorizer malfunction", "{sql}");
    }
    // The malfunction does not linger into the next statement.
    unsafe { sqlite3_set_authorizer(db, Some(deny_all), ptr::null_mut()) };
    let (rc, _) = prepare_and_step(db, "SELECT x FROM t");
    assert_eq!(rc, SQLITE_AUTH);
    assert_eq!(errmsg(db), "not authorized");
    unsafe { sqlite3_set_authorizer(db, None, ptr::null_mut()) };
    assert_eq!(prepare_and_step(db, "SELECT x FROM t").0, SQLITE_DONE);
    // SAFETY: `db` is live and closed exactly once.
    unsafe { rldb_close(db) };
}

#[path = "authorizer_paths/update.rs"]
mod update;
