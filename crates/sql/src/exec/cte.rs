//! Common Table Expression (CTE) binding and execution.
//!
//! See `synth_table_def` for the synthetic-`TableDef` trick used to feed
//! pre-materialized CTE rows through the join executor.
//!
//! Both non-recursive and recursive `WITH` clauses are handled by
//! pre-materializing each CTE body into rows during statement binding.
//! The CTE rows are then exposed via [`SelectSource::Cte`] when the
//! trailing query references the CTE name.
//!
//! Recursive evaluation follows the SQL standard's working-set semantics:
//!   1. Evaluate the anchor branch (the side that does not reference the
//!      CTE) and seed the result set.
//!   2. Repeatedly evaluate the recursive branch against the *just-emitted*
//!      rows, appending new rows to the result set.
//!   3. Stop when an iteration produces zero new rows. `UNION` (without
//!      `ALL`) deduplicates across iterations so cycles in the recursion
//!      terminate naturally; a configurable bound caps run-away cases.
//!
//! See `crates/sql/tests/parity_cte.rs` for differential coverage.

#[path = "cte_registry.rs"]
pub(crate) mod registry;

#[path = "cte_recursive.rs"]
mod recursive;

#[path = "cte_row_cap.rs"]
mod row_cap;

use std::collections::HashMap;
use std::sync::Arc;

use sqlparser::ast::With;

use crate::connection::Connection;
use crate::error::Result;
use crate::statement::{
    BoundTable, ParamLayout, PreparedKind, PreparedTemplate, SelectPlan, SelectSource,
};
use crate::value::SqlValue;
use redlinedb_kernel::catalog::{
    Affinity, ColumnDef, ColumnId, SchemaEpoch, SchemaId, SchemaSnapshot, TableDef, TableId,
};
use redlinedb_kernel::format::RelId;

use registry::register_cte_rows;
pub(crate) use registry::{register_external_rows, rows_for_relation};
use sqlparser::ast::Query;

/// Sentinel relation id used by synthetic CTE table defs. Real relations
/// allocate from a monotonic counter starting at 1, so picking the top
/// bit of u64 keeps us comfortably out of any plausible real-id range.
const CTE_RELATION_TAG: u64 = 0xC7E0_0000_0000_0000;

/// Returns true if this `TableDef` was synthesized for a CTE, a view,
/// or a cross-database alias (all three share the same row-storage
/// backing via `register_external_rows` and `rows_for_relation`). The
/// view, CTE, and cross-DB tags share the same row-registry namespace.
pub(crate) fn is_cte_table_def(def: &TableDef) -> bool {
    let tag = def.relation_id.0 & 0xFFFF_0000_0000_0000;
    tag == CTE_RELATION_TAG
        || tag == super::view::VIEW_RELATION_TAG
        || tag == super::cross_db::CROSS_DB_RELATION_TAG
}

/// A pre-materialized CTE definition: rows + column names + optional
/// synthetic `TableDef` so the join executor can treat the CTE name as
/// a real table.
#[derive(Clone)]
pub(crate) struct CteDef {
    pub(crate) name: Arc<str>,
    pub(crate) columns: Arc<[String]>,
    pub(crate) rows: Arc<[Vec<SqlValue>]>,
    /// Synthetic TableDef; populated on demand the first time the CTE
    /// gets resolved through `try_resolve_cte_join_table`.
    pub(crate) table_def: Option<Arc<TableDef>>,
    /// Each column's comparison affinity, from the body's result
    /// expressions (`None`: every column reads as BLOB).
    pub(crate) affinities: Option<registry::ColumnAffinities>,
}

thread_local! {
    static CTE_SCOPE: std::cell::RefCell<Vec<HashMap<String, CteDef>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Q5-09: synthetic CTE relation ids are process-unique and never reused;
/// restarting them at 1 on every WITH let a nested WITH overwrite the
/// outer CTE's rows.
fn next_cte_rel_id() -> RelId {
    super::bind_env::next_synthetic_id(CTE_RELATION_TAG)
}

/// Keeps the CTE scopes a binder pushed from outliving it: on drop, on the
/// success and the error path alike, the scope stack returns to the depth
/// it had when the guard was taken.
pub(crate) struct ScopeGuard {
    depth: usize,
}

impl ScopeGuard {
    pub(crate) fn enter() -> Self {
        Self {
            depth: CTE_SCOPE.with(|cell| cell.borrow().len()),
        }
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        CTE_SCOPE.with(|cell| cell.borrow_mut().truncate(self.depth));
    }
}

/// Binds a view body or a trigger body in a scope of its own: it sees
/// neither the CTEs of the statement that uses it nor that statement's
/// bind-time scopes, as in SQLite. What the body registers is dropped with
/// the guard (Q5-09).
pub(crate) struct Isolated {
    scopes: Vec<HashMap<String, CteDef>>,
    _capture: super::bind_env::Capture,
}

impl Isolated {
    pub(crate) fn enter() -> Self {
        Self {
            scopes: CTE_SCOPE.with(|cell| std::mem::take(&mut *cell.borrow_mut())),
            _capture: super::bind_env::Capture::barrier(),
        }
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let scopes = std::mem::take(&mut self.scopes);
        CTE_SCOPE.with(|cell| *cell.borrow_mut() = scopes);
    }
}

/// Build a synthetic `TableDef` for a CTE so the join executor can treat
/// the CTE name as a real table. Column types are inferred from the
/// first non-NULL value in each column; declared NOT NULL is left off so
/// any value flows.
pub(crate) fn synth_table_def(
    name: &str,
    columns: &[String],
    rows: &[Vec<SqlValue>],
) -> Arc<TableDef> {
    let folded_name = name.to_ascii_lowercase();
    let folded_columns: Vec<String> = columns.iter().map(|n| n.to_ascii_lowercase()).collect();
    synth_table_def_with_folded(name, columns, &folded_name, &folded_columns, rows)
}

/// Phase 4.4: variant of `synth_table_def` that accepts pre-computed
/// lowercase forms of the name + columns. Called per recursive CTE
/// iteration from `cte_recursive::materialize_cte` to skip
/// `to_ascii_lowercase` allocations on every iteration when the names
/// don't change.
pub(crate) fn synth_table_def_with_folded(
    name: &str,
    columns: &[String],
    folded_name: &str,
    folded_columns: &[String],
    rows: &[Vec<SqlValue>],
) -> Arc<TableDef> {
    debug_assert_eq!(folded_columns.len(), columns.len());
    let column_defs: Vec<ColumnDef> = columns
        .iter()
        .enumerate()
        .map(|(idx, col_name)| ColumnDef {
            column_id: ColumnId((idx + 1) as u64),
            ordinal: idx as u16,
            name: Box::from(col_name.as_str()),
            folded: Box::from(folded_columns[idx].as_str()),
            declared_type: None,
            affinity: infer_affinity(rows, idx),
            not_null: false,
            default_value: None,
            default_expr: None,
            generated: None,
            collation: None,
        })
        .collect();
    let rel = next_cte_rel_id();
    Arc::new(TableDef {
        table_id: TableId(rel.0),
        schema_id: SchemaId(0),
        relation_id: rel,
        name: Box::from(name),
        folded: Box::from(folded_name),
        columns: column_defs,
        indexes: Vec::new(),
        constraints: Vec::new(),
        checks: Vec::new(),
        foreign_keys: Vec::new(),
        rowid_alias_column: None,
        flags: 0,
        normalized_sql: None,
    })
}

fn infer_affinity(rows: &[Vec<SqlValue>], col: usize) -> Affinity {
    for row in rows {
        if let Some(v) = row.get(col) {
            match v {
                SqlValue::Integer(_) => return Affinity::Integer,
                SqlValue::Real(_) => return Affinity::Real,
                SqlValue::Text(_) => return Affinity::Text,
                SqlValue::Blob(_) => return Affinity::Blob,
                SqlValue::Null => continue,
            }
        }
    }
    Affinity::Blob
}

/// Build a [`CteDef`] from a pre-materialized row set, registering the
/// row payload in the thread-local relation store so the join executor
/// can resolve the synthetic relation_id at execution time.
pub(crate) fn build_cte_def_from_rows(
    name: &str,
    columns: Vec<String>,
    rows: Vec<Vec<SqlValue>>,
) -> CteDef {
    let table_def = synth_table_def(name, &columns, &rows);
    let rows_arc: Arc<Vec<Vec<SqlValue>>> = Arc::new(rows);
    register_cte_rows(table_def.relation_id, Arc::clone(&rows_arc));
    let row_slice: Arc<[Vec<SqlValue>]> = Arc::from(rows_arc.as_slice().to_vec());
    CteDef {
        name: Arc::from(name),
        columns: Arc::from(columns),
        rows: row_slice,
        table_def: Some(table_def),
        affinities: None,
    }
}

/// Record `affinities` for the synthetic table of `def` (after its rows).
pub(crate) fn register_def_affinities(def: &CteDef) {
    if let (Some(table), Some(affinities)) = (&def.table_def, &def.affinities) {
        registry::register_column_affinities(table.relation_id, Arc::clone(affinities));
    }
}

/// Push a CTE scope (visible to bind-time lookups). Pairs with `pop_scope`.
pub(crate) fn push_scope(scope: HashMap<String, CteDef>) {
    CTE_SCOPE.with(|cell| cell.borrow_mut().push(scope));
}

pub(crate) fn pop_scope() {
    CTE_SCOPE.with(|cell| {
        cell.borrow_mut().pop();
    });
}

/// Look up a CTE by (case-insensitive) name in the current scope chain.
pub(crate) fn lookup(name: &str) -> Option<CteDef> {
    CTE_SCOPE.with(|cell| {
        for scope in cell.borrow().iter().rev() {
            for (key, value) in scope.iter() {
                if key.eq_ignore_ascii_case(name) {
                    return Some(value.clone());
                }
            }
        }
        None
    })
}

/// True if there is any CTE in the current scope chain.
pub(crate) fn scope_active() -> bool {
    CTE_SCOPE.with(|cell| !cell.borrow().is_empty())
}

/// Bind a `WITH ... query` form. Pre-executes each CTE body (handling
/// recursive references) and pushes a CTE scope before binding the
/// trailing query. The scopes are popped before returning, on every path;
/// each CTE is *also* registered with the preparing statement
/// (`bind_env::register_cte`) so subqueries that bind while it runs, after
/// the scope stack has been torn down, still resolve the name.
pub(crate) fn bind_with_query(
    conn: &Connection,
    schema: Arc<SchemaSnapshot>,
    schema_epoch: SchemaEpoch,
    sql: &str,
    with: With,
    body_query: Query,
) -> Result<PreparedTemplate> {
    let With {
        recursive,
        cte_tables,
        ..
    } = with;

    // Q5-09: scopes pushed below are popped when this guard drops, also
    // when a CTE body fails to bind.
    let _scopes = ScopeGuard::enter();
    // Q5-03: derive every cap before materializing, while the sibling
    // CTE bodies are still at hand for `derive_cte_row_cap` to inspect.
    let row_caps: Vec<Option<usize>> = cte_tables
        .iter()
        .enumerate()
        .map(|(index, cte)| {
            row_cap::derive_cte_row_cap(&body_query, &cte.alias.name.value, &cte_tables, index)
        })
        .collect();
    for (cte, row_cap) in cte_tables.into_iter().zip(row_caps) {
        let def = recursive::materialize_cte(
            conn,
            Arc::clone(&schema),
            schema_epoch,
            sql,
            &cte,
            recursive,
            row_cap,
        )?;
        super::bind_env::register_cte(&def);
        let mut single = HashMap::new();
        single.insert(def.name.to_string(), def);
        push_scope(single);
    }
    super::super::parser::bind_query(conn, schema, schema_epoch, sql, body_query)
}

/// Run a Query and materialize all output rows. Returns rows plus column names.
/// A query's rows, its column names, and each column's comparison affinity
/// (from its result expressions, when the query is a plain SELECT).
pub(crate) type QueryRows = (
    Vec<Vec<SqlValue>>,
    Vec<String>,
    Option<registry::ColumnAffinities>,
);

pub(crate) fn run_query_to_rows(
    conn: &Connection,
    schema: Arc<SchemaSnapshot>,
    schema_epoch: SchemaEpoch,
    sql: &str,
    query: Query,
    declared_columns: &[String],
) -> Result<QueryRows> {
    let template = super::super::parser::bind_query(conn, schema, schema_epoch, sql, query)?;
    super::bind_env::note_materialization();
    if !template.readonly {
        super::bind_env::note_bind_write();
    }
    let columns: Vec<String> = if !declared_columns.is_empty() {
        declared_columns.to_vec()
    } else {
        template.output_columns.iter().cloned().collect()
    };
    let affinities = super::view::defining_affinities(&template, columns.len());
    // Q5-08: a statement bound again at execution shows its parameters.
    let bindings = super::rebind::bind_time_bindings();
    let rows = super::materialize_prepared_rows(conn, &template, &bindings)?;
    Ok((rows, columns, affinities))
}

/// Try to interpret a FROM table reference as a CTE. Returns a
/// `SelectSource::Cte` if the name matches a CTE in the active scope
/// or, failing that, in the per-thread permanent CTE registry (for
/// subqueries that bind at exec time after the scope stack has been
/// torn down).
pub(crate) fn try_resolve_cte_source(
    name: &sqlparser::ast::ObjectName,
    alias: Option<&Arc<str>>,
    _params: &mut ParamLayout,
) -> Option<SelectSource> {
    let last = name.0.last()?;
    let ident_name = match last {
        sqlparser::ast::ObjectNamePart::Identifier(ident) => &ident.value,
        _ => return None,
    };
    let def = resolve_cte_def(ident_name)?;
    Some(SelectSource::Cte {
        name: def.name,
        alias: alias.cloned(),
        columns: def.columns,
        rows: def.rows,
        affinities: def.affinities,
    })
}

/// Try to interpret a FROM/JOIN table reference as a CTE table. Returns
/// a synthetic `BoundTable` whose `TableDef.relation_id` is registered
/// in the global CTE row map.
pub(crate) fn try_resolve_cte_bound_table(
    name: &sqlparser::ast::ObjectName,
    alias: Option<&Arc<str>>,
) -> Option<BoundTable> {
    let last = name.0.last()?;
    let ident_name = match last {
        sqlparser::ast::ObjectNamePart::Identifier(ident) => &ident.value,
        _ => return None,
    };
    let def = resolve_cte_def(ident_name)?;
    let table = def.table_def?;
    Some(BoundTable {
        table,
        alias: alias.cloned(),
        index_hint: None,
    })
}

/// Single resolution point that consults both tiers in a deterministic
/// order: the active scope (the local `WITH` being bound) wins over the
/// CTEs the preparing or running statement registered (an enclosing `WITH`
/// whose scope has already been popped — used by subqueries that bind at
/// exec time). A CTE never outlives its statement (Q5-09).
fn resolve_cte_def(ident_name: &str) -> Option<CteDef> {
    if scope_active()
        && let Some(def) = lookup(ident_name)
    {
        return Some(def);
    }
    super::bind_env::lookup_cte(ident_name)
}

#[allow(dead_code)]
pub(crate) fn from_static(
    name: &str,
    alias: Option<&str>,
    columns: Vec<String>,
    rows: Vec<Vec<SqlValue>>,
) -> SelectPlan {
    SelectPlan {
        source: SelectSource::Cte {
            name: Arc::from(name),
            alias: alias.map(Arc::from),
            columns: Arc::from(columns),
            rows: Arc::from(rows),
            affinities: None,
        },
        distinct: false,
        distinct_on: Vec::new(),
        projection: Vec::new(),
        selection: None,
        group_by: Vec::new(),
        having: None,
        order_by: Vec::new(),
        limit: None,
        offset: None,
        table_hint: None,
    }
}

#[allow(dead_code)]
pub(crate) fn template_from_static(
    sql: &str,
    schema_epoch: SchemaEpoch,
    output_columns: Arc<[String]>,
    rows: Arc<[Vec<SqlValue>]>,
) -> PreparedTemplate {
    PreparedTemplate {
        sql: Arc::from(sql),
        schema_epoch,
        stats_epoch: 0,
        optimizer_hash: 0,
        param_layout: ParamLayout::default(),
        output_columns,
        readonly: true,
        kind: PreparedKind::Select(SelectPlan {
            source: SelectSource::StaticRows { rows },
            distinct: false,
            distinct_on: Vec::new(),
            projection: Vec::new(),
            selection: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            table_hint: None,
        }),
    }
}

// Re-export Distinct so the parser scope picks it up if needed.
#[allow(unused_imports)]
use sqlparser::ast::Distinct;
