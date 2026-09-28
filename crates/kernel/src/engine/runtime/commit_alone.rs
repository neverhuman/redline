//! A commit that must be the only transaction in the engine.
//!
//! `REINDEX` fills a new B-tree from the rows its snapshot sees and swaps it
//! in at COMMIT. Another transaction open meanwhile would go wrong: a reader
//! whose snapshot predates the swap cannot see the entries the rebuild wrote,
//! and a writer's changes land in the B-tree the swap retires. SQLite takes
//! an exclusive lock for REINDEX; [`Engine::commit_alone`] is that lock at
//! commit time.

use std::time::{Duration, Instant};

use crate::format::Csn;
use crate::{Error, Result};

use super::{CommitOutcome, Engine, Txn};

impl Engine {
    /// Commit `tx` only while it is the one open transaction and nothing
    /// committed since `since`, the snapshot its rebuild read. New
    /// transactions wait to begin until this returns. Others still open are
    /// waited for up to the busy timeout; if one is still open then, or any
    /// transaction committed after `since`, `tx` rolls back and the error is
    /// [`Error::LockTimeout`] (the database is busy; retry).
    pub fn commit_alone(&self, tx: Txn, since: Csn) -> Result<CommitOutcome> {
        let _alone = self.locks.begin_exclusive();
        let deadline = Instant::now() + self.locks.timeout();
        loop {
            if self.txs.published_csn() != since {
                break;
            }
            if self.txs.active_transaction_count() <= 1 {
                return self.commit(tx);
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.rollback(tx)?;
        Err(Error::LockTimeout)
    }
}
