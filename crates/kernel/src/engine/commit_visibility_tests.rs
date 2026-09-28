//! COMMIT returns only once a new snapshot sees it (workplan R8).
//!
//! A snapshot sees every commit up to the published CSN, the highest CSN
//! below which every commit has published. A commit that publishes while an
//! earlier CSN is still between its WAL barrier and its publish is outside
//! every new snapshot until that earlier commit publishes too. These tests
//! park the earlier commit there with the per-thread commit-publish hook.

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use super::{CommitDurability, CommitOutcome, Engine, EngineConfig};
use crate::txn::Isolation;
use crate::wal::{WalConfig, set_before_commit_publish_hook};

/// Bounds every wait, so a deadlock fails the test instead of hanging it.
const DEADLINE: Duration = Duration::from_secs(20);
/// How long the later commit gets to (wrongly) return while the earlier one
/// is parked.
const EARLY_RETURN_WINDOW: Duration = Duration::from_millis(300);

fn config(durability: CommitDurability) -> EngineConfig {
    EngineConfig {
        commit_durability: durability,
        wal: WalConfig {
            group_commit_delay_us: 0,
            ..WalConfig::default()
        },
        ..EngineConfig::default()
    }
}

#[test]
fn commit_returns_after_new_snapshot_sees_it() {
    for durability in [
        CommitDurability::Strict,
        CommitDurability::Normal,
        CommitDurability::UnsafeDev,
    ] {
        let dir = TempDir::new().unwrap();
        let engine = Engine::create(dir.path(), config(durability)).unwrap();

        // A takes the lower CSN and parks after its WAL barrier, before it
        // publishes.
        let (parked, wait_parked) = mpsc::channel();
        let (release, wait_release) = mpsc::channel::<()>();
        let a_engine = Arc::clone(&engine);
        let a = thread::spawn(move || {
            let mut tx = a_engine.begin(Isolation::Snapshot).unwrap();
            let row = a_engine.insert(&mut tx, b"a".to_vec()).unwrap();
            set_before_commit_publish_hook(Some(Box::new(move || {
                parked.send(()).unwrap();
                wait_release
                    .recv_timeout(DEADLINE)
                    .expect("the test never released A");
            })));
            let outcome = a_engine.commit(tx);
            set_before_commit_publish_hook(None);
            (row, outcome)
        });
        wait_parked
            .recv_timeout(DEADLINE)
            .expect("A never reached its publish");

        // B takes the next CSN and commits while A is parked.
        let (returned, wait_returned) = mpsc::channel();
        let b_engine = Arc::clone(&engine);
        let b = thread::spawn(move || {
            let mut tx = b_engine.begin(Isolation::Snapshot).unwrap();
            let row = b_engine.insert(&mut tx, b"b".to_vec()).unwrap();
            let outcome = b_engine.commit(tx);
            returned.send(()).unwrap();
            // The same thread's next transaction must read its own commit.
            let mut next = b_engine.begin(Isolation::Snapshot).unwrap();
            let seen = b_engine.get(&mut next, row).unwrap();
            (row, outcome, seen)
        });
        let early = wait_returned.recv_timeout(EARLY_RETURN_WINDOW);

        // A snapshot taken now, before A publishes, must never see A or B.
        let mut older = engine.begin(Isolation::Snapshot).unwrap();
        release.send(()).unwrap();
        let (a_row, a_outcome) = a.join().expect("A panicked");
        if early.is_err() {
            wait_returned
                .recv_timeout(DEADLINE)
                .expect("B never returned after A published");
        }
        let (b_row, b_outcome, seen_by_b) = b.join().expect("B panicked");

        assert!(
            matches!(a_outcome, Ok(CommitOutcome::Committed(_))),
            "{durability:?}: A {a_outcome:?}"
        );
        assert!(
            matches!(b_outcome, Ok(CommitOutcome::Committed(_))),
            "{durability:?}: B {b_outcome:?}"
        );
        assert!(
            early.is_err(),
            "{durability:?}: B's commit returned while A, which holds an \
             earlier CSN, was still unpublished"
        );
        assert_eq!(
            seen_by_b.as_deref(),
            Some(&b"b"[..]),
            "{durability:?}: B's next transaction does not see B's commit"
        );
        let mut fresh = engine.begin(Isolation::Snapshot).unwrap();
        assert_eq!(
            engine.get(&mut fresh, a_row).unwrap().as_deref(),
            Some(&b"a"[..])
        );
        assert_eq!(
            engine.get(&mut fresh, b_row).unwrap().as_deref(),
            Some(&b"b"[..])
        );
        assert_eq!(
            engine.get(&mut older, a_row).unwrap(),
            None,
            "{durability:?}: an older snapshot changed"
        );
        assert_eq!(
            engine.get(&mut older, b_row).unwrap(),
            None,
            "{durability:?}: an older snapshot changed"
        );
    }
}

/// A commit waits for earlier CSNs only after it released its row locks, so
/// a writer queued on one of its rows goes ahead while it waits. Holding
/// them would stall that writer until its lock timeout, or deadlock if the
/// earlier commit ever needed one.
#[test]
fn waiting_commit_has_released_its_row_locks() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::create(dir.path(), config(CommitDurability::Strict)).unwrap();
    let rel_id = engine.config().rel_id;
    let mut setup = engine.begin(Isolation::Snapshot).unwrap();
    let row = engine.insert(&mut setup, b"v0".to_vec()).unwrap();
    engine.commit(setup).unwrap();

    let (parked, wait_parked) = mpsc::channel();
    let (release, wait_release) = mpsc::channel::<()>();
    let a_engine = Arc::clone(&engine);
    let a = thread::spawn(move || {
        let mut tx = a_engine.begin(Isolation::Snapshot).unwrap();
        a_engine.insert(&mut tx, b"a".to_vec()).unwrap();
        set_before_commit_publish_hook(Some(Box::new(move || {
            parked.send(()).unwrap();
            wait_release
                .recv_timeout(DEADLINE)
                .expect("the test never released A");
        })));
        let outcome = a_engine.commit(tx);
        set_before_commit_publish_hook(None);
        outcome
    });
    wait_parked
        .recv_timeout(DEADLINE)
        .expect("A never reached its publish");

    // B updates the row, holding its lock, and commits behind A.
    let (b_id, wait_b_id) = mpsc::channel();
    let (returned, wait_returned) = mpsc::channel();
    let b_engine = Arc::clone(&engine);
    let b = thread::spawn(move || {
        let mut tx = b_engine.begin(Isolation::Snapshot).unwrap();
        b_engine.update(&mut tx, row, b"v1".to_vec()).unwrap();
        b_id.send(tx.id()).unwrap();
        let outcome = b_engine.commit(tx);
        returned.send(()).unwrap();
        outcome
    });
    let b_id = wait_b_id.recv_timeout(DEADLINE).expect("B never updated");
    let published = std::time::Instant::now();
    while !matches!(engine.tx_state(b_id), crate::txn::TxState::Committed(_)) {
        assert!(published.elapsed() < DEADLINE, "B never published");
        thread::yield_now();
    }

    // C takes B's row lock while B still waits for A.
    let mut c = engine.begin(Isolation::Snapshot).unwrap();
    let locked = engine.lock_row_for_relation(&mut c, rel_id, row);
    let b_still_waiting = wait_returned.try_recv().is_err();
    drop(c);
    release.send(()).unwrap();
    let a_outcome = a.join().expect("A panicked");
    let b_outcome = b.join().expect("B panicked");

    assert_eq!(
        locked,
        Ok(()),
        "B kept its row lock while it waited for A's CSN"
    );
    assert!(b_still_waiting, "B returned before A published");
    assert!(matches!(a_outcome, Ok(CommitOutcome::Committed(_))));
    assert!(matches!(b_outcome, Ok(CommitOutcome::Committed(_))));
    let mut fresh = engine.begin(Isolation::Snapshot).unwrap();
    assert_eq!(
        engine.get(&mut fresh, row).unwrap().as_deref(),
        Some(&b"v1"[..])
    );
}
