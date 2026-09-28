use std::borrow::Borrow;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Instant;

use crate::format::{Csn, TxId};
use crate::txn::{Isolation, Snapshot, TxState};
use crate::{Error, Result};

use super::{Txn, TxnLifecycle};

#[cfg(test)]
thread_local! {
    static ACTIVE_UNREGISTERS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Active-snapshot removals on this thread since the last call.
#[cfg(test)]
pub(super) fn take_active_unregisters() -> u64 {
    ACTIVE_UNREGISTERS.with(|count| count.replace(0))
}

#[derive(Clone, Debug)]
pub struct ConcurrentTxStatus {
    inner: Arc<TxStatusInner>,
}

#[derive(Debug)]
pub(super) struct TxStatusInner {
    shards: Vec<RwLock<HashMap<TxId, TxState>>>,
    active_snapshots: Mutex<HashMap<TxId, Csn>>,
    next_tx: AtomicU64,
    next_csn: AtomicU64,
    published_csn: AtomicU64,
    frontier: Mutex<CsnFrontier>,
    /// Signalled, under `frontier`, when `published_csn` advances while a
    /// thread waits in [`ConcurrentTxStatus::wait_published`].
    frontier_advanced: Condvar,
}

#[derive(Debug, Default)]
struct CsnFrontier {
    pending: BTreeSet<u64>,
    completed: BTreeSet<u64>,
    skipped: BTreeSet<u64>,
    /// Threads in `wait_published`. A publish with none skips the notify.
    waiters: usize,
}

/// A commit sequence number reserved for one commit.
///
/// Snapshots see commits up to the published CSN, which advances only over
/// contiguous CSNs that published or were given up. Dropping the
/// reservation without [`ReservedCsn::publish`] gives the CSN up, so a
/// commit that fails or panics after reserving cannot hold every later
/// commit out of new snapshots, and out of `wait_published`, forever.
#[must_use = "dropping a reservation gives its CSN up"]
#[derive(Debug)]
pub(crate) struct ReservedCsn<'a> {
    txs: &'a ConcurrentTxStatus,
    csn: Csn,
    settled: bool,
}

impl ReservedCsn<'_> {
    pub(crate) fn csn(&self) -> Csn {
        self.csn
    }

    /// Mark `tx` committed at this CSN and publish it.
    pub(crate) fn publish(mut self, tx: TxId) {
        self.settled = true;
        self.txs.publish_commit(tx, self.csn);
    }
}

impl Borrow<Csn> for ReservedCsn<'_> {
    fn borrow(&self) -> &Csn {
        &self.csn
    }
}

impl Drop for ReservedCsn<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.txs.cancel_reserved_csn(self.csn);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxStatusStats {
    pub next_tx: TxId,
    pub next_csn: Csn,
    pub published_csn: Csn,
    pub active_transactions: usize,
    pub active_snapshots: usize,
    pub committed_states: usize,
    pub pending_csns: usize,
}

impl ConcurrentTxStatus {
    pub fn new() -> Self {
        Self::with_shards(64)
    }

    pub fn with_shards(shard_count: usize) -> Self {
        let shard_count = shard_count.max(1);
        let mut shards = Vec::with_capacity(shard_count);
        for _ in 0..shard_count {
            shards.push(RwLock::new(HashMap::new()));
        }
        Self {
            inner: Arc::new(TxStatusInner {
                shards,
                active_snapshots: Mutex::new(HashMap::new()),
                next_tx: AtomicU64::new(1),
                next_csn: AtomicU64::new(1),
                published_csn: AtomicU64::new(0),
                frontier: Mutex::new(CsnFrontier::default()),
                frontier_advanced: Condvar::new(),
            }),
        }
    }

    pub fn begin(&self) -> TxId {
        let tx = TxId(self.inner.next_tx.fetch_add(1, Ordering::Relaxed));
        self.inner.set_state(tx, TxState::InProgress);
        tx
    }

    pub(crate) fn begin_txn(&self, isolation: Isolation) -> Txn {
        let tx = self.begin();
        let snapshot = self.snapshot();
        let lifecycle = Arc::new(TxnLifecycle {
            tx_id: tx,
            inner: Arc::downgrade(&self.inner),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        self.inner.set_active_snapshot(tx, snapshot.visible_csn);
        Txn::new(tx, isolation, snapshot, lifecycle)
    }

    pub fn reserve_csn(&self) -> Csn {
        self.reserve_commit_csn()
    }

    pub fn reserve_commit_csn(&self) -> Csn {
        let csn = Csn(self.inner.next_csn.fetch_add(1, Ordering::SeqCst));
        let mut frontier = self
            .inner
            .frontier
            .lock()
            .expect("csn frontier mutex poisoned");
        frontier.pending.insert(csn.0);
        csn
    }

    /// Reserve the next CSN for a commit; see [`ReservedCsn`].
    pub(crate) fn reserve_commit(&self) -> ReservedCsn<'_> {
        ReservedCsn {
            txs: self,
            csn: self.reserve_commit_csn(),
            settled: false,
        }
    }

    /// Wait until the published CSN reaches `csn`, so that a snapshot taken
    /// afterwards sees the commit at `csn`. Returns false if `deadline`
    /// passes first; `None` waits without a bound.
    ///
    /// Every lower CSN must publish or be given up first. A committer holds
    /// no lock another committer needs between reserving and publishing, and
    /// a dropped [`ReservedCsn`] gives its CSN up, so the wait ends.
    pub fn wait_published(&self, csn: Csn, deadline: Option<Instant>) -> bool {
        if self.inner.published_csn.load(Ordering::Acquire) >= csn.0 {
            return true;
        }
        let mut frontier = self
            .inner
            .frontier
            .lock()
            .expect("csn frontier mutex poisoned");
        frontier.waiters += 1;
        let reached = loop {
            if self.inner.published_csn.load(Ordering::Acquire) >= csn.0 {
                break true;
            }
            frontier = match deadline {
                None => self
                    .inner
                    .frontier_advanced
                    .wait(frontier)
                    .expect("csn frontier mutex poisoned"),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        break false;
                    }
                    self.inner
                        .frontier_advanced
                        .wait_timeout(frontier, deadline - now)
                        .expect("csn frontier mutex poisoned")
                        .0
                }
            };
        };
        frontier.waiters -= 1;
        reached
    }

    pub fn publish_commit(&self, tx: TxId, csn: Csn) {
        self.inner.set_state(tx, TxState::Committed(csn));
        self.inner.complete_csn(csn);
        self.inner.unregister_active(tx);
    }

    pub fn cancel_reserved_csn(&self, csn: Csn) {
        self.inner.skip_csn(csn);
    }

    pub fn publish_recovered_commit(&self, tx: TxId, csn: Csn) {
        self.inner.set_state(tx, TxState::Committed(csn));
        advance_atomic_past(&self.inner.next_tx, tx.0);
        advance_atomic_past(&self.inner.next_csn, csn.0);
        self.inner.complete_csn(csn);
    }

    /// Keep `tx` from being handed out again. Recovery calls this for every
    /// transaction the WAL names, committed, rolled back or abandoned: none
    /// of them logs an abort, so a reused id that later commits would make
    /// the old transaction's logged changes and page tuples committed too.
    ///
    /// The last id has no successor to hand out next. Wrapping the counter
    /// would reissue every id from zero, so that fails as corrupt WAL. The
    /// last id is the index's non-transactional delete marker, never a
    /// transaction, so an id whose successor would be the marker fails the
    /// same way; recovery skips the marker itself.
    pub(crate) fn advance_next_tx_past(&self, tx: TxId) -> Result<()> {
        let next =
            tx.0.checked_add(1)
                .filter(|next| *next < crate::index::NON_TRANSACTIONAL_DELETE_TX.0)
                .ok_or(Error::CorruptWal(
                    "wal names a transaction id with no successor",
                ))?;
        advance_atomic_to_at_least(&self.inner.next_tx, next);
        Ok(())
    }

    /// Keep `csn` from being handed out again. Recovery calls this for the
    /// CSN of every commit record the WAL holds, including a commit it does
    /// not recover because it lies past the target or on an abandoned
    /// timeline, so a CSN names at most one commit in the log.
    pub(crate) fn advance_next_csn_past(&self, csn: Csn) {
        advance_atomic_past(&self.inner.next_csn, csn.0);
    }

    pub fn restore_frontier(&self, next_tx: TxId, next_csn: Csn, published_csn: Csn) {
        advance_atomic_to_at_least(&self.inner.next_tx, next_tx.0.max(1));
        advance_atomic_to_at_least(&self.inner.next_csn, next_csn.0.max(1));
        let frontier = self
            .inner
            .frontier
            .lock()
            .expect("csn frontier mutex poisoned");
        advance_atomic_to_at_least(&self.inner.published_csn, published_csn.0);
        self.inner.notify_waiters(&frontier);
    }

    /// Settle every CSN below the next one when recovery ends.
    ///
    /// Recovery published each commit it found. Any lower CSN it did not
    /// find belongs to a commit whose record never reached the WAL, whose
    /// reservation was given up before the crash, and it will never publish.
    /// Left pending, one such CSN below a recovered commit would keep that
    /// commit out of every snapshot. A CSN names no committed transaction
    /// unless recovery marked one, so settling the rest shows nothing new.
    pub(crate) fn seal_recovered_frontier(&self) {
        let mut frontier = self
            .inner
            .frontier
            .lock()
            .expect("csn frontier mutex poisoned");
        let last = self.inner.next_csn.load(Ordering::SeqCst).saturating_sub(1);
        frontier.pending.clear();
        frontier.completed.retain(|csn| *csn > last);
        frontier.skipped.retain(|csn| *csn > last);
        advance_atomic_to_at_least(&self.inner.published_csn, last);
        self.inner.advance_published_csn(&mut frontier);
        self.inner.notify_waiters(&frontier);
    }

    pub fn committed_states(&self) -> Vec<(TxId, Csn)> {
        let mut entries = Vec::new();
        for shard in &self.inner.shards {
            let shard = shard.read().expect("tx status shard poisoned");
            for (tx, state) in shard.iter() {
                if let TxState::Committed(csn) = state {
                    entries.push((*tx, *csn));
                }
            }
        }
        entries.sort_unstable_by_key(|(tx, _)| tx.0);
        entries
    }

    pub fn abort(&self, tx: TxId) {
        self.inner.abort(tx);
    }

    pub fn snapshot(&self) -> Snapshot {
        let next_tx = self.inner.next_tx.load(Ordering::SeqCst);
        Snapshot {
            visible_csn: Csn(self.inner.published_csn.load(Ordering::Acquire)),
            xmin: TxId(next_tx),
            xmax: TxId(next_tx),
            active: BTreeSet::new(),
        }
    }

    pub fn state(&self, tx: TxId) -> TxState {
        self.inner.state(tx)
    }

    pub fn is_tx_visible(&self, tx: TxId, snapshot: &Snapshot, owner: Option<TxId>) -> bool {
        if Some(tx) == owner {
            return true;
        }
        match self.state(tx) {
            TxState::Committed(csn) => csn <= snapshot.visible_csn,
            TxState::InProgress | TxState::Aborted => false,
        }
    }

    pub fn oldest_active_snapshot_csn(&self) -> Csn {
        let active = self
            .inner
            .active_snapshots
            .lock()
            .expect("active snapshot mutex poisoned");
        active
            .values()
            .copied()
            .min()
            .unwrap_or(Csn(self.inner.published_csn.load(Ordering::Acquire)))
    }

    pub fn next_tx(&self) -> TxId {
        TxId(self.inner.next_tx.load(Ordering::SeqCst))
    }

    pub fn next_csn(&self) -> Csn {
        Csn(self.inner.next_csn.load(Ordering::SeqCst))
    }

    pub fn published_csn(&self) -> Csn {
        Csn(self.inner.published_csn.load(Ordering::Acquire))
    }

    pub fn stats(&self) -> TxStatusStats {
        let mut active_transactions = 0_usize;
        let mut committed_states = 0_usize;
        for shard in &self.inner.shards {
            let shard = shard.read().expect("tx status shard poisoned");
            for state in shard.values() {
                match state {
                    TxState::InProgress => active_transactions += 1,
                    TxState::Committed(_) => committed_states += 1,
                    TxState::Aborted => {}
                }
            }
        }
        let active_snapshots = self
            .inner
            .active_snapshots
            .lock()
            .expect("active snapshot mutex poisoned")
            .len();
        let pending_csns = self
            .inner
            .frontier
            .lock()
            .expect("csn frontier mutex poisoned")
            .pending
            .len();
        TxStatusStats {
            next_tx: self.next_tx(),
            next_csn: self.next_csn(),
            published_csn: self.published_csn(),
            active_transactions,
            active_snapshots,
            committed_states,
            pending_csns,
        }
    }
}

impl TxStatusInner {
    pub(super) fn abort(&self, tx: TxId) {
        self.set_state(tx, TxState::Aborted);
        self.unregister_active(tx);
    }

    fn state(&self, tx: TxId) -> TxState {
        let shard = self.shard(tx).read().expect("tx status shard poisoned");
        shard.get(&tx).copied().unwrap_or(TxState::Aborted)
    }

    fn set_state(&self, tx: TxId, state: TxState) {
        let mut shard = self.shard(tx).write().expect("tx status shard poisoned");
        shard.insert(tx, state);
    }

    fn shard(&self, tx: TxId) -> &RwLock<HashMap<TxId, TxState>> {
        &self.shards[tx.0 as usize % self.shards.len()]
    }

    pub(super) fn set_active_snapshot(&self, tx: TxId, csn: Csn) {
        let mut active = self
            .active_snapshots
            .lock()
            .expect("active snapshot mutex poisoned");
        active.insert(tx, csn);
    }

    pub(super) fn unregister_active(&self, tx: TxId) {
        #[cfg(test)]
        ACTIVE_UNREGISTERS.with(|count| count.set(count.get() + 1));
        let mut active = self
            .active_snapshots
            .lock()
            .expect("active snapshot mutex poisoned");
        active.remove(&tx);
    }

    fn complete_csn(&self, csn: Csn) {
        let mut frontier = self.frontier.lock().expect("csn frontier mutex poisoned");
        frontier.pending.remove(&csn.0);
        frontier.completed.insert(csn.0);
        self.advance_published_csn(&mut frontier);
    }

    fn skip_csn(&self, csn: Csn) {
        let mut frontier = self.frontier.lock().expect("csn frontier mutex poisoned");
        frontier.pending.remove(&csn.0);
        frontier.skipped.insert(csn.0);
        self.advance_published_csn(&mut frontier);
    }

    fn advance_published_csn(&self, frontier: &mut CsnFrontier) {
        let mut published = self.published_csn.load(Ordering::Acquire);
        let start = published;
        loop {
            let next = published.saturating_add(1);
            if frontier.completed.remove(&next) || frontier.skipped.remove(&next) {
                published = next;
                self.published_csn.store(published, Ordering::Release);
            } else {
                break;
            }
        }
        if published != start {
            self.notify_waiters(frontier);
        }
    }

    /// Wake `wait_published` callers. The caller holds the frontier lock,
    /// which a waiter holds from its check until it sleeps, so none misses
    /// the advance.
    fn notify_waiters(&self, frontier: &CsnFrontier) {
        if frontier.waiters > 0 {
            self.frontier_advanced.notify_all();
        }
    }
}

fn advance_atomic_past(value: &AtomicU64, seen: u64) {
    advance_atomic_to_at_least(value, seen.saturating_add(1));
}

fn advance_atomic_to_at_least(value: &AtomicU64, target: u64) {
    let mut current = value.load(Ordering::SeqCst);
    while current < target {
        match value.compare_exchange(current, target, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
}

impl Default for ConcurrentTxStatus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use super::ConcurrentTxStatus;
    use crate::Error;
    use crate::format::{Csn, TxId};

    /// Workplan R8: a commit that publishes while a lower CSN is unpublished
    /// is outside new snapshots, and `wait_published` holds its caller until
    /// the lower CSN publishes.
    #[test]
    fn published_frontier_waits_for_lower_csn() {
        let txs = ConcurrentTxStatus::new();
        let (a, b) = (txs.begin(), txs.begin());
        let csn_a = txs.reserve_commit_csn();
        let csn_b = txs.reserve_commit_csn();
        assert!(csn_a < csn_b);

        txs.publish_commit(b, csn_b);
        assert!(txs.snapshot().visible_csn < csn_b);
        let started = Instant::now();
        assert!(!txs.wait_published(csn_b, Some(started + Duration::from_millis(50))));
        assert!(started.elapsed() >= Duration::from_millis(50));

        let waiter = {
            let txs = txs.clone();
            thread::spawn(move || {
                let reached =
                    txs.wait_published(csn_b, Some(Instant::now() + Duration::from_secs(20)));
                (reached, txs.snapshot().visible_csn)
            })
        };
        thread::sleep(Duration::from_millis(20));
        txs.publish_commit(a, csn_a);
        let (reached, visible) = waiter.join().unwrap();
        assert!(
            reached,
            "the waiter timed out after the lower CSN published"
        );
        assert!(visible >= csn_b, "the waiter woke before B was visible");
        assert!(txs.wait_published(csn_b, Some(Instant::now())));
        assert!(txs.wait_published(Csn(0), None));
    }

    /// A reservation dropped before it publishes gives its CSN up, so the
    /// frontier moves past it to a later commit.
    #[test]
    fn dropped_reservation_gives_its_csn_up() {
        let txs = ConcurrentTxStatus::new();
        let later_tx = txs.begin();
        let failed = txs.reserve_commit();
        let later = txs.reserve_commit();
        let later_csn = later.csn();
        later.publish(later_tx);
        assert!(txs.published_csn() < later_csn);
        drop(failed);
        assert_eq!(txs.published_csn(), later_csn);
    }

    /// A CSN recovery never found cannot hold back the commits after it.
    #[test]
    fn sealed_recovery_frontier_passes_a_csn_the_wal_lacks() {
        let txs = ConcurrentTxStatus::new();
        txs.publish_recovered_commit(TxId(1), Csn(1));
        txs.publish_recovered_commit(TxId(3), Csn(3));
        assert_eq!(txs.published_csn(), Csn(1));
        txs.seal_recovered_frontier();
        assert_eq!(txs.published_csn(), Csn(3));
        assert_eq!(txs.stats().pending_csns, 0);
        let csn = txs.reserve_commit_csn();
        assert_eq!(csn, Csn(4));
        txs.publish_commit(txs.begin(), csn);
        assert_eq!(txs.published_csn(), Csn(4));
    }

    #[test]
    fn advancing_past_a_tx_id_never_lowers_or_wraps_the_next_id() {
        let txs = ConcurrentTxStatus::new();
        txs.advance_next_tx_past(TxId(41)).unwrap();
        assert_eq!(txs.next_tx(), TxId(42));
        txs.advance_next_tx_past(TxId(7)).unwrap();
        assert_eq!(txs.next_tx(), TxId(42));
        txs.advance_next_tx_past(TxId(u64::MAX - 2)).unwrap();
        assert_eq!(txs.next_tx(), TxId(u64::MAX - 1));
        // u64::MAX is the index's non-transactional delete marker: neither
        // it nor an id whose successor it is may move the counter.
        for tx in [u64::MAX - 1, u64::MAX] {
            assert_eq!(
                txs.advance_next_tx_past(TxId(tx)),
                Err(Error::CorruptWal(
                    "wal names a transaction id with no successor"
                ))
            );
        }
        assert_eq!(txs.next_tx(), TxId(u64::MAX - 1));
    }
}
