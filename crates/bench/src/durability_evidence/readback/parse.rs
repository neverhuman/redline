//! The parser that turns a recovery pass's sectioned output back into a
//! [`RecoveredState`] for the recover oracle, each row rebuilt from its
//! `hex(x)` and `typeof(x)` pairs.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};

use crate::engine::CellValue;
use crate::recover::oracle::{
    self, IndexCheck, KV_INDEX, KV_TABLE, KV_TENANTS, PROGRESS_TABLE, RecoveredState,
};

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
