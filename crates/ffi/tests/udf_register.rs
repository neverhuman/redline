//! B2 scalar UDF registration + invocation through SQL.
//!
//! Verifies that a C-callback UDF registered via `sqlite3_create_function*`
//! is dispatched when the SQL evaluator encounters that function name.

use std::ffi::{CStr, CString};
use std::os::raw::{c_int, c_void};
use std::ptr;

use redlinedb::sqlite3_api::context::RldbContext;
use redlinedb::sqlite3_api::result::{sqlite3_result_int, sqlite3_result_text};
use redlinedb::sqlite3_api::value::{RldbValue, sqlite3_value_int64};
use redlinedb::types::rldb;
use redlinedb::{rldb_close, rldb_exec, rldb_open, sqlite3_create_function_v2};
use tempfile::TempDir;

fn open_db() -> (TempDir, *mut rldb) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("udf.redline");
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    let mut db: *mut rldb = ptr::null_mut();
    let rc = unsafe { rldb_open(c_path.as_ptr(), &mut db) };
    assert_eq!(rc, 0);
    (dir, db)
}

fn exec(db: *mut rldb, sql: &str) {
    let c_sql = CString::new(sql).unwrap();
    let mut err: *mut std::os::raw::c_char = ptr::null_mut();
    let rc = unsafe { rldb_exec(db, c_sql.as_ptr(), None, ptr::null_mut(), &mut err) };
    assert_eq!(rc, 0, "exec({sql}) failed");
}

unsafe extern "C" fn times_two(ctx: *mut RldbContext, argc: c_int, argv: *mut *mut RldbValue) {
    assert_eq!(argc, 1);
    let val = unsafe { *argv };
    let n = unsafe { sqlite3_value_int64(val) };
    unsafe { sqlite3_result_int(ctx, (n * 2) as c_int) };
}

unsafe extern "C" fn echo_label(ctx: *mut RldbContext, _argc: c_int, _argv: *mut *mut RldbValue) {
    let label = CString::new("hello-udf").unwrap();
    unsafe { sqlite3_result_text(ctx, label.as_ptr(), -1, None) };
}

#[test]
fn times_two_scalar_udf_invoked_from_select() {
    let (_dir, db) = open_db();
    let name = CString::new("times_two").unwrap();
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            1,
            0,
            ptr::null_mut(),
            Some(times_two),
            None,
            None,
            None,
        )
    };
    assert_eq!(rc, 0);
    // Drive a single-row callback to read the UDF's output.
    let saw: std::sync::Arc<std::sync::Mutex<Option<i64>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let saw_ptr = std::sync::Arc::into_raw(saw.clone()) as *mut c_void;
    extern "C" fn cb(
        ctx: *mut c_void,
        ncol: c_int,
        argv: *mut *mut std::os::raw::c_char,
        _argn: *mut *mut std::os::raw::c_char,
    ) -> c_int {
        assert_eq!(ncol, 1);
        let val = unsafe { *argv };
        let cstr = unsafe { CStr::from_ptr(val) };
        let s = cstr.to_string_lossy().into_owned();
        let arc = unsafe { std::sync::Arc::from_raw(ctx as *const std::sync::Mutex<Option<i64>>) };
        *arc.lock().unwrap() = s.parse::<i64>().ok();
        let _ = std::sync::Arc::into_raw(arc);
        0
    }
    let c_sql = CString::new("SELECT times_two(21)").unwrap();
    let rc = unsafe { rldb_exec(db, c_sql.as_ptr(), Some(cb), saw_ptr, ptr::null_mut()) };
    assert_eq!(rc, 0);
    let _ = unsafe { std::sync::Arc::from_raw(saw_ptr as *const std::sync::Mutex<Option<i64>>) };
    assert_eq!(*saw.lock().unwrap(), Some(42));
    unsafe { rldb_close(db) };
}

#[test]
fn arity_any_matches_when_exact_arity_missing() {
    let (_dir, db) = open_db();
    let name = CString::new("echo_label").unwrap();
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            -1, // any arity
            0,
            ptr::null_mut(),
            Some(echo_label),
            None,
            None,
            None,
        )
    };
    assert_eq!(rc, 0);
    exec(db, "SELECT echo_label(1, 2, 3)");
    unsafe { rldb_close(db) };
}

#[test]
fn null_db_returns_misuse() {
    let name = CString::new("noop").unwrap();
    let rc = unsafe {
        sqlite3_create_function_v2(
            ptr::null_mut(),
            name.as_ptr(),
            1,
            0,
            ptr::null_mut(),
            Some(times_two),
            None,
            None,
            None,
        )
    };
    assert_eq!(rc, 21); // RLDB_MISUSE
}

// Aggregate UDF: sum_squares(x) = sum(x*x). xStep is called once per row;
// xFinal returns the accumulator. The accumulator lives in a static for
// test simplicity (per-test isolation is provided by the unique table
// name + per-test rldb_open); production aggregates would use the
// `agg_state_*` slot on RldbContext.
use std::sync::atomic::{AtomicI64, Ordering as AtomicOrdering};
static SUM_SQ_ACC: AtomicI64 = AtomicI64::new(0);

unsafe extern "C" fn sum_sq_step(_ctx: *mut RldbContext, argc: c_int, argv: *mut *mut RldbValue) {
    assert_eq!(argc, 1);
    let val = unsafe { *argv };
    let n = unsafe { sqlite3_value_int64(val) };
    SUM_SQ_ACC.fetch_add(n * n, AtomicOrdering::Relaxed);
}

unsafe extern "C" fn sum_sq_final(ctx: *mut RldbContext) {
    let total = SUM_SQ_ACC.swap(0, AtomicOrdering::Relaxed);
    unsafe { sqlite3_result_int(ctx, total as c_int) };
}

#[test]
fn aggregate_udf_sum_squares_invoked_per_group() {
    let (_dir, db) = open_db();
    let name = CString::new("sum_squares").unwrap();
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            1,
            0,
            ptr::null_mut(),
            None,
            Some(sum_sq_step),
            Some(sum_sq_final),
            None,
        )
    };
    assert_eq!(rc, 0);
    exec(db, "CREATE TABLE t(k INTEGER, v INTEGER)");
    exec(db, "INSERT INTO t VALUES (1, 2), (1, 3), (2, 4), (2, 5)");

    // Drive a multi-row callback to read aggregate output per group.
    let saw: std::sync::Arc<std::sync::Mutex<Vec<(i64, i64)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let saw_ptr = std::sync::Arc::into_raw(saw.clone()) as *mut c_void;
    extern "C" fn cb(
        ctx: *mut c_void,
        ncol: c_int,
        argv: *mut *mut std::os::raw::c_char,
        _argn: *mut *mut std::os::raw::c_char,
    ) -> c_int {
        assert_eq!(ncol, 2);
        let k = unsafe { CStr::from_ptr(*argv) }
            .to_string_lossy()
            .parse::<i64>()
            .unwrap_or(0);
        let s = unsafe { CStr::from_ptr(*argv.add(1)) }
            .to_string_lossy()
            .parse::<i64>()
            .unwrap_or(0);
        let arc =
            unsafe { std::sync::Arc::from_raw(ctx as *const std::sync::Mutex<Vec<(i64, i64)>>) };
        arc.lock().unwrap().push((k, s));
        let _ = std::sync::Arc::into_raw(arc);
        0
    }
    let c_sql = CString::new("SELECT k, sum_squares(v) FROM t GROUP BY k ORDER BY k").unwrap();
    let rc = unsafe { rldb_exec(db, c_sql.as_ptr(), Some(cb), saw_ptr, ptr::null_mut()) };
    assert_eq!(rc, 0);
    let _ =
        unsafe { std::sync::Arc::from_raw(saw_ptr as *const std::sync::Mutex<Vec<(i64, i64)>>) };
    let saw = saw.lock().unwrap();
    // Group k=1: 2^2 + 3^2 = 13. Group k=2: 4^2 + 5^2 = 41.
    assert_eq!(*saw, vec![(1, 13), (2, 41)]);
    unsafe { rldb_close(db) };
}

// ---- S8-04: a registration belongs to its connection ----------------------
// ---- S8-05: function flags RedlineDB cannot enforce are refused -----------
//
// Tests run in parallel, so every destructor counter is the test's own
// `AtomicUsize`, passed as `user_data` and kept alive until after close.

use std::sync::atomic::AtomicUsize;

use redlinedb::sqlite3_api::udf::{
    __test_function_flags, FinalFn, StepFn, sqlite3_create_window_function,
};
use redlinedb::{
    rldb_column_int64, rldb_errmsg, rldb_finalize, rldb_prepare_v2, rldb_step,
    sqlite3_create_function,
};

const SQLITE_OK: c_int = 0;
const SQLITE_ERROR: c_int = 1;
const SQLITE_MISUSE: c_int = 21;
const SQLITE_ROW: c_int = 100;
const SQLITE_UTF8: c_int = 1;
const SQLITE_DETERMINISTIC: c_int = 0x0000_0800;
const SQLITE_DIRECTONLY: c_int = 0x0008_0000;
const SQLITE_SUBTYPE: c_int = 0x0010_0000;
const SQLITE_INNOCUOUS: c_int = 0x0020_0000;
const SQLITE_RESULT_SUBTYPE: c_int = 0x0100_0000;
const SQLITE_SELFORDER1: c_int = 0x0200_0000;

unsafe extern "C" fn count_destroy(user_data: *mut c_void) {
    // SAFETY: every registration below passes a pointer to an AtomicUsize
    // that its test keeps alive until after the connection is closed.
    let counter = unsafe { &*(user_data as *const AtomicUsize) };
    counter.fetch_add(1, AtomicOrdering::SeqCst);
}

unsafe extern "C" fn window_value(_ctx: *mut RldbContext) {}

unsafe extern "C" fn window_inverse(
    _ctx: *mut RldbContext,
    _argc: c_int,
    _argv: *mut *mut RldbValue,
) {
}

fn counter() -> Box<AtomicUsize> {
    Box::new(AtomicUsize::new(0))
}

fn user_data(counter: &AtomicUsize) -> *mut c_void {
    counter as *const AtomicUsize as *mut c_void
}

fn count(counter: &AtomicUsize) -> usize {
    counter.load(AtomicOrdering::SeqCst)
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

/// Register `times_two` as `name` with a counting destructor.
fn register_scalar(db: *mut rldb, name: &str, enc: c_int, counter: &AtomicUsize) -> c_int {
    let name = CString::new(name).unwrap();
    unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            1,
            enc,
            user_data(counter),
            Some(times_two),
            None,
            None,
            Some(count_destroy),
        )
    }
}

fn try_exec(db: *mut rldb, sql: &str) -> c_int {
    let c_sql = CString::new(sql).unwrap();
    unsafe { rldb_exec(db, c_sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) }
}

fn errmsg(db: *mut rldb) -> String {
    unsafe { CStr::from_ptr(rldb_errmsg(db)) }
        .to_string_lossy()
        .into_owned()
}

/// First column of the first row, or `None` when the statement fails.
fn query_i64(db: *mut rldb, sql: &str) -> Option<i64> {
    let c_sql = CString::new(sql).unwrap();
    let mut stmt = ptr::null_mut();
    let rc = unsafe { rldb_prepare_v2(db, c_sql.as_ptr(), -1, &mut stmt, ptr::null_mut()) };
    if rc != SQLITE_OK {
        return None;
    }
    let value =
        (unsafe { rldb_step(stmt) } == SQLITE_ROW).then(|| unsafe { rldb_column_int64(stmt, 0) });
    unsafe { rldb_finalize(stmt) };
    value
}

#[test]
fn udf_destructor_runs_once_on_replace() {
    let (_dir, db) = open_db();
    let first = counter();
    let second = counter();
    assert_eq!(register_scalar(db, "twice", SQLITE_UTF8, &first), SQLITE_OK);
    assert_eq!(
        register_scalar(db, "TWICE", SQLITE_UTF8, &second),
        SQLITE_OK
    );
    assert_eq!(
        count(&first),
        1,
        "replacing a function destroys the old user_data"
    );
    assert_eq!(count(&second), 0);
    assert_eq!(query_i64(db, "SELECT twice(21)"), Some(42));
    close(db);
    assert_eq!(
        count(&first),
        1,
        "the replaced user_data is destroyed only once"
    );
    assert_eq!(count(&second), 1);
}

#[test]
fn udf_destructor_runs_on_close() {
    let (_dir, db) = open_db();
    let scalar = counter();
    let aggregate = counter();
    assert_eq!(
        register_scalar(db, "on_close", SQLITE_UTF8, &scalar),
        SQLITE_OK
    );
    let name = CString::new("agg_on_close").unwrap();
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            1,
            SQLITE_UTF8,
            user_data(&aggregate),
            None,
            Some(sum_sq_step),
            Some(sum_sq_final),
            Some(count_destroy),
        )
    };
    assert_eq!(rc, SQLITE_OK);
    assert_eq!((count(&scalar), count(&aggregate)), (0, 0));
    close(db);
    assert_eq!((count(&scalar), count(&aggregate)), (1, 1));
}

#[test]
fn udf_null_callbacks_unregister_and_destroy() {
    let (_dir, db) = open_db();
    let destroyed = counter();
    assert_eq!(
        register_scalar(db, "gone", SQLITE_UTF8, &destroyed),
        SQLITE_OK
    );
    assert_eq!(query_i64(db, "SELECT gone(4)"), Some(8));
    let name = CString::new("gone").unwrap();
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            1,
            SQLITE_UTF8,
            ptr::null_mut(),
            None,
            None,
            None,
            None,
        )
    };
    assert_eq!(rc, SQLITE_OK, "all-NULL callbacks delete the function");
    assert_eq!(count(&destroyed), 1);
    assert_ne!(try_exec(db, "SELECT gone(1)"), SQLITE_OK);
    close(db);
    assert_eq!(count(&destroyed), 1);
}

#[test]
fn udf_failed_registration_invokes_destructor() {
    let (_dir, db) = open_db();
    let destroyed = counter();
    let both = CString::new("both_fn").unwrap();
    // xFunc together with xStep is misuse, as upstream.
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            both.as_ptr(),
            1,
            SQLITE_UTF8,
            user_data(&destroyed),
            Some(times_two),
            Some(sum_sq_step),
            None,
            Some(count_destroy),
        )
    };
    assert_eq!(rc, SQLITE_MISUSE);
    assert_eq!(count(&destroyed), 1);
    // xStep without xFinal.
    let half = CString::new("half_fn").unwrap();
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            half.as_ptr(),
            1,
            SQLITE_UTF8,
            user_data(&destroyed),
            None,
            Some(sum_sq_step),
            None,
            Some(count_destroy),
        )
    };
    assert_eq!(rc, SQLITE_MISUSE);
    assert_eq!(count(&destroyed), 2);
    // A NULL name.
    let rc = unsafe {
        sqlite3_create_function_v2(
            db,
            ptr::null(),
            1,
            SQLITE_UTF8,
            user_data(&destroyed),
            Some(times_two),
            None,
            None,
            Some(count_destroy),
        )
    };
    assert_eq!(rc, SQLITE_MISUSE);
    assert_eq!(count(&destroyed), 3);
    // A NULL connection is rejected before anything is owned.
    let rc = unsafe {
        sqlite3_create_function_v2(
            ptr::null_mut(),
            both.as_ptr(),
            1,
            SQLITE_UTF8,
            user_data(&destroyed),
            Some(times_two),
            None,
            None,
            Some(count_destroy),
        )
    };
    assert_eq!(rc, SQLITE_MISUSE);
    assert_eq!(count(&destroyed), 3);
    assert_ne!(try_exec(db, "SELECT both_fn(1)"), SQLITE_OK);
    close(db);
    assert_eq!(
        count(&destroyed),
        3,
        "nothing failed registration left behind"
    );
}

#[test]
fn closed_connection_udf_not_inherited_by_reused_address() {
    // A handle's address is the registry key, and the allocator hands a
    // freed handle's address to a later open. Closing a batch and opening
    // another reuses one in about half the rounds on glibc.
    let mut reused = false;
    for _ in 0..64 {
        let destroyed = counter();
        let closed: Vec<usize> = (0..8)
            .map(|_| {
                let db = open_memory();
                assert_eq!(
                    register_scalar(db, "sentinel_fn", SQLITE_UTF8, &destroyed),
                    SQLITE_OK
                );
                db as usize
            })
            .collect();
        for &db in &closed {
            close(db as *mut rldb);
        }
        assert_eq!(
            count(&destroyed),
            8,
            "close destroys the connection's functions"
        );
        let opened: Vec<*mut rldb> = (0..8).map(|_| open_memory()).collect();
        for &db in &opened {
            if closed.contains(&(db as usize)) {
                reused = true;
                assert_ne!(
                    try_exec(db, "SELECT sentinel_fn(1)"),
                    SQLITE_OK,
                    "a new connection at a reused address inherited a closed connection's function"
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
fn two_live_connections_isolated() {
    let (_dir_a, a) = open_db();
    let (_dir_b, b) = open_db();
    let on_a = counter();
    let on_b = counter();
    assert_eq!(register_scalar(a, "iso_fn", SQLITE_UTF8, &on_a), SQLITE_OK);
    assert_ne!(
        try_exec(b, "SELECT iso_fn(1)"),
        SQLITE_OK,
        "b must not see a's function"
    );
    assert_eq!(register_scalar(b, "iso_fn", SQLITE_UTF8, &on_b), SQLITE_OK);
    assert_eq!(
        (count(&on_a), count(&on_b)),
        (0, 0),
        "b's registration does not replace a's"
    );
    assert_eq!(query_i64(a, "SELECT iso_fn(21)"), Some(42));
    close(a);
    assert_eq!((count(&on_a), count(&on_b)), (1, 0));
    assert_eq!(query_i64(b, "SELECT iso_fn(21)"), Some(42));
    close(b);
    assert_eq!((count(&on_a), count(&on_b)), (1, 1));
}

#[test]
fn directonly_registration_rejected_zero_side_effects() {
    let (_dir, db) = open_db();
    let kept = counter();
    let rejected = counter();
    assert_eq!(
        register_scalar(db, "kept_fn", SQLITE_UTF8, &kept),
        SQLITE_OK
    );

    let rc = register_scalar(db, "direct_fn", SQLITE_UTF8 | SQLITE_DIRECTONLY, &rejected);
    assert_eq!(rc, SQLITE_ERROR);
    assert_eq!(errmsg(db), "SQLITE_DIRECTONLY is not enforced by RedlineDB");
    assert_eq!(
        count(&rejected),
        1,
        "a refused registration destroys its user_data"
    );
    assert_ne!(try_exec(db, "SELECT direct_fn(1)"), SQLITE_OK);

    // Refusing a replacement leaves the existing function in place.
    let rc = register_scalar(db, "kept_fn", SQLITE_UTF8 | SQLITE_DIRECTONLY, &rejected);
    assert_eq!(rc, SQLITE_ERROR);
    assert_eq!((count(&kept), count(&rejected)), (0, 2));
    assert_eq!(query_i64(db, "SELECT kept_fn(21)"), Some(42));
    close(db);
    assert_eq!((count(&kept), count(&rejected)), (1, 2));
}

#[test]
fn deterministic_flag_accepted() {
    let (_dir, db) = open_db();
    let destroyed = counter();
    let accepted = [
        SQLITE_DETERMINISTIC,
        SQLITE_INNOCUOUS,
        SQLITE_SUBTYPE,
        SQLITE_RESULT_SUBTYPE,
        SQLITE_DETERMINISTIC | SQLITE_INNOCUOUS | SQLITE_SUBTYPE | SQLITE_RESULT_SUBTYPE,
    ];
    for (i, flags) in accepted.into_iter().enumerate() {
        let name = format!("flagged_{i}");
        let rc = register_scalar(db, &name, SQLITE_UTF8 | flags, &destroyed);
        assert_eq!(rc, SQLITE_OK, "flags {flags:#x}");
        assert_eq!(
            __test_function_flags(db, &name, 1),
            Some(flags),
            "flags are recorded"
        );
        assert_eq!(query_i64(db, &format!("SELECT {name}(21)")), Some(42));
    }
    assert_eq!(count(&destroyed), 0);
    close(db);
    assert_eq!(count(&destroyed), accepted.len());
}

#[test]
fn every_sqlite_text_encoding_accepted() {
    let (_dir, db) = open_db();
    let destroyed = counter();
    // 0 is not a named encoding; upstream registers it as UTF-8.
    for enc in 0..=5 {
        let name = format!("enc_{enc}");
        assert_eq!(
            register_scalar(db, &name, enc, &destroyed),
            SQLITE_OK,
            "enc {enc}"
        );
        assert_eq!(query_i64(db, &format!("SELECT {name}(21)")), Some(42));
    }
    close(db);
    assert_eq!(count(&destroyed), 6);
}

#[test]
fn unknown_flag_bits_rejected() {
    let (_dir, db) = open_db();
    let destroyed = counter();
    let refused = [
        SQLITE_UTF8 | 0x4000_0000,
        SQLITE_UTF8 | SQLITE_SELFORDER1,
        SQLITE_UTF8 | 0x08, // SQLITE_UTF16_ALIGNED is for collations only
        SQLITE_UTF8 | i32::MIN,
        6,
        7,
    ];
    for (i, enc) in refused.into_iter().enumerate() {
        let name = format!("unknown_{i}");
        assert_eq!(
            register_scalar(db, &name, enc, &destroyed),
            SQLITE_ERROR,
            "enc {enc:#x}"
        );
        assert_eq!(count(&destroyed), i + 1, "enc {enc:#x}");
        assert!(
            errmsg(db).contains("unsupported"),
            "enc {enc:#x}: {}",
            errmsg(db)
        );
        assert_ne!(try_exec(db, &format!("SELECT {name}(1)")), SQLITE_OK);
    }
    close(db);
    assert_eq!(count(&destroyed), refused.len());
}

#[test]
fn legacy_create_function_passes_flags_through() {
    let (_dir, db) = open_db();
    let direct = CString::new("legacy_direct").unwrap();
    let rc = unsafe {
        sqlite3_create_function(
            db,
            direct.as_ptr(),
            1,
            SQLITE_UTF8 | SQLITE_DIRECTONLY,
            ptr::null_mut(),
            Some(times_two),
            None,
            None,
        )
    };
    assert_eq!(rc, SQLITE_ERROR);
    let det = CString::new("legacy_det").unwrap();
    let rc = unsafe {
        sqlite3_create_function(
            db,
            det.as_ptr(),
            1,
            SQLITE_UTF8 | SQLITE_DETERMINISTIC,
            ptr::null_mut(),
            Some(times_two),
            None,
            None,
        )
    };
    assert_eq!(rc, SQLITE_OK);
    assert_eq!(query_i64(db, "SELECT legacy_det(21)"), Some(42));
    close(db);
}

#[test]
fn window_callbacks_rejected_and_destroyed() {
    let (_dir, db) = open_db();
    let refused = counter();
    let name = CString::new("win_fn").unwrap();
    // Either callback alone is refused too: registering it as an aggregate
    // would silently ignore it.
    let window_callbacks = [
        (Some(window_value as FinalFn), None),
        (None, Some(window_inverse as StepFn)),
        (
            Some(window_value as FinalFn),
            Some(window_inverse as StepFn),
        ),
    ];
    for (attempt, (value, inverse)) in window_callbacks.into_iter().enumerate() {
        let rc = unsafe {
            sqlite3_create_window_function(
                db,
                name.as_ptr(),
                1,
                SQLITE_UTF8,
                user_data(&refused),
                Some(sum_sq_step),
                Some(sum_sq_final),
                value,
                inverse,
                Some(count_destroy),
            )
        };
        assert_eq!(rc, SQLITE_ERROR, "attempt {attempt}");
        assert!(errmsg(db).contains("xValue"), "{}", errmsg(db));
        assert_eq!(count(&refused), attempt + 1, "attempt {attempt}: destroyed");
        assert_ne!(
            try_exec(db, "SELECT win_fn(1)"),
            0,
            "attempt {attempt}: nothing was registered"
        );
    }

    // Without xValue/xInverse the window entry point registers an aggregate.
    let aggregate = counter();
    let rc = unsafe {
        sqlite3_create_window_function(
            db,
            name.as_ptr(),
            1,
            SQLITE_UTF8,
            user_data(&aggregate),
            Some(sum_sq_step),
            Some(sum_sq_final),
            None,
            None,
            Some(count_destroy),
        )
    };
    assert_eq!(rc, SQLITE_OK);
    close(db);
    assert_eq!((count(&refused), count(&aggregate)), (3, 1));
}
