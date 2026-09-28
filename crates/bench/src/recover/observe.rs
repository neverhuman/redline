//! Read a recovered database into a [`RecoveredState`] for the oracle.
//!
//! The observer never creates a database: a missing file after READY is a
//! failure, not "zero rows". Every keyed table is read in full, every
//! workload index is compared with a NOT INDEXED scan, and EXPLAIN QUERY
//! PLAN confirms which path each read took.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::config::{DurabilityKind, EngineKind, RunSpec, WorkloadKind};
use crate::engine::{self, BenchConn, CellValue};

use super::oracle::{self, IndexCheck, RecoveredState};

#[cfg(test)]
#[path = "observe_tests.rs"]
mod tests;

/// Where each engine keeps its database inside a child's `db_dir`.
pub(crate) fn database_path(engine: EngineKind, db_dir: &Path) -> PathBuf {
    match engine {
        EngineKind::Redline => db_dir.join("bench.redline"),
        EngineKind::Sqlite => db_dir.join("bench.sqlite3"),
    }
}

/// Open the database (running recovery), read it, optionally checkpoint,
/// and close it again. Returns a state with only the image fields set.
pub(crate) fn observe(
    engine_kind: EngineKind,
    durability: DurabilityKind,
    db_dir: &Path,
    checkpoint_after: bool,
) -> Result<RecoveredState> {
    let path = database_path(engine_kind, db_dir);
    if !path.exists() {
        bail!("database {} does not exist", path.display());
    }
    let spec = RunSpec {
        engine: engine_kind,
        workload: WorkloadKind::SingleRowInsert,
        durability,
        threads: 1,
        rows: 1,
        duration: Duration::from_secs(1),
        cache_bytes: 8 * 1024 * 1024,
        seed: 7,
        base_dir: db_dir.parent().unwrap_or(db_dir).to_path_buf(),
    };
    let engine = engine::open(&spec, db_dir)?;
    let mut state = {
        let mut conn = engine.connect(0)?;
        read_state(&mut *conn)?
    };
    if checkpoint_after {
        engine.checkpoint()?;
    }
    drop(engine);
    state.child_started = false;
    state.fault_observed = false;
    Ok(state)
}

fn read_state(conn: &mut dyn BenchConn) -> Result<RecoveredState> {
    let mut state = RecoveredState::default();
    for row in conn.query_all(
        "SELECT name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
        &[],
    )? {
        state.schema.insert(text(&row, 0)?);
    }
    state.integrity_check = conn
        .query_all("PRAGMA integrity_check", &[])?
        .iter()
        .map(|row| text(row, 0))
        .collect::<Result<_>>()?;

    let keyed: Vec<String> = state
        .schema
        .iter()
        .filter(|name| oracle::is_keyed_table(name))
        .cloned()
        .collect();
    for table in keyed {
        let columns = match table.as_str() {
            oracle::KV_TABLE => "k, tenant, v, version",
            oracle::PROGRESS_TABLE => "id, scenario, note",
            _ => "id, note",
        };
        let mut rows = BTreeMap::new();
        for row in conn.query_all(&format!("SELECT {columns} FROM {table} ORDER BY 1"), &[])? {
            let key = key(&row)?;
            if rows.insert(key, oracle::row_digest(&row)).is_some() {
                state.duplicates.push(format!("table {table}: key {key}"));
            }
        }
        state.tables.insert(table, rows);
    }

    if state.schema.contains(oracle::KV_INDEX) {
        let tenants: Vec<CellValue> = (0..oracle::KV_TENANTS as i64)
            .map(CellValue::Integer)
            .collect();
        state.index_checks.push(check_index(
            conn,
            oracle::KV_TABLE,
            oracle::KV_INDEX,
            "k",
            "tenant",
            tenants,
            &mut state.duplicates,
        )?);
    }
    for slot in 0..oracle::CATALOG_SLOTS {
        let table = oracle::scratch_table(slot);
        let index = oracle::scratch_index(slot);
        if state.schema.contains(&table) && state.schema.contains(&index) {
            state.index_checks.push(check_index(
                conn,
                &table,
                &index,
                "id",
                "note",
                Vec::new(),
                &mut state.duplicates,
            )?);
        }
    }
    Ok(state)
}

/// Compare `index` point lookups with a NOT INDEXED scan of `table`, for
/// every value in `extra_probes` and every value the scan finds. A key
/// either read returns twice goes to `duplicates`.
fn check_index(
    conn: &mut dyn BenchConn,
    table: &str,
    index: &str,
    pk: &str,
    column: &str,
    extra_probes: Vec<CellValue>,
    duplicates: &mut Vec<String>,
) -> Result<IndexCheck> {
    let scan_sql = format!("SELECT {pk}, {column} FROM {table} NOT INDEXED");
    let probe_sql = format!("SELECT {pk} FROM {table} INDEXED BY {index} WHERE {column} = ?1");
    let mut check = IndexCheck {
        index: index.to_owned(),
        table: table.to_owned(),
        ..IndexCheck::default()
    };
    let mut probes: BTreeMap<String, CellValue> = extra_probes
        .into_iter()
        .map(|value| (label(&value), value))
        .collect();
    for probe in probes.keys() {
        check.via_scan.insert(probe.clone(), BTreeSet::new());
    }
    for row in conn.query_all(&scan_sql, &[])? {
        let value = row.get(1).cloned().unwrap_or(CellValue::Null);
        let probe = label(&value);
        let key = key(&row)?;
        if !check.via_scan.entry(probe.clone()).or_default().insert(key) {
            duplicates.push(format!("scan of {table} for {column} = {probe}: key {key}"));
        }
        probes.entry(probe).or_insert(value);
    }
    for (probe, value) in &probes {
        let mut keys = BTreeSet::new();
        for row in conn.query_all(&probe_sql, std::slice::from_ref(value))? {
            let key = key(&row)?;
            if !keys.insert(key) {
                duplicates.push(format!("index {index} probe {column} = {probe}: key {key}"));
            }
        }
        check.via_index.insert(probe.clone(), keys);
    }
    let sample = probes
        .values()
        .next()
        .cloned()
        .unwrap_or(CellValue::Integer(0));
    let probe_plan = plan(conn, &probe_sql, &[sample])?;
    check.index_path_used = probe_plan.contains(&format!("INDEX {index}"));
    check.scan_avoided_index = !plan(conn, &scan_sql, &[])?.contains("INDEX");
    Ok(check)
}

fn plan(conn: &mut dyn BenchConn, sql: &str, params: &[CellValue]) -> Result<String> {
    let rows = conn.query_all(&format!("EXPLAIN QUERY PLAN {sql}"), params)?;
    let mut out = String::new();
    for row in rows {
        if let Some(CellValue::Text(detail)) = row.last() {
            out.push_str(detail);
            out.push('\n');
        }
    }
    Ok(out)
}

fn label(value: &CellValue) -> String {
    match value {
        CellValue::Integer(v) => v.to_string(),
        CellValue::Text(v) => v.clone(),
        other => format!("{other:?}"),
    }
}

fn key(row: &[CellValue]) -> Result<u64> {
    match row.first() {
        Some(CellValue::Integer(v)) if *v >= 0 => Ok(*v as u64),
        other => bail!("expected a non-negative integer key, got {other:?}"),
    }
}

fn text(row: &[CellValue], idx: usize) -> Result<String> {
    match row.get(idx) {
        Some(CellValue::Text(v)) => Ok(v.clone()),
        other => bail!("expected text in column {idx}, got {other:?}"),
    }
}
