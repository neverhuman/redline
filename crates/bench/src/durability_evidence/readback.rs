//! The scripts the shipped shell runs, and the parser that turns a
//! recovery pass's output back into a
//! [`RecoveredState`](crate::recover::oracle::RecoveredState) for the
//! recover oracle.
//!
//! Output is sectioned by `.print @@<name>` lines. Every value is printed
//! as `hex(x)` next to `typeof(x)`, so a row can be rebuilt with its exact
//! type and bytes, and no value can break the line or column framing.

#[path = "readback/parse.rs"]
mod parse;

use std::fmt::Write as _;

use anyhow::{Result, bail};

use crate::engine::CellValue;
use crate::recover::oracle::{self, KV_INDEX, KV_TABLE, KV_TENANTS, PROGRESS_TABLE, Workload};

pub use parse::{PassObservation, parse_readback};

/// The workload the receipt runs: the recover oracle's `wal` scenario, one
/// kv row and one crash_progress row per transaction.
pub const WORKLOAD: Workload = Workload::RecoverWal;
pub const READY_MARKER: &str = "@@ready";
pub const MODE_MARKER: &str = "@@mode";
pub const ACK_PREFIX: &str = "ack|";

/// The long-running workload: create the schema, report the mode, print
/// READY, then one transaction per key, each followed by an ack line that
/// `.output stdout` flushes to the pipe at once. The ack for key `k` can
/// only reach the harness after `COMMIT` for `k` returned.
pub fn workload_script(rows: usize) -> Result<String> {
    let mut script = String::with_capacity(rows * 256 + 1024);
    script.push_str(".print @@mode\nPRAGMA redline_durability;\n");
    writeln!(
        script,
        "CREATE TABLE {KV_TABLE}(k INTEGER PRIMARY KEY, tenant INTEGER, v BLOB, version INTEGER);"
    )?;
    writeln!(script, "CREATE INDEX {KV_INDEX} ON {KV_TABLE}(tenant);")?;
    writeln!(
        script,
        "CREATE TABLE {PROGRESS_TABLE}(id INTEGER PRIMARY KEY, scenario TEXT, note TEXT);"
    )?;
    writeln!(script, ".print {READY_MARKER}\n.output stdout")?;
    for key in 0..rows as u64 {
        let kv = oracle::kv_row_values(WORKLOAD, key, rows);
        let progress = oracle::progress_row_values(WORKLOAD, key);
        writeln!(script, "BEGIN;")?;
        writeln!(
            script,
            "INSERT INTO {KV_TABLE}(k, tenant, v, version) VALUES ({});",
            literals(&kv)?
        )?;
        writeln!(
            script,
            "INSERT INTO {PROGRESS_TABLE}(id, scenario, note) VALUES ({});",
            literals(&progress)?
        )?;
        writeln!(script, "COMMIT;\nSELECT 'ack', {key};\n.output stdout")?;
    }
    Ok(script)
}

fn literals(values: &[CellValue]) -> Result<String> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(match value {
            CellValue::Null => "NULL".to_owned(),
            CellValue::Integer(v) => v.to_string(),
            CellValue::Text(v) => format!("'{}'", v.replace('\'', "''")),
            CellValue::Blob(v) => format!("X'{}'", hex_upper(v)),
            CellValue::Real(v) => bail!("the workload has no REAL values, got {v}"),
        });
    }
    Ok(out.join(", "))
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02X}");
    }
    out
}

/// What one recovery pass reads: mode, recovery report, schema, integrity
/// check, both tables in full, the index against a forced full scan, and
/// both query plans.
pub fn readback_script() -> String {
    let mut script = String::new();
    script.push_str(".print @@mode\nPRAGMA redline_durability;\n");
    script.push_str(".print @@report\nPRAGMA redline_recovery_report;\n");
    script.push_str(
        ".print @@schema\nSELECT name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name;\n",
    );
    script.push_str(".print @@integrity\nPRAGMA integrity_check;\n");
    let _ = writeln!(
        script,
        ".print @@{KV_TABLE}\nSELECT {} FROM {KV_TABLE} ORDER BY k;",
        typed(&["k", "tenant", "v", "version"])
    );
    let _ = writeln!(
        script,
        ".print @@{PROGRESS_TABLE}\nSELECT {} FROM {PROGRESS_TABLE} ORDER BY id;",
        typed(&["id", "scenario", "note"])
    );
    let _ = writeln!(
        script,
        ".print @@scan\nSELECT {} FROM {KV_TABLE} NOT INDEXED;",
        typed(&["k", "tenant"])
    );
    for tenant in 0..KV_TENANTS {
        let _ = writeln!(
            script,
            ".print @@probe {tenant}\nSELECT {} FROM {KV_TABLE} INDEXED BY {KV_INDEX} WHERE tenant = {tenant};",
            typed(&["k"])
        );
    }
    let _ = writeln!(
        script,
        ".print @@plan-probe\nEXPLAIN QUERY PLAN SELECT k FROM {KV_TABLE} INDEXED BY {KV_INDEX} WHERE tenant = 0;"
    );
    let _ = writeln!(
        script,
        ".print @@plan-scan\nEXPLAIN QUERY PLAN SELECT k, tenant FROM {KV_TABLE} NOT INDEXED;"
    );
    script.push_str(".print @@end\n");
    script
}

fn typed(columns: &[&str]) -> String {
    columns
        .iter()
        .map(|column| format!("hex({column}), typeof({column})"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recover::oracle::AckLedger;

    /// The output a correct shell prints for `keys`, built from the oracle's
    /// own row values.
    fn healthy_output(keys: std::ops::Range<u64>, rows: usize) -> String {
        let mut out = String::from("@@mode\nstrict\n@@report\nlatest||1|2|0|3|0|0|0|||||||||||\n");
        out.push_str("@@schema\ncrash_progress\nkv\nkv_tenant_idx\n@@integrity\nok\n@@kv\n");
        let hexed = |values: Vec<CellValue>| {
            values
                .iter()
                .map(|value| match value {
                    CellValue::Integer(v) => {
                        format!("{}|integer", hex_upper(v.to_string().as_bytes()))
                    }
                    CellValue::Text(v) => format!("{}|text", hex_upper(v.as_bytes())),
                    CellValue::Blob(v) => format!("{}|blob", hex_upper(v)),
                    other => panic!("{other:?}"),
                })
                .collect::<Vec<_>>()
                .join("|")
        };
        for key in keys.clone() {
            out.push_str(&hexed(oracle::kv_row_values(WORKLOAD, key, rows)));
            out.push('\n');
        }
        out.push_str("@@crash_progress\n");
        for key in keys.clone() {
            out.push_str(&hexed(oracle::progress_row_values(WORKLOAD, key)));
            out.push('\n');
        }
        out.push_str("@@scan\n");
        for key in keys.clone() {
            let row = vec![
                CellValue::Integer(key as i64),
                CellValue::Integer((key % KV_TENANTS) as i64),
            ];
            out.push_str(&hexed(row));
            out.push('\n');
        }
        for tenant in 0..KV_TENANTS {
            out.push_str(&format!("@@probe {tenant}\n"));
            for key in keys.clone().filter(|key| key % KV_TENANTS == tenant) {
                out.push_str(&hexed(vec![CellValue::Integer(key as i64)]));
                out.push('\n');
            }
        }
        out.push_str(
            "@@plan-probe\n1|0|0|SEARCH TABLE kv USING INDEX kv_tenant_idx: PointLookup\n",
        );
        out.push_str("@@plan-scan\n1|0|0|SCAN TABLE kv columns=[k, tenant]\n@@end\n");
        out
    }

    fn ledger(keys: std::ops::Range<u64>, rows: usize) -> AckLedger {
        let mut ledger = AckLedger::new(WORKLOAD, rows, true);
        for key in keys {
            ledger.ack(key);
        }
        ledger
    }

    fn graded(stdout: &str, acked: std::ops::Range<u64>) -> (PassObservation, bool) {
        let mut pass = parse_readback(stdout);
        pass.state.child_started = true;
        pass.state.fault_observed = true;
        pass.state.harness_errors = pass.errors.clone();
        let verdict = oracle::evaluate(&ledger(acked, 64), &pass.state);
        (pass, verdict.qualified)
    }

    #[test]
    fn a_healthy_readback_qualifies() {
        let (pass, qualified) = graded(&healthy_output(0..40, 64), 0..40);
        assert!(pass.errors.is_empty(), "{:?}", pass.errors);
        assert_eq!(pass.mode.as_deref(), Some("strict"));
        assert!(qualified);
        // The in-flight key may also be there.
        assert!(graded(&healthy_output(0..41, 64), 0..40).1);
    }

    #[test]
    fn a_lost_ack_a_changed_value_or_a_truncated_pass_disqualifies() {
        assert!(!graded(&healthy_output(0..39, 64), 0..40).1, "lost ack");
        let changed = healthy_output(0..40, 64)
            .replace(&hex_upper(b"value-00000007"), &hex_upper(b"value-0000000X"));
        assert!(!graded(&changed, 0..40).1, "changed value");
        let typed_wrong = healthy_output(0..40, 64).replacen("|blob", "|text", 1);
        assert!(!graded(&typed_wrong, 0..40).1, "blob read back as text");
        let truncated = healthy_output(0..40, 64).replace("@@end\n", "");
        assert!(!graded(&truncated, 0..40).1, "no @@end");
        let two_ahead = healthy_output(0..42, 64);
        assert!(
            !graded(&two_ahead, 0..40).1,
            "an unacked key past the in-flight one"
        );
    }

    #[test]
    fn workload_script_acks_each_key_after_its_commit() {
        let script = workload_script(3).expect("script");
        let commit = script
            .find("COMMIT;\nSELECT 'ack', 1;")
            .expect("ack after commit");
        let insert = script
            .find("VALUES (1, 1, X'")
            .expect("kv insert for key 1");
        assert!(insert < commit);
        assert_eq!(script.matches(".output stdout").count(), 4);
        assert!(script.contains("'ack-2'"));
    }
}
