use anyhow::Result;

use crate::config::RecoveryScenarioKind;
use crate::engine;

use super::oracle::{self, Workload};

/// Run transaction `key` and return the digest of the rows it leaves
/// behind, which the child writes to its ack ledger after the commit.
/// Row values come from the oracle so the workload and the check that
/// grades it cannot drift apart.
pub fn commit_recovery_unit(
    engine: &dyn engine::BenchEngine,
    conn: &mut dyn engine::BenchConn,
    scenario: RecoveryScenarioKind,
    key: usize,
    total_rows: usize,
    checkpoint_every_rows: usize,
) -> Result<String> {
    let workload = Workload::from_scenario(scenario);
    let key_u64 = key as u64;
    conn.begin_immediate()?;
    match scenario {
        RecoveryScenarioKind::Wal | RecoveryScenarioKind::Checkpoint => {
            let params = oracle::kv_row_values(workload, key_u64, total_rows);
            let _ = conn.execute(
                "INSERT OR REPLACE INTO kv(k, tenant, v, version) VALUES (?1, ?2, ?3, ?4)",
                &params,
            )?;
        }
        RecoveryScenarioKind::Catalog => {
            recovery_catalog_unit(conn, key_u64)?;
        }
    }
    conn.execute(
        "INSERT OR REPLACE INTO crash_progress(id, scenario, note) VALUES (?1, ?2, ?3)",
        &oracle::progress_row_values(workload, key_u64),
    )?;
    conn.commit()?;
    if matches!(scenario, RecoveryScenarioKind::Checkpoint)
        && checkpoint_every_rows > 0
        && key > 0
        && key.is_multiple_of(checkpoint_every_rows)
    {
        engine.checkpoint()?;
    }
    let rows = oracle::expected_row_values(workload, key_u64, total_rows)
        .into_iter()
        .map(|(table, values)| (table, oracle::row_digest(&values)))
        .collect();
    Ok(oracle::txn_digest(&rows))
}

/// Create `scratch_{key % 8}` with an index, insert the key, and for even
/// keys drop the index and table again inside the same transaction.
fn recovery_catalog_unit(conn: &mut dyn engine::BenchConn, key: u64) -> Result<()> {
    let (table, values) = oracle::catalog_scratch_values(key);
    let index = oracle::scratch_index(key % oracle::CATALOG_SLOTS);
    conn.execute(
        &format!("CREATE TABLE IF NOT EXISTS {table}(id INTEGER PRIMARY KEY, note TEXT)"),
        &[],
    )?;
    conn.execute(
        &format!("CREATE INDEX IF NOT EXISTS {index} ON {table}(note)"),
        &[],
    )?;
    conn.execute(
        &format!("INSERT INTO {table}(id, note) VALUES (?1, ?2)"),
        &values,
    )?;
    if key.is_multiple_of(2) {
        let _ = conn.execute(&format!("DROP INDEX IF EXISTS {index}"), &[])?;
        let _ = conn.execute(&format!("DROP TABLE IF EXISTS {table}"), &[])?;
    }
    Ok(())
}

pub fn ensure_crash_schema(conn: &mut dyn engine::BenchConn) -> Result<()> {
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS crash_progress(id INTEGER PRIMARY KEY, scenario TEXT, note TEXT)",
        &[],
    )?;
    Ok(())
}
