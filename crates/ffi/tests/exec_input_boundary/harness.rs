//! Connection helpers for the `rldb_exec` input-boundary tests: a unique
//! database file per test, open and close, and the `users` table.

use super::*;

static TEST_SEQ: AtomicUsize = AtomicUsize::new(0);

fn unique_db_path(label: &str) -> (TempDir, CString) {
    let dir = tempfile::tempdir().expect("tempdir");
    let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let name = format!("rldb-{label}-{seq}-{nanos}.redline");
    let path = dir.path().join(name);
    let c_path = CString::new(path.to_str().expect("utf8 path")).expect("nul-free path");
    (dir, c_path)
}

pub(super) fn open_db(label: &str) -> (TempDir, *mut rldb) {
    let (dir, c_path) = unique_db_path(label);
    let mut db: *mut rldb = ptr::null_mut();
    let rc = unsafe { rldb_open(c_path.as_ptr(), &mut db) };
    assert_eq!(rc, RLDB_OK, "rldb_open failed rc={rc}");
    assert!(!db.is_null());
    (dir, db)
}

pub(super) fn close_db(db: *mut rldb) {
    let rc = unsafe { rldb_close(db) };
    assert_eq!(rc, RLDB_OK, "rldb_close failed rc={rc}");
}

pub(super) fn exec(db: *mut rldb, sql: &str) -> c_int {
    let cs = CString::new(sql).expect("sql nul-free");
    let mut errmsg: *mut c_char = ptr::null_mut();
    unsafe { rldb_exec(db, cs.as_ptr(), None, ptr::null_mut(), &mut errmsg) }
}

pub(super) fn exec_expect_ok(db: *mut rldb, sql: &str) {
    let rc = exec(db, sql);
    assert_eq!(rc, RLDB_OK, "rldb_exec({sql:?}) rc={rc}");
}

pub(super) fn create_users_table(db: *mut rldb) {
    exec_expect_ok(
        db,
        "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, payload BLOB)",
    );
}

pub(super) fn insert_user(db: *mut rldb, id: i64, name: &str) {
    let sql = format!("INSERT INTO users(id, name) VALUES({id}, '{name}')");
    exec_expect_ok(db, &sql);
}
