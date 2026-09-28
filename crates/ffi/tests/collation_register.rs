//! B2 custom collation registration + ORDER BY round-trip.
//!
//! Registers `REVERSE_NOCASE`: a case-insensitive compare that reverses
//! lexicographic order (so 'a' > 'b'). Verifies ORDER BY uses it.

use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::ptr;

use redlinedb::types::rldb;
use redlinedb::{
    rldb_close, rldb_exec, rldb_open, sqlite3_collation_needed, sqlite3_create_collation_v2,
};
use tempfile::TempDir;

fn open_db() -> (TempDir, *mut rldb) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("col.redline");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    let mut db: *mut rldb = ptr::null_mut();
    let rc = unsafe { rldb_open(c_path.as_ptr(), &mut db) };
    assert_eq!(rc, 0);
    (dir, db)
}

unsafe extern "C" fn reverse_nocase(
    _user: *mut c_void,
    na: c_int,
    a: *const c_void,
    nb: c_int,
    b: *const c_void,
) -> c_int {
    let a_bytes = unsafe { std::slice::from_raw_parts(a as *const u8, na as usize) };
    let b_bytes = unsafe { std::slice::from_raw_parts(b as *const u8, nb as usize) };
    let lower_a: String = a_bytes
        .iter()
        .map(|b| (*b as char).to_ascii_lowercase())
        .collect();
    let lower_b: String = b_bytes
        .iter()
        .map(|b| (*b as char).to_ascii_lowercase())
        .collect();
    // Reverse the cmp result.
    match lower_a.cmp(&lower_b) {
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Greater => -1,
        std::cmp::Ordering::Equal => 0,
    }
}

unsafe extern "C" fn needed_cb(
    user: *mut c_void,
    db: *mut rldb,
    _encoding: c_int,
    name: *const std::os::raw::c_char,
) {
    let cstr = unsafe { std::ffi::CStr::from_ptr(name) };
    // Caller signalled it needs `cstr`; the test stashes the name into the
    // user_data pointer (cast as Mutex<String>).
    let arc = unsafe { std::sync::Arc::from_raw(user as *const std::sync::Mutex<String>) };
    {
        let mut s = arc.lock().unwrap();
        *s = cstr.to_string_lossy().into_owned();
    }
    let _ = std::sync::Arc::into_raw(arc);
    let _ = db;
}

fn exec(db: *mut rldb, sql: &str) {
    let c_sql = CString::new(sql).unwrap();
    let rc = unsafe { rldb_exec(db, c_sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
    assert_eq!(rc, 0, "exec({sql})");
}

#[test]
fn reverse_nocase_orders_descending() {
    let (_dir, db) = open_db();
    let name = CString::new("REVERSE_NOCASE").unwrap();
    let rc = unsafe {
        sqlite3_create_collation_v2(
            db,
            name.as_ptr(),
            0,
            ptr::null_mut(),
            Some(reverse_nocase),
            None,
        )
    };
    assert_eq!(rc, 0);
    exec(db, "CREATE TABLE t(s TEXT)");
    exec(db, "INSERT INTO t VALUES ('Banana'), ('apple'), ('cherry')");

    let collected: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let ctx = std::sync::Arc::into_raw(collected.clone()) as *mut c_void;
    extern "C" fn cb(
        ctx: *mut c_void,
        _ncol: c_int,
        argv: *mut *mut std::os::raw::c_char,
        _argn: *mut *mut std::os::raw::c_char,
    ) -> c_int {
        let val = unsafe { *argv };
        let s = unsafe { std::ffi::CStr::from_ptr(val) }
            .to_string_lossy()
            .into_owned();
        let arc = unsafe { std::sync::Arc::from_raw(ctx as *const std::sync::Mutex<Vec<String>>) };
        arc.lock().unwrap().push(s);
        let _ = std::sync::Arc::into_raw(arc);
        0
    }
    let c_sql = CString::new("SELECT s FROM t ORDER BY s COLLATE REVERSE_NOCASE").unwrap();
    let rc = unsafe { rldb_exec(db, c_sql.as_ptr(), Some(cb), ctx, ptr::null_mut()) };
    assert_eq!(rc, 0);
    let _ = unsafe { std::sync::Arc::from_raw(ctx as *const std::sync::Mutex<Vec<String>>) };
    let got = collected.lock().unwrap().clone();
    // Reverse alphabetical (case-insensitive).
    assert_eq!(got, vec!["cherry", "Banana", "apple"]);
    unsafe { rldb_close(db) };
}

#[test]
fn collation_needed_fires_when_invoked() {
    use redlinedb::sqlite3_api::collation::__test_invoke_needed;
    let (_dir, db) = open_db();
    let captured: std::sync::Arc<std::sync::Mutex<String>> =
        std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let user = std::sync::Arc::into_raw(captured.clone()) as *mut c_void;
    let rc = unsafe { sqlite3_collation_needed(db, user, Some(needed_cb)) };
    assert_eq!(rc, 0);
    __test_invoke_needed(db, "FOO");
    let _ = unsafe { std::sync::Arc::from_raw(user as *const std::sync::Mutex<String>) };
    assert_eq!(*captured.lock().unwrap(), "FOO");
    unsafe { rldb_close(db) };
}

#[test]
fn null_db_to_create_collation_returns_misuse() {
    let name = CString::new("X").unwrap();
    let rc = unsafe {
        sqlite3_create_collation_v2(
            ptr::null_mut(),
            name.as_ptr(),
            0,
            ptr::null_mut(),
            Some(reverse_nocase),
            None,
        )
    };
    assert_eq!(rc, 21); // RLDB_MISUSE
}

// ---- S8-04: a collation belongs to its connection -------------------------
//
// Tests run in parallel, so every destructor counter is the test's own
// `AtomicUsize`, passed as `user_data` and kept alive until after close.

use std::sync::atomic::{AtomicUsize, Ordering};

use redlinedb::sqlite3_api::collation::__test_invoke_needed;

const SQLITE_OK: c_int = 0;
const SQLITE_MISUSE: c_int = 21;
const SQLITE_UTF8: c_int = 1;

unsafe extern "C" fn count_destroy(user_data: *mut c_void) {
    // SAFETY: every registration below passes a pointer to an AtomicUsize
    // that its test keeps alive until after the connection is closed.
    let counter = unsafe { &*(user_data as *const AtomicUsize) };
    counter.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn count_needed(
    user_data: *mut c_void,
    _db: *mut rldb,
    _encoding: c_int,
    _name: *const std::os::raw::c_char,
) {
    // SAFETY: the test passes a pointer to an AtomicUsize it keeps alive.
    let counter = unsafe { &*(user_data as *const AtomicUsize) };
    counter.fetch_add(1, Ordering::SeqCst);
}

fn counter() -> Box<AtomicUsize> {
    Box::new(AtomicUsize::new(0))
}

fn user_data(counter: &AtomicUsize) -> *mut c_void {
    counter as *const AtomicUsize as *mut c_void
}

fn count(counter: &AtomicUsize) -> usize {
    counter.load(Ordering::SeqCst)
}

fn open_memory() -> *mut rldb {
    let mut db: *mut rldb = ptr::null_mut();
    let rc = unsafe { rldb_open(c":memory:".as_ptr(), &mut db) };
    assert_eq!(rc, SQLITE_OK);
    db
}

fn close(db: *mut rldb) {
    assert_eq!(unsafe { rldb_close(db) }, SQLITE_OK);
}

/// Register `reverse_nocase` as `name` with a counting destructor.
fn register(db: *mut rldb, name: &str, counter: &AtomicUsize) -> c_int {
    let name = CString::new(name).unwrap();
    unsafe {
        sqlite3_create_collation_v2(
            db,
            name.as_ptr(),
            SQLITE_UTF8,
            user_data(counter),
            Some(reverse_nocase),
            Some(count_destroy),
        )
    }
}

fn unregister(db: *mut rldb, name: &str) -> c_int {
    let name = CString::new(name).unwrap();
    unsafe {
        sqlite3_create_collation_v2(db, name.as_ptr(), SQLITE_UTF8, ptr::null_mut(), None, None)
    }
}

fn seed(db: *mut rldb) {
    exec(db, "CREATE TABLE fruit(s TEXT)");
    exec(
        db,
        "INSERT INTO fruit VALUES ('apple'), ('Banana'), ('cherry')",
    );
}

/// `ORDER BY s COLLATE <name>` over the seeded rows: the ordered values, or
/// the failing return code.
fn ordered_by(db: *mut rldb, collation: &str) -> Result<Vec<String>, c_int> {
    let collected: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    extern "C" fn cb(
        ctx: *mut c_void,
        _ncol: c_int,
        argv: *mut *mut std::os::raw::c_char,
        _argn: *mut *mut std::os::raw::c_char,
    ) -> c_int {
        let value = unsafe { std::ffi::CStr::from_ptr(*argv) }
            .to_string_lossy()
            .into_owned();
        let collected = unsafe { &*(ctx as *const std::sync::Mutex<Vec<String>>) };
        collected.lock().unwrap().push(value);
        0
    }
    let sql = CString::new(format!(
        "SELECT s FROM fruit ORDER BY s COLLATE {collation}"
    ))
    .unwrap();
    let ctx = &collected as *const std::sync::Mutex<Vec<String>> as *mut c_void;
    let rc = unsafe { rldb_exec(db, sql.as_ptr(), Some(cb), ctx, ptr::null_mut()) };
    if rc != SQLITE_OK {
        return Err(rc);
    }
    Ok(collected.into_inner().unwrap())
}

const REVERSED: [&str; 3] = ["cherry", "Banana", "apple"];

#[test]
fn collation_destructor_runs_once_on_replace() {
    let (_dir, db) = open_db();
    seed(db);
    let first = counter();
    let second = counter();
    assert_eq!(register(db, "REV_A", &first), SQLITE_OK);
    assert_eq!(register(db, "rev_a", &second), SQLITE_OK);
    assert_eq!((count(&first), count(&second)), (1, 0));
    assert_eq!(ordered_by(db, "REV_A").unwrap(), REVERSED);
    close(db);
    assert_eq!((count(&first), count(&second)), (1, 1));
}

#[test]
fn collation_destructor_runs_on_close() {
    let (_dir, db) = open_db();
    let destroyed = counter();
    assert_eq!(register(db, "REV_CLOSE", &destroyed), SQLITE_OK);
    assert_eq!(count(&destroyed), 0);
    close(db);
    assert_eq!(count(&destroyed), 1);
}

#[test]
fn collation_null_compare_unregisters_and_destroys() {
    let (_dir, db) = open_db();
    seed(db);
    let destroyed = counter();
    assert_eq!(register(db, "REV_GONE", &destroyed), SQLITE_OK);
    assert_eq!(ordered_by(db, "REV_GONE").unwrap(), REVERSED);
    assert_eq!(unregister(db, "REV_GONE"), SQLITE_OK);
    assert_eq!(count(&destroyed), 1);
    assert!(ordered_by(db, "REV_GONE").is_err(), "the collation is gone");
    close(db);
    assert_eq!(count(&destroyed), 1);
}

#[test]
fn collation_failed_registration_does_not_invoke_destructor() {
    // Unlike every other SQLite interface, sqlite3_create_collation_v2 does
    // not call xDestroy when it fails: the caller keeps ownership.
    let (_dir, db) = open_db();
    let destroyed = counter();
    let invalid_utf8 = [0xFFu8, 0xFE, 0x00];
    let rc = unsafe {
        sqlite3_create_collation_v2(
            db,
            invalid_utf8.as_ptr().cast(),
            SQLITE_UTF8,
            user_data(&destroyed),
            Some(reverse_nocase),
            Some(count_destroy),
        )
    };
    assert_eq!(rc, SQLITE_MISUSE);
    let rc = unsafe {
        sqlite3_create_collation_v2(
            db,
            ptr::null(),
            SQLITE_UTF8,
            user_data(&destroyed),
            Some(reverse_nocase),
            Some(count_destroy),
        )
    };
    assert_eq!(rc, SQLITE_MISUSE);
    close(db);
    assert_eq!(count(&destroyed), 0);
}

#[test]
fn closed_connection_collation_not_inherited_by_reused_address() {
    // A handle's address is the registry key, and the allocator hands a
    // freed handle's address to a later open. Closing a batch and opening
    // another reuses one in about half the rounds on glibc.
    let mut reused = false;
    for _ in 0..64 {
        let destroyed = counter();
        let closed: Vec<usize> = (0..8)
            .map(|_| {
                let db = open_memory();
                assert_eq!(register(db, "REV_SENTINEL", &destroyed), SQLITE_OK);
                db as usize
            })
            .collect();
        for &db in &closed {
            close(db as *mut rldb);
        }
        assert_eq!(
            count(&destroyed),
            8,
            "close destroys the connection's collations"
        );
        let opened: Vec<*mut rldb> = (0..8).map(|_| open_memory()).collect();
        for &db in &opened {
            if closed.contains(&(db as usize)) {
                reused = true;
                seed(db);
                assert!(
                    ordered_by(db, "REV_SENTINEL").is_err(),
                    "a new connection at a reused address inherited a closed connection's collation"
                );
            }
        }
        opened.into_iter().for_each(close);
        if reused {
            break;
        }
    }
    if !reused {
        eprintln!("note: the allocator did not reuse a handle address in 64 rounds");
    }
}

#[test]
fn two_live_connections_collations_isolated() {
    let (_dir_a, a) = open_db();
    let (_dir_b, b) = open_db();
    seed(b);
    let on_a = counter();
    let on_b = counter();
    assert_eq!(register(a, "REV_ISO", &on_a), SQLITE_OK);
    assert!(
        ordered_by(b, "REV_ISO").is_err(),
        "b must not see a's collation"
    );
    assert_eq!(register(b, "REV_ISO", &on_b), SQLITE_OK);
    assert_eq!((count(&on_a), count(&on_b)), (0, 0));
    close(a);
    assert_eq!((count(&on_a), count(&on_b)), (1, 0));
    assert_eq!(ordered_by(b, "REV_ISO").unwrap(), REVERSED);
    close(b);
    assert_eq!((count(&on_a), count(&on_b)), (1, 1));
}

#[test]
fn collation_needed_is_per_connection() {
    let (_dir_a, a) = open_db();
    let (_dir_b, b) = open_db();
    let hits = counter();
    let rc = unsafe { sqlite3_collation_needed(a, user_data(&hits), Some(count_needed)) };
    assert_eq!(rc, SQLITE_OK);
    __test_invoke_needed(b, "FOO");
    assert_eq!(
        count(&hits),
        0,
        "b must not call a's collation-needed callback"
    );
    __test_invoke_needed(a, "FOO");
    assert_eq!(count(&hits), 1);
    close(a);
    close(b);
}
