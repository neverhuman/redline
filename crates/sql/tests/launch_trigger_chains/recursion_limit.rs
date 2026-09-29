//! A statement that fails on the trigger-depth limit leaves no trigger running.

use super::*;

#[test]
fn a_recursion_limit_error_leaves_no_trigger_running() {
    let lab = Lab::new();
    lab.setup(&[
        "PRAGMA recursive_triggers = ON",
        "CREATE TABLE t(n)",
        "CREATE TABLE seen(n)",
        "CREATE TRIGGER forever AFTER INSERT ON t BEGIN INSERT INTO t VALUES (NEW.n + 1); END",
    ]);
    let (sqlite, redline) = lab.run("INSERT INTO t VALUES (1)");
    assert!(
        sqlite.is_err() && redline.is_err(),
        "{sqlite:?} {redline:?}"
    );
    lab.assert_same("SELECT count(*) FROM t");
    // After the failure the next statements start from depth zero and see
    // no trigger as already running: with recursion off, `forever` fires
    // once for this insert (it is not running yet), and the p -> q -> p
    // chain fires p_to_q, q_to_p, and stops at the running p_to_q.
    lab.setup(&[
        "PRAGMA recursive_triggers = OFF",
        "CREATE TABLE p(n)",
        "CREATE TABLE q(n)",
        "CREATE TRIGGER p_to_q AFTER INSERT ON p BEGIN INSERT INTO q VALUES (NEW.n + 1); END",
        "CREATE TRIGGER q_to_p AFTER INSERT ON q BEGIN INSERT INTO p VALUES (NEW.n + 1); END",
    ]);
    lab.run_same("INSERT INTO t VALUES (1)");
    lab.run_same("INSERT INTO p VALUES (1)");
    for table in ["t", "p", "q", "seen"] {
        lab.assert_same(&format!("SELECT n FROM {table} ORDER BY n"));
    }
}
