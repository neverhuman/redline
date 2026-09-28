use super::*;
use crate::statement::SelectPlan;

// ============================================================
// WS-C3 R2: parallel covering-scan gate
// ============================================================

/// Outcome of the WS-C3 R2 parallel covering-scan gate. The variants
/// document *why* the gate chose one path over the other; the
/// `Dispatch` variant is the only one that would actually fan work
/// out across the rayon pool. Today no production covering pipeline
/// reaches `Dispatch` because the path walks index leaves rather
/// than heap pages — see the module note at the gate site for
/// the rationale (kept here so tests can assert the predicate's
/// per-condition behaviour without rebuilding the gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParallelCoveringDecision {
    /// All gate conditions hold AND a downstream consumer
    /// (HashAggregator / SpillSort) was detected — the executor
    /// would dispatch the parallel scan if the underlying engine
    /// were appropriate.
    Dispatch { worker_count: usize },
    /// `LIMIT` clause present — covering scans with a hard cap stay
    /// serial so the early-stop semantics survive.
    FallbackLimitPresent,
    /// `OUTER_ROW_STACK` is non-empty — a correlated outer row is
    /// in scope and a worker thread would lose access to it
    /// (the stack is thread-local).
    FallbackOuterRowStack,
    /// No rayon pool installed on the per-thread slot — operators
    /// must take the serial path until WS-C7 installs one.
    FallbackNoPool,
    /// Downstream consumer is neither HashAggregator nor SpillSort
    /// — the parallel result-ordering relaxation does not apply.
    FallbackDownstreamNotAggregator,
}

impl ParallelCoveringDecision {
    pub fn would_dispatch(self) -> bool {
        matches!(self, Self::Dispatch { .. })
    }
}

/// WS-C3 R2 gate predicate. Returns the decision the gate would
/// make for `plan` under the current thread-local context (rayon
/// pool slot + correlated-row stack). The actual heap-side
/// dispatch is [`dispatch_parallel_covering_scan`];
/// today the covering path serves results directly from the
/// index leaf chain, so even a `Dispatch` decision is honoured
/// by walking the existing serial cursor — the wiring is in
/// place for the future, the perf delta is what R1-D shipped.
pub(crate) fn decide_parallel_covering_scan(
    plan: &SelectPlan,
    limit: usize,
) -> ParallelCoveringDecision {
    if plan.limit.is_some() || limit != usize::MAX {
        return ParallelCoveringDecision::FallbackLimitPresent;
    }
    if !super::outer_row_stack_is_empty() {
        return ParallelCoveringDecision::FallbackOuterRowStack;
    }
    let pool = match super::current_rayon_pool() {
        Some(pool) => pool,
        None => return ParallelCoveringDecision::FallbackNoPool,
    };
    if !plan_downstream_is_aggregator_or_spill_sort(plan) {
        return ParallelCoveringDecision::FallbackDownstreamNotAggregator;
    }
    ParallelCoveringDecision::Dispatch {
        worker_count: pool.current_num_threads().max(1),
    }
}

/// Returns `true` when `plan`'s downstream operator (after the
/// covering scan emits rows) is a `HashAggregator` or `SpillSort`
/// — both tolerate unordered input which is what the parallel
/// scan produces. Today this is approximated by the presence of
/// `GROUP BY` / aggregate projections (→ HashAggregator). ORDER BY is
/// not included: the parallel covering scan does not sort after merge.
fn plan_downstream_is_aggregator_or_spill_sort(plan: &SelectPlan) -> bool {
    if !plan.group_by.is_empty() {
        return true;
    }
    if super::agg::select_requires_aggregation(plan) {
        return true;
    }
    // ORDER BY does not tolerate an unordered parallel covering scan.
    // A later sort would have to run after the workers merge. Until that
    // sort is on this path, an ORDER BY plan stays serial.
    false
}

thread_local! {
    /// Last `ParallelCoveringDecision` emitted on this thread, set by
    /// [`record_parallel_covering_decision`]. The SQL gate's tests read
    /// it to verify per-condition branching without intercepting the
    /// kernel scan call. Cleared lazily — readers should call
    /// [`take_last_parallel_covering_decision`] so a follow-up SELECT
    /// does not observe stale state.
    static LAST_PARALLEL_DECISION: std::cell::Cell<Option<ParallelCoveringDecision>> =
        const { std::cell::Cell::new(None) };
}

pub(crate) fn record_parallel_covering_decision(decision: ParallelCoveringDecision) {
    LAST_PARALLEL_DECISION.with(|cell| cell.set(Some(decision)));
}

/// WS-C3 R2 test hook: read and clear the most recent decision the
/// parallel covering-scan gate made on this thread. Returns `None`
/// when no covering-eligible SELECT has run since the last read.
pub fn take_last_parallel_covering_decision() -> Option<ParallelCoveringDecision> {
    LAST_PARALLEL_DECISION.with(|cell| cell.take())
}

/// WS-C3 R3-C: dispatch the heap-side parallel scan and reshape its
/// output into the same `Vec<Vec<SqlValue>>` projection that the
/// serial covering path produces. The serial path walks the index
/// leaf chain; the parallel path reads every row of
/// `table.relation_id` on the pool's workers, applies the WHERE
/// predicate post-scan, then evaluates `projection` per surviving row.
///
/// The kernel reads each row as the serial `get_for_relation` does:
/// the version `tx` sees, never a superseded or deleted one, including
/// rows on pages still only in the buffer pool.
///
/// Result ordering is intentionally NOT preserved: rows arrive in an
/// order determined by which worker drains its pages first, and the
/// WS-C3 R2 gate refuses to dispatch when an `ORDER BY` consumer
/// downstream would observe a different shape. The two downstream
/// operators that tolerate this — HashAggregator and SpillSort —
/// re-establish order from the data itself.
///
/// The `pool.install(|| ...)` wrap is what keeps the worker
/// threads bound to the database's dedicated pool; without it, the
/// kernel would still parallelise but the `std::thread::scope`
/// workers would not see the pool's affinity / NUMA hints.
#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_parallel_covering_scan(
    engine: &Engine,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    selection: &Option<Expr>,
    projection: &[SelectItem],
    bindings: &[Option<SqlValue>],
    worker_count: usize,
    pool: &Arc<rayon::ThreadPool>,
) -> Result<Vec<Vec<SqlValue>>> {
    let heap_rows =
        pool.install(|| engine.parallel_scan_relation(tx, table.relation_id, worker_count))?;

    // Decode payloads -> SqlRow, apply WHERE, project. The decoding
    // and predicate evaluation stay serial (downstream operator) —
    // the parallel speedup lives in the I/O + page-pin phase.
    let mut out: Vec<Vec<SqlValue>> = Vec::with_capacity(heap_rows.len());
    for heap_row in heap_rows {
        let Some((table_id, mut values)) = decode_sql_row(&heap_row.payload)? else {
            continue;
        };
        if table_id != table.table_id.0 {
            continue;
        }
        values = complete_short_row(table, values)?;
        let table_row = TableRow {
            rowid: heap_row.row_id,
            values,
            table: Arc::clone(table),
            alias: None,
        };
        let row = SqlRow::Table(table_row);
        if !selection_passes(selection, &row, bindings)? {
            continue;
        }
        out.push(project_row(projection, &row, bindings)?);
    }
    Ok(out)
}

/// Test hook: run a single-table `SELECT` (a projection and an optional
/// `WHERE`, nothing else) through the heap scan that the parallel
/// covering-scan gate dispatches to, on the Rayon pool installed with
/// [`super::with_current_rayon_pool`]. No SQL plan reaches that dispatch
/// today: the covering scan needs a plan without aggregation and the gate
/// dispatches only for one with it. The tests call it here so its answers
/// are held to the serial scan's. Reads in the connection's open
/// transaction when there is one, as a `SELECT` would.
#[doc(hidden)]
pub fn parallel_heap_scan_select(conn: &Arc<Connection>, sql: &str) -> Result<Vec<Vec<SqlValue>>> {
    use crate::statement::PreparedKind;

    let pool = super::current_rayon_pool().ok_or_else(|| {
        Error::UnsupportedSql("the parallel heap scan needs an installed rayon pool".to_owned())
    })?;
    let template = conn.prepare_cached(sql)?;
    let PreparedKind::Select(plan) = &template.kind else {
        return Err(Error::UnsupportedSql("not a SELECT".to_owned()));
    };
    let SelectSource::Table(table) = &plan.source else {
        return Err(Error::UnsupportedSql(
            "not a single-table SELECT".to_owned(),
        ));
    };
    if !plan.group_by.is_empty()
        || super::agg::select_requires_aggregation(plan)
        || plan.having.is_some()
        || plan.distinct
        || !plan.distinct_on.is_empty()
        || !plan.order_by.is_empty()
        || plan.limit.is_some()
        || plan.offset.is_some()
    {
        return Err(Error::UnsupportedSql(
            "the parallel heap scan takes a projection and a WHERE only".to_owned(),
        ));
    }
    let (mut tx, restore_tx) = super::select_top::begin_select_tx(conn)?;
    let rows = match tx.as_mut() {
        Some(tx_ref) => dispatch_parallel_covering_scan(
            conn.engine(),
            tx_ref,
            table,
            &plan.selection,
            &plan.projection,
            &[],
            pool.current_num_threads().max(1),
            &pool,
        ),
        None => Err(Error::UnsupportedSql(
            "no transaction to read in".to_owned(),
        )),
    };
    if let Some(owned) = tx.take_owned() {
        if restore_tx {
            conn.with_session(|session| {
                session.tx = Some(owned);
                Ok(())
            })?;
        } else {
            conn.engine().rollback(owned)?;
        }
    }
    rows
}
