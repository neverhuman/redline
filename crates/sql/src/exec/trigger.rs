//! Trigger fire-hook.
//!
//! Loaded by INSERT / UPDATE / DELETE executors after each row mutation
//! (and before, for BEFORE triggers). For each matching trigger:
//!
//! 1. Apply the optional `UPDATE OF cols` filter — UPDATE triggers only
//!    fire if at least one of the listed columns actually changed.
//! 2. Push synthesised `OLD` / `NEW` row contexts onto the correlated
//!    row stack so identifier resolution finds `OLD.col` / `NEW.col`.
//! 3. Evaluate the optional `WHEN` predicate; skip the body when it is
//!    not truthy.
//! 4. Re-parse the body SQL on each fire and execute every statement in
//!    the body against the live connection.
//!
//! Every running trigger program is a [`TriggerFrame`] on a per-thread
//! stack, entered before its `WHEN` clause and left when it ends, however it
//! ends. As in SQLite (`OP_Program`), with `recursive_triggers` off a
//! trigger does not fire while that same trigger is already running; every
//! other trigger fires at any depth. Past [`TRIGGER_DEPTH_CAP`] nested
//! programs the statement fails with "too many levels of trigger recursion".
//!
//! `INSTEAD OF INSERT` triggers on views fire through
//! [`fire_instead_of_insert`], under the same frames and in one write
//! transaction.

use std::cell::RefCell;
use std::sync::Arc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::exec::expr::scalar::row::{SqlRow, TableRow};
use crate::value::SqlValue;
use redlinedb_kernel::catalog::{
    Affinity, ColumnDef, ColumnId, ObjectId, SchemaId, SchemaSnapshot, TableDef, TableId,
    TriggerDef, TriggerEventKind, TriggerTimeKind, triggers_for,
};
use redlinedb_kernel::engine::Txn;
use redlinedb_kernel::format::RowId;

/// Most trigger programs one connection runs nested at once. SQLite's
/// default limit (`SQLITE_MAX_TRIGGER_DEPTH`) is 1000; each nested program
/// here re-enters the whole statement executor on the native stack, so the
/// cap stays at 8, which a debug build's test threads survive. The program
/// that would be the ninth fails its statement with "too many levels of
/// trigger recursion", as SQLite does past its limit.
pub(crate) const TRIGGER_DEPTH_CAP: u32 = 8;

const TOO_DEEP: &str = "too many levels of trigger recursion";

thread_local! {
    /// The trigger programs running on this thread, outermost first.
    static RUNNING: RefCell<Vec<RunningTrigger>> = const { RefCell::new(Vec::new()) };
}

/// One running trigger program: which connection runs which trigger.
/// Triggers another connection runs on this thread (from a callback) are
/// separate programs with their own depth.
#[derive(Clone, PartialEq, Eq)]
struct RunningTrigger {
    connection: usize,
    schema: SchemaId,
    trigger: ObjectId,
}

impl RunningTrigger {
    fn of(conn: &Connection, trigger: &TriggerDef) -> Self {
        Self {
            connection: conn as *const Connection as usize,
            schema: trigger.schema_id,
            trigger: trigger.trigger_id,
        }
    }
}

/// A trigger program while it runs. [`TriggerFrame::enter`] refuses the
/// program past [`TRIGGER_DEPTH_CAP`] and otherwise marks the trigger as
/// running; dropping the frame (on success, error or unwind) unmarks it and
/// every frame entered after it, so a failed statement leaves no trigger
/// marked as running.
struct TriggerFrame {
    slot: usize,
}

impl TriggerFrame {
    fn enter(conn: &Connection, trigger: &TriggerDef) -> Result<Self> {
        let running = RunningTrigger::of(conn, trigger);
        RUNNING.with(|stack| {
            let mut stack = stack.borrow_mut();
            let depth = stack
                .iter()
                .filter(|frame| frame.connection == running.connection)
                .count();
            if depth >= TRIGGER_DEPTH_CAP as usize {
                return Err(Error::UnsupportedSql(TOO_DEEP.to_owned()));
            }
            let slot = stack.len();
            stack.push(running);
            Ok(Self { slot })
        })
    }

    /// Whether `trigger` is running on `conn`, anywhere in the nesting.
    fn is_running(conn: &Connection, trigger: &TriggerDef) -> bool {
        let running = RunningTrigger::of(conn, trigger);
        RUNNING.with(|stack| stack.borrow().contains(&running))
    }
}

impl Drop for TriggerFrame {
    fn drop(&mut self) {
        // `try_with`: nothing to unmark once the thread's stack is gone.
        let _ = RUNNING.try_with(|stack| stack.borrow_mut().truncate(self.slot));
    }
}

/// Fire all triggers that match `(table, event, time)`. For UPDATE
/// triggers the optional `changed_cols` filter restricts firing to
/// triggers whose `UPDATE OF` list intersects the set of columns whose
/// values actually changed.
pub(crate) fn fire_triggers(
    conn: &Connection,
    _tx: &mut Txn,
    schema: &SchemaSnapshot,
    table: &Arc<TableDef>,
    event: TriggerEventKind,
    time: TriggerTimeKind,
    old: Option<TriggerRowValues>,
    new: Option<TriggerRowValues>,
    changed_cols: Option<&[String]>,
) -> Result<()> {
    let triggers = triggers_for(schema, table.schema_id, &table.folded, event, time);
    if triggers.is_empty() {
        return Ok(());
    }
    // `PRAGMA recursive_triggers = OFF` mirrors SQLite's OP_Program rule:
    // a trigger does not fire while that same trigger is already running;
    // every other trigger fires, however deeply nested. The flag is read
    // through the re-entrant session pointer because the session mutex is
    // already held by the enclosing DML executor — calling
    // `conn.recursive_triggers()` would re-lock and deadlock.
    let recursive = current_recursive_triggers();
    for trigger in triggers {
        if event == TriggerEventKind::Update
            && !trigger.when_cols.is_empty()
            && !any_column_in_filter(&trigger.when_cols, changed_cols)
        {
            continue;
        }
        if !recursive && TriggerFrame::is_running(conn, &trigger) {
            continue;
        }
        fire_one(conn, table, &trigger, old.as_ref(), new.as_ref())?;
    }
    Ok(())
}

/// Captured row values for a single OLD or NEW context. Owning the
/// values lets the fire-hook materialise a synthetic `TableRow` keyed by
/// the `OLD` / `NEW` alias.
#[derive(Clone)]
pub(crate) struct TriggerRowValues {
    pub(crate) rowid: RowId,
    pub(crate) values: Vec<SqlValue>,
}

fn any_column_in_filter(filter: &[Box<str>], changed: Option<&[String]>) -> bool {
    let Some(changed) = changed else {
        return true;
    };
    filter
        .iter()
        .any(|f| changed.iter().any(|c| c.eq_ignore_ascii_case(f.as_ref())))
}

/// Read `session.recursive_triggers` without taking the session mutex.
/// With no session on the thread-local pointer (a caller that never went
/// through `with_write_tx`), recursion is allowed; the depth cap still
/// bounds it.
fn current_recursive_triggers() -> bool {
    match crate::exec::current_session_ptr() {
        Some(ptr) => {
            // SAFETY: pointer installed by `with_write_tx`; valid for
            // the strictly-synchronous scope of trigger firing.
            let session: &crate::session::SessionState = unsafe { &*ptr };
            session.recursive_triggers
        }
        None => true,
    }
}

/// Run one trigger's program (its `WHEN` clause, then its body) in a frame.
fn fire_one(
    conn: &Connection,
    table: &Arc<TableDef>,
    trigger: &TriggerDef,
    old: Option<&TriggerRowValues>,
    new: Option<&TriggerRowValues>,
) -> Result<()> {
    // A replay would fire the trigger again (S9-05).
    crate::replay::mark_hazard();
    let _frame = TriggerFrame::enter(conn, trigger)?;
    run_body_with_context(conn, table, trigger, old, new)
}

fn run_body_with_context(
    conn: &Connection,
    table: &Arc<TableDef>,
    trigger: &TriggerDef,
    old: Option<&TriggerRowValues>,
    new: Option<&TriggerRowValues>,
) -> Result<()> {
    let old_row = old.map(|v| make_table_row(table, "OLD", v));
    let new_row = new.map(|v| make_table_row(table, "NEW", v));

    // Push contexts onto the outer-row stack so `OLD.col`/`NEW.col`
    // resolve via the qualified-identifier path. Push in a fixed order
    // so the body always sees both contexts when present.
    let mut pushed = 0u32;
    if let Some(row) = old_row.clone() {
        crate::exec::push_outer_row(SqlRow::Table(row));
        pushed += 1;
    }
    if let Some(row) = new_row.clone() {
        crate::exec::push_outer_row(SqlRow::Table(row));
        pushed += 1;
    }
    // The body names real tables, whatever the firing statement calls a
    // CTE (Q5-09).
    let own_scope = crate::exec::cte::Isolated::enter();
    let result = || -> Result<()> {
        if let Some(predicate_sql) = &trigger.when_predicate_sql
            && !evaluate_when_predicate(conn, predicate_sql)?
        {
            return Ok(());
        }
        execute_body_statements(conn, trigger.body_sql.as_ref())?;
        Ok(())
    }();
    drop(own_scope);
    for _ in 0..pushed {
        crate::exec::pop_outer_row();
    }
    result
}

pub(crate) fn fire_instead_of_insert(
    conn: &Connection,
    view_name: &str,
    columns: &[String],
    values: Vec<SqlValue>,
) -> Result<()> {
    let schema = conn.schema_snapshot();
    let schema_id = schema.lookup_namespace("main").unwrap_or(SchemaId(1));
    let triggers = triggers_for(
        &schema,
        schema_id,
        &view_name.to_ascii_lowercase(),
        TriggerEventKind::Insert,
        TriggerTimeKind::InsteadOf,
    );
    if triggers.is_empty() {
        return Err(Error::UnsupportedSql(format!(
            "cannot modify {view_name} because it is a view"
        )));
    }
    crate::replay::mark_hazard();
    let table = synth_view_trigger_table(view_name, columns);
    let row = TriggerRowValues {
        rowid: RowId(1),
        values,
    };
    // One write transaction for the whole firing (the enclosing one when
    // this INSERT runs inside another trigger), so the body's statements and
    // everything they fire fail or commit together, as one statement does.
    super::with_write_tx(conn, |session, _tx| {
        let recursive = session.recursive_triggers;
        for trigger in &triggers {
            // A view insert whose trigger is already running (recursion
            // off) fires nothing and changes nothing, as in SQLite.
            if !recursive && TriggerFrame::is_running(conn, trigger) {
                continue;
            }
            fire_one(conn, &table, trigger, None, Some(&row))?;
        }
        Ok(())
    })
}

fn synth_view_trigger_table(view_name: &str, columns: &[String]) -> Arc<TableDef> {
    Arc::new(TableDef {
        table_id: TableId(0),
        schema_id: SchemaId(0),
        relation_id: redlinedb_kernel::format::RelId(0),
        name: Box::from(view_name),
        folded: Box::from(view_name.to_ascii_lowercase()),
        columns: columns
            .iter()
            .enumerate()
            .map(|(idx, name)| ColumnDef {
                column_id: ColumnId((idx + 1) as u64),
                ordinal: idx as u16,
                name: Box::from(name.as_str()),
                folded: Box::from(name.to_ascii_lowercase()),
                declared_type: None,
                affinity: Affinity::Blob,
                not_null: false,
                default_value: None,
                default_expr: None,
                generated: None,
            })
            .collect(),
        indexes: Vec::new(),
        constraints: Vec::new(),
        checks: Vec::new(),
        foreign_keys: Vec::new(),
        rowid_alias_column: None,
        flags: 0,
        normalized_sql: None,
    })
}

fn make_table_row(table: &Arc<TableDef>, alias: &str, values: &TriggerRowValues) -> TableRow {
    TableRow {
        rowid: values.rowid,
        values: values.values.clone(),
        table: Arc::clone(table),
        alias: Some(Arc::from(alias)),
    }
}

/// Evaluate a `WHEN` predicate against the active OLD/NEW row context.
/// The expression is parsed inside a synthetic `SELECT <expr>` so the
/// existing expression evaluator handles it without bespoke parsing.
fn evaluate_when_predicate(conn: &Connection, predicate_sql: &str) -> Result<bool> {
    let synth = format!("SELECT ({predicate_sql})"); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=predicate-sql-is-a-parsed-ast-fragment-from-the-trigger-body-not-external-input expires=2027-06-01
    let template = crate::parser::parse_prepared_template(conn, &synth)?;
    let rows = crate::exec::materialize_prepared_rows(conn, &template, &[])?;
    let truthy = rows
        .first()
        .and_then(|r| r.first())
        .map(crate::value::is_truthy)
        .unwrap_or(false);
    Ok(truthy)
}

/// Split the body SQL into individual statements and execute each.
///
/// The body comes from sqlparser's `ConditionalStatements` Display, which
/// emits `BEGIN\n<stmt>;\n<stmt>;\nEND`. The SQLite dialect rejects
/// standalone `BEGIN ... END` blocks, so we strip the wrapper and run
/// the inner statement list one at a time. Each statement shares the
/// live trigger row context already on the outer-row stack.
fn execute_body_statements(conn: &Connection, body_sql: &str) -> Result<()> {
    let inner = strip_begin_end_wrapper(body_sql);
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let mut rest = trimmed;
    while !rest.is_empty() {
        if crate::parser::is_blank_sql(rest) {
            break;
        }
        let (head, tail) = crate::parser::split_first_statement(rest);
        if head.is_empty() {
            break;
        }
        if !crate::parser::is_blank_sql(head) {
            conn_execute_quiet(conn, head)?;
        }
        rest = tail;
    }
    Ok(())
}

/// Trim a leading `BEGIN` and trailing `END` from a trigger body, if
/// present. The sqlparser Display always emits the wrapper; we strip it
/// because the SQLite dialect rejects `BEGIN ... END` as a top-level
/// statement.
fn strip_begin_end_wrapper(body: &str) -> &str {
    let trimmed = body.trim();
    let lower = trimmed.to_ascii_lowercase();
    let after_begin = if lower.starts_with("begin")
        && trimmed
            .as_bytes()
            .get(5)
            .map(|b| b.is_ascii_whitespace())
            .unwrap_or(false)
    {
        &trimmed[5..]
    } else {
        trimmed
    };
    let after_begin = after_begin.trim_start();
    let lower = after_begin.to_ascii_lowercase();
    if lower.ends_with("end") {
        let cut = after_begin.len() - 3;
        let before_end = after_begin[..cut].trim_end();
        let before_end = before_end.trim_end_matches(';').trim_end();
        return before_end;
    }
    after_begin
}

fn conn_execute_quiet(conn: &Connection, sql: &str) -> Result<()> {
    let template = crate::parser::parse_prepared_template(conn, sql)?;
    let _ = crate::exec::materialize_prepared_rows(conn, &template, &[])?;
    Ok(())
}
