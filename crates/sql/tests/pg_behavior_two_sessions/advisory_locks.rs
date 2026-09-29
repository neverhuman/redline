//! PG-01 advisory locks across two sessions: contention, waiting for the
//! session that has the lock, the busy timeout, and release when a session
//! is dropped.

use super::*;

#[test]
fn advisory_locks_contend_between_sessions() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();

    assert_eq!(one(&a, "SELECT pg_advisory_lock(1)"), void());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(1)"), f());
    // B does not hold key 1, so it cannot release it.
    assert_eq!(one(&b, "SELECT pg_advisory_unlock(1)"), f());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(1)"), f());

    // The lock is reentrant: two locks need two unlocks.
    assert_eq!(one(&a, "SELECT pg_advisory_lock(1)"), void());
    assert_eq!(one(&a, "SELECT pg_try_advisory_lock(1)"), t());
    assert_eq!(one(&a, "SELECT pg_advisory_unlock(1)"), t());
    assert_eq!(one(&a, "SELECT pg_advisory_unlock(1)"), t());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(1)"), f());
    assert_eq!(one(&a, "SELECT pg_advisory_unlock(1)"), t());
    assert_eq!(one(&a, "SELECT pg_advisory_unlock(1)"), f());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(1)"), t());
    assert_eq!(one(&a, "SELECT pg_try_advisory_lock(1)"), f());
    assert_eq!(one(&b, "SELECT pg_advisory_unlock(1)"), t());

    // The two-integer form is a separate key space.
    assert_eq!(one(&a, "SELECT pg_advisory_lock(0, 1)"), void());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(1)"), t());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(0, 1)"), f());
    assert_eq!(one(&a, "SELECT pg_advisory_unlock(0, 1)"), t());
    assert_eq!(one(&b, "SELECT pg_advisory_unlock(1)"), t());

    // pg_advisory_unlock_all releases every level of every key.
    assert_eq!(one(&a, "SELECT pg_advisory_lock(7)"), void());
    assert_eq!(one(&a, "SELECT pg_advisory_lock(7)"), void());
    assert_eq!(one(&a, "SELECT pg_advisory_lock(8)"), void());
    assert_eq!(one(&a, "SELECT pg_advisory_unlock_all()"), void());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(7)"), t());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(8)"), t());
    assert_eq!(one(&b, "SELECT pg_advisory_unlock_all()"), void());

    // NULL is not a key: the functions are strict.
    assert_eq!(one(&a, "SELECT pg_try_advisory_lock(NULL)"), SqlValue::Null);
}

#[test]
fn a_blocked_advisory_lock_waits_for_the_holder() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    assert_eq!(one(&a, "SELECT pg_advisory_lock(42)"), void());

    let (done_tx, done_rx) = mpsc::channel();
    let waiter = {
        let b = Arc::clone(&b);
        thread::spawn(move || {
            let value = one(&b, "SELECT pg_advisory_lock(42)");
            done_tx.send(value).expect("send");
        })
    };
    // B is still waiting while A holds the key.
    assert!(
        done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "B acquired a key A holds"
    );
    assert_eq!(one(&a, "SELECT pg_advisory_unlock(42)"), t());
    let granted = done_rx
        .recv_timeout(BUSY_TIMEOUT)
        .expect("B is granted the key once A releases it");
    assert_eq!(granted, void());
    waiter.join().expect("waiter");
    assert_eq!(one(&a, "SELECT pg_try_advisory_lock(42)"), f());
    assert_eq!(one(&b, "SELECT pg_advisory_unlock(42)"), t());
}

#[test]
fn a_blocked_advisory_lock_gives_up_after_the_busy_timeout() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    assert_eq!(one(&a, "SELECT pg_advisory_lock(5)"), void());
    db.set_busy_timeout(Duration::from_millis(150));
    let started = Instant::now();
    let err = error_of(&b, "SELECT pg_advisory_lock(5)");
    let waited = started.elapsed();
    assert!(err.contains("lock timeout"), "{err}");
    assert!(
        waited >= Duration::from_millis(150),
        "gave up after {waited:?}"
    );
    // B did not get the key, and A still holds it.
    assert_eq!(one(&b, "SELECT pg_advisory_unlock(5)"), f());
    assert_eq!(one(&a, "SELECT pg_advisory_unlock(5)"), t());
}

#[test]
fn dropping_a_session_releases_its_advisory_locks() {
    let (_dir, db) = open_pg();
    let a = db.connect();
    let b = db.connect();
    assert_eq!(one(&a, "SELECT pg_advisory_lock(3)"), void());
    assert_eq!(one(&a, "SELECT pg_advisory_lock(3)"), void());
    assert_eq!(one(&a, "SELECT pg_try_advisory_lock(4)"), t());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(3)"), f());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(4)"), f());
    drop(a);
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(3)"), t());
    assert_eq!(one(&b, "SELECT pg_try_advisory_lock(4)"), t());
    // A new session is a new owner: it cannot release B's keys.
    let c = db.connect();
    assert_eq!(one(&c, "SELECT pg_advisory_unlock(3)"), f());
    assert_eq!(one(&c, "SELECT pg_try_advisory_lock(3)"), f());
}
