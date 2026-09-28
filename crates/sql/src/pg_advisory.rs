//! Postgres session-level advisory locks (PG-01).
//!
//! `pg_advisory_lock` and friends used to be stand-ins that took no lock:
//! `pg_try_advisory_lock` always answered `t` and `pg_advisory_unlock` of a
//! key nobody held answered `t` as well. These are real exclusive locks,
//! shared by every connection of one [`crate::Database`] and owned by the
//! connection that took them, the way a PostgreSQL backend owns them:
//!
//! - a lock nests: the owner may take a key again, and it stays held until
//!   it has been released as many times as it was taken;
//! - `pg_advisory_lock` waits for another owner up to the database's busy
//!   timeout, then fails with `lock timeout`;
//! - `pg_advisory_unlock` answers `t` only when this connection held the
//!   key, and `f` otherwise;
//! - they outlive transactions, and are released by
//!   `pg_advisory_unlock_all()` or when the connection is dropped.
//!
//! The single `bigint` key and the `(int, int)` pair are separate key
//! spaces, as in PostgreSQL. Shared and transaction-scoped variants are not
//! provided. The locks exist only inside this process.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use redlinedb_kernel::Error as KernelError;

use crate::error::{Error, Result};

/// A lock key. `One` is the `bigint` form, `Two` the `(int, int)` form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AdvisoryKey {
    One(i64),
    Two(i32, i32),
}

#[derive(Clone, Copy, Debug)]
struct Holder {
    owner: u64,
    depth: u32,
}

/// Every advisory lock held on one database.
#[derive(Debug)]
pub(crate) struct AdvisoryLocks {
    held: Mutex<HashMap<AdvisoryKey, Holder>>,
    released: Condvar,
    timeout: RwLock<Duration>,
}

/// Owner ids are never reused, so a new connection can never release a
/// lock an earlier one took.
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// A fresh owner id for a new connection.
pub(crate) fn next_owner() -> u64 {
    NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
}

impl AdvisoryLocks {
    pub(crate) fn new(timeout: Duration) -> Self {
        Self {
            held: Mutex::new(HashMap::new()),
            released: Condvar::new(),
            timeout: RwLock::new(timeout),
        }
    }

    pub(crate) fn set_timeout(&self, timeout: Duration) {
        *self.timeout.write().expect("advisory timeout poisoned") = timeout;
    }

    /// Take `key` for `owner`, waiting up to the busy timeout for another
    /// owner to release it.
    pub(crate) fn lock(&self, key: AdvisoryKey, owner: u64) -> Result<()> {
        let timeout = *self.timeout.read().expect("advisory timeout poisoned");
        let deadline = Instant::now() + timeout;
        let mut held = self.held.lock().expect("advisory locks poisoned");
        loop {
            if Self::grant(&mut held, key, owner) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::Kernel(KernelError::LockTimeout));
            }
            held = self
                .released
                .wait_timeout(held, deadline - now)
                .expect("advisory locks poisoned")
                .0;
        }
    }

    /// Take `key` for `owner` if nobody else holds it.
    pub(crate) fn try_lock(&self, key: AdvisoryKey, owner: u64) -> bool {
        let mut held = self.held.lock().expect("advisory locks poisoned");
        Self::grant(&mut held, key, owner)
    }

    /// Release one level of `key`; false when `owner` does not hold it.
    pub(crate) fn unlock(&self, key: AdvisoryKey, owner: u64) -> bool {
        let mut held = self.held.lock().expect("advisory locks poisoned");
        let Some(holder) = held.get_mut(&key) else {
            return false;
        };
        if holder.owner != owner {
            return false;
        }
        holder.depth -= 1;
        if holder.depth == 0 {
            held.remove(&key);
            self.released.notify_all();
        }
        true
    }

    /// Release every level of every key `owner` holds.
    pub(crate) fn unlock_all(&self, owner: u64) {
        // A connection dropped while unwinding must not panic again.
        let Ok(mut held) = self.held.lock() else {
            return;
        };
        let before = held.len();
        held.retain(|_, holder| holder.owner != owner);
        if held.len() != before {
            self.released.notify_all();
        }
    }

    fn grant(held: &mut HashMap<AdvisoryKey, Holder>, key: AdvisoryKey, owner: u64) -> bool {
        let holder = held.entry(key).or_insert(Holder { owner, depth: 0 });
        if holder.owner != owner {
            return false;
        }
        holder.depth = holder.depth.saturating_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_nest_per_owner_and_exclude_others() {
        let locks = AdvisoryLocks::new(Duration::from_millis(20));
        let (a, b) = (next_owner(), next_owner());
        let key = AdvisoryKey::One(7);
        assert!(locks.try_lock(key, a));
        assert!(locks.try_lock(key, a));
        assert!(!locks.try_lock(key, b));
        assert!(!locks.unlock(key, b));
        assert!(locks.unlock(key, a));
        assert!(!locks.try_lock(key, b));
        assert!(locks.unlock(key, a));
        assert!(!locks.unlock(key, a));
        assert!(locks.try_lock(key, b));
        let err = locks.lock(key, a).expect_err("b holds the key");
        assert_eq!(err, Error::Kernel(KernelError::LockTimeout));
        // The two key forms never collide.
        assert!(locks.try_lock(AdvisoryKey::Two(0, 7), a));
        locks.unlock_all(b);
        assert!(locks.try_lock(key, a));
        locks.unlock_all(a);
        assert!(locks.held.lock().unwrap().is_empty());
    }
}
