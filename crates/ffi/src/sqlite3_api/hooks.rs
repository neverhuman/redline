//! Per-connection callback hooks: commit / rollback / update / trace /
//! profile / authorizer / busy handler.
//!
//! Hook callbacks are stored on the owning `*mut rldb` via [`HookSlots`].
//! The hooks themselves are fired from the FFI surface layer at well-defined
//! sites (statement step, `sqlite3_exec` commit detection, blob writes) so
//! the underlying SQL/kernel layer stays pure-Rust. The firing helpers live
//! in `hooks_fire.rs`.

use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_uint};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::collation::CollationNeededFn;
use crate::types::*;
use crate::util::record_status_with_message;

pub type CommitHookFn = unsafe extern "C" fn(user_data: *mut c_void) -> c_int;
pub type RollbackHookFn = unsafe extern "C" fn(user_data: *mut c_void);
pub type UpdateHookFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    op: c_int,
    db_name: *const c_char,
    table: *const c_char,
    rowid: i64,
);
pub type TraceFn = unsafe extern "C" fn(user_data: *mut c_void, sql: *const c_char);
pub type ProfileFn =
    unsafe extern "C" fn(user_data: *mut c_void, sql: *const c_char, nanoseconds: u64);
pub type TraceV2Fn = unsafe extern "C" fn(
    mask: c_uint,
    user_data: *mut c_void,
    event_ptr: *mut c_void,
    event_detail: *mut c_void,
) -> c_int;
pub type BusyHandlerFn = unsafe extern "C" fn(user_data: *mut c_void, attempts: c_int) -> c_int;
pub type AuthorizerFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    action: c_int,
    arg3: *const c_char,
    arg4: *const c_char,
    arg5: *const c_char,
    arg6: *const c_char,
) -> c_int;

/// Per-connection hook slots. `user_data` and the callback pointer are
/// stored together to ensure they are updated atomically by the registrar.
/// `user_data` is held as `usize` so the struct is auto-`Send + Sync`; it
/// is cast back to `*mut c_void` only at the C callback invocation site.
pub struct HookSlots {
    pub commit: Mutex<Option<(CommitHookFn, usize)>>,
    pub rollback: Mutex<Option<(RollbackHookFn, usize)>>,
    pub update: Mutex<Option<(UpdateHookFn, usize)>>,
    pub trace: Mutex<Option<(TraceFn, usize)>>,
    pub profile: Mutex<Option<(ProfileFn, usize)>>,
    pub busy: Mutex<Option<(BusyHandlerFn, usize)>>,
    pub authorizer: Mutex<Option<(AuthorizerFn, usize)>>,
    /// `sqlite3_collation_needed`, which upstream scopes to the connection.
    pub collation_needed: Mutex<Option<(CollationNeededFn, usize)>>,
    /// Set when the authorizer returned a code other than OK, DENY or
    /// IGNORE during the current statement. The statement fails as denied,
    /// and this turns its error into SQLite's "authorizer malfunction".
    pub authorizer_malfunction: AtomicBool,
}

impl Default for HookSlots {
    fn default() -> Self {
        Self {
            commit: Mutex::new(None),
            rollback: Mutex::new(None),
            update: Mutex::new(None),
            trace: Mutex::new(None),
            profile: Mutex::new(None),
            busy: Mutex::new(None),
            authorizer: Mutex::new(None),
            collation_needed: Mutex::new(None),
            authorizer_malfunction: AtomicBool::new(false),
        }
    }
}

fn validate_db(db: *mut rldb) -> Option<&'static rldb> {
    if db.is_null() {
        return None;
    }
    // SAFETY: caller obligation — db is a non-null rldb* from sqlite3_open
    // that has not yet been closed; we hold only a shared borrow.
    Some(unsafe { &*(db as *const rldb) })
}

/// One hook slot: the callback and its `user_data` address.
type HookSlot<F> = Mutex<Option<(F, usize)>>;

fn swap_slot<F: Copy>(
    db: *mut rldb,
    pick: fn(&HookSlots) -> &HookSlot<F>,
    cb: Option<F>,
    user_data: *mut c_void,
) -> *mut c_void {
    let Some(handle) = validate_db(db) else {
        return std::ptr::null_mut();
    };
    let mut slot = pick(&handle.hooks).lock().expect("hook slot poisoned");
    let prev = slot
        .map(|(_, prev_user)| prev_user as *mut c_void)
        .unwrap_or(std::ptr::null_mut());
    *slot = cb.map(|f| (f, user_data as usize));
    prev
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL (unregister) or valid
/// C function pointer for the documented commit-hook ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_commit_hook(
    db: *mut rldb,
    cb: Option<CommitHookFn>,
    user_data: *mut c_void,
) -> *mut c_void {
    swap_slot(db, |h| &h.commit, cb, user_data)
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL or valid C function pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_rollback_hook(
    db: *mut rldb,
    cb: Option<RollbackHookFn>,
    user_data: *mut c_void,
) -> *mut c_void {
    swap_slot(db, |h| &h.rollback, cb, user_data)
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL or valid C function pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_update_hook(
    db: *mut rldb,
    cb: Option<UpdateHookFn>,
    user_data: *mut c_void,
) -> *mut c_void {
    // Wire the SQL-side mutation dispatcher the first time any callback is
    // registered so the executor knows where to fire its per-row hook.
    super::hooks_fire::ensure_sql_dispatchers_installed();
    swap_slot(db, |h| &h.update, cb, user_data)
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL or valid C function pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_trace(
    db: *mut rldb,
    cb: Option<TraceFn>,
    user_data: *mut c_void,
) -> *mut c_void {
    swap_slot(db, |h| &h.trace, cb, user_data)
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL or valid C function pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_profile(
    db: *mut rldb,
    cb: Option<ProfileFn>,
    user_data: *mut c_void,
) -> *mut c_void {
    swap_slot(db, |h| &h.profile, cb, user_data)
}

/// RedlineDB never delivers `sqlite3_trace_v2` events (STMT, PROFILE, ROW or
/// CLOSE), so installing a callback for any of them fails with
/// `SQLITE_ERROR` rather than reporting success for events that never
/// arrive; nothing changes. A NULL callback or a zero mask turns tracing off
/// and, as upstream, also cancels a `sqlite3_trace` callback.
///
/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL or valid C function pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_trace_v2(
    db: *mut rldb,
    mask: c_uint,
    cb: Option<TraceV2Fn>,
    _user_data: *mut c_void,
) -> c_int {
    let Some(handle) = validate_db(db) else {
        return RLDB_MISUSE;
    };
    if cb.is_some() && mask != 0 {
        record_status_with_message(
            db,
            RLDB_ERROR,
            "sqlite3_trace_v2 events are not supported by RedlineDB",
        );
        return RLDB_ERROR;
    }
    *handle.hooks.trace.lock().expect("trace hook poisoned") = None;
    RLDB_OK
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL or valid C function pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_busy_handler(
    db: *mut rldb,
    cb: Option<BusyHandlerFn>,
    user_data: *mut c_void,
) -> c_int {
    let Some(handle) = validate_db(db) else {
        return RLDB_MISUSE;
    };
    let mut slot = handle.hooks.busy.lock().expect("busy hook poisoned");
    *slot = cb.map(|f| (f, user_data as usize));
    RLDB_OK
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `cb` either NULL or valid C function pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_set_authorizer(
    db: *mut rldb,
    cb: Option<AuthorizerFn>,
    user_data: *mut c_void,
) -> c_int {
    let Some(handle) = validate_db(db) else {
        return RLDB_MISUSE;
    };
    // Wire the SQL-side authorizer dispatcher the first time any callback is
    // registered so the planner/executor knows where to consult.
    super::hooks_fire::ensure_sql_dispatchers_installed();
    let mut slot = handle.hooks.authorizer.lock().expect("authorizer poisoned");
    *slot = cb.map(|f| (f, user_data as usize));
    handle
        .hooks
        .authorizer_malfunction
        .store(false, Ordering::Relaxed);
    RLDB_OK
}
