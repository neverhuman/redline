//! Schema for executable beyond-SQLite oracle cases.
//!
//! Distinct from the SQLite-parity `Case` shape because the comparison
//! contract is different: there is no shipped `expected_stdout` — the
//! reference (psql) IS the expected output. Each case declares which
//! normalizers to apply (see `normalize::Normalizer`) and which engine
//! features it depends on (so cases for features the target hasn't shipped
//! can be skipped gracefully).

use serde::{Deserialize, Serialize};

use super::normalize::Normalizer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareMode {
    /// Row order must match between reference and target.
    OrderedRows,
    /// Compare row multisets (sort both before equality).
    SortedRows,
    /// Compare row sets (sort + dedupe both).
    Set,
}

impl Default for CompareMode {
    fn default() -> Self {
        Self::OrderedRows
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BeyondPriority {
    P0,
    P1,
    P2,
    P3,
    P4,
}

impl BeyondPriority {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
            Self::P4 => "P4",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeyondCase {
    pub id: u64,
    pub name: String,
    /// Maps back to the `rank` in `metadata/beyond_sqlite/features.json`.
    pub feature_rank: usize,
    pub category: String,
    pub priority: BeyondPriority,
    pub description: String,
    #[serde(default)]
    pub setup_stdin: Option<String>,
    pub stdin: String,
    /// Explicit reference outcome: successful SQL by default; reviewed negative
    /// fixtures declare psql's ON_ERROR_STOP exit code (3).
    #[serde(default)]
    pub expected_reference_exit: i32,
    /// What the target's stderr must contain for a case that BOTH engines
    /// reject to count as agreement.
    ///
    /// Absent -- the default, and the safe direction -- means a shared
    /// non-zero exit is never a pass. That default exists because the two
    /// engines can reject the same script for unrelated reasons: case 20021
    /// asks Postgres to refuse a bad enum value, and RedlineDB refuses the
    /// `DROP TYPE` in its *setup*, never reaching the statement under test.
    /// Counting that as agreement would be a false pass of exactly the kind
    /// this suite exists to catch.
    ///
    /// Declaring a value moves the burden to the corpus author, who has to
    /// write down the semantic message the engine is expected to produce. A
    /// capability gap ("unsupported sql: ...") cannot be spelled that way
    /// without the diff saying so out loud.
    #[serde(default)]
    pub expected_target_stderr_contains: Option<String>,
    #[serde(default)]
    pub compare_mode: CompareMode,
    #[serde(default)]
    pub value_normalizers: Vec<Normalizer>,
    /// Features the case depends on (e.g. ["jsonb", "for_update"]). The
    /// oracle skips a case whose required features the target doesn't
    /// advertise.
    #[serde(default)]
    pub requires_engine_features: Vec<String>,
    /// `SET pg_setting = value` pairs run on the reference connection
    /// before the case (e.g. `("timezone", "UTC")`).
    #[serde(default)]
    pub pg_settings: Vec<(String, String)>,
    /// Hard timeout in milliseconds; defaults to 5_000.
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    /// Optional bash script invoked instead of psql for multi-process
    /// cases. The script receives `$PG_URL`, `$RDB_BIN`, `$BEYOND_TMP`.
    #[serde(default)]
    pub script: Option<String>,
    /// Optional fixture files materialized into the case tempdir before
    /// running.
    #[serde(default)]
    pub files: Vec<(String, String)>,
    #[serde(default)]
    pub notes: String,
    /// Free-form tags useful for reports (e.g. `["mvcc", "for_update"]`).
    #[serde(default)]
    pub tags: Vec<String>,
}

impl BeyondCase {
    #[allow(dead_code)]
    pub fn display_id(&self) -> String {
        format!("BEYOND-{:05}", self.id)
    }

    pub fn timeout_ms(&self) -> u32 {
        self.timeout_ms.unwrap_or(5_000)
    }
}
