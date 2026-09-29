//! The oracle's decision for one run, and the one line that names every
//! reason it is not qualified.

use serde::{Deserialize, Serialize};

/// The oracle's decision for one run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryVerdict {
    pub acknowledged: usize,
    /// Acknowledged transactions recovered with their exact contents.
    pub recovered_acked: usize,
    /// The unacknowledged in-flight key, when it did commit.
    pub in_flight_committed: Option<u64>,
    pub lost_ack_ids: Vec<u64>,
    pub unexpected_effects: Vec<String>,
    pub partial_transactions: Vec<u64>,
    pub integrity_errors: Vec<String>,
    pub harness_errors: Vec<String>,
    pub fault_observed: bool,
    pub fault_required: bool,
    pub child_started: bool,
    pub qualified: bool,
}

impl RecoveryVerdict {
    /// One line naming every reason the run is not qualified.
    pub fn summary(&self) -> String {
        if self.qualified {
            return format!(
                "qualified: {} acked transactions recovered exactly{}",
                self.recovered_acked,
                match self.in_flight_committed {
                    Some(key) => format!(", in-flight key {key} committed atomically"),
                    None => String::new(),
                }
            );
        }
        let mut reasons = Vec::new();
        if !self.child_started {
            reasons.push("child never reported READY".to_owned());
        }
        if self.fault_required && !self.fault_observed {
            reasons.push("no fault was observed".to_owned());
        }
        if !self.lost_ack_ids.is_empty() {
            reasons.push(format!(
                "{} acked transactions lost or changed (first: {:?})",
                self.lost_ack_ids.len(),
                &self.lost_ack_ids[..self.lost_ack_ids.len().min(8)]
            ));
        }
        if !self.partial_transactions.is_empty() {
            reasons.push(format!(
                "partial transactions {:?}",
                &self.partial_transactions[..self.partial_transactions.len().min(8)]
            ));
        }
        for list in [
            &self.unexpected_effects,
            &self.integrity_errors,
            &self.harness_errors,
        ] {
            if let Some(first) = list.first() {
                reasons.push(format!("{first} ({} total)", list.len()));
            }
        }
        format!("not qualified: {}", reasons.join("; "))
    }
}
