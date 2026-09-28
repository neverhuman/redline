//! The scripts the shipped shell runs, and the parser that turns a
//! recovery pass's output back into a [`RecoveredState`] for the recover
//! oracle.
//!
//! Output is sectioned by `.print @@<name>` lines. Every value is printed
//! as `hex(x)` next to `typeof(x)`, so a row can be rebuilt with its exact
//! type and bytes, and no value can break the line or column framing.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};

use crate::engine::CellValue;
use crate::recover::oracle::{
    self, IndexCheck, KV_INDEX, KV_TABLE, KV_TENANTS, PROGRESS_TABLE, RecoveredState, Workload,
};

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

/// A parsed recovery pass.
#[derive(Debug, Clone, Default)]
pub struct PassObservation {
    pub mode: Option<String>,
    pub report: Vec<String>,
    pub state: RecoveredState,
    /// Output the parser could not account for. Any entry disqualifies.
    pub errors: Vec<String>,
}

/// Split output into `@@name` sections.
fn sections(stdout: &str, errors: &mut Vec<String>) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in stdout.lines() {
        if let Some(name) = line.strip_prefix("@@") {
            if out.contains_key(name) {
                errors.push(format!("section @@{name} appears twice"));
            }
            out.insert(name.to_owned(), Vec::new());
            current = Some(name.to_owned());
            continue;
        }
        match &current {
            Some(name) => out.entry(name.clone()).or_default().push(line.to_owned()),
            None => errors.push(format!("output before the first section: {line:?}")),
        }
    }
    out
}

pub fn parse_readback(stdout: &str) -> PassObservation {
    let mut errors = Vec::new();
    let sections = sections(stdout, &mut errors);
    let mut pass = PassObservation::default();
    if !sections.contains_key("end") {
        errors.push("the pass stopped before its last section (@@end missing)".to_owned());
    }
    let section = |name: &str| sections.get(name).map(Vec::as_slice).unwrap_or(&[]);
    pass.mode = single_line(section("mode"), "mode", &mut errors);
    pass.report = section("report").to_vec();
    let state = &mut pass.state;
    state.schema = section("schema").iter().cloned().collect();
    state.integrity_check = section("integrity").to_vec();

    for (table, width) in [(KV_TABLE, 4), (PROGRESS_TABLE, 3)] {
        let mut rows = BTreeMap::new();
        for line in section(table) {
            match typed_row(line, width).and_then(|row| Ok((key(&row)?, row))) {
                Ok((key, row)) => {
                    if rows.insert(key, oracle::row_digest(&row)).is_some() {
                        errors.push(format!("{table}: key {key} appears twice"));
                    }
                }
                Err(err) => errors.push(format!("{table} row {line:?}: {err:#}")),
            }
        }
        if state.schema.contains(table) {
            state.tables.insert(table.to_owned(), rows);
        } else if !rows.is_empty() {
            errors.push(format!("{table} returned rows but is not in the schema"));
        }
    }

    if state.schema.contains(KV_INDEX) {
        state.index_checks.push(index_check(&sections, &mut errors));
    }
    pass.errors = errors;
    pass
}

fn index_check(sections: &BTreeMap<String, Vec<String>>, errors: &mut Vec<String>) -> IndexCheck {
    let mut check = IndexCheck {
        index: KV_INDEX.to_owned(),
        table: KV_TABLE.to_owned(),
        ..IndexCheck::default()
    };
    let labels: Vec<String> = (0..KV_TENANTS).map(|tenant| tenant.to_string()).collect();
    for label in &labels {
        check.via_scan.insert(label.clone(), BTreeSet::new());
    }
    for line in sections.get("scan").map(Vec::as_slice).unwrap_or(&[]) {
        match typed_row(line, 2).and_then(|row| Ok((key(&row)?, label(&row[1])))) {
            Ok((key, probe)) => {
                if !check.via_scan.entry(probe.clone()).or_default().insert(key) {
                    errors.push(format!("scan for tenant {probe}: key {key} appears twice"));
                }
            }
            Err(err) => errors.push(format!("scan row {line:?}: {err:#}")),
        }
    }
    for label in &labels {
        let Some(lines) = sections.get(&format!("probe {label}")) else {
            errors.push(format!("probe {label} is missing"));
            continue;
        };
        let mut keys = BTreeSet::new();
        for line in lines {
            match typed_row(line, 1).and_then(|row| key(&row)) {
                Ok(key) => {
                    if !keys.insert(key) {
                        errors.push(format!("probe {label}: key {key} appears twice"));
                    }
                }
                Err(err) => errors.push(format!("probe {label} row {line:?}: {err:#}")),
            }
        }
        check.via_index.insert(label.clone(), keys);
    }
    let plan = |name: &str| {
        sections
            .get(name)
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| line.splitn(4, '|').last().unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    check.index_path_used = plan("plan-probe").contains(&format!("INDEX {KV_INDEX}"));
    let scan_plan = plan("plan-scan");
    check.scan_avoided_index = !scan_plan.is_empty() && !scan_plan.contains("INDEX");
    check
}

fn single_line(lines: &[String], name: &str, errors: &mut Vec<String>) -> Option<String> {
    match lines {
        [line] => Some(line.clone()),
        other => {
            errors.push(format!("section {name}: expected one line, got {other:?}"));
            None
        }
    }
}

/// `hex(a)|typeof(a)|hex(b)|typeof(b)|...` back into typed values.
fn typed_row(line: &str, width: usize) -> Result<Vec<CellValue>> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() != width * 2 {
        bail!("expected {} fields, got {}", width * 2, fields.len());
    }
    fields
        .chunks(2)
        .map(|pair| decode_cell(pair[0], pair[1]))
        .collect()
}

fn decode_cell(hex: &str, kind: &str) -> Result<CellValue> {
    let bytes = decode_hex(hex)?;
    Ok(match kind {
        "null" if bytes.is_empty() => CellValue::Null,
        "integer" => CellValue::Integer(
            std::str::from_utf8(&bytes)?
                .parse()
                .context("integer text")?,
        ),
        "real" => CellValue::Real(std::str::from_utf8(&bytes)?.parse().context("real text")?),
        "text" => CellValue::Text(String::from_utf8(bytes).context("text is not UTF-8")?),
        "blob" => CellValue::Blob(bytes),
        other => bail!("typeof {other:?} with hex {hex:?}"),
    })
}

fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        bail!("odd-length hex {hex:?}");
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).with_context(|| format!("hex {hex:?}")))
        .collect()
}

fn key(row: &[CellValue]) -> Result<u64> {
    match row.first() {
        Some(CellValue::Integer(v)) if *v >= 0 => Ok(*v as u64),
        other => bail!("expected a non-negative integer key, got {other:?}"),
    }
}

fn label(value: &CellValue) -> String {
    match value {
        CellValue::Integer(v) => v.to_string(),
        CellValue::Text(v) => v.clone(),
        other => format!("{other:?}"),
    }
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
