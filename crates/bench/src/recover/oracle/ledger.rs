//! The ack ledger: what the child acknowledged, one `key<TAB>digest` line
//! per committed transaction, parsed so that it fails closed.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};

use super::{Workload, expected_txn_digest};

/// What the child acknowledged, plus what the run expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckLedger {
    pub workload: Workload,
    pub total_rows: usize,
    /// Acknowledged key -> transaction digest the child recorded.
    pub acked: BTreeMap<u64, String>,
    /// The run is only evidence when the harness observed the fault.
    pub expect_fault: bool,
    /// Bytes after the last newline: an ack the child never finished
    /// writing, so that key counts as in flight, not acknowledged.
    pub torn_tail: Option<String>,
}

impl AckLedger {
    pub fn new(workload: Workload, total_rows: usize, expect_fault: bool) -> Self {
        Self {
            workload,
            total_rows,
            acked: BTreeMap::new(),
            expect_fault,
            torn_tail: None,
        }
    }

    /// Record `key` as acknowledged with its expected digest.
    pub fn ack(&mut self, key: u64) {
        let digest = expected_txn_digest(self.workload, key, self.total_rows);
        self.acked.insert(key, digest);
    }

    /// Parse a ledger. Fails closed on any complete line that is malformed,
    /// out of order, or whose digest differs from the workload's contents
    /// for that key (for example a child built from other sources).
    pub fn parse(
        workload: Workload,
        total_rows: usize,
        expect_fault: bool,
        text: &str,
    ) -> Result<Self> {
        let mut ledger = Self::new(workload, total_rows, expect_fault);
        let (complete, tail) = match text.rfind('\n') {
            Some(end) => (Some(&text[..end]), &text[end + 1..]),
            None => (None, text),
        };
        if !tail.is_empty() {
            ledger.torn_tail = Some(tail.to_owned());
        }
        for (line_no, line) in complete.into_iter().flat_map(|c| c.split('\n')).enumerate() {
            let line_no = line_no + 1;
            let Some((key, digest)) = line.split_once('\t') else {
                bail!("ack ledger line {line_no}: expected `key<TAB>digest`, got {line:?}");
            };
            let key: u64 = key
                .parse()
                .with_context(|| format!("ack ledger line {line_no}: key {key:?}"))?;
            if key != line_no as u64 - 1 {
                bail!(
                    "ack ledger line {line_no}: key {key} out of order (children ack keys 0,1,2,... in order)"
                );
            }
            let expected = expected_txn_digest(workload, key, total_rows);
            if digest != expected {
                bail!(
                    "ack ledger line {line_no}: digest {digest} for key {key} does not match the workload contents {expected}"
                );
            }
            ledger.acked.insert(key, digest.to_owned());
        }
        Ok(ledger)
    }

    /// The one transaction that may have committed without an ack.
    pub fn in_flight_key(&self) -> Option<u64> {
        let next = self.acked.keys().next_back().map_or(0, |last| last + 1);
        (next < self.total_rows as u64).then_some(next)
    }
}
