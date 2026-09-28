//! View binding and expansion.
//!
//! Views are persisted in the catalog as raw SELECT bodies. When the
//! binder encounters a name in a FROM clause that resolves to a view,
//! [`try_resolve_view_source`] re-parses the body, executes it against
//! the live connection, and produces a `SelectSource::Cte`-style row
//! container. JOINs against the view route through the synthetic
//! `TableDef` returned by [`try_resolve_view_bound_table`].
//!
//! Like CTE materialization, view rows belong to the statement whose
//! binding read them (`crate::exec::bind_env`). Templates that embed them
//! stay out of both statement caches, and a statement that read a view
//! binds again when it is executed, so it reads the rows of the moment it
//! runs (`crate::exec::rebind`, Q5-08). A view body is bound in its own
//! scope: the CTEs of the statement around it are not visible to it.

use std::sync::Arc;

use redlinedb_kernel::catalog::{
    Affinity, ColumnDef, ColumnId, SchemaId, SchemaSnapshot, TableDef, TableId, ViewDef,
};
use redlinedb_kernel::format::RelId;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::statement::{BoundTable, ParamLayout, SelectSource};
use crate::value::SqlValue;

/// Sentinel relation id used by synthetic view table defs. Disjoint
/// from CTE ids (`0xC7E0_0000_0000_0000`) so the two namespaces never
/// collide inside one query.
pub(crate) const VIEW_RELATION_TAG: u64 = 0xC1E0_0000_0000_0000;

/// True if this `TableDef` was synthesized for a view expansion.
pub(crate) fn is_view_table_def(def: &TableDef) -> bool {
    (def.relation_id.0 & 0xFFFF_0000_0000_0000) == VIEW_RELATION_TAG
}

/// Process-unique, never reused (Q5-09).
fn next_view_rel_id() -> RelId {
    super::bind_env::next_synthetic_id(VIEW_RELATION_TAG)
}

/// Try to interpret a FROM table reference as a view. Returns
/// `Ok(Some(...))` when the name resolves to a registered view and the
/// body executed without error. Returns `Ok(None)` when the name is not
/// a view in scope (the caller falls back to ordinary table lookup).
pub(crate) fn try_resolve_view_source(
    schema: &SchemaSnapshot,
    name: &sqlparser::ast::ObjectName,
    alias: Option<&Arc<str>>,
    _params: &mut ParamLayout,
) -> Result<Option<SelectSource>> {
    let Some(view) = lookup_in_schema(schema, name) else {
        return Ok(None);
    };
    let Some(conn) = super::current_connection() else {
        return Err(Error::UnsupportedSql(
            "view expansion requires an active connection context".to_owned(),
        ));
    };
    let (rows, columns, affinities) = materialize_view(conn, &view)?;
    Ok(Some(SelectSource::Cte {
        name: Arc::from(view.name.as_ref()),
        alias: alias.cloned(),
        columns: Arc::from(columns),
        rows: Arc::from(rows),
        affinities,
    }))
}

/// Try to interpret a FROM/JOIN table factor as a view. Returns a
/// synthetic [`BoundTable`] whose `TableDef` is recognised by
/// `crate::exec::cte::rows_for_relation`-style lookups via the SQL
/// crate's CTE row registry (we re-use it because the storage shape is
/// identical).
pub(crate) fn try_resolve_view_bound_table(
    schema: &SchemaSnapshot,
    name: &sqlparser::ast::ObjectName,
    alias: Option<&Arc<str>>,
) -> Result<Option<BoundTable>> {
    let Some(view) = lookup_in_schema(schema, name) else {
        return Ok(None);
    };
    let Some(conn) = super::current_connection() else {
        return Err(Error::UnsupportedSql(
            "view expansion requires an active connection context".to_owned(),
        ));
    };
    let (rows, columns, affinities) = materialize_view(conn, &view)?;
    let table_def = synth_view_table_def(&view.name, &columns, &rows);
    // Register the rows under the synthesised relation_id so the join
    // executor's row-source lookup (`exec::cte::rows_for_relation`)
    // can fetch them by id. We reuse the CTE row registry: there is no
    // ambiguity because view relation_ids are tagged disjointly.
    super::cte::register_external_rows(table_def.relation_id, Arc::new(rows));
    if let Some(affinities) = affinities {
        super::cte::registry::register_column_affinities(table_def.relation_id, affinities);
    }
    Ok(Some(BoundTable {
        table: table_def,
        alias: alias.cloned(),
        index_hint: None,
    }))
}

/// Lookup a view by `ObjectName` (case-insensitive, single-part names
/// only — multi-part `schema.view` is supported when the schema matches
/// `main`).
fn lookup_in_schema(
    schema: &SchemaSnapshot,
    name: &sqlparser::ast::ObjectName,
) -> Option<Arc<ViewDef>> {
    let schema_id = schema.lookup_namespace("main")?;
    let parts = &name.0;
    let last = parts.last()?;
    let ident = match last {
        sqlparser::ast::ObjectNamePart::Identifier(i) => &i.value,
        _ => return None,
    };
    schema.lookup_view(schema_id, ident)
}

/// Re-parse the view body, execute it, and return `(rows, columns)`.
/// The column list respects the alias-column list when non-empty.
/// A view's rows, its column names, and the comparison affinity of each
/// column: that of its defining expression, as SQLite gives a view column
/// (`sqlite3SubqueryColumnTypes`), when the body is a plain SELECT.
type MaterializedView = (
    Vec<Vec<SqlValue>>,
    Vec<String>,
    Option<super::cte::registry::ColumnAffinities>,
);

fn materialize_view(conn: &Connection, view: &ViewDef) -> Result<MaterializedView> {
    // The body names real tables, whatever the query around it calls a CTE.
    let _own_scope = super::cte::Isolated::enter();
    let template = crate::parser::parse_prepared_template(conn, view.body_sql.as_ref())?;
    super::bind_env::note_materialization();
    let column_names: Vec<String> = if !view.columns.is_empty() {
        view.columns.iter().map(|c| c.as_ref().to_owned()).collect()
    } else {
        template.output_columns.iter().cloned().collect()
    };
    let affinities = defining_affinities(&template, column_names.len());
    let rows = super::materialize_prepared_rows(conn, &template, &[])?;
    Ok((rows, column_names, affinities))
}

/// The affinity of each result column of a plain SELECT template.
pub(crate) fn defining_affinities(
    template: &crate::statement::PreparedTemplate,
    count: usize,
) -> Option<super::cte::registry::ColumnAffinities> {
    let crate::statement::PreparedKind::Select(plan) = &template.kind else {
        return None;
    };
    Some(
        (0..count)
            .map(|index| crate::exec::expr::affinity::plan_column_affinity(plan, index))
            .collect(),
    )
}

/// Build a synthetic `TableDef` for a view, mirroring the CTE strategy.
fn synth_view_table_def(name: &str, columns: &[String], rows: &[Vec<SqlValue>]) -> Arc<TableDef> {
    let column_defs: Vec<ColumnDef> = columns
        .iter()
        .enumerate()
        .map(|(idx, name)| ColumnDef {
            column_id: ColumnId((idx + 1) as u64),
            ordinal: idx as u16,
            name: Box::from(name.as_str()),
            folded: Box::from(name.to_ascii_lowercase().as_str()),
            declared_type: None,
            affinity: infer_affinity(rows, idx),
            not_null: false,
            default_value: None,
            default_expr: None,
            generated: None,
            collation: None,
        })
        .collect();
    let rel = next_view_rel_id();
    Arc::new(TableDef {
        table_id: TableId(rel.0),
        schema_id: SchemaId(0),
        relation_id: rel,
        name: Box::from(name),
        folded: Box::from(name.to_ascii_lowercase().as_str()),
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

/// Return a "cannot modify a view" error matching SQLite's wording.
pub(crate) fn cannot_modify_view_error(name: &str) -> Error {
    Error::UnsupportedSql(format!("cannot modify {name} because it is a view"))
}

/// True if `name` resolves to a view in the current schema snapshot.
/// Used by DML binders to reject `INSERT/UPDATE/DELETE` against views.
pub(crate) fn name_is_view(schema: &SchemaSnapshot, name: &sqlparser::ast::ObjectName) -> bool {
    lookup_in_schema(schema, name).is_some()
}

pub(crate) fn view_column_names(
    conn: &Connection,
    name: &sqlparser::ast::ObjectName,
) -> Result<Vec<String>> {
    let schema = conn.schema_snapshot();
    let Some(view) = lookup_in_schema(&schema, name) else {
        return Err(Error::UnknownTable(name.to_string()));
    };
    if !view.columns.is_empty() {
        return Ok(view.columns.iter().map(|c| c.as_ref().to_owned()).collect());
    }
    let _own_scope = super::cte::Isolated::enter();
    let template = crate::parser::parse_prepared_template(conn, view.body_sql.as_ref())?;
    Ok(template.output_columns.iter().cloned().collect())
}
