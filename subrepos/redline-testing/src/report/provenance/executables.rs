//! The executables a run measured with, as its official evidence names
//! them, and the check that every executed raw sample names the same ones.

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use super::Executable;
use crate::report::types::RawRecord;

/// `section`'s (path, sha256, version) from the run evidence; missing,
/// empty or `<unknown>` values and malformed digests are errors.
pub(super) fn executable(run: &Value, section: &str, keys: [&str; 3]) -> Result<Executable> {
    let [path, sha256, version] = keys.map(|key| {
        run.get(section)
            .and_then(|entry| entry.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty() && *text != "<unknown>")
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("official evidence {section}.{key} is missing or unknown"))
    });
    let sha256 = sha256?;
    if !is_sha256(&sha256) {
        bail!(
            "official evidence {section}.{} is not a SHA-256 digest: {sha256:?}",
            keys[1]
        );
    }
    Ok(Executable {
        path: path?,
        sha256,
        version: version?,
    })
}

/// Every executed sample must name the executables the evidence names.
pub(super) fn check_raw_identity(
    suite: &str,
    records: &[RawRecord],
    target: &Executable,
    sqlite: &Executable,
) -> Result<()> {
    // Beyond-SQLite records describe a PostgreSQL oracle, not sqlite3.
    let sqlite_reference = suite != "beyond_sqlite";
    for record in records.iter().filter(|record| record.status != "skipped") {
        let mut checks = vec![(
            "target_executable_sha256",
            &record.target_executable_sha256,
            &target.sha256,
        )];
        if sqlite_reference {
            checks.push((
                "reference_executable_sha256",
                &record.reference_executable_sha256,
                &sqlite.sha256,
            ));
            checks.push((
                "reference_version",
                &record.reference_version,
                &sqlite.version,
            ));
        }
        for (field, actual, expected) in checks {
            if actual != expected {
                bail!(
                    "raw record {} ({}): {field} {actual:?} does not match the official evidence {expected:?}",
                    record.case_id,
                    record.sample_role
                );
            }
        }
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
