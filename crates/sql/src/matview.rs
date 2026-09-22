//! Materialized views: a table of rows plus the query that fills it.
//!
//! `REFRESH` deletes and reinserts. `CONCURRENTLY` still requires a unique
//! index, matching Postgres, and then does the same replacement. The result
//! matches the shell corpus; the refresh is not run as a second transaction.

use std::collections::BTreeMap;

use std::sync::Arc;

use redlinedb_kernel::catalog::{CreateTableSpec, DbName};

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::exec::{execute_prepared, materialize_prepared_rows, with_session_reentrant};
use crate::parser::{build_ctas_columns, parse_prepared_template, replace_table_ident};
use crate::session::{MatViewDef, SessionState};
use crate::statement::{CreateTableAsSelectSpec, PreparedKind, PreparedTemplate};
use crate::value::{SqlValue, postgres_result_dialect};

pub(crate) fn create(conn: &Connection, name: &str, query: &str, populated: bool) -> Result<()> {
    let folded = require_ident(name)?;
    let select = select_template(conn, query)?;
    let PreparedKind::Select(plan) = select.kind.clone() else {
        return Err(not_select());
    };
    if populated {
        let columns = build_ctas_columns(&plan)?;
        execute_prepared(
            conn,
            &table_template(
                conn,
                query,
                PreparedKind::CreateTableAsSelect(CreateTableAsSelectSpec {
                    table: table_spec(&folded, columns),
                    select: Some(plan),
                }),
            ),
            &[],
        )?;
    } else {
        let columns = build_ctas_columns(&plan)?;
        execute_prepared(
            conn,
            &table_template(
                conn,
                query,
                PreparedKind::CreateTable(table_spec(&folded, columns)),
            ),
            &[],
        )?;
    }
    with_session_reentrant(conn, |session| {
        session.pg_matviews.insert(
            folded,
            MatViewDef {
                query_sql: query.to_owned(),
                populated,
            },
        );
        Ok(())
    })?;
    Ok(())
}

pub(crate) fn refresh(
    conn: &Connection,
    name: &str,
    concurrently: bool,
    no_data: bool,
) -> Result<()> {
    let folded = require_ident(name)?;
    let query = with_session_reentrant(conn, |session| {
        session
            .pg_matviews
            .get(&folded)
            .map(|view| view.query_sql.clone())
            .ok_or_else(|| missing(&folded))
    })?;
    if concurrently && !has_unique_index(conn, &folded) {
        return Err(Error::UnsupportedSql(format!(
            "cannot refresh materialized view \"{folded}\" concurrently"
        )));
    }
    delete_all(conn, &folded)?;
    if !no_data {
        let select = select_template(conn, &query)?;
        let rows = materialize_prepared_rows(conn, &select, &[])?;
        insert_rows(conn, &folded, &rows)?;
    }
    with_session_reentrant(conn, |session| {
        if let Some(view) = session.pg_matviews.get_mut(&folded) {
            view.populated = !no_data;
        }
        Ok(())
    })?;
    Ok(())
}

pub(crate) fn drop(conn: &Connection, name: &str, if_exists: bool, cascade: bool) -> Result<()> {
    let folded = require_ident(name)?;
    let targets = with_session_reentrant(conn, |session| {
        if !session.pg_matviews.contains_key(&folded) {
            return if if_exists {
                Ok(Vec::new())
            } else {
                Err(missing(&folded))
            };
        }
        Ok(if cascade {
            cascade_order(session, &folded)
        } else {
            vec![folded.clone()]
        })
    })?;
    for target in targets {
        let mut sql = String::from("DROP TABLE IF EXISTS ");
        sql.push_str(&target);
        run_sql(conn, &sql)?;
        with_session_reentrant(conn, |session| {
            session.pg_matviews.remove(&target);
            Ok(())
        })?;
    }
    Ok(())
}

pub(crate) fn rename(conn: &Connection, from: &str, to: &str) -> Result<()> {
    let from = require_ident(from)?;
    let to = require_ident(to)?;
    let exists =
        with_session_reentrant(conn, |session| Ok(session.pg_matviews.contains_key(&from)))?;
    if !exists {
        return Err(missing(&from));
    }
    let mut sql = String::from("ALTER TABLE ");
    sql.push_str(&from);
    sql.push_str(" RENAME TO ");
    sql.push_str(&to);
    run_sql(conn, &sql)?;
    with_session_reentrant(conn, |session| {
        if let Some(view) = session.pg_matviews.remove(&from) {
            session.pg_matviews.insert(to, view);
        }
        Ok(())
    })?;
    Ok(())
}

pub(crate) fn forget(session: &mut SessionState, folded: &str) {
    session.pg_matviews.remove(folded);
}

pub(crate) fn snapshot_tx(session: &mut SessionState) {
    session.pg_matviews_tx_snapshot = Some(session.pg_matviews.clone());
}

pub(crate) fn release_tx(session: &mut SessionState) {
    session.pg_matviews_tx_snapshot = None;
}

pub(crate) fn restore_tx(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_matviews_tx_snapshot.take() {
        session.pg_matviews = snapshot;
    }
}

pub(crate) fn restore_tx_keep(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_matviews_tx_snapshot.as_ref() {
        session.pg_matviews = snapshot.clone();
    }
}

/// `pg_matviews`, `pg_indexes`, and `relispopulated` embed the current
/// session state. Refresh does not change `schema_epoch`, so those reads
/// must not be reused from the statement cache.
pub(crate) fn sql_reads_matview_catalog(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    lower.contains("pg_matviews")
        || lower.contains("pg_indexes")
        || lower.contains("relispopulated")
}

pub(crate) fn rewrite_catalog(conn: &Connection, sql: &str) -> Option<String> {
    if !postgres_result_dialect() {
        return None;
    }
    let lower = sql.to_ascii_lowercase();
    let wants_views = from_table(&lower, "pg_matviews");
    let wants_indexes = from_table(&lower, "pg_indexes");
    if !wants_views && !wants_indexes {
        return None;
    }
    let (views, indexes) = catalog_rows(conn).ok()?;
    let mut out = sql.to_owned();
    if wants_views {
        out = replace_table_ident(&out, "pg_matviews", &matview_source(&views));
    }
    if wants_indexes {
        out = replace_table_ident(&out, "pg_indexes", &index_source(&indexes));
    }
    if out == sql { None } else { Some(out) }
}

pub(crate) fn populated_flags(session: &SessionState) -> BTreeMap<String, bool> {
    session
        .pg_matviews
        .iter()
        .map(|(name, view)| (name.clone(), view.populated))
        .collect()
}

fn catalog_rows(conn: &Connection) -> Result<(Vec<(String, bool)>, Vec<(String, String)>)> {
    let views = with_session_reentrant(conn, |session| {
        Ok(session
            .pg_matviews
            .iter()
            .map(|(name, view)| (name.clone(), view.populated))
            .collect::<Vec<_>>())
    })?;
    let snapshot = conn.schema_snapshot();
    let mut indexes = Vec::new();
    for table in snapshot.tables.iter() {
        for index in &table.indexes {
            indexes.push((
                index.name.as_ref().to_owned(),
                table.name.as_ref().to_owned(),
            ));
        }
    }
    Ok((views, indexes))
}

fn matview_source(views: &[(String, bool)]) -> String {
    if views.is_empty() {
        return "(SELECT NULL AS matviewname, NULL AS ispopulated WHERE 0) AS pg_matviews"
            .to_owned();
    }
    let mut sql = String::from("(VALUES ");
    for (idx, (name, populated)) in views.iter().enumerate() {
        if idx > 0 {
            sql.push_str(", ");
        }
        let flag = if *populated { "t" } else { "f" };
        sql.push_str("('");
        sql.push_str(&name.replace('\'', "''"));
        sql.push_str("', '");
        sql.push_str(flag);
        sql.push_str("')");
    }
    sql.push_str(") AS pg_matviews(matviewname, ispopulated)");
    sql
}

fn index_source(indexes: &[(String, String)]) -> String {
    if indexes.is_empty() {
        return "(SELECT NULL AS indexname, NULL AS tablename WHERE 0) AS pg_indexes".to_owned();
    }
    let mut sql = String::from("(VALUES ");
    for (idx, (index, table)) in indexes.iter().enumerate() {
        if idx > 0 {
            sql.push_str(", ");
        }
        sql.push_str("('");
        sql.push_str(&index.replace('\'', "''"));
        sql.push_str("', '");
        sql.push_str(&table.replace('\'', "''"));
        sql.push_str("')");
    }
    sql.push_str(") AS pg_indexes(indexname, tablename)");
    sql
}

fn has_unique_index(conn: &Connection, name: &str) -> bool {
    let snapshot = conn.schema_snapshot();
    snapshot.tables.iter().any(|table| {
        table.folded.eq_ignore_ascii_case(name) && table.indexes.iter().any(|index| index.unique)
    })
}

fn cascade_order(session: &SessionState, root: &str) -> Vec<String> {
    let mut doomed = vec![root.to_owned()];
    loop {
        let mut added = false;
        for (name, view) in &session.pg_matviews {
            if doomed.iter().any(|item| item == name) {
                continue;
            }
            if doomed.iter().any(|item| mentions(&view.query_sql, item)) {
                doomed.push(name.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    doomed.reverse();
    doomed
}

fn mentions(query: &str, name: &str) -> bool {
    let lower = query.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let needle = name.as_bytes();
    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let before = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after = i + needle.len();
            let after_ok = after == bytes.len() || !is_ident_byte(bytes[after]);
            if before && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn from_table(lower: &str, name: &str) -> bool {
    let mut needle = String::from(" from ");
    needle.push_str(name);
    let bytes = lower.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut i = 0usize;
    while i + needle_bytes.len() <= bytes.len() {
        if &bytes[i..i + needle_bytes.len()] == needle_bytes {
            let after = i + needle_bytes.len();
            if after == bytes.len() || !is_ident_byte(bytes[after]) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn missing(name: &str) -> Error {
    Error::UnsupportedSql(format!("materialized view \"{name}\" does not exist"))
}

fn not_select() -> Error {
    Error::UnsupportedSql("materialized view query must be a single SELECT".to_owned())
}

fn require_ident(name: &str) -> Result<String> {
    let folded = name.to_ascii_lowercase();
    let bytes = folded.as_bytes();
    let ok = !bytes.is_empty()
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
    if !ok {
        return Err(Error::UnsupportedSql(
            "materialized view name must be a simple identifier".to_owned(),
        ));
    }
    Ok(folded)
}

fn select_template(conn: &Connection, query: &str) -> Result<PreparedTemplate> {
    let template = parse_prepared_template(conn, query)?;
    if !matches!(template.kind, PreparedKind::Select(_)) {
        return Err(not_select());
    }
    Ok(template)
}

fn table_spec(name: &str, columns: Vec<redlinedb_kernel::catalog::ColumnSpec>) -> CreateTableSpec {
    CreateTableSpec {
        schema: None,
        name: DbName::new(name),
        if_not_exists: false,
        columns,
        constraints: Vec::new(),
        strict: false,
        without_rowid: false,
        normalized_sql: None,
    }
}

fn table_template(conn: &Connection, query: &str, kind: PreparedKind) -> PreparedTemplate {
    PreparedTemplate {
        sql: Arc::from(query),
        schema_epoch: conn.schema_epoch(),
        stats_epoch: 0,
        optimizer_hash: 0,
        param_layout: crate::statement::ParamLayout::default(),
        output_columns: Arc::from([]),
        readonly: false,
        kind,
    }
}

fn delete_all(conn: &Connection, table: &str) -> Result<()> {
    let table = require_ident(table)?;
    let mut sql = String::from("DELETE FROM ");
    sql.push_str(&table);
    run_sql(conn, &sql)
}

fn insert_rows(conn: &Connection, table: &str, rows: &[Vec<SqlValue>]) -> Result<()> {
    let table = require_ident(table)?;
    if rows.is_empty() {
        return Ok(());
    }
    let width = rows[0].len();
    let mut sql = String::from("INSERT INTO ");
    sql.push_str(&table);
    sql.push_str(" VALUES (");
    for idx in 0..width {
        if idx > 0 {
            sql.push(',');
        }
        sql.push('?');
    }
    sql.push(')');
    let template = parse_prepared_template(conn, &sql)?;
    for row in rows {
        let mut bindings = Vec::with_capacity(width + 1);
        bindings.push(None);
        for value in row {
            bindings.push(Some(value.clone()));
        }
        execute_prepared(conn, &template, &bindings)?;
    }
    Ok(())
}

fn run_sql(conn: &Connection, sql: &str) -> Result<()> {
    let template = parse_prepared_template(conn, sql)?;
    execute_prepared(conn, &template, &[])?;
    Ok(())
}
