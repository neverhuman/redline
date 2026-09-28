//! Which journaled statements ROLLBACK TO may re-execute (S9-05).
//!
//! The kernel has no partial undo. ROLLBACK TO rolls the whole transaction
//! back and re-executes the statements journaled before the savepoint, in
//! the transaction's original snapshot. Re-execution is faithful only for a
//! statement that computes the same rows again from the same data. A
//! statement that read the clock, drew a random value, called a user
//! function, read `changes()` / `last_insert_rowid()` or a sequence, took an
//! advisory lock or read the transaction id, fired a trigger, returned rows,
//! or changed the schema is recorded as not replay-safe, and ROLLBACK TO
//! refuses to replay a prefix that holds one.

use std::cell::Cell;

use crate::statement::PreparedKind;

thread_local! {
    /// Set when the running statement did something a replay would not
    /// reproduce.
    static HAZARD: Cell<bool> = const { Cell::new(false) };
}

/// Note that the running statement cannot be replayed faithfully.
pub(crate) fn mark_hazard() {
    HAZARD.with(|hazard| hazard.set(true));
}

/// Watches one statement execution. Nested statements (trigger bodies,
/// function bodies) report to the statement that contains them.
pub(crate) struct HazardScope {
    outer: bool,
    clock_reads: u64,
}

impl HazardScope {
    pub(crate) fn begin() -> Self {
        Self {
            outer: HAZARD.with(|hazard| hazard.replace(false)),
            clock_reads: redlinedb_kernel::catalog::clock_reads(),
        }
    }

    /// Whether the watched execution did anything a replay would not
    /// reproduce, including a kernel `CURRENT_TIMESTAMP` default.
    pub(crate) fn finish(self) -> bool {
        let hazard =
            HAZARD.with(Cell::get) || redlinedb_kernel::catalog::clock_reads() != self.clock_reads;
        HAZARD.with(|slot| slot.set(self.outer || hazard));
        // Disarm Drop: the flag already holds the combined value.
        std::mem::forget(self);
        hazard
    }
}

impl Drop for HazardScope {
    /// An execution that failed still reports to its container.
    fn drop(&mut self) {
        let outer = self.outer;
        HAZARD.with(|hazard| hazard.set(outer || hazard.get()));
    }
}

/// Whether a statement of this kind can be re-executed at all: DML without
/// RETURNING and session settings can; DDL, RETURNING and anything else
/// cannot.
pub(crate) fn kind_is_replayable(kind: &PreparedKind) -> bool {
    match kind {
        PreparedKind::Insert(plan) => plan.returning.is_none(),
        PreparedKind::Update(plan) => plan.returning.is_none(),
        PreparedKind::Delete(plan) => plan.returning.is_none(),
        PreparedKind::InsertView(_)
        | PreparedKind::Merge(_)
        | PreparedKind::Pragma(_)
        | PreparedKind::Analyze(_)
        | PreparedKind::Reindex
        | PreparedKind::SetTransactionIsolation { .. }
        | PreparedKind::SetSearchPath { .. }
        | PreparedKind::SetPgCitext { .. }
        | PreparedKind::Listen { .. }
        | PreparedKind::Unlisten { .. }
        | PreparedKind::PgLockTable
        | PreparedKind::PgSearchNoop
        | PreparedKind::ShowVariable { .. }
        | PreparedKind::Select(_)
        | PreparedKind::Explain(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hazard_reaches_the_enclosing_scope_and_no_further() {
        let outer = HazardScope::begin();
        let inner = HazardScope::begin();
        mark_hazard();
        assert!(inner.finish());
        let sibling = HazardScope::begin();
        assert!(!sibling.finish());
        assert!(outer.finish());
        let next = HazardScope::begin();
        assert!(!next.finish());
        // A scope dropped on an error path still reports.
        let outer = HazardScope::begin();
        {
            let _failed = HazardScope::begin();
            mark_hazard();
        }
        assert!(outer.finish());
        // Nothing carries over into the next statement.
        let next = HazardScope::begin();
        assert!(!next.finish());
    }
}
