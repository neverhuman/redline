//! The two `user_data` lifetime guarantees of function and collation
//! registrations (docs/security-capabilities.md, "Destructors run once"):
//!
//! 1. A destructor never runs while a registry lock is held, so it may call
//!    back into the library (register or delete a function or collation, or
//!    close another connection). If it ran under the lock, the re-entrant
//!    call would deadlock; these tests run such destructors on a worker
//!    thread and fail the process when it does not finish.
//! 2. A destructor never runs while a callback that uses its `user_data` is
//!    still running. A scalar function, an aggregate `xStep` and a collation
//!    compare below each replace their own registration from inside the
//!    callback and check that the old `user_data` is still alive until the
//!    callback (or the aggregate run) returns.
//!
//! A close refused with `SQLITE_BUSY` keeps the connection's registrations
//! and their `user_data`.

use std::ffi::{CStr, CString};
use std::io::Write;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use redlinedb::sqlite3_api::context::{RldbContext, sqlite3_user_data};
use redlinedb::sqlite3_api::result::sqlite3_result_int;
use redlinedb::sqlite3_api::value::RldbValue;
use redlinedb::types::rldb;
use redlinedb::{
    rldb_close, rldb_exec, rldb_open, sqlite3_create_collation_v2, sqlite3_create_function_v2,
};
use tempfile::TempDir;

const SQLITE_OK: c_int = 0;
const SQLITE_UTF8: c_int = 1;
/// Longest a destructor that re-enters the library may take.
const DEADLOCK_TIMEOUT: Duration = Duration::from_secs(30);

fn open_db(dir: &TempDir, name: &str) -> *mut rldb {
    let path = CString::new(dir.path().join(name).to_str().unwrap()).unwrap();
    let mut db: *mut rldb = ptr::null_mut();
    // SAFETY: `path` is NUL-terminated and `db` is a local out slot.
    assert_eq!(unsafe { rldb_open(path.as_ptr(), &mut db) }, SQLITE_OK);
    db
}

extern "C" fn first_value(
    out: *mut c_void,
    _argc: c_int,
    argv: *mut *mut c_char,
    _names: *mut *mut c_char,
) -> c_int {
    // SAFETY: `out` is the `Option<String>` passed by `query` below; argv
    // holds at least one entry for the duration of the call.
    let out = unsafe { &mut *(out as *mut Option<String>) };
    let value = unsafe { *argv };
    if out.is_none() && !value.is_null() {
        *out = Some(
            unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    0
}

/// Run `sql`; return its code and the first column of its first row.
fn query(db: *mut rldb, sql: &str) -> (c_int, Option<String>) {
    let sql = CString::new(sql).unwrap();
    let mut out: Option<String> = None;
    // SAFETY: `db` is live; `out` outlives the call; errmsg is not requested.
    let rc = unsafe {
        rldb_exec(
            db,
            sql.as_ptr(),
            Some(first_value),
            &mut out as *mut Option<String> as *mut c_void,
            ptr::null_mut(),
        )
    };
    (rc, out)
}

fn exec_ok(db: *mut rldb, sql: &str) {
    assert_eq!(query(db, sql).0, SQLITE_OK, "{sql}");
}

/// Run `body` on a worker thread and fail the whole process if it has not
/// finished within `DEADLOCK_TIMEOUT`: a destructor that ran under a
/// registry lock deadlocks re-locking it, and that lock would then hang
/// every later test in this binary too.
fn without_deadlock(what: &'static str, body: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        body();
        let _ = done.send(());
    });
    match finished.recv_timeout(DEADLOCK_TIMEOUT) {
        Ok(()) => worker.join().expect("worker"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // The body panicked; surface its assertion.
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Write past libtest's output capture, which exit discards.
            let _ = writeln!(
                std::io::stderr(),
                "{what}: a destructor that re-enters the library did not return; \
                 it ran under a registry lock"
            );
            std::process::exit(101); // jankurai:allow HLT-008-FALSE-GREEN-RISK reason=a-hung-worker-holds-a-registry-lock-so-the-test-must-fail-by-exiting-nonzero expires=2027-06-01
        }
    }
}

// ---- Guarantee 2: no destructor while its callback runs -----------------

/// `user_data` for a registration that replaces itself from inside its own
/// callback.
struct SelfReplacing {
    db: *mut rldb,
    /// How many times this registration's destructor ran.
    destroyed: AtomicUsize,
    /// The destructor count seen inside the callback, after the replacement.
    seen_inside: AtomicUsize,
    /// The replacement registration's return code.
    replace_rc: AtomicI32,
    replaced: AtomicBool,
}

impl SelfReplacing {
    fn new(db: *mut rldb) -> Self {
        Self {
            db,
            destroyed: AtomicUsize::new(0),
            seen_inside: AtomicUsize::new(usize::MAX),
            replace_rc: AtomicI32::new(-1),
            replaced: AtomicBool::new(false),
        }
    }

    fn as_user_data(&self) -> *mut c_void {
        self as *const Self as *mut c_void
    }

    /// Record the destructor count as seen from inside a callback.
    fn observe(&self) {
        self.seen_inside
            .store(self.destroyed.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

unsafe extern "C" fn destroy_self_replacing(user_data: *mut c_void) {
    // SAFETY: every registration with this destructor passes a
    // `SelfReplacing` its test keeps alive until the end of the test.
    let state = unsafe { &*(user_data as *const SelfReplacing) };
    state.destroyed.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn returns_two(ctx: *mut RldbContext, _: c_int, _: *mut *mut RldbValue) {
    // SAFETY: `ctx` is the live context of this call.
    unsafe { sqlite3_result_int(ctx, 2) };
}

unsafe extern "C" fn scalar_replaces_itself(
    ctx: *mut RldbContext,
    _: c_int,
    _: *mut *mut RldbValue,
) {
    // SAFETY: `ctx` is live; its user_data is the test's `SelfReplacing`.
    let state = unsafe { &*(sqlite3_user_data(ctx) as *const SelfReplacing) };
    if !state.replaced.swap(true, Ordering::SeqCst) {
        let name = c"self_replacing";
        // SAFETY: `state.db` is the live connection running this callback.
        let rc = unsafe {
            sqlite3_create_function_v2(
                state.db,
                name.as_ptr(),
                1,
                SQLITE_UTF8,
                ptr::null_mut(),
                Some(returns_two),
                None,
                None,
                None,
            )
        };
        state.replace_rc.store(rc, Ordering::SeqCst);
        // The old registration is gone, but this call still holds its
        // user_data, so its destructor must not have run yet.
        state.observe();
    }
    // SAFETY: `ctx` is the live context of this call.
    unsafe { sqlite3_result_int(ctx, 1) };
}

#[test]
fn scalar_replacing_itself_keeps_user_data_until_the_call_returns() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(&dir, "scalar.redline");
    let state = SelfReplacing::new(db);
    let name = c"self_replacing";
    // SAFETY: `db` is live; `state` outlives the registration.
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            1,
            SQLITE_UTF8,
            state.as_user_data(),
            Some(scalar_replaces_itself),
            None,
            None,
            Some(destroy_self_replacing),
        )
    };
    assert_eq!(rc, SQLITE_OK);
    assert_eq!(
        query(db, "SELECT self_replacing(0)"),
        (SQLITE_OK, Some("1".into()))
    );
    assert_eq!(state.replace_rc.load(Ordering::SeqCst), SQLITE_OK);
    assert_eq!(
        state.seen_inside.load(Ordering::SeqCst),
        0,
        "the destructor ran while the callback using its user_data was running"
    );
    assert_eq!(
        state.destroyed.load(Ordering::SeqCst),
        1,
        "released once, after the call"
    );
    // The replacement answers from now on.
    assert_eq!(
        query(db, "SELECT self_replacing(0)"),
        (SQLITE_OK, Some("2".into()))
    );
    // SAFETY: `db` is live and closed exactly once.
    assert_eq!(unsafe { rldb_close(db) }, SQLITE_OK);
    assert_eq!(state.destroyed.load(Ordering::SeqCst), 1);
}

unsafe extern "C" fn step_replaces_itself(ctx: *mut RldbContext, _: c_int, _: *mut *mut RldbValue) {
    // SAFETY: `ctx` is live; its user_data is the test's `SelfReplacing`.
    let state = unsafe { &*(sqlite3_user_data(ctx) as *const SelfReplacing) };
    if !state.replaced.swap(true, Ordering::SeqCst) {
        let name = c"self_replacing_agg";
        // SAFETY: `state.db` is the live connection running this callback.
        let rc = unsafe {
            sqlite3_create_function_v2(
                state.db,
                name.as_ptr(),
                1,
                SQLITE_UTF8,
                ptr::null_mut(),
                None,
                Some(noop_step),
                Some(final_two),
                None,
            )
        };
        state.replace_rc.store(rc, Ordering::SeqCst);
    }
    state.observe();
}

unsafe extern "C" fn final_seen(ctx: *mut RldbContext) {
    // SAFETY: `ctx` is live; its user_data is the test's `SelfReplacing`.
    let state = unsafe { &*(sqlite3_user_data(ctx) as *const SelfReplacing) };
    // xFinal of the same run still uses the old user_data.
    let destroyed = state.destroyed.load(Ordering::SeqCst);
    let seen = state.seen_inside.load(Ordering::SeqCst);
    state
        .seen_inside
        .store(seen.max(destroyed), Ordering::SeqCst);
    // SAFETY: `ctx` is the live context of this call.
    unsafe { sqlite3_result_int(ctx, 1) };
}

unsafe extern "C" fn noop_step(_: *mut RldbContext, _: c_int, _: *mut *mut RldbValue) {}

unsafe extern "C" fn final_two(ctx: *mut RldbContext) {
    // SAFETY: `ctx` is the live context of this call.
    unsafe { sqlite3_result_int(ctx, 2) };
}

#[test]
fn aggregate_replacing_itself_keeps_user_data_until_the_run_ends() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(&dir, "aggregate.redline");
    exec_ok(db, "CREATE TABLE t(x); INSERT INTO t VALUES (1), (2), (3)");
    let state = SelfReplacing::new(db);
    let name = c"self_replacing_agg";
    // SAFETY: `db` is live; `state` outlives the registration.
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            1,
            SQLITE_UTF8,
            state.as_user_data(),
            None,
            Some(step_replaces_itself),
            Some(final_seen),
            Some(destroy_self_replacing),
        )
    };
    assert_eq!(rc, SQLITE_OK);
    assert_eq!(
        query(db, "SELECT self_replacing_agg(x) FROM t"),
        (SQLITE_OK, Some("1".into()))
    );
    assert_eq!(state.replace_rc.load(Ordering::SeqCst), SQLITE_OK);
    assert_eq!(
        state.seen_inside.load(Ordering::SeqCst),
        0,
        "the destructor ran during the xStep/xFinal run using its user_data"
    );
    assert_eq!(state.destroyed.load(Ordering::SeqCst), 1);
    assert_eq!(
        query(db, "SELECT self_replacing_agg(x) FROM t"),
        (SQLITE_OK, Some("2".into()))
    );
    // SAFETY: `db` is live and closed exactly once.
    assert_eq!(unsafe { rldb_close(db) }, SQLITE_OK);
    assert_eq!(state.destroyed.load(Ordering::SeqCst), 1);
}

unsafe extern "C" fn plain_compare(
    _: *mut c_void,
    na: c_int,
    a: *const c_void,
    nb: c_int,
    b: *const c_void,
) -> c_int {
    // SAFETY: the library passes `na`/`nb` readable bytes at `a`/`b`.
    let (a, b) = unsafe {
        (
            std::slice::from_raw_parts(a as *const u8, na as usize),
            std::slice::from_raw_parts(b as *const u8, nb as usize),
        )
    };
    a.cmp(b) as c_int
}

unsafe extern "C" fn compare_replaces_itself(
    user_data: *mut c_void,
    na: c_int,
    a: *const c_void,
    nb: c_int,
    b: *const c_void,
) -> c_int {
    // SAFETY: the registration passes the test's `SelfReplacing`.
    let state = unsafe { &*(user_data as *const SelfReplacing) };
    if !state.replaced.swap(true, Ordering::SeqCst) {
        let name = c"self_replacing_coll";
        // SAFETY: `state.db` is the live connection running this callback.
        let rc = unsafe {
            sqlite3_create_collation_v2(
                state.db,
                name.as_ptr(),
                SQLITE_UTF8,
                ptr::null_mut(),
                Some(plain_compare),
                None,
            )
        };
        state.replace_rc.store(rc, Ordering::SeqCst);
        state.observe();
    }
    // SAFETY: forwarded unchanged from this call's arguments.
    unsafe { plain_compare(ptr::null_mut(), na, a, nb, b) }
}

#[test]
fn collation_replacing_itself_keeps_user_data_until_the_compare_returns() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(&dir, "collation.redline");
    exec_ok(
        db,
        "CREATE TABLE t(s TEXT); INSERT INTO t VALUES ('b'), ('c'), ('a')",
    );
    let state = SelfReplacing::new(db);
    let name = c"self_replacing_coll";
    // SAFETY: `db` is live; `state` outlives the registration.
    let rc = unsafe {
        sqlite3_create_collation_v2(
            db,
            name.as_ptr(),
            SQLITE_UTF8,
            state.as_user_data(),
            Some(compare_replaces_itself),
            Some(destroy_self_replacing),
        )
    };
    assert_eq!(rc, SQLITE_OK);
    assert_eq!(
        query(db, "SELECT s FROM t ORDER BY s COLLATE self_replacing_coll"),
        (SQLITE_OK, Some("a".into()))
    );
    assert_eq!(state.replace_rc.load(Ordering::SeqCst), SQLITE_OK);
    assert_eq!(
        state.seen_inside.load(Ordering::SeqCst),
        0,
        "the destructor ran while the compare using its user_data was running"
    );
    assert_eq!(state.destroyed.load(Ordering::SeqCst), 1);
    // SAFETY: `db` is live and closed exactly once.
    assert_eq!(unsafe { rldb_close(db) }, SQLITE_OK);
    assert_eq!(state.destroyed.load(Ordering::SeqCst), 1);
}

// ---- Guarantee 1: no destructor under a registry lock -------------------

/// `user_data` whose destructor registers a function and a collation on a
/// spare connection, which takes both registry locks.
struct ReEntrant {
    spare: *mut rldb,
    destroyed: AtomicUsize,
    /// OR of every return code the destructor's registrations got.
    rc: AtomicI32,
}

unsafe extern "C" fn destroy_re_entrant(user_data: *mut c_void) {
    // SAFETY: every registration with this destructor passes a `ReEntrant`
    // its test keeps alive, whose spare connection outlives it.
    let state = unsafe { &*(user_data as *const ReEntrant) };
    let function = c"from_destructor";
    let collation = c"from_destructor_coll";
    // SAFETY: `state.spare` is a live connection.
    let rc = unsafe {
        sqlite3_create_function_v2(
            state.spare,
            function.as_ptr(),
            0,
            SQLITE_UTF8,
            ptr::null_mut(),
            Some(returns_two),
            None,
            None,
            None,
        ) | sqlite3_create_collation_v2(
            state.spare,
            collation.as_ptr(),
            SQLITE_UTF8,
            ptr::null_mut(),
            Some(plain_compare),
            None,
        )
    };
    state.rc.fetch_or(rc, Ordering::SeqCst);
    state.destroyed.fetch_add(1, Ordering::SeqCst);
}

/// Raw handles cross to the worker thread as addresses.
#[derive(Clone, Copy)]
struct Handles {
    db: usize,
    state: usize,
}

fn register_function(db: *mut rldb, name: &CStr, state: *mut c_void) -> c_int {
    // SAFETY: `db` is live; `state` is a `ReEntrant` the test keeps alive.
    unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            0,
            SQLITE_UTF8,
            state,
            Some(returns_two),
            None,
            None,
            Some(destroy_re_entrant),
        )
    }
}

fn register_collation(db: *mut rldb, name: &CStr, state: *mut c_void) -> c_int {
    // SAFETY: `db` is live; `state` is a `ReEntrant` the test keeps alive.
    unsafe {
        sqlite3_create_collation_v2(
            db,
            name.as_ptr(),
            SQLITE_UTF8,
            state,
            Some(plain_compare),
            Some(destroy_re_entrant),
        )
    }
}

#[test]
fn destructors_may_re_enter_the_library_on_replace_delete_and_close() {
    let dir = tempfile::tempdir().unwrap();
    let spare = open_db(&dir, "spare.redline");
    let db = open_db(&dir, "main.redline");
    let state = Box::new(ReEntrant {
        spare,
        destroyed: AtomicUsize::new(0),
        rc: AtomicI32::new(SQLITE_OK),
    });
    let handles = Handles {
        db: db as usize,
        state: &*state as *const ReEntrant as usize,
    };
    without_deadlock("replace, delete and close", move || {
        let db = handles.db as *mut rldb;
        let user_data = handles.state as *mut c_void;
        // SAFETY: the test keeps the `ReEntrant` alive until after join.
        let state = unsafe { &*(handles.state as *const ReEntrant) };
        let destroyed = || state.destroyed.load(Ordering::SeqCst);
        // Function: replace, then delete (every callback NULL).
        assert_eq!(register_function(db, c"victim", user_data), SQLITE_OK);
        assert_eq!(register_function(db, c"victim", user_data), SQLITE_OK);
        assert_eq!(destroyed(), 1, "replacing a function runs its destructor");
        // SAFETY: `db` is live; NULL callbacks delete the registration.
        let rc = unsafe {
            sqlite3_create_function_v2(
                db,
                c"victim".as_ptr(),
                0,
                SQLITE_UTF8,
                ptr::null_mut(),
                None,
                None,
                None,
                None,
            )
        };
        assert_eq!(rc, SQLITE_OK);
        assert_eq!(destroyed(), 2, "deleting a function runs its destructor");
        // Collation: replace, then delete (compare NULL).
        assert_eq!(register_collation(db, c"victim_coll", user_data), SQLITE_OK);
        assert_eq!(register_collation(db, c"victim_coll", user_data), SQLITE_OK);
        assert_eq!(destroyed(), 3, "replacing a collation runs its destructor");
        // SAFETY: `db` is live; a NULL compare deletes the registration.
        let rc = unsafe {
            sqlite3_create_collation_v2(
                db,
                c"victim_coll".as_ptr(),
                SQLITE_UTF8,
                ptr::null_mut(),
                None,
                None,
            )
        };
        assert_eq!(rc, SQLITE_OK);
        assert_eq!(destroyed(), 4, "deleting a collation runs its destructor");
        // Close releases both registries' entries of this connection.
        assert_eq!(register_function(db, c"victim", user_data), SQLITE_OK);
        assert_eq!(register_collation(db, c"victim_coll", user_data), SQLITE_OK);
        // SAFETY: `db` is live and closed exactly once.
        assert_eq!(unsafe { rldb_close(db) }, SQLITE_OK);
        assert_eq!(destroyed(), 6, "close runs both destructors");
    });
    assert_eq!(state.rc.load(Ordering::SeqCst), SQLITE_OK);
    // What the destructors registered on the spare connection works.
    exec_ok(
        spare,
        "CREATE TABLE t(s TEXT); INSERT INTO t VALUES ('y'), ('x')",
    );
    assert_eq!(
        query(spare, "SELECT from_destructor()"),
        (SQLITE_OK, Some("2".into()))
    );
    assert_eq!(
        query(
            spare,
            "SELECT s FROM t ORDER BY s COLLATE from_destructor_coll"
        ),
        (SQLITE_OK, Some("x".into()))
    );
    // SAFETY: `spare` is live and closed exactly once.
    assert_eq!(unsafe { rldb_close(spare) }, SQLITE_OK);
}

#[test]
fn a_close_refused_as_busy_keeps_registrations_and_user_data() {
    use redlinedb::rldb_finalize;
    use redlinedb::rldb_prepare_v2;
    use redlinedb::types::rldb_stmt;

    const SQLITE_BUSY: c_int = 5;
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(&dir, "busy.redline");
    let function = SelfReplacing::new(db);
    let collation = SelfReplacing::new(db);
    // SAFETY: `db` is live; both states outlive their registrations.
    unsafe {
        assert_eq!(
            sqlite3_create_function_v2(
                db,
                c"kept".as_ptr(),
                0,
                SQLITE_UTF8,
                function.as_user_data(),
                Some(returns_two),
                None,
                None,
                Some(destroy_self_replacing),
            ),
            SQLITE_OK
        );
        assert_eq!(
            sqlite3_create_collation_v2(
                db,
                c"kept_coll".as_ptr(),
                SQLITE_UTF8,
                collation.as_user_data(),
                Some(plain_compare),
                Some(destroy_self_replacing),
            ),
            SQLITE_OK
        );
    }
    let mut pending: *mut rldb_stmt = ptr::null_mut();
    // SAFETY: `db` is live; `pending` is a local slot finalized below.
    let rc =
        unsafe { rldb_prepare_v2(db, c"SELECT 1".as_ptr(), -1, &mut pending, ptr::null_mut()) };
    assert_eq!(rc, SQLITE_OK);
    // SAFETY: `db` is live; the close is refused, so it stays live.
    assert_eq!(unsafe { rldb_close(db) }, SQLITE_BUSY);
    assert_eq!(function.destroyed.load(Ordering::SeqCst), 0);
    assert_eq!(collation.destroyed.load(Ordering::SeqCst), 0);
    // Both registrations still work on the open connection.
    assert_eq!(query(db, "SELECT kept()"), (SQLITE_OK, Some("2".into())));
    exec_ok(
        db,
        "CREATE TABLE t(s TEXT); INSERT INTO t VALUES ('b'), ('a')",
    );
    assert_eq!(
        query(db, "SELECT s FROM t ORDER BY s COLLATE kept_coll"),
        (SQLITE_OK, Some("a".into()))
    );
    // SAFETY: `pending` is live and finalized once; then `db` closes once.
    unsafe {
        assert_eq!(rldb_finalize(pending), SQLITE_OK);
        assert_eq!(rldb_close(db), SQLITE_OK);
    }
    assert_eq!(function.destroyed.load(Ordering::SeqCst), 1);
    assert_eq!(collation.destroyed.load(Ordering::SeqCst), 1);
}
