//! When a SELECT may take the routed full scan (`morsel::route`) instead of
//! the access path the planner chose.
//!
//! The routed scan reads every row of the table in rowid order. It must not
//! pre-empt a lookup that returns the same rows in the same order without
//! reading the table: an integer primary-key equality, or an equality on
//! every key column of an index. EXPLAIN reports those lookups, so the
//! executor now does what EXPLAIN says.

use std::sync::Arc;

use redlinedb_kernel::catalog::TableDef;
use redlinedb_kernel::engine::Engine;
use sqlparser::ast::Expr;

use crate::error::Result;
use crate::statement::{SelectPlan, SelectSource, TableAccessHint};
use crate::value::SqlValue;

use super::index_access::{IndexProbe, try_match_index_access_hinted};
use super::selection_rowid_eq;

/// Whether `plan` may take the routed full scan: routing is on, the source
/// is a table, and neither a rowid point nor an index point answers it.
pub(super) fn routed_scan_table<'a>(
    engine: &Engine,
    plan: &'a SelectPlan,
    bindings: &[Option<SqlValue>],
) -> Result<Option<&'a Arc<TableDef>>> {
    if super::morsel::morsel_route_mode().is_none() {
        return Ok(None);
    }
    let SelectSource::Table(table) = &plan.source else {
        return Ok(None);
    };
    let hint = plan.table_hint.as_ref();
    if rowid_point_preempts_route(table, &plan.selection, bindings, hint)?
        || index_point_in_rowid_order(engine, table, &plan.selection, bindings, hint)
        || rowid_range_preempts_route(engine, table, &plan.selection, bindings, hint)?
    {
        return Ok(None);
    }
    Ok(Some(table))
}

/// A rowid range reads only the rows in it, in rowid order, the scan's
/// order. It runs when no index answers the query, so only then does it
/// take the scan's place; an index range would return another order.
fn rowid_range_preempts_route(
    engine: &Engine,
    table: &Arc<TableDef>,
    selection: &Option<Expr>,
    bindings: &[Option<SqlValue>],
    hint: Option<&TableAccessHint>,
) -> Result<bool> {
    if hint.is_some() || !super::bounds_rowid(table, selection, bindings)? {
        return Ok(false);
    }
    Ok(try_match_index_access_hinted(engine, table, selection, bindings, hint).is_none())
}

/// An integer primary-key equality returns at most one row. NOT INDEXED
/// does not take the rowid shortcut.
fn rowid_point_preempts_route(
    table: &Arc<TableDef>,
    selection: &Option<Expr>,
    bindings: &[Option<SqlValue>],
    hint: Option<&TableAccessHint>,
) -> Result<bool> {
    if matches!(hint, Some(TableAccessHint::NotIndexed)) {
        return Ok(false);
    }
    Ok(selection_rowid_eq(table, selection, bindings)?.is_some())
}

/// An equality on every key column of an index returns its rows in rowid
/// order, the scan's order: an index's physical key is the logical key
/// followed by the big-endian rowid, so the entries for one key are sorted
/// by rowid. Unique or not, and whatever conjuncts are left for the rows to
/// pass, the lookup yields the rows the scan would, in the same order.
///
/// Only a point qualifies. An equality on a prefix of the key, or a range,
/// yields rows sorted by the rest of the key, and a batched range recheck
/// groups them by heap page, so those keep the scan.
///
/// A partial index never qualifies: it holds only the rows its WHERE clause
/// admits, so the scan stays the reference for it. (A partial index is a
/// candidate at all only when the query repeats its WHERE clause, and that
/// conjunct is then a residual, so this costs nothing in practice.)
fn index_point_in_rowid_order(
    engine: &Engine,
    table: &Arc<TableDef>,
    selection: &Option<Expr>,
    bindings: &[Option<SqlValue>],
    hint: Option<&TableAccessHint>,
) -> bool {
    let Some(matched) = try_match_index_access_hinted(engine, table, selection, bindings, hint)
    else {
        return false;
    };
    matches!(matched.probe, IndexProbe::Point { .. })
        && matched.equality_prefix_len == matched.index.keys.len()
        && matched.index.predicate_sql.is_none()
        && matched.index.meta_page_id.is_some()
        && engine.index_handle(matched.index.index_id).is_some()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tempfile::tempdir;

    use crate::connection::{Connection, Database, DbOptions};
    use crate::statement::Step;
    use crate::value::SqlValue;

    use super::super::morsel::route::take_routed_full_scans;

    fn open() -> (tempfile::TempDir, Arc<Connection>) {
        let dir = tempdir().expect("scratch");
        let db = Database::create(
            dir.path().join("unique-point.db"),
            DbOptions {
                busy_timeout: Duration::from_secs(5),
                ..DbOptions::default()
            },
        )
        .expect("create");
        (dir, db.connect())
    }

    fn one_integer(conn: &Arc<Connection>, sql: &str) -> i64 {
        let _ = take_routed_full_scans();
        let mut stmt = conn.prepare(sql).expect("prepare");
        assert_eq!(stmt.step().expect("step"), Step::Row);
        let SqlValue::Integer(value) = stmt.column_value(0).expect("value").clone() else {
            panic!("expected integer");
        };
        assert_eq!(stmt.step().expect("done"), Step::Done);
        value
    }

    fn integers(conn: &Arc<Connection>, sql: &str) -> Vec<i64> {
        let mut stmt = conn.prepare(sql).expect("prepare");
        let mut out = Vec::new();
        while stmt.step().expect("step") == Step::Row {
            let SqlValue::Integer(value) = stmt.column_value(0).expect("value").clone() else {
                panic!("expected integer");
            };
            out.push(value);
        }
        out
    }

    #[test]
    fn unique_point_does_not_full_scan() {
        let (_dir, conn) = open();
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER)")
            .unwrap();
        conn.execute("CREATE UNIQUE INDEX t_k ON t(k)").unwrap();
        let mut insert = conn
            .prepare("INSERT INTO t(id, k, v) VALUES (?1, ?2, ?3)")
            .unwrap();
        for i in 0..40 {
            insert.bind_i64(1, i).unwrap();
            insert.bind_i64(2, i + 100).unwrap();
            insert.bind_i64(3, i + 1000).unwrap();
            assert_eq!(insert.step().unwrap(), Step::Done);
            insert.reset().unwrap();
        }
        let _ = take_routed_full_scans();
        assert_eq!(one_integer(&conn, "SELECT v FROM t WHERE k = 120"), 1020);
        assert_eq!(take_routed_full_scans(), 0);
    }

    #[test]
    fn non_unique_point_does_not_full_scan_and_keeps_rowid_order() {
        let (_dir, conn) = open();
        conn.execute("CREATE TABLE u(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER)")
            .unwrap();
        conn.execute("CREATE INDEX u_k ON u(k)").unwrap();
        // Rows go in with descending rowids, so neither insertion order nor
        // heap order is the rowid order the scan returns.
        let mut insert = conn
            .prepare("INSERT INTO u(id, k, v) VALUES (?1, ?2, ?1)")
            .unwrap();
        for i in (0..40).rev() {
            insert.bind_i64(1, i).unwrap();
            insert.bind_i64(2, if i % 2 == 0 { 7 } else { 8 }).unwrap();
            assert_eq!(insert.step().unwrap(), Step::Done);
            insert.reset().unwrap();
        }
        let even: Vec<i64> = (0..40).step_by(2).collect();
        let _ = take_routed_full_scans();
        assert_eq!(integers(&conn, "SELECT v FROM u WHERE k = 7"), even);
        assert_eq!(take_routed_full_scans(), 0);
        assert_eq!(
            integers(&conn, "SELECT v FROM u WHERE k = 7 AND v >= 0"),
            even,
            "a residual conjunct keeps the index point"
        );
        assert_eq!(take_routed_full_scans(), 0);
        assert_eq!(
            integers(&conn, "SELECT v FROM u NOT INDEXED WHERE k = 7"),
            even
        );
        assert_eq!(
            take_routed_full_scans(),
            1,
            "NOT INDEXED keeps the full scan"
        );
    }

    #[test]
    fn key_prefix_keeps_the_routed_scan() {
        let (_dir, conn) = open();
        conn.execute("CREATE TABLE w(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, v INTEGER)")
            .unwrap();
        conn.execute("CREATE INDEX w_ab ON w(a, b)").unwrap();
        for i in 0..40 {
            conn.execute(&format!("INSERT INTO w VALUES ({i}, 1, {}, {i})", 40 - i))
                .unwrap();
        }
        // The index would return these rows ordered by b, the reverse of
        // the scan's rowid order. (`v` keeps the index from covering the
        // query, which would answer it before routing is considered.)
        let _ = take_routed_full_scans();
        assert_eq!(
            integers(&conn, "SELECT v FROM w WHERE a = 1"),
            (0..40).collect::<Vec<i64>>()
        );
        assert_eq!(take_routed_full_scans(), 1);
    }

    #[test]
    fn partial_unique_point_keeps_the_routed_scan() {
        let (_dir, conn) = open();
        // The query repeats the index's WHERE clause and the point consumes
        // it, so before Q5-02 this unique point skipped the scan.
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER)")
            .unwrap();
        conn.execute("CREATE UNIQUE INDEX t_k5 ON t(k) WHERE k = 5")
            .unwrap();
        for i in 0..40 {
            let k = if i == 5 { 5 } else { i + 100 };
            conn.execute(&format!("INSERT INTO t VALUES ({i}, {k}, {i})"))
                .unwrap();
        }
        assert_eq!(one_integer(&conn, "SELECT v FROM t WHERE k = 5"), 5);
        assert_eq!(take_routed_full_scans(), 1);
    }

    #[test]
    fn primary_key_equality_does_not_full_scan() {
        let (_dir, conn) = open();
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER)")
            .unwrap();
        let mut insert = conn
            .prepare("INSERT INTO t(id, v) VALUES (?1, ?2)")
            .unwrap();
        for i in 0..40 {
            insert.bind_i64(1, i).unwrap();
            insert.bind_i64(2, i + 500).unwrap();
            assert_eq!(insert.step().unwrap(), Step::Done);
            insert.reset().unwrap();
        }
        let _ = take_routed_full_scans();
        assert_eq!(one_integer(&conn, "SELECT v FROM t WHERE id = 20"), 520);
        assert_eq!(take_routed_full_scans(), 0);

        let _ = take_routed_full_scans();
        let mut stmt = conn
            .prepare("SELECT v FROM t NOT INDEXED WHERE id = 20")
            .unwrap();
        assert_eq!(stmt.step().unwrap(), Step::Row);
        assert_eq!(stmt.step().unwrap(), Step::Done);
        assert_eq!(
            take_routed_full_scans(),
            1,
            "NOT INDEXED keeps the full scan"
        );
    }
}
