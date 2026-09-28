//! `PRAGMA redline_recovery_report`: what the crash recovery that opened
//! this database found and did (workplan R9, step 4). Also `PRAGMA
//! redline_durability`, the commit durability in force (workplan R10).
//!
//! One row for a database this process opened from disk, none for one it
//! created or holds only in memory. Counts and LSNs are integers; a value
//! that does not apply is NULL.

use std::path::Path;
use std::sync::Arc;

use redlinedb_kernel::engine::{CommitDurability, RecoveryReport, RecoveryTarget};

use crate::connection::Connection;
use crate::value::SqlValue;

pub(super) const COLUMNS: &[&str] = &[
    "target",
    "checkpoint_generation",
    "scanned_records",
    "valid_end_lsn",
    "replay_from_lsn",
    "commits_recovered",
    "page_images_redone",
    "legacy_mutations_redone",
    "torn_tail",
    "torn_tail_lsn",
    "torn_tail_segment",
    "torn_tail_offset",
    "torn_tail_bytes",
    "torn_tail_reason",
    "salvage",
    "skipped_generation",
    "fork_lsn",
    "fork_record_lsn",
    "abandoned_wal",
    "warnings",
];

/// `PRAGMA redline_durability`: the commit durability the engine applies
/// now, named as `REDLINEDB_DEFAULT_DURABILITY` names it. `PRAGMA
/// synchronous` cannot answer this: it keeps SQLite's per-connection recall
/// value, which starts at FULL whatever mode the database was opened in.
pub(super) fn durability(conn: &Connection) -> SqlValue {
    text(match conn.engine().commit_durability() {
        CommitDurability::Strict => "strict",
        CommitDurability::Normal => "normal",
        CommitDurability::UnsafeDev => "unsafe_dev",
    })
}

pub(super) fn rows(conn: &Connection) -> Vec<Vec<SqlValue>> {
    conn.engine()
        .last_recovery_report()
        .map(|report| vec![row(&report, conn.database_path())])
        .unwrap_or_default()
}

fn row(report: &RecoveryReport, root: &Path) -> Vec<SqlValue> {
    let tail = report.tail.as_ref();
    let fork = report.timeline_fork.as_ref();
    let salvage = tail
        .map(|tail| {
            tail.salvage
                .iter()
                .map(|path| {
                    path.strip_prefix(root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let abandoned = report
        .abandoned_wal
        .iter()
        .map(|(from, to)| format!("{}-{}", from.0, to.0))
        .collect::<Vec<_>>();
    vec![
        text(&match report.target {
            RecoveryTarget::Latest => "latest".to_owned(),
            RecoveryTarget::Lsn(lsn) => format!("lsn {}", lsn.0),
            RecoveryTarget::Csn(csn) => format!("csn {}", csn.0),
        }),
        optional(report.checkpoint_generation),
        integer(report.scanned_records as u64),
        integer(report.valid_end_lsn.0),
        integer(report.replay_from_lsn.0),
        integer(report.commits_recovered as u64),
        integer(report.page_images_redone as u64),
        integer(report.legacy_mutations_redone as u64),
        SqlValue::Integer(i64::from(report.torn_tail)),
        optional(tail.map(|tail| tail.lsn.0)),
        optional(tail.map(|tail| tail.segment)),
        optional(tail.map(|tail| tail.offset)),
        optional(tail.map(|tail| tail.bytes)),
        tail.map_or(SqlValue::Null, |tail| text(tail.reason.as_str())),
        joined(&salvage, ", "),
        optional(report.skipped_generation),
        optional(fork.map(|fork| fork.fork_lsn.0)),
        optional(fork.map(|fork| fork.record_lsn.0)),
        joined(&abandoned, ", "),
        joined(&report.warnings, "; "),
    ]
}

fn integer(value: u64) -> SqlValue {
    SqlValue::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

fn optional(value: Option<u64>) -> SqlValue {
    value.map_or(SqlValue::Null, integer)
}

fn text(value: &str) -> SqlValue {
    SqlValue::Text(Arc::from(value))
}

fn joined(values: &[String], separator: &str) -> SqlValue {
    if values.is_empty() {
        SqlValue::Null
    } else {
        text(&values.join(separator))
    }
}
