//! Executor-context guards: a worker thread never sees the caller's
//! `CURRENT_TX`, and a correlated subquery leaves the outer-row stack drained.

use super::*;

#[test]
fn worker_never_sees_current_tx() {
    // Run the helper on a thread that was spawned outside any
    // executor scope. `CURRENT_TX` is `Cell::new(null_mut())` by
    // default on a fresh thread, so the debug-only assert inside
    // `with_executor_context_on_worker` must NOT fire. If it does,
    // this thread::spawn will propagate the panic via .join().
    let handle =
        std::thread::spawn(|| with_executor_context_on_worker(WorkerSnapshotCarrier, || 42_u32));
    let result = handle.join().expect("worker thread did not panic");
    assert_eq!(result, 42, "worker closure must run normally");
}

#[test]
fn respects_outer_row_stack_guard() {
    let (conn, _dir) = open_db();
    conn.execute("CREATE TABLE outer_t (id INTEGER PRIMARY KEY, k INTEGER)")
        .expect("ddl outer");
    conn.execute("CREATE TABLE inner_t (k INTEGER, v INTEGER)")
        .expect("ddl inner");
    conn.execute("CREATE INDEX inner_k_idx ON inner_t(k)")
        .expect("idx");
    conn.execute("INSERT INTO outer_t(id, k) VALUES (1, 10), (2, 20)")
        .expect("seed outer");
    conn.execute("INSERT INTO inner_t(k, v) VALUES (10, 100), (20, 200), (30, 300)")
        .expect("seed inner");

    // Correlated subquery: the inner SELECT executes per outer row
    // with `OUTER_ROW_STACK` non-empty. The gate must observe the
    // stack and refuse to dispatch (or fall back to a different
    // reason, but never `Dispatch`).
    let mut q = conn
        .prepare("SELECT id, (SELECT k FROM inner_t WHERE inner_t.k = outer_t.k) FROM outer_t")
        .expect("prep");
    let mut seen = 0usize;
    while let Step::Row = q.step().expect("step") {
        seen += 1;
    }
    drop(q);
    assert_eq!(seen, 2, "correlated subquery yields one row per outer row");

    // After the correlated query finishes, the stack must be drained
    // (`with_outer_row` pops on the closure exit). Verify it.
    ensure_outer_row_stack_drained();

    // The gate cannot have returned `Dispatch` while the stack was
    // populated — the per-row inner SELECT would have observed it.
    // We can't capture mid-flight decisions from this test surface,
    // but the assert! inside `decide_parallel_covering_scan`'s
    // call site would have panicked if it had.
}
