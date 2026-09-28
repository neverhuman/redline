//! Custom collation (`sqlite3_create_collation*`) registration surface.
//!
//! Registered collation callbacks are stored in a global registry keyed by
//! `(connection_id, lowercased name)`. The SQL `Collation::Custom` variant
//! consults this registry through `redlinedb_sql::udf::install_collation_dispatch`.
//!
//! A registration belongs to its connection: replacing or deleting it, or
//! closing the connection ([`purge_connection`]), releases its `user_data`
//! through the registrar's destructor, once. The `sqlite3_collation_needed`
//! callback is a per-connection hook slot.

use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::sync::{Arc, Mutex};

use redlinedb_sql::udf as sql_udf;

use super::user_data::UserData;
use crate::types::*;

pub type CompareFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    nbytes_a: c_int,
    a: *const c_void,
    nbytes_b: c_int,
    b: *const c_void,
) -> c_int;
pub type CollationDestructorFn = unsafe extern "C" fn(*mut c_void);

#[allow(dead_code)]
type _CollationCompareCheck = CompareFn;
pub type CollationNeededFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    db: *mut rldb,
    encoding: c_int,
    name: *const c_char,
);

#[derive(Clone)]
pub(crate) struct CollationEntry {
    pub callback: CompareFn,
    /// The caller's opaque pointer. Dropping the last reference runs the
    /// registrar's destructor.
    pub user_data: Arc<UserData>,
}

type RegistryMap = std::collections::HashMap<(usize, String), CollationEntry>;

static REGISTRY: Mutex<Option<RegistryMap>> = Mutex::new(None);

fn registry() -> std::sync::MutexGuard<'static, Option<RegistryMap>> {
    let mut guard = REGISTRY.lock().expect("collation registry poisoned");
    if guard.is_none() {
        *guard = Some(std::collections::HashMap::new());
        sql_udf::install_collation_dispatch(dispatch_from_sql);
    }
    guard
}

/// Remove every collation `db_addr` registered and release their
/// `user_data`. `rldb_close` calls this before freeing the handle, so a
/// connection that later gets the same address starts with no collations.
pub(crate) fn purge_connection(db_addr: usize) {
    let removed: Vec<CollationEntry> = {
        let mut guard = REGISTRY.lock().expect("collation registry poisoned");
        let Some(map) = guard.as_mut() else {
            return;
        };
        let keys: Vec<_> = map.keys().filter(|key| key.0 == db_addr).cloned().collect();
        keys.iter().filter_map(|key| map.remove(key)).collect()
    };
    // The lock is released: destructors may re-enter the library.
    drop(removed);
}

fn dispatch_from_sql(db_addr: usize, name: &str, a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let key = (db_addr, name.to_ascii_lowercase());
    let registry = registry();
    let map = registry.as_ref()?;
    // The clone keeps user_data alive for the call, even if the collation
    // is replaced meanwhile.
    let entry = map.get(&key)?.clone();
    drop(registry);
    let cmp = entry.callback;
    let user_data = entry.user_data.as_ptr();
    // SAFETY: callback signature matches FFI ABI; we pass byte slices with
    // explicit lengths; the registered callback agreed to inspect those
    // bytes for the call duration only; user_data cast back from usize.
    let rc = unsafe {
        cmp(
            user_data,
            a.len() as c_int,
            a.as_ptr() as *const c_void,
            b.len() as c_int,
            b.as_ptr() as *const c_void,
        )
    };
    Some(match rc.cmp(&0) {
        std::cmp::Ordering::Less => std::cmp::Ordering::Less,
        std::cmp::Ordering::Equal => std::cmp::Ordering::Equal,
        std::cmp::Ordering::Greater => std::cmp::Ordering::Greater,
    })
}

fn invoke_needed(db: *mut rldb, name: &str) {
    if db.is_null() {
        return;
    }
    // SAFETY: `db` non-null (checked); the caller passes a live connection
    // handle; only its hook slot is read, through a shared borrow.
    let handle = unsafe { &*db };
    // Copy the slot and release its lock: the callback usually registers a
    // collation and may replace itself.
    let slot = *handle
        .hooks
        .collation_needed
        .lock()
        .expect("collation-needed hook poisoned");
    if let Some((cb, user_data)) = slot
        && let Ok(cstr) = std::ffi::CString::new(name)
    {
        let user_data = user_data as *mut c_void;
        // SAFETY: callback signature matches the FFI ABI for
        // sqlite3_collation_needed; user_data is the pointer the
        // registrar provided (stored as usize, cast back here) and
        // remains valid for the lifetime guaranteed by the SQLite ABI;
        // cstr lives for the entire call; ledgered at
        // .jankurai/unsafe-ledger.toml (file=crates/ffi/src/sqlite3_api/collation.rs,
        // line=141, detector=rust.unsafe.extern-fn).
        unsafe {
            // SAFETY: see the documented FFI-ABI callback invariant above.
            cb(user_data, db, 1 /* SQLITE_UTF8 */, cstr.as_ptr());
        }
    }
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `name` NUL-terminated; `compare` either
/// NULL (unregister) or a valid C function pointer per the SQLite ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_create_collation(
    db: *mut rldb,
    name: *const c_char,
    _enc: c_int,
    user_data: *mut c_void,
    compare: Option<CompareFn>,
) -> c_int {
    // SAFETY: delegates to v2 which performs all argument checks.
    unsafe { sqlite3_create_collation_v2(db, name, _enc, user_data, compare, None) }
}

/// Registers, replaces or (with `compare` NULL) deletes a collation. On
/// success the connection owns `user_data`: `destroy(user_data)` runs once,
/// when the collation is replaced or deleted or the connection closes. As
/// upstream, and unlike every other registration, a failed call does not
/// call `destroy`; the caller still owns `user_data`.
///
/// # Safety
/// `db` non-NULL valid sqlite3*; `name` NUL-terminated; `compare` either
/// NULL (unregister) or valid C function pointer; `destroy` either NULL or
/// valid destructor for `user_data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_create_collation_v2(
    db: *mut rldb,
    name: *const c_char,
    _enc: c_int,
    user_data: *mut c_void,
    compare: Option<CompareFn>,
    destroy: Option<CollationDestructorFn>,
) -> c_int {
    if db.is_null() || name.is_null() {
        return RLDB_MISUSE;
    }
    // SAFETY: caller obligation — name NUL-terminated per the SQLite ABI.
    let name_bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    let name_string = match String::from_utf8(name_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => return RLDB_MISUSE,
    };
    let key = (db as usize, name_string.to_ascii_lowercase());
    let user_data = Arc::new(UserData::new(user_data, destroy));
    let displaced = {
        let mut registry = registry();
        let map = registry.as_mut().expect("registry init");
        match compare {
            Some(callback) => map.insert(
                key,
                CollationEntry {
                    callback,
                    user_data: Arc::clone(&user_data),
                },
            ),
            None => map.remove(&key),
        }
    };
    // The lock is released: destructors may re-enter the library. When
    // deleting, nothing keeps this call's user_data, so it is released too.
    drop((displaced, user_data));
    RLDB_OK
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL (unregister) or valid
/// C function pointer per the SQLite ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_collation_needed(
    db: *mut rldb,
    user_data: *mut c_void,
    cb: Option<CollationNeededFn>,
) -> c_int {
    if db.is_null() {
        return RLDB_MISUSE;
    }
    // SAFETY: `db` non-null (checked); a live connection handle per the
    // # Safety contract; only its hook slot is written, behind a Mutex.
    let handle = unsafe { &*db };
    *handle
        .hooks
        .collation_needed
        .lock()
        .expect("collation-needed hook poisoned") = cb.map(|cb| (cb, user_data as usize));
    RLDB_OK
}

/// Test-only helper: pretend a collation was requested. Lets the
/// `collation_needed` test path exercise the dispatch round-trip without
/// running a full SQL statement against an unregistered collation.
#[doc(hidden)]
pub fn __test_invoke_needed(db: *mut rldb, name: &str) {
    invoke_needed(db, name);
}
