//! `SQLITE_UPDATE`: asked once per assigned column with its name in `arg4`,
//! and `SQLITE_IGNORE` leaves only that column unchanged.

use super::*;

// ---- SQLITE_UPDATE -------------------------------------------------------

type Call = (c_int, Option<String>, Option<String>, Option<String>);

/// Records every call, and answers SQLITE_UPDATE on column `column` with
/// `decision` (every other call is allowed).
struct UpdatePolicy {
    column: &'static str,
    decision: c_int,
    calls: Mutex<Vec<Call>>,
}

unsafe extern "C" fn update_policy(
    user_data: *mut c_void,
    action: c_int,
    arg3: *const c_char,
    arg4: *const c_char,
    arg5: *const c_char,
    _: *const c_char,
) -> c_int {
    // SAFETY: each test passes a pointer to an UpdatePolicy it keeps alive
    // until it clears the authorizer.
    let policy = unsafe { &*(user_data as *const UpdatePolicy) };
    let column = text(arg4);
    policy
        .calls
        .lock()
        .unwrap()
        .push((action, text(arg3), column.clone(), text(arg5)));
    if action == SQLITE_UPDATE && column.as_deref() == Some(policy.column) {
        policy.decision
    } else {
        SQLITE_OK
    }
}

fn with_policy(db: *mut rldb, policy: &UpdatePolicy, sql: &str) -> (c_int, String, Vec<Call>) {
    let user_data = policy as *const UpdatePolicy as *mut c_void;
    // SAFETY: `db` is live; `policy` outlives the registration, which is
    // cleared before this function returns.
    unsafe { sqlite3_set_authorizer(db, Some(update_policy), user_data) };
    let (rc, message) = exec(db, sql);
    unsafe { sqlite3_set_authorizer(db, None, ptr::null_mut()) };
    let calls = std::mem::take(&mut *policy.calls.lock().unwrap());
    (rc, message, calls)
}

fn rows(db: *mut rldb) -> Vec<String> {
    prepare_and_step(db, "SELECT a || '/' || b || '/' || c FROM t ORDER BY a").1
}

#[test]
fn update_asks_for_each_assigned_column_by_name() {
    let (_dir, db) = open_db();
    assert_eq!(
        exec(
            db,
            "CREATE TABLE t(a, b, c); INSERT INTO t VALUES (1, 2, 3)"
        )
        .0,
        SQLITE_OK
    );
    let policy = UpdatePolicy {
        column: "",
        decision: SQLITE_OK,
        calls: Mutex::new(Vec::new()),
    };
    let (rc, _, calls) = with_policy(db, &policy, "UPDATE t SET c = 30, a = 10");
    assert_eq!(rc, SQLITE_OK);
    let updates: Vec<&Call> = calls
        .iter()
        .filter(|call| call.0 == SQLITE_UPDATE)
        .collect();
    // Upstream: one SQLITE_UPDATE per SET column, in SET order, with the
    // table in arg3, the column in arg4 and the database in arg5.
    let t = || Some("t".to_owned());
    let main = || Some("main".to_owned());
    assert_eq!(
        updates,
        [
            &(SQLITE_UPDATE, t(), Some("c".to_owned()), main()),
            &(SQLITE_UPDATE, t(), Some("a".to_owned()), main()),
        ],
        "all calls: {calls:?}"
    );
    assert_eq!(rows(db), ["10/2/30"]);
    // SAFETY: `db` is live and closed exactly once.
    unsafe { rldb_close(db) };
}

#[test]
fn update_deny_on_one_column_fails_the_statement() {
    let (_dir, db) = open_db();
    assert_eq!(
        exec(
            db,
            "CREATE TABLE t(a, b, c); INSERT INTO t VALUES (1, 2, 3)"
        )
        .0,
        SQLITE_OK
    );
    let policy = UpdatePolicy {
        column: "b",
        decision: SQLITE_DENY,
        calls: Mutex::new(Vec::new()),
    };
    // A statement that does not assign b is allowed.
    let (rc, _, _) = with_policy(db, &policy, "UPDATE t SET a = 5");
    assert_eq!(rc, SQLITE_OK);
    let (rc, message, _) = with_policy(db, &policy, "UPDATE t SET a = 6, b = 7");
    assert_eq!((rc, message.as_str()), (SQLITE_AUTH, "not authorized"));
    assert_eq!(rows(db), ["5/2/3"], "a denied UPDATE changes nothing");
    // SAFETY: `db` is live and closed exactly once.
    unsafe { rldb_close(db) };
}

#[test]
fn update_ignore_on_one_column_leaves_only_that_column_unchanged() {
    let (_dir, db) = open_db();
    assert_eq!(
        exec(
            db,
            "CREATE TABLE t(a, b, c); INSERT INTO t VALUES (1, 2, 3), (4, 5, 6)"
        )
        .0,
        SQLITE_OK
    );
    let policy = UpdatePolicy {
        column: "b",
        decision: SQLITE_IGNORE,
        calls: Mutex::new(Vec::new()),
    };
    let (rc, _, _) = with_policy(db, &policy, "UPDATE t SET b = b + 100, c = c * 10");
    assert_eq!(rc, SQLITE_OK);
    assert_eq!(rows(db), ["1/2/30", "4/5/60"], "b is ignored, c is updated");
    // With every assigned column ignored, nothing changes.
    let (rc, _, _) = with_policy(db, &policy, "UPDATE t SET b = 0");
    assert_eq!(rc, SQLITE_OK);
    assert_eq!(rows(db), ["1/2/30", "4/5/60"]);
    // SAFETY: `db` is live and closed exactly once.
    unsafe { rldb_close(db) };
}
