use super::*;

#[derive(Debug, Clone)]
struct UniqueConflict {
    rowid: RowId,
    constraint_name: Option<Arc<str>>,
    key_ordinals: Vec<usize>,
    /// The index name when a key is an expression, which sqlite3 reports
    /// instead of columns (`UNIQUE constraint failed: index 'i'`).
    expression_index: Option<Arc<str>>,
}

/// The UNIQUE violation sqlite3 reports for the first of `conflicts`.
fn unique_error(table: &TableDef, conflicts: &[UniqueConflict]) -> Error {
    match conflicts.first() {
        Some(conflict) => crate::sqlite_errors::unique_failed(
            table,
            &conflict.key_ordinals,
            conflict.expression_index.as_deref(),
        ),
        None => Error::ConstraintViolation(format!("UNIQUE constraint failed: {}", table.name)),
    }
}

fn expression_index_name(index: &redlinedb_kernel::catalog::IndexDef) -> Option<Arc<str>> {
    index
        .keys
        .iter()
        .any(|key| {
            matches!(
                key.source,
                redlinedb_kernel::catalog::IndexKeySource::Expression { .. }
            )
        })
        .then(|| Arc::from(index.name.as_ref()))
}

struct UpsertUpdateContext<'a> {
    conn: &'a Connection,
    session: &'a mut SessionState,
    tx: &'a mut Txn,
    table: &'a Arc<TableDef>,
    excluded: &'a [SqlValue],
    conflict: &'a UniqueConflict,
    bindings: &'a [Option<SqlValue>],
}

#[derive(Debug)]
pub(in crate::exec) enum InsertOutcome {
    Inserted { rowid: RowId, values: Vec<SqlValue> },
    Updated { rowid: RowId, values: Vec<SqlValue> },
    Ignored,
}

fn collect_unique_conflicts(
    conn: &Connection,
    session: &mut SessionState,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    values: &[SqlValue],
    skip_rowid: Option<RowId>,
) -> Result<Vec<UniqueConflict>> {
    // Lane B: the physical-index probe replaces the O(N) heap scan when the
    // index has been allocated by Lane A. The default path (no
    // `meta_page_id`) preserves the original O(N) behavior so databases
    // without physical indexes still enforce UNIQUE.
    //
    // SQLite enforces UNIQUE on every unique index — both inline UNIQUE
    // constraints (which create a backing index AND a Constraint row) and
    // standalone `CREATE UNIQUE INDEX` statements (which only create the
    // index). We therefore enumerate `table.indexes` and only fall back to
    // the constraints list to recover the original constraint name when a
    // matching one exists.
    let mut conflicts = Vec::new();
    let mut pending_indexes: Vec<&redlinedb_kernel::catalog::IndexDef> = Vec::new();
    for index in &table.indexes {
        if !index.unique && !index.primary {
            continue;
        }
        if let Some(predicate) = index.predicate_sql.as_deref()
            && !index_predicate::eval_index_predicate(table, predicate, values)?
        {
            continue;
        }
        let constraint_name = match table
            .constraints
            .iter()
            .find(|c| {
                (c.kind == ConstraintKind::Unique || c.kind == ConstraintKind::PrimaryKey)
                    && c.index_id == Some(index.index_id)
            })
            .and_then(|c| c.name.as_deref().map(Arc::<str>::from))
        {
            Some(name) => Some(name),
            None => Some(Arc::from(index.name.as_ref())),
        };
        // SQLite NULL parity: a NULL anywhere in the unique-key tuple
        // disables the conflict check entirely. Build through the same
        // SQL-side key adapter used by DML maintenance so expression-source
        // indexes evaluate their stored key SQL before conflict probing.
        let built = crate::exec::index_dml::build_index_key_with_values(table, index, values)?;
        if built.key.contains_null {
            continue;
        }
        if let Some(handle) =
            crate::exec::index_dml::open_index_handle_for_tx(conn.engine(), tx, index)
        {
            let (kernel_guard, hit) = crate::exec::index_dml::probe_unique_for_conflict(
                conn.engine(),
                &handle,
                tx,
                skip_rowid,
                &built.key,
            )?;
            if let Some(rowid) = hit {
                conflicts.push(UniqueConflict {
                    rowid,
                    constraint_name: constraint_name.clone(),
                    key_ordinals: index.keys.iter().map(|key| key.ordinal as usize).collect(),
                    expression_index: expression_index_name(index),
                });
            }
            // Hold the kernel `UniqueKeyGuard` until end-of-transaction so
            // the probe-to-insert window stays serialized — dropping the
            // guard between `point_lookup` and `insert_tx` reopens the race
            // where two writers both see "no duplicate" and both commit.
            // The guard lives in `SessionState::kernel_unique_guards` and
            // releases on commit/rollback when that vector is cleared.
            session.kernel_unique_guards.push(kernel_guard);

            // We still take a SQL-side guard so callers that share
            // `unique_locks()` continue to serialize against this key. The
            // dual locking is harmless and matches the default path below;
            // the SQL guard is also released on commit/rollback.
            let sql_lock_key = unique_key_bytes(table.table_id.0, index.index_id.0, &built.values)?;
            let sql_guard = conn.unique_locks().lock(sql_lock_key, tx.id().0)?;
            session.unique_guards.push(sql_guard);
            continue;
        }
        pending_indexes.push(index);
    }

    // O(N) heap scan for any indexes Lane A did not allocate yet.
    if pending_indexes.is_empty() {
        return Ok(conflicts);
    }
    if let Some(candidate) = rowid_primary_candidate(table, &pending_indexes, values) {
        return rowid_primary_conflicts(
            conn,
            session,
            tx,
            table,
            values,
            &pending_indexes,
            candidate,
            skip_rowid,
            conflicts,
        );
    }
    if let Some(ordinals) = plain_key_ordinals(table, &pending_indexes) {
        return column_key_conflicts(
            conn,
            session,
            tx,
            table,
            values,
            &pending_indexes,
            &ordinals,
            skip_rowid,
            conflicts,
        );
    }
    let rows = collect_table_rows(conn.engine(), tx, table)?;
    for index in pending_indexes {
        let built = crate::exec::index_dml::build_index_key_with_values(table, index, values)?;
        if built.key.contains_null {
            continue;
        }
        let lock_key = unique_key_bytes(table.table_id.0, index.index_id.0, &built.values)?;
        let guard = conn.unique_locks().lock(lock_key, tx.id().0)?;
        session.unique_guards.push(guard);
        for row in &rows {
            if skip_rowid == Some(row.rowid) {
                continue;
            }
            if let Some(predicate) = index.predicate_sql.as_deref()
                && !index_predicate::eval_index_predicate(table, predicate, &row.values)?
            {
                continue;
            }
            let other =
                crate::exec::index_dml::build_index_key_with_values(table, index, &row.values)?;
            if key_values_equal(&built.values, &other.values) {
                let constraint_name = match table
                    .constraints
                    .iter()
                    .find(|c| c.index_id == Some(index.index_id))
                    .and_then(|c| c.name.as_deref().map(Arc::<str>::from))
                {
                    Some(name) => Some(name),
                    None => Some(Arc::from(index.name.as_ref())),
                };
                conflicts.push(UniqueConflict {
                    rowid: row.rowid,
                    constraint_name,
                    key_ordinals: index.keys.iter().map(|key| key.ordinal as usize).collect(),
                    expression_index: expression_index_name(index),
                });
                break;
            }
        }
    }
    Ok(conflicts)
}

/// Stored columns that every pending key reads, or `None` when a key needs
/// more than the stored bytes: a partial-index predicate, an expression,
/// or a VIRTUAL generated column (the heap stores NULL there and only the
/// full row load computes it).
fn plain_key_ordinals(
    table: &TableDef,
    indexes: &[&redlinedb_kernel::catalog::IndexDef],
) -> Option<Vec<usize>> {
    let mut ordinals = Vec::new();
    for index in indexes {
        if index.predicate_sql.is_some() {
            return None;
        }
        for key in &index.keys {
            let redlinedb_kernel::catalog::IndexKeySource::Column { attnum } = &key.source else {
                return None;
            };
            let virtual_column = table.columns.get(*attnum as usize).is_some_and(|column| {
                column.generated.as_ref().is_some_and(|generated| {
                    matches!(
                        generated.kind,
                        redlinedb_kernel::catalog::GeneratedColumnKind::Virtual
                    )
                })
            });
            if virtual_column {
                return None;
            }
            ordinals.push(*attnum as usize);
        }
    }
    ordinals.sort_unstable();
    ordinals.dedup();
    Some(ordinals)
}

#[allow(clippy::too_many_arguments)]
fn column_key_conflicts(
    conn: &Connection,
    session: &mut SessionState,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    values: &[SqlValue],
    indexes: &[&redlinedb_kernel::catalog::IndexDef],
    ordinals: &[usize],
    skip_rowid: Option<RowId>,
    mut conflicts: Vec<UniqueConflict>,
) -> Result<Vec<UniqueConflict>> {
    let mut scan = conn.engine().relation_rowids(table.relation_id)?;
    scan.sort();
    let mut projected = Vec::with_capacity(scan.len());
    for rowid in scan {
        let Some(payload) = conn
            .engine()
            .get_for_relation(tx, table.relation_id, rowid)?
        else {
            continue;
        };
        match decode_sql_key_columns(&payload, ordinals, table.columns.len())? {
            Some((table_id, row_values)) => {
                if table_id == table.table_id.0 {
                    projected.push((rowid, row_values));
                }
            }
            // The row predates a key column; load it whole so that column
            // reads as its DEFAULT, not NULL.
            None => {
                if let Some(row) = load_table_row_by_rowid(conn.engine(), tx, table, rowid)? {
                    projected.push((rowid, row.values));
                }
            }
        }
    }
    for index in indexes {
        let built = crate::exec::index_dml::build_index_key_with_values(table, index, values)?;
        if built.key.contains_null {
            continue;
        }
        let lock_key = unique_key_bytes(table.table_id.0, index.index_id.0, &built.values)?;
        let guard = conn.unique_locks().lock(lock_key, tx.id().0)?;
        session.unique_guards.push(guard);
        for (rowid, row_values) in &projected {
            if skip_rowid == Some(*rowid) {
                continue;
            }
            let other =
                crate::exec::index_dml::build_index_key_with_values(table, index, row_values)?;
            if key_values_equal(&built.values, &other.values) {
                let constraint_name = match table
                    .constraints
                    .iter()
                    .find(|c| c.index_id == Some(index.index_id))
                    .and_then(|c| c.name.as_deref().map(Arc::<str>::from))
                {
                    Some(name) => Some(name),
                    None => Some(Arc::from(index.name.as_ref())),
                };
                conflicts.push(UniqueConflict {
                    rowid: *rowid,
                    constraint_name,
                    key_ordinals: index.keys.iter().map(|key| key.ordinal as usize).collect(),
                    expression_index: expression_index_name(index),
                });
                break;
            }
        }
    }
    Ok(conflicts)
}

fn is_rowid_primary_key(table: &TableDef, index: &redlinedb_kernel::catalog::IndexDef) -> bool {
    let Some(alias) = table.rowid_alias_column else {
        return false;
    };
    if !index.primary || index.predicate_sql.is_some() || index.keys.len() != 1 {
        return false;
    }
    // The key reads `attnum` (see `build_index_key_with_values`), so only
    // that names the alias column.
    matches!(
        &index.keys[0].source,
        redlinedb_kernel::catalog::IndexKeySource::Column { attnum } if *attnum == alias
    )
}

/// The rowid that every pending index keys on, when they are all the
/// integer primary key and the new key is a non-negative INTEGER. Affinity
/// has already turned an exact REAL or numeric TEXT into one. Any other
/// value (NULL, a REAL past the i64 range, TEXT, a negative) returns
/// `None` so the caller compares key values instead: a rowid match would
/// miss a stored key that equals this value.
fn rowid_primary_candidate(
    table: &TableDef,
    indexes: &[&redlinedb_kernel::catalog::IndexDef],
    values: &[SqlValue],
) -> Option<RowId> {
    if !indexes
        .iter()
        .all(|index| is_rowid_primary_key(table, index))
    {
        return None;
    }
    match values.get(usize::from(table.rowid_alias_column?))? {
        SqlValue::Integer(v) if *v >= 0 => Some(RowId(*v as u64)),
        _ => None,
    }
}

/// The integer primary key is the rowid, so the key conflicts only if a
/// row holds `candidate` (from [`rowid_primary_candidate`]): one read after
/// the key's unique lock is held, where this used to read every row of the
/// table. The read uses the transaction's snapshot and own writes, as the
/// rest of the statement sees the table; a row committed after the
/// snapshot is refused by the kernel when the row is written
/// (`insert_row`), so it is never written over.
#[allow(clippy::too_many_arguments)]
fn rowid_primary_conflicts(
    conn: &Connection,
    session: &mut SessionState,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    values: &[SqlValue],
    indexes: &[&redlinedb_kernel::catalog::IndexDef],
    candidate: RowId,
    skip_rowid: Option<RowId>,
    mut conflicts: Vec<UniqueConflict>,
) -> Result<Vec<UniqueConflict>> {
    let mut held = None;
    for index in indexes {
        let built = crate::exec::index_dml::build_index_key_with_values(table, index, values)?;
        if built.key.contains_null {
            continue;
        }
        let lock_key = unique_key_bytes(table.table_id.0, index.index_id.0, &built.values)?;
        let guard = conn.unique_locks().lock(lock_key, tx.id().0)?;
        session.unique_guards.push(guard);
        let taken = match held {
            Some(taken) => taken,
            None => {
                let taken = rowid_holds_row(conn, tx, table, candidate)?;
                held = Some(taken);
                taken
            }
        };
        if taken && skip_rowid != Some(candidate) {
            let constraint_name = match table
                .constraints
                .iter()
                .find(|c| c.index_id == Some(index.index_id))
                .and_then(|c| c.name.as_deref().map(Arc::<str>::from))
            {
                Some(name) => Some(name),
                None => Some(Arc::from(index.name.as_ref())),
            };
            conflicts.push(UniqueConflict {
                rowid: candidate,
                constraint_name,
                key_ordinals: index.keys.iter().map(|key| key.ordinal as usize).collect(),
                expression_index: expression_index_name(index),
            });
        }
    }
    Ok(conflicts)
}

/// Whether a row of `table` that `tx` sees holds `rowid`. A CTE's rows are
/// not in the heap, so its rowids are listed instead.
fn rowid_holds_row(
    conn: &Connection,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    rowid: RowId,
) -> Result<bool> {
    if crate::exec::cte::is_cte_table_def(table) {
        return Ok(collect_table_rowids(conn.engine(), tx, table)?.contains(&rowid));
    }
    Ok(
        match conn
            .engine()
            .get_for_relation(tx, table.relation_id, rowid)?
        {
            Some(payload) => sql_row_table_id(&payload)? == Some(table.table_id.0),
            None => false,
        },
    )
}

fn unique_conflict_matches_target(
    conflict: &UniqueConflict,
    target: &crate::statement::UpsertTarget,
) -> bool {
    match target {
        crate::statement::UpsertTarget::Columns(columns) => conflict.key_ordinals == *columns,
        crate::statement::UpsertTarget::Constraint(name) => conflict
            .constraint_name
            .as_ref()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name)),
    }
}

fn apply_upsert_update(
    ctx: UpsertUpdateContext<'_>,
    update: &crate::statement::UpsertUpdatePlan,
) -> Result<InsertOutcome> {
    ctx.conn
        .engine()
        .lock_row_for_relation(ctx.tx, ctx.table.relation_id, ctx.conflict.rowid)?;
    let existing =
        match load_table_row_by_rowid(ctx.conn.engine(), ctx.tx, ctx.table, ctx.conflict.rowid)? {
            Some(row) => row,
            None => {
                return Err(Error::ConstraintViolation(format!(
                    "UPSERT conflict row missing for table {}",
                    ctx.table.name
                )));
            }
        };
    let upsert_row = RowContext::Upsert {
        current: &existing,
        excluded: ctx.excluded,
    };
    if let Some(selection) = &update.selection
        && !is_truthy(&eval_scalar(selection, &upsert_row, ctx.bindings)?)
    {
        return Ok(InsertOutcome::Ignored);
    }
    let old_values = existing.values.clone();
    let mut values = existing.values.clone();
    let mut scratch = EvalScratch::default();
    for (ordinal, expr) in &update.assignments {
        if *ordinal >= values.len() {
            return Err(Error::UnknownColumn(format!("ordinal {ordinal}")));
        }
        values[*ordinal] = evaluate_dml_value(
            ctx.table,
            *ordinal,
            expr,
            &upsert_row,
            ctx.bindings,
            &mut scratch,
        )?;
    }
    values = apply_row_affinity(ctx.table, values)?;
    // Phase-11 SQL-D A6: an UPSERT DO UPDATE may have touched an input
    // to a STORED generated column. Recompute every STORED column
    // before the persisted row is materialised.
    values = compute_stored_generated_columns(ctx.table, values)?;
    let new_rowid = choose_rowid_for_update(ctx.conn.engine(), ctx.table, &values, existing.rowid)?;
    if let Some(alias) = ctx.table.rowid_alias_column
        && let Some(slot) = values.get_mut(alias as usize)
        && matches!(&*slot, SqlValue::Null)
    {
        *slot = SqlValue::Integer(new_rowid.0 as i64);
    }
    apply_constraints(ctx.table, &values)?;
    ensure_unique_constraints(
        ctx.conn,
        ctx.session,
        ctx.tx,
        ctx.table,
        &values,
        Some(existing.rowid),
    )?;
    let payload = encode_sql_row(ctx.table.table_id.0, &values)?;
    if new_rowid == existing.rowid {
        ctx.conn.engine().update_for_relation(
            ctx.tx,
            ctx.table.relation_id,
            existing.rowid,
            payload,
        )?;
    } else {
        ctx.conn
            .engine()
            .delete_for_relation(ctx.tx, ctx.table.relation_id, existing.rowid)?;
        super::insert_row(ctx.conn, ctx.tx, ctx.table, new_rowid, payload)?;
        super::lower_rowid_allocator_after_delete(ctx.conn, ctx.tx, ctx.table, existing.rowid)?;
    }
    crate::exec::index_dml::maintain_indexes_on_update(
        ctx.conn.engine(),
        ctx.tx,
        ctx.table,
        &old_values,
        &values,
        existing.rowid,
        new_rowid,
    )?;
    Ok(InsertOutcome::Updated {
        rowid: new_rowid,
        values,
    })
}

/// Phase 10 Lane SQL-C: SQLite-style conflict resolution matrix.
///
/// SQLite documents five `ON CONFLICT` resolution algorithms (see
/// <https://sqlite.org/lang_conflict.html>):
///
/// | Action   | NOT NULL / CHECK fail | UNIQUE / PK fail              |
/// |----------|-----------------------|-------------------------------|
/// | ABORT    | error, undo this stmt | error, undo this stmt         |
/// | FAIL     | error, keep prior work| error, keep prior work        |
/// | IGNORE   | skip row silently     | skip row silently             |
/// | REPLACE  | error (no row to del) | delete conflicting row, insert|
/// | ROLLBACK | error, abort whole tx | error, abort whole tx         |
///
/// Centralising the matrix here means NOT NULL/CHECK and UNIQUE failures
/// dispatch to the same helper, which fixes the long-standing bug where
/// `INSERT OR IGNORE` did *not* swallow NOT NULL/CHECK failures because
/// `apply_constraints` ran before any conflict-action plumbing.
///
/// **Deviations from SQLite (documented):**
/// - `OR FAIL` and `OR ROLLBACK` currently behave like `OR ABORT` because
///   the surrounding `with_write_tx` machinery in `exec.rs` (out of scope
///   for this lane) treats every `Err` return as "rollback the implicit
///   tx" and every error inside an explicit tx as "session is poisoned".
///   The parser still distinguishes the verbs and the conflict helper
///   below records the intended action; full FAIL/ROLLBACK semantics
///   require statement-level partial commit and explicit-tx-aware error
///   classification, both of which span outside Lane SQL-C's allowed
///   files. See the `phase10_sqlc_conflict_matrix` tests for the
///   currently-asserted behaviour.
fn conflict_action_for(conflict: Option<&crate::statement::InsertConflict>) -> ConflictAction {
    match conflict {
        Some(crate::statement::InsertConflict::Sqlite(algo)) => match algo {
            crate::statement::ConflictAlgorithm::Abort => ConflictAction::Abort,
            crate::statement::ConflictAlgorithm::Fail => ConflictAction::Fail,
            crate::statement::ConflictAlgorithm::Ignore => ConflictAction::Ignore,
            crate::statement::ConflictAlgorithm::Replace => ConflictAction::Replace,
            crate::statement::ConflictAlgorithm::Rollback => ConflictAction::Rollback,
        },
        Some(crate::statement::InsertConflict::Upsert(_)) | None => ConflictAction::Abort,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConflictAction {
    Abort,
    Fail,
    Ignore,
    Replace,
    Rollback,
}

impl ConflictAction {
    /// True iff a NOT NULL or CHECK violation should be silently ignored
    /// (turning the row into `InsertOutcome::Ignored`). REPLACE does NOT
    /// help NOT NULL/CHECK because there is no conflicting row to delete
    /// — those constraints fire before any unique probe.
    fn ignores_check_or_not_null(self) -> bool {
        matches!(self, ConflictAction::Ignore)
    }
}

/// Run NOT NULL and CHECK validation against `values`, applying the
/// conflict-action verb. Returns `Ok(true)` to mean "row was dropped per
/// IGNORE", `Ok(false)` to mean "constraints passed", and `Err` for any
/// other action where a violation was found.
fn apply_constraints_with_action(
    table: &TableDef,
    values: &[SqlValue],
    action: ConflictAction,
) -> Result<bool> {
    match apply_constraints(table, values) {
        Ok(()) => Ok(false),
        Err(err) => {
            if action.ignores_check_or_not_null() {
                Ok(true)
            } else {
                // FAIL/ABORT/ROLLBACK and REPLACE all surface the
                // violation. The deviation note in the helper docs above
                // covers the FAIL/ROLLBACK collapse.
                Err(err)
            }
        }
    }
}

pub(in crate::exec) fn insert_row_with_resolution(
    conn: &Connection,
    session: &mut SessionState,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    values: &mut Vec<SqlValue>,
    conflict: Option<&crate::statement::InsertConflict>,
    bindings: &[Option<SqlValue>],
) -> Result<InsertOutcome> {
    values.resize(table.columns.len(), SqlValue::Null);
    *values = apply_row_affinity(table, std::mem::take(values))?;
    // Phase-11 SQL-D A6: compute STORED generated columns from inputs
    // (idempotent if already computed by a build_row* path) before any
    // constraint validation or unique-conflict checks observe the row.
    *values = compute_stored_generated_columns(table, std::mem::take(values))?;
    let rowid = choose_rowid_for_insert(session, conn.engine(), tx, table, values)?;

    let action = conflict_action_for(conflict);
    if apply_constraints_with_action(table, values, action)? {
        // IGNORE swallowed a NOT NULL or CHECK violation. The row is
        // dropped silently — no heap insert, no rowid bump beyond what
        // `choose_rowid_for_insert` already reserved (matches SQLite,
        // which advances the implicit rowid even for ignored rows).
        return Ok(InsertOutcome::Ignored);
    }

    let conflicts = collect_unique_conflicts(conn, session, tx, table, values, None)?;
    if conflicts.is_empty() {
        // Order: index unique-conflict check (already done above) ->
        // heap insert -> index inserts. If the heap insert succeeds but
        // the index insert fails, the kernel rolls back the whole tx so
        // recovery either replays both or neither.
        let payload = encode_sql_row(table.table_id.0, values)?;
        super::insert_row(conn, tx, table, rowid, payload)?;
        crate::exec::index_dml::maintain_indexes_on_insert(
            conn.engine(),
            tx,
            table,
            values,
            rowid,
        )?;
        // A6 SQLite parity: verify every declared FK against its parent.
        // Deferred constraints are queued for COMMIT; immediate ones must
        // resolve right now.
        crate::exec::fk::enforce_fk_on_insert(conn, session, tx, table, values, rowid)?;
        super::record_sqlite_sequence_rowid(session, table, rowid);
        session.last_insert_rowid = Some(rowid.0 as i64);
        return Ok(InsertOutcome::Inserted {
            rowid,
            values: values.clone(),
        });
    }

    apply_unique_conflict_resolution(
        conn, session, tx, table, rowid, values, &conflicts, conflict, action, bindings,
    )
}

/// UNIQUE / PK conflict-resolution matrix. Called only when at least one
/// conflicting row was found. NOT NULL / CHECK violations are handled
/// separately by `apply_constraints_with_action` because they can fire
/// even when there is no other row to compare against.
#[allow(clippy::too_many_arguments)]
fn apply_unique_conflict_resolution(
    conn: &Connection,
    session: &mut SessionState,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    rowid: RowId,
    values: &mut Vec<SqlValue>,
    conflicts: &[UniqueConflict],
    conflict: Option<&crate::statement::InsertConflict>,
    action: ConflictAction,
    bindings: &[Option<SqlValue>],
) -> Result<InsertOutcome> {
    // UPSERT (`ON CONFLICT(col) DO ...`) is its own branch; `action` is
    // ABORT for that case (we map UPSERT to ABORT in `conflict_action_for`)
    // because the dispatch lives below.
    if let Some(crate::statement::InsertConflict::Upsert(upsert)) = conflict {
        return apply_upsert_branch(
            conn, session, tx, table, values, conflicts, upsert, bindings,
        );
    }

    match action {
        ConflictAction::Ignore => Ok(InsertOutcome::Ignored),
        ConflictAction::Replace => {
            // INSERT OR REPLACE: delete each conflicting heap row (and
            // its index entries) before inserting the new tuple. Note
            // that REPLACE does NOT bypass NOT NULL/CHECK — those were
            // already validated above.
            let mut deleted = std::collections::HashSet::new();
            let mut same_row_old_values = None;
            for conflict in conflicts {
                if deleted.insert(conflict.rowid) {
                    let old_row =
                        load_table_row_by_rowid(conn.engine(), tx, table, conflict.rowid)?;
                    if conflict.rowid == rowid {
                        let Some(old_row) = old_row else {
                            return Err(Error::ConstraintViolation(format!(
                                "REPLACE conflict row missing for table {}",
                                table.name
                            )));
                        };
                        same_row_old_values = Some(old_row.values);
                        continue;
                    }
                    conn.engine()
                        .delete_for_relation(tx, table.relation_id, conflict.rowid)?;
                    super::lower_rowid_allocator_after_delete(conn, tx, table, conflict.rowid)?;
                    if let Some(old_row) = old_row {
                        crate::exec::index_dml::maintain_indexes_on_delete(
                            conn.engine(),
                            tx,
                            table,
                            &old_row.values,
                            conflict.rowid,
                        )?;
                        crate::exec::fk::enforce_fk_on_parent_delete(
                            conn,
                            session,
                            tx,
                            table,
                            &old_row.values,
                        )?;
                    }
                }
            }
            let payload = encode_sql_row(table.table_id.0, values)?;
            if let Some(old_values) = same_row_old_values {
                conn.engine()
                    .update_for_relation(tx, table.relation_id, rowid, payload)?;
                crate::exec::index_dml::maintain_indexes_on_update(
                    conn.engine(),
                    tx,
                    table,
                    &old_values,
                    values,
                    rowid,
                    rowid,
                )?;
                crate::exec::fk::enforce_fk_on_insert(conn, session, tx, table, values, rowid)?;
                crate::exec::fk::enforce_fk_on_parent_update(
                    conn,
                    session,
                    tx,
                    table,
                    &old_values,
                    values,
                )?;
            } else {
                super::insert_row(conn, tx, table, rowid, payload)?;
                crate::exec::index_dml::maintain_indexes_on_insert(
                    conn.engine(),
                    tx,
                    table,
                    values,
                    rowid,
                )?;
                crate::exec::fk::enforce_fk_on_insert(conn, session, tx, table, values, rowid)?;
            }
            super::record_sqlite_sequence_rowid(session, table, rowid);
            session.last_insert_rowid = Some(rowid.0 as i64);
            Ok(InsertOutcome::Inserted {
                rowid,
                values: values.clone(),
            })
        }
        // ABORT, FAIL and ROLLBACK all surface the violation here. See
        // the deviation note on `conflict_action_for` — full FAIL /
        // ROLLBACK semantics require changes outside Lane SQL-C's allowed
        // files.
        ConflictAction::Abort | ConflictAction::Fail | ConflictAction::Rollback => {
            Err(unique_error(table, conflicts))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_upsert_branch(
    conn: &Connection,
    session: &mut SessionState,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    values: &mut Vec<SqlValue>,
    conflicts: &[UniqueConflict],
    upsert: &crate::statement::UpsertPlan,
    bindings: &[Option<SqlValue>],
) -> Result<InsertOutcome> {
    for arm in upsert.arms.iter() {
        let hit = if let Some(target) = &arm.target {
            conflicts
                .iter()
                .find(|conflict| unique_conflict_matches_target(conflict, target))
        } else {
            conflicts.first()
        };
        let Some(hit) = hit else {
            continue;
        };
        return match &arm.action {
            crate::statement::UpsertAction::DoNothing => Ok(InsertOutcome::Ignored),
            crate::statement::UpsertAction::DoUpdate(update) => apply_upsert_update(
                UpsertUpdateContext {
                    conn,
                    session,
                    tx,
                    table,
                    excluded: values,
                    conflict: hit,
                    bindings,
                },
                update,
            ),
        };
    }
    Err(unique_error(table, conflicts))
}

pub(crate) fn ensure_unique_constraints(
    conn: &Connection,
    session: &mut SessionState,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    values: &[SqlValue],
    skip_rowid: Option<RowId>,
) -> Result<()> {
    let conflicts = collect_unique_conflicts(conn, session, tx, table, values, skip_rowid)?;
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(unique_error(table, &conflicts))
    }
}

#[cfg(test)]
#[path = "tail_conflict_real_key_tests.rs"]
mod real_key_tests;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::tempdir;

    use crate::connection::{Database, DbOptions};
    use crate::error::Error;
    use crate::statement::Step;

    use super::take_table_row_decodes;

    #[test]
    fn primary_key_insert_does_not_decode_existing_rows() {
        let dir = tempdir().unwrap();
        let db = Database::create(
            dir.path().join("pk-conflict.db"),
            DbOptions {
                busy_timeout: Duration::from_secs(5),
                ..DbOptions::default()
            },
        )
        .unwrap();
        let conn = db.connect();
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER)")
            .unwrap();
        let mut insert = conn
            .prepare("INSERT INTO t(id, v) VALUES (?1, ?2)")
            .unwrap();
        for i in 1..=20 {
            insert.bind_i64(1, i).unwrap();
            insert.bind_i64(2, i).unwrap();
            assert_eq!(insert.step().unwrap(), Step::Done);
            insert.reset().unwrap();
        }
        let _ = take_table_row_decodes();
        conn.execute("INSERT INTO t(id, v) VALUES (21, 21)")
            .unwrap();
        assert_eq!(take_table_row_decodes(), 0);
        let err = conn
            .execute("INSERT INTO t(id, v) VALUES (7, 70)")
            .unwrap_err();
        assert!(
            matches!(err, Error::ConstraintViolation(ref message) if message.contains("UNIQUE")),
            "{err:?}"
        );
    }

    #[test]
    fn unique_key_insert_does_not_decode_other_columns() {
        let dir = tempdir().unwrap();
        let db = Database::create(
            dir.path().join("unique-key.db"),
            DbOptions {
                busy_timeout: Duration::from_secs(5),
                ..DbOptions::default()
            },
        )
        .unwrap();
        let conn = db.connect();
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER UNIQUE, blob TEXT)")
            .unwrap();
        let mut insert = conn
            .prepare("INSERT INTO t(id, k, blob) VALUES (?1, ?2, ?3)")
            .unwrap();
        for i in 1..=20 {
            insert.bind_i64(1, i).unwrap();
            insert.bind_i64(2, i).unwrap();
            insert
                .bind_text(3, "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx")
                .unwrap();
            assert_eq!(insert.step().unwrap(), Step::Done);
            insert.reset().unwrap();
        }
        let _ = take_table_row_decodes();
        conn.execute("INSERT INTO t(id, k, blob) VALUES (21, 21, 'y')")
            .unwrap();
        assert_eq!(take_table_row_decodes(), 0);
        let err = conn
            .execute("INSERT INTO t(id, k, blob) VALUES (22, 7, 'z')")
            .unwrap_err();
        assert!(
            matches!(err, Error::ConstraintViolation(ref message) if message.contains("UNIQUE")),
            "{err:?}"
        );
    }
}
