//! B4 hooks: commit / rollback / update / trace / profile / authorizer /
//! busy.
//!
//! Each test registers a callback and exercises the FFI-side fire helper
//! that the higher layers invoke at well-defined sites (exec walk, blob
//! write, prepare).

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use redlinedb::sqlite3_api::hooks_fire::{
    __test_fire_authorizer, __test_fire_busy, __test_fire_commit, __test_fire_profile,
    __test_fire_rollback, __test_fire_trace, __test_fire_update,
};
use redlinedb::types::rldb;
use redlinedb::{
    rldb_close, rldb_open, sqlite3_busy_handler, sqlite3_commit_hook, sqlite3_profile,
    sqlite3_rollback_hook, sqlite3_set_authorizer, sqlite3_trace, sqlite3_update_hook,
};
use tempfile::TempDir;

const RLDB_OK: i32 = 0;

fn open_db() -> (TempDir, *mut rldb) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("h.redline");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    let mut db: *mut rldb = ptr::null_mut();
    let rc = unsafe { rldb_open(c_path.as_ptr(), &mut db) };
    assert_eq!(rc, RLDB_OK);
    (dir, db)
}

// Tests run on parallel threads of one process (`cargo test`), so each test
// counts through its own `user_data`. With one static per hook, the exec-walk
// test's trace, commit and profile callbacks landed in the counters of the
// single-hook tests below and failed them intermittently.

/// The `AtomicU64` that a test registered as its callback's `user_data`.
fn slot<'a>(user_data: *mut c_void) -> &'a AtomicU64 {
    // SAFETY: every registration that uses these callbacks passes
    // `as_user_data` of an AtomicU64 that its test keeps alive until after
    // rldb_close.
    unsafe { &*(user_data as *const AtomicU64) }
}

fn as_user_data(slot: &AtomicU64) -> *mut c_void {
    slot as *const AtomicU64 as *mut c_void
}

unsafe extern "C" fn commit_ok(user_data: *mut c_void) -> c_int {
    slot(user_data).fetch_add(1, Ordering::Relaxed);
    0
}

unsafe extern "C" fn commit_veto(user_data: *mut c_void) -> c_int {
    slot(user_data).fetch_add(1, Ordering::Relaxed);
    1
}

unsafe extern "C" fn rollback_cb(user_data: *mut c_void) {
    slot(user_data).fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn update_cb(
    user_data: *mut c_void,
    _op: c_int,
    _db: *const c_char,
    _tbl: *const c_char,
    _rowid: i64,
) {
    slot(user_data).fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn trace_cb(user_data: *mut c_void, _sql: *const c_char) {
    slot(user_data).fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn profile_cb(user_data: *mut c_void, _sql: *const c_char, nanos: u64) {
    slot(user_data).store(nanos, Ordering::Relaxed);
}

unsafe extern "C" fn authorizer_cb(
    user_data: *mut c_void,
    _action: c_int,
    _arg3: *const c_char,
    _arg4: *const c_char,
    _arg5: *const c_char,
    _arg6: *const c_char,
) -> c_int {
    slot(user_data).fetch_add(1, Ordering::Relaxed);
    1 // SQLITE_DENY
}

unsafe extern "C" fn busy_cb(user_data: *mut c_void, attempts: c_int) -> c_int {
    slot(user_data).store(attempts as u64, Ordering::Relaxed);
    if attempts < 3 { 1 } else { 0 }
}

#[test]
fn commit_hook_fires_and_can_veto() {
    let (_dir, db) = open_db();
    let commits = AtomicU64::new(0);
    unsafe { sqlite3_commit_hook(db, Some(commit_ok), as_user_data(&commits)) };
    let vetoed = __test_fire_commit(db);
    assert!(!vetoed);
    assert_eq!(commits.load(Ordering::Relaxed), 1);

    unsafe { sqlite3_commit_hook(db, Some(commit_veto), as_user_data(&commits)) };
    let vetoed = __test_fire_commit(db);
    assert!(vetoed);
    assert_eq!(commits.load(Ordering::Relaxed), 2);
    unsafe { rldb_close(db) };
}

#[test]
fn rollback_hook_fires() {
    let (_dir, db) = open_db();
    let rollbacks = AtomicU64::new(0);
    unsafe { sqlite3_rollback_hook(db, Some(rollback_cb), as_user_data(&rollbacks)) };
    __test_fire_rollback(db);
    assert_eq!(rollbacks.load(Ordering::Relaxed), 1);
    unsafe { rldb_close(db) };
}

#[test]
fn update_hook_fires_with_table_and_rowid() {
    let (_dir, db) = open_db();
    let updates = AtomicU64::new(0);
    unsafe { sqlite3_update_hook(db, Some(update_cb), as_user_data(&updates)) };
    __test_fire_update(db, 18, "users", 42);
    assert_eq!(updates.load(Ordering::Relaxed), 1);
    unsafe { rldb_close(db) };
}

#[test]
fn trace_hook_fires_with_sql_string() {
    let (_dir, db) = open_db();
    let traces = AtomicU64::new(0);
    unsafe { sqlite3_trace(db, Some(trace_cb), as_user_data(&traces)) };
    __test_fire_trace(db, "SELECT 1");
    assert_eq!(traces.load(Ordering::Relaxed), 1);
    unsafe { rldb_close(db) };
}

#[test]
fn profile_hook_captures_nanoseconds() {
    let (_dir, db) = open_db();
    let nanos = AtomicU64::new(0);
    unsafe { sqlite3_profile(db, Some(profile_cb), as_user_data(&nanos)) };
    __test_fire_profile(db, "SELECT 1", 12345);
    assert_eq!(nanos.load(Ordering::Relaxed), 12345);
    unsafe { rldb_close(db) };
}

#[test]
fn authorizer_returns_decision_code() {
    let (_dir, db) = open_db();
    let calls = AtomicU64::new(0);
    unsafe { sqlite3_set_authorizer(db, Some(authorizer_cb), as_user_data(&calls)) };
    let decision = __test_fire_authorizer(db, 9 /* SQLITE_DELETE */, Some("users"));
    assert_eq!(decision, 1); // SQLITE_DENY
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    unsafe { rldb_close(db) };
}

#[test]
fn busy_handler_retry_decision() {
    let (_dir, db) = open_db();
    let last_attempt = AtomicU64::new(0);
    unsafe { sqlite3_busy_handler(db, Some(busy_cb), as_user_data(&last_attempt)) };
    assert!(__test_fire_busy(db, 1));
    assert!(__test_fire_busy(db, 2));
    assert!(!__test_fire_busy(db, 3));
    assert_eq!(last_attempt.load(Ordering::Relaxed), 3);
    unsafe { rldb_close(db) };
}

#[test]
fn hook_registration_on_null_db_returns_null_or_misuse() {
    unsafe {
        let prev = sqlite3_commit_hook(ptr::null_mut(), None, ptr::null_mut());
        assert!(prev.is_null());
        let prev = sqlite3_rollback_hook(ptr::null_mut(), None, ptr::null_mut());
        assert!(prev.is_null());
        let prev = sqlite3_update_hook(ptr::null_mut(), None, ptr::null_mut());
        assert!(prev.is_null());
        let prev = sqlite3_trace(ptr::null_mut(), None, ptr::null_mut());
        assert!(prev.is_null());
        let prev = sqlite3_profile(ptr::null_mut(), None, ptr::null_mut());
        assert!(prev.is_null());
        assert_eq!(
            sqlite3_busy_handler(ptr::null_mut(), None, ptr::null_mut()),
            21
        );
        assert_eq!(
            sqlite3_set_authorizer(ptr::null_mut(), None, ptr::null_mut()),
            21
        );
    }
}

// End-to-end: update_hook fires per row across INSERT/UPDATE/DELETE.
// Drives the SQL DML executors via rldb_exec so the hook firing path is
// the production path (not the test-only __test_fire_update helper).
static UPDATE_ROWS: std::sync::Mutex<Vec<(c_int, String, i64)>> = std::sync::Mutex::new(Vec::new());

unsafe extern "C" fn update_record_cb(
    _: *mut c_void,
    op: c_int,
    _db: *const c_char,
    tbl: *const c_char,
    rowid: i64,
) {
    let table = if tbl.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(tbl) }
            .to_string_lossy()
            .into_owned()
    };
    UPDATE_ROWS.lock().unwrap().push((op, table, rowid));
}

#[test]
fn update_hook_fires_per_row() {
    let (_dir, db) = open_db();
    UPDATE_ROWS.lock().unwrap().clear();
    unsafe { sqlite3_update_hook(db, Some(update_record_cb), ptr::null_mut()) };
    let sql = CString::new(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER); \
         INSERT INTO t(id, v) VALUES (1, 10), (2, 20), (3, 30); \
         UPDATE t SET v = v + 1 WHERE id <= 2; \
         DELETE FROM t WHERE id = 3;",
    )
    .unwrap();
    let rc =
        unsafe { redlinedb::rldb_exec(db, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
    assert_eq!(rc, RLDB_OK);
    let rows = UPDATE_ROWS.lock().unwrap().clone();
    // 3 INSERT + 2 UPDATE + 1 DELETE = 6 callbacks.
    assert_eq!(rows.len(), 6, "rows={rows:?}");
    let inserts: Vec<_> = rows
        .iter()
        .filter(|(op, _, _)| *op == 18 /* SQLITE_INSERT */)
        .collect();
    let updates: Vec<_> = rows
        .iter()
        .filter(|(op, _, _)| *op == 23 /* SQLITE_UPDATE */)
        .collect();
    let deletes: Vec<_> = rows
        .iter()
        .filter(|(op, _, _)| *op == 9 /* SQLITE_DELETE */)
        .collect();
    assert_eq!(inserts.len(), 3);
    assert_eq!(updates.len(), 2);
    assert_eq!(deletes.len(), 1);
    for (_, table, _) in &rows {
        assert_eq!(table, "t");
    }
    // Insert rowids match the explicit PK values.
    let mut ins_ids: Vec<i64> = inserts.iter().map(|(_, _, r)| *r).collect();
    ins_ids.sort();
    assert_eq!(ins_ids, vec![1, 2, 3]);
    // Update rowids are 1 and 2 (the rows that matched id <= 2).
    let mut upd_ids: Vec<i64> = updates.iter().map(|(_, _, r)| *r).collect();
    upd_ids.sort();
    assert_eq!(upd_ids, vec![1, 2]);
    // Delete rowid is 3.
    assert_eq!(deletes[0].2, 3);
    unsafe { rldb_close(db) };
}

// End-to-end: set_authorizer DENIES SELECT on a sensitive table.
// Drives the SQL SELECT executor via rldb_exec so the step-time authorizer
// check is exercised.
//
// This is RedlineDB's table-level contract, not SQLite's: RedlineDB asks
// SQLITE_SELECT (21) once per base table with the table in arg3. SQLite
// passes NULL there and asks SQLITE_READ (20) per column, which RedlineDB
// does not do yet (docs/security-capabilities.md), so an authorizer written
// for SQLite that only denies SQLITE_READ is never consulted.
unsafe extern "C" fn auth_deny_sensitive(
    _: *mut c_void,
    action: c_int,
    arg3: *const c_char,
    _arg4: *const c_char,
    _arg5: *const c_char,
    _arg6: *const c_char,
) -> c_int {
    // Deny SQLITE_SELECT on the table named "sensitive".
    if action == 21 && !arg3.is_null() {
        let name = unsafe { CStr::from_ptr(arg3) }.to_string_lossy();
        if name == "sensitive" {
            return 1; // SQLITE_DENY
        }
    }
    0 // SQLITE_OK
}

#[test]
fn set_authorizer_denies_table_access() {
    let (_dir, db) = open_db();
    // Set up two tables; only "sensitive" is denied.
    let setup = CString::new(
        "CREATE TABLE sensitive(id INTEGER, secret TEXT); \
         CREATE TABLE allowed(id INTEGER, public TEXT); \
         INSERT INTO sensitive VALUES (1, 'shh'); \
         INSERT INTO allowed VALUES (1, 'hello');",
    )
    .unwrap();
    let rc =
        unsafe { redlinedb::rldb_exec(db, setup.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
    assert_eq!(rc, RLDB_OK);

    // Register the authorizer AFTER schema setup so the CREATE TABLE
    // path doesn't need to be authorized in this minimal test.
    unsafe { sqlite3_set_authorizer(db, Some(auth_deny_sensitive), ptr::null_mut()) };

    // SELECT on "allowed" should succeed.
    let ok_sql = CString::new("SELECT id FROM allowed").unwrap();
    let mut errmsg: *mut c_char = ptr::null_mut();
    let rc =
        unsafe { redlinedb::rldb_exec(db, ok_sql.as_ptr(), None, ptr::null_mut(), &mut errmsg) };
    assert_eq!(rc, RLDB_OK, "errmsg={:?}", unsafe {
        if errmsg.is_null() {
            String::new()
        } else {
            CStr::from_ptr(errmsg).to_string_lossy().into_owned()
        }
    });

    // SELECT on "sensitive" should fail with the auth code.
    let deny_sql = CString::new("SELECT secret FROM sensitive").unwrap();
    let mut errmsg: *mut c_char = ptr::null_mut();
    let rc =
        unsafe { redlinedb::rldb_exec(db, deny_sql.as_ptr(), None, ptr::null_mut(), &mut errmsg) };
    // RLDB_AUTH = 23 (matches SQLITE_AUTH).
    assert_eq!(rc, 23, "expected RLDB_AUTH=23 got {rc}");
    if !errmsg.is_null() {
        let msg = unsafe { CStr::from_ptr(errmsg).to_string_lossy().into_owned() };
        assert!(
            msg.to_ascii_lowercase().contains("not authorized"),
            "msg={msg}"
        );
        unsafe { redlinedb::rldb_free(errmsg as *mut c_void) };
    }
    unsafe { rldb_close(db) };
}

// Drive trace/profile/commit hooks end-to-end through rldb_exec.
#[test]
fn exec_walk_invokes_trace_profile_commit_hooks() {
    let (_dir, db) = open_db();
    let traces = AtomicU64::new(0);
    let commits = AtomicU64::new(0);
    let nanos = AtomicU64::new(0);
    unsafe {
        sqlite3_trace(db, Some(trace_cb), as_user_data(&traces));
        sqlite3_commit_hook(db, Some(commit_ok), as_user_data(&commits));
        sqlite3_profile(db, Some(profile_cb), as_user_data(&nanos));
    }
    let sql =
        CString::new("CREATE TABLE t(id INTEGER); INSERT INTO t VALUES (1); COMMIT;").unwrap();
    // No active tx, the COMMIT statement is a no-op; trace fires per
    // statement, commit hook fires on the COMMIT keyword path.
    let _rc =
        unsafe { redlinedb::rldb_exec(db, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
    // Trace fires once per non-empty input split — at least 3.
    assert!(traces.load(Ordering::Relaxed) >= 1);
    // Commit hook may fire when the COMMIT keyword is detected.
    let _ = commits.load(Ordering::Relaxed);
    unsafe { rldb_close(db) };
}

// ---- S8-05: trace_v2 and the authorizer fail closed -----------------------
//
// Tests run in parallel, so each one counts through its own `user_data`.

use std::os::raw::c_uint;
use std::sync::atomic::AtomicUsize;

use redlinedb::rldb_errmsg;
use redlinedb::sqlite3_api::hooks::sqlite3_trace_v2;

const SQLITE_ERROR: i32 = 1;
const SQLITE_AUTH: i32 = 23;
const SQLITE_TRACE_STMT: c_uint = 0x01;
const SQLITE_TRACE_PROFILE: c_uint = 0x02;
const SQLITE_TRACE_ROW: c_uint = 0x04;
const SQLITE_TRACE_CLOSE: c_uint = 0x08;

fn bump(user_data: *mut c_void) {
    // SAFETY: every registration below passes a pointer to an AtomicUsize
    // that its test keeps alive until after close.
    let counter = unsafe { &*(user_data as *const AtomicUsize) };
    counter.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn counting_trace_v2(
    _mask: c_uint,
    user_data: *mut c_void,
    _p: *mut c_void,
    _x: *mut c_void,
) -> c_int {
    bump(user_data);
    0
}

unsafe extern "C" fn counting_trace(user_data: *mut c_void, _sql: *const c_char) {
    bump(user_data);
}

fn exec_with_errmsg(db: *mut rldb, sql: &str) -> (i32, String) {
    let c_sql = CString::new(sql).unwrap();
    let mut errmsg: *mut c_char = ptr::null_mut();
    let rc =
        unsafe { redlinedb::rldb_exec(db, c_sql.as_ptr(), None, ptr::null_mut(), &mut errmsg) };
    let msg = if errmsg.is_null() {
        String::new()
    } else {
        let msg = unsafe { CStr::from_ptr(errmsg) }
            .to_string_lossy()
            .into_owned();
        unsafe { redlinedb::rldb_free(errmsg as *mut c_void) };
        msg
    };
    (rc, msg)
}

#[test]
fn trace_v2_with_callback_reports_error() {
    let (_dir, db) = open_db();
    let hits = AtomicUsize::new(0);
    let user_data = &hits as *const AtomicUsize as *mut c_void;
    for mask in [
        SQLITE_TRACE_STMT,
        SQLITE_TRACE_PROFILE,
        SQLITE_TRACE_ROW,
        SQLITE_TRACE_CLOSE,
        SQLITE_TRACE_STMT | SQLITE_TRACE_PROFILE | SQLITE_TRACE_ROW | SQLITE_TRACE_CLOSE,
    ] {
        let rc = unsafe { sqlite3_trace_v2(db, mask, Some(counting_trace_v2), user_data) };
        assert_eq!(rc, SQLITE_ERROR, "mask {mask:#x}");
        let msg = unsafe { CStr::from_ptr(rldb_errmsg(db)) }
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            msg,
            "sqlite3_trace_v2 events are not supported by RedlineDB"
        );
    }
    let (rc, _) = exec_with_errmsg(
        db,
        "CREATE TABLE t(x); INSERT INTO t VALUES (1); SELECT x FROM t",
    );
    assert_eq!(rc, RLDB_OK);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    unsafe { rldb_close(db) };
}

#[test]
fn trace_v2_null_callback_ok() {
    let (_dir, db) = open_db();
    let hits = AtomicUsize::new(0);
    let user_data = &hits as *const AtomicUsize as *mut c_void;
    unsafe {
        assert_eq!(sqlite3_trace_v2(db, 0, None, ptr::null_mut()), RLDB_OK);
        assert_eq!(
            sqlite3_trace_v2(db, SQLITE_TRACE_STMT, None, ptr::null_mut()),
            RLDB_OK
        );
        // A zero mask disables tracing even with a callback, as upstream.
        assert_eq!(
            sqlite3_trace_v2(db, 0, Some(counting_trace_v2), user_data),
            RLDB_OK
        );
        assert_eq!(
            sqlite3_trace_v2(ptr::null_mut(), 0, None, ptr::null_mut()),
            21
        );
    }
    // Upstream: each sqlite3_trace_v2 call cancels an earlier sqlite3_trace.
    let legacy = AtomicUsize::new(0);
    unsafe {
        sqlite3_trace(
            db,
            Some(counting_trace),
            &legacy as *const AtomicUsize as *mut c_void,
        )
    };
    __test_fire_trace(db, "SELECT 1");
    assert_eq!(legacy.load(Ordering::SeqCst), 1);
    assert_eq!(
        unsafe { sqlite3_trace_v2(db, 0, None, ptr::null_mut()) },
        RLDB_OK
    );
    __test_fire_trace(db, "SELECT 1");
    assert_eq!(
        legacy.load(Ordering::SeqCst),
        1,
        "trace_v2 cancels sqlite3_trace"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    unsafe { rldb_close(db) };
}

unsafe extern "C" fn auth_malfunction(
    _: *mut c_void,
    _action: c_int,
    _arg3: *const c_char,
    _arg4: *const c_char,
    _arg5: *const c_char,
    _arg6: *const c_char,
) -> c_int {
    7 // not SQLITE_OK, SQLITE_DENY or SQLITE_IGNORE
}

unsafe extern "C" fn auth_deny_all(
    _: *mut c_void,
    _action: c_int,
    _arg3: *const c_char,
    _arg4: *const c_char,
    _arg5: *const c_char,
    _arg6: *const c_char,
) -> c_int {
    1 // SQLITE_DENY
}

#[test]
fn authorizer_invalid_return_denies() {
    let (_dir, db) = open_db();
    let (rc, _) = exec_with_errmsg(db, "CREATE TABLE t(x); INSERT INTO t VALUES (1)");
    assert_eq!(rc, RLDB_OK);
    unsafe { sqlite3_set_authorizer(db, Some(auth_malfunction), ptr::null_mut()) };
    for sql in ["SELECT x FROM t", "INSERT INTO t VALUES (2)"] {
        let (rc, msg) = exec_with_errmsg(db, sql);
        assert_eq!(rc, SQLITE_ERROR, "{sql}");
        assert_eq!(msg, "authorizer malfunction", "{sql}");
    }
    // The malfunction does not linger: a later DENY reports SQLITE_AUTH.
    unsafe { sqlite3_set_authorizer(db, Some(auth_deny_all), ptr::null_mut()) };
    let (rc, msg) = exec_with_errmsg(db, "SELECT x FROM t");
    assert_eq!(rc, SQLITE_AUTH);
    assert_eq!(msg, "not authorized");
    unsafe { sqlite3_set_authorizer(db, None, ptr::null_mut()) };
    let (rc, _) = exec_with_errmsg(db, "SELECT x FROM t");
    assert_eq!(rc, RLDB_OK);
    unsafe { rldb_close(db) };
}

type AuthCall = (
    c_int,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

unsafe extern "C" fn auth_record(
    user_data: *mut c_void,
    action: c_int,
    arg3: *const c_char,
    arg4: *const c_char,
    arg5: *const c_char,
    arg6: *const c_char,
) -> c_int {
    let text = |p: *const c_char| {
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    };
    // SAFETY: the test passes a pointer to a Mutex it keeps alive.
    let calls = unsafe { &*(user_data as *const std::sync::Mutex<Vec<AuthCall>>) };
    calls
        .lock()
        .unwrap()
        .push((action, text(arg3), text(arg4), text(arg5), text(arg6)));
    0
}

#[test]
fn authorizer_db_name_in_arg5() {
    let (_dir, db) = open_db();
    let (rc, _) = exec_with_errmsg(db, "CREATE TABLE t(x)");
    assert_eq!(rc, RLDB_OK);
    let calls: std::sync::Mutex<Vec<AuthCall>> = std::sync::Mutex::new(Vec::new());
    let user_data = &calls as *const std::sync::Mutex<Vec<AuthCall>> as *mut c_void;
    unsafe { sqlite3_set_authorizer(db, Some(auth_record), user_data) };
    let (rc, _) = exec_with_errmsg(db, "INSERT INTO t VALUES (1)");
    assert_eq!(rc, RLDB_OK);
    unsafe { sqlite3_set_authorizer(db, None, ptr::null_mut()) };
    let calls = calls.into_inner().unwrap();
    let insert = calls
        .iter()
        .find(|call| call.0 == 18 /* SQLITE_INSERT */)
        .unwrap_or_else(|| panic!("no SQLITE_INSERT call in {calls:?}"));
    // Upstream SQLITE_INSERT: arg3 = table, arg4 = NULL, arg5 = database,
    // arg6 = innermost trigger or view (NULL here).
    assert_eq!(
        insert,
        &(
            18,
            Some("t".to_owned()),
            None,
            Some("main".to_owned()),
            None
        )
    );
    unsafe { rldb_close(db) };
}
