//! Which engine runs first within a sample (BM3-04).
//!
//! The runner used to start every sample with the SQLite reference and
//! then run the target, so any cost of going second (a warmer page cache,
//! a CPU the first run just left busy) always fell on the same engine.
//! `--order` makes that choice explicit and records it on every raw
//! record: `sqlite-first` (the default, the correctness lane's historical
//! order), `target-first`, or `alternate`, which starts even sample
//! indexes (counting warmups) with the reference and odd ones with the
//! target.

use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementOrder {
    #[default]
    SqliteFirst,
    TargetFirst,
    Alternate,
}

/// The engine a sample ran first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstEngine {
    Reference,
    Target,
}

impl MeasurementOrder {
    pub fn first_engine(self, sample_index: usize) -> FirstEngine {
        match self {
            Self::SqliteFirst => FirstEngine::Reference,
            Self::TargetFirst => FirstEngine::Target,
            Self::Alternate if sample_index.is_multiple_of(2) => FirstEngine::Reference,
            Self::Alternate => FirstEngine::Target,
        }
    }

    /// The name records and manifests carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SqliteFirst => "sqlite_first",
            Self::TargetFirst => "target_first",
            Self::Alternate => "alternate",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FirstEngine, MeasurementOrder};

    #[test]
    fn alternate_starts_even_samples_with_the_reference() {
        let firsts = (0..4)
            .map(|index| MeasurementOrder::Alternate.first_engine(index))
            .collect::<Vec<_>>();
        assert_eq!(
            firsts,
            [
                FirstEngine::Reference,
                FirstEngine::Target,
                FirstEngine::Reference,
                FirstEngine::Target
            ]
        );
        assert_eq!(
            MeasurementOrder::default().first_engine(1),
            FirstEngine::Reference
        );
        assert_eq!(
            MeasurementOrder::TargetFirst.first_engine(0),
            FirstEngine::Target
        );
        assert_eq!(
            serde_json::to_value(MeasurementOrder::Alternate).expect("serialize"),
            MeasurementOrder::Alternate.as_str()
        );
    }
}
