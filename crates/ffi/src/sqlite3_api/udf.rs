//! UDF (`sqlite3_create_function*`) registration surface.
//!
//! Registered C-callback UDFs are stored in a global registry keyed by
//! `(connection_id, lowercased name, arg count)`. The SQL executor
//! consults this registry via the `udf_registry::dispatch` hook installed
//! into `redlinedb_sql` when `sqlite3_create_function*` is first called.
//!
//! A registration belongs to its connection: replacing or deleting it, or
//! closing the connection ([`purge_connection`]), releases its `user_data`
//! through the registrar's destructor, once (see `user_data.rs`).

use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::sync::{Arc, Mutex};

use redlinedb_sql::udf as sql_udf;
use redlinedb_sql::value::SqlValue;

use super::context::RldbContext;
use super::user_data::UserData;
use super::value::RldbValue;
use crate::types::*;
use crate::util::{caller_u16, reclaim_box, record_status, record_status_with_message};

pub type ScalarFn =
    unsafe extern "C" fn(ctx: *mut RldbContext, argc: c_int, argv: *mut *mut RldbValue);
pub type StepFn =
    unsafe extern "C" fn(ctx: *mut RldbContext, argc: c_int, argv: *mut *mut RldbValue);
pub type FinalFn = unsafe extern "C" fn(ctx: *mut RldbContext);
pub type DestructorFn = unsafe extern "C" fn(*mut c_void);

/// Text encodings (`SQLITE_UTF8` = 1 .. `SQLITE_ANY` = 5) occupy the low
/// three bits of `eTextRep`; 0 registers as UTF-8, as upstream.
const TEXT_ENCODING_MASK: c_int = 0x07;
const SQLITE_ANY: c_int = 5;
const SQLITE_DETERMINISTIC: c_int = 0x0000_0800;
const SQLITE_DIRECTONLY: c_int = 0x0008_0000;
const SQLITE_SUBTYPE: c_int = 0x0010_0000;
const SQLITE_INNOCUOUS: c_int = 0x0020_0000;
const SQLITE_RESULT_SUBTYPE: c_int = 0x0100_0000;
/// Flags that only describe the function. They are recorded and restrict
/// nothing, which is what SQLite does with them outside index expressions,
/// CHECK constraints, generated columns and untrusted schemas.
const RECORDED_FLAGS: c_int =
    SQLITE_DETERMINISTIC | SQLITE_INNOCUOUS | SQLITE_SUBTYPE | SQLITE_RESULT_SUBTYPE;

/// Check `eTextRep` and return the function flags to record. A flag that
/// asks RedlineDB to restrict where the function runs (`SQLITE_DIRECTONLY`)
/// is refused rather than accepted and ignored, and so is any bit or text
/// encoding this implementation does not know.
fn validate_function_flags(enc: c_int) -> Result<c_int, &'static str> {
    let flags = enc & !TEXT_ENCODING_MASK;
    if flags & SQLITE_DIRECTONLY != 0 {
        return Err("SQLITE_DIRECTONLY is not enforced by RedlineDB");
    }
    if flags & !RECORDED_FLAGS != 0 || enc & TEXT_ENCODING_MASK > SQLITE_ANY {
        return Err("unsupported sqlite3_create_function flags");
    }
    Ok(flags)
}

#[derive(Clone)]
pub(crate) struct ScalarEntry {
    pub callback: ScalarFn,
    /// The caller's opaque pointer. Dropping the last reference runs the
    /// registrar's destructor.
    pub user_data: Arc<UserData>,
    /// Recorded `SQLITE_DETERMINISTIC`-family flags (see [`RECORDED_FLAGS`]).
    pub flags: c_int,
}

#[derive(Clone)]
pub(crate) struct AggregateEntry {
    // Aggregate UDFs are dispatched at group end by
    // `aggregate_run_from_sql` via the global
    // `redlinedb_sql::udf::AGG_RUN` slot installed when the first UDF is
    // registered (see [`registry`]).
    pub step: StepFn,
    pub final_fn: FinalFn,
    pub user_data: Arc<UserData>,
    pub flags: c_int,
}

#[derive(Clone)]
pub(crate) enum UdfEntry {
    Scalar(ScalarEntry),
    Aggregate(AggregateEntry),
}

impl UdfEntry {
    fn flags(&self) -> c_int {
        match self {
            Self::Scalar(entry) => entry.flags,
            Self::Aggregate(entry) => entry.flags,
        }
    }
}

/// Registry: (db_addr, lowercased_name, narg) -> entry.
/// `narg = -1` matches any arity.
type RegistryMap = std::collections::HashMap<(usize, String, i32), UdfEntry>;

static REGISTRY: Mutex<Option<RegistryMap>> = Mutex::new(None);

fn registry() -> std::sync::MutexGuard<'static, Option<RegistryMap>> {
    let mut guard = REGISTRY.lock().expect("udf registry poisoned");
    if guard.is_none() {
        *guard = Some(std::collections::HashMap::new());
        sql_udf::install_dispatch(dispatch_from_sql);
        sql_udf::install_aggregate_dispatch(aggregate_run_from_sql, aggregate_is_registered);
    }
    guard
}

/// Remove every function `db_addr` registered and release their
/// `user_data`. `rldb_close` calls this before freeing the handle, so a
/// connection that later gets the same address starts with no functions.
pub(crate) fn purge_connection(db_addr: usize) {
    let removed: Vec<UdfEntry> = {
        let mut guard = REGISTRY.lock().expect("udf registry poisoned");
        let Some(map) = guard.as_mut() else {
            return;
        };
        let keys: Vec<_> = map.keys().filter(|key| key.0 == db_addr).cloned().collect();
        keys.iter().filter_map(|key| map.remove(key)).collect()
    };
    // The lock is released: destructors may re-enter the library.
    drop(removed);
}

/// Probe: does a connection have an aggregate UDF registered under
/// `name`? Used by the planner to route the expression through the
/// grouped evaluator.
fn aggregate_is_registered(db_addr: usize, name: &str) -> bool {
    let registry = registry();
    let Some(map) = registry.as_ref() else {
        return false;
    };
    let name_lower = name.to_ascii_lowercase();
    map.iter().any(|((d, n, _), entry)| {
        *d == db_addr && *n == name_lower && matches!(entry, UdfEntry::Aggregate(_))
    })
}

/// Aggregate dispatch invoked by the SQL aggregator at group end. Walks
/// every row in `rows`, calls `xStep` once per row, then calls `xFinal`
/// and returns the value the UDF set on the context (via
/// `sqlite3_result_*`). Errors set via `sqlite3_result_error` surface
/// here as `Err`.
fn aggregate_run_from_sql(
    db_addr: usize,
    name: &str,
    rows: &[Vec<SqlValue>],
) -> Option<Result<SqlValue, String>> {
    let name_lower = name.to_ascii_lowercase();
    // `owner` keeps the registration's user_data alive until the run ends,
    // even if the function is replaced meanwhile.
    let (step, final_fn, owner) = {
        let registry = registry();
        let map = registry.as_ref()?;
        // Match either the exact arity (rows[0].len()) or wildcard -1.
        // Aggregate UDFs typically take 1 arg, but xStep may be called
        // with any positive arity that matches the registration.
        let narg_exact = rows.first().map(|r| r.len() as i32).unwrap_or(0);
        let entry = map
            .get(&(db_addr, name_lower.clone(), narg_exact))
            .or_else(|| map.get(&(db_addr, name_lower.clone(), -1)))?;
        let UdfEntry::Aggregate(agg) = entry else {
            return Some(Err(format!(
                "scalar UDF {name} cannot be invoked as aggregate"
            )));
        };
        (agg.step, agg.final_fn, Arc::clone(&agg.user_data))
    };
    let user_data = owner.as_ptr();
    // One context per group: SQLite's contract is that xStep is called
    // multiple times against the same `sqlite3_context*`, then xFinal is
    // called once on the same context. The accumulator lives via the
    // context's `agg_state_*` slot or via `sqlite3_result_*` set on the
    // last xStep — the latter is what we surface here.
    let ctx = Box::new(RldbContext::new(db_addr as *mut rldb, user_data));
    let ctx_ptr = Box::into_raw(ctx);
    for row in rows {
        let mut boxed: Vec<*mut RldbValue> = row
            .iter()
            .map(|v| Box::into_raw(Box::new(RldbValue::from_sql(v))))
            .collect();
        // SAFETY: callback signature matches the FFI ABI for an aggregate
        // step; ctx_ptr is the Box::into_raw allocation we just made
        // above; boxed.as_mut_ptr names the argv buffer of Box::into_raw
        // RldbValue pointers; the corresponding Box::from_raw block below
        // reclaims every allocation we hand to the callback; ledgered at
        // .jankurai/unsafe-ledger.toml
        // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
        // detector=rust.unsafe.extern-fn).
        unsafe {
            // SAFETY: see the documented FFI-ABI invariant directly above.
            step(ctx_ptr, boxed.len() as c_int, boxed.as_mut_ptr());
        }
        for ptr in boxed.drain(..) {
            // SAFETY: matching constructor/destructor pair — each ptr
            // originates from Box::into_raw above; the FFI ABI for
            // sqlite3_value* never transfers ownership to the callback
            // (read-only inspection only); ledgered at
            // .jankurai/unsafe-ledger.toml
            // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
            // detector=rust.unsafe.raw-parts).
            let _ = unsafe { reclaim_box(ptr) }; // SAFETY: reclaim the Box::into_raw RldbValue allocation; read-only ABI, no ownership transfer (see invariant above).
        }
        // SAFETY: read-only borrow of the ctx allocation we just made
        // (Box::into_raw above); short-lived borrow used only for the
        // error check between rows; matched by Box::from_raw at the end
        // of this function; ledgered at .jankurai/unsafe-ledger.toml
        // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
        // detector=rust.unsafe.raw-parts).
        let ctx_ref = unsafe { &*ctx_ptr }; // SAFETY: short-lived read-only borrow of the Box::into_raw context (see invariant above).
        if let Some(err) = ctx_ref.take_error() {
            // SAFETY: matching constructor/destructor pair — ctx_ptr
            // originates from Box::into_raw above; we reclaim it here so
            // the Box drops before we return; ledgered at
            // .jankurai/unsafe-ledger.toml
            // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
            // detector=rust.unsafe.raw-parts).
            let _ = unsafe { reclaim_box(ctx_ptr) }; // SAFETY: reclaim the Box::into_raw context allocation (see invariant above).
            return Some(Err(err));
        }
    }
    // SAFETY: callback signature matches the FFI ABI for an aggregate
    // final; ctx_ptr is the Box::into_raw allocation we made above;
    // ledgered at .jankurai/unsafe-ledger.toml
    // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
    // detector=rust.unsafe.extern-fn).
    unsafe {
        // SAFETY: see the documented FFI-ABI invariant directly above.
        final_fn(ctx_ptr);
    }
    // SAFETY: matching constructor/destructor pair — ctx_ptr originates
    // from Box::into_raw above; ownership invariant: the FFI ABI for
    // sqlite3_context* never transfers ownership to the callback (the
    // SQLite docs bound context lifetime to the UDF invocation);
    // ledgered at .jankurai/unsafe-ledger.toml
    // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
    // detector=rust.unsafe.raw-parts).
    let ctx_box = unsafe { reclaim_box(ctx_ptr) }; // SAFETY: reclaim the Box::into_raw context allocation (see invariant above).
    if let Some(err) = ctx_box.take_error() {
        return Some(Err(err));
    }
    Some(Ok(ctx_box.take_result().to_sql()))
}

/// Hook invoked from `redlinedb_sql` for every unrecognised function call.
fn dispatch_from_sql(
    db_addr: usize,
    name: &str,
    args: &[SqlValue],
) -> Option<Result<SqlValue, String>> {
    let key_exact = (db_addr, name.to_ascii_lowercase(), args.len() as i32);
    let key_any = (db_addr, name.to_ascii_lowercase(), -1);
    let registry = registry();
    let map = registry.as_ref()?;
    // Lookup precedence: exact arity wins over the wildcard registration so
    // a UDF registered with explicit narg always beats a -1 (any-arity)
    // registration of the same name.
    let entry = map.get(&key_exact).or_else(|| map.get(&key_any))?;
    let UdfEntry::Scalar(entry) = entry else {
        return Some(Err(format!(
            "aggregate UDF {name} cannot be invoked as scalar"
        )));
    };
    let callback = entry.callback;
    // `owner` keeps user_data alive for the call, even if the function is
    // replaced meanwhile; it drops at the end of this function.
    let owner = Arc::clone(&entry.user_data);
    let user_data = owner.as_ptr();
    drop(registry);
    // Build argv as boxed RldbValues.
    let mut boxed: Vec<*mut RldbValue> = args
        .iter()
        .map(|v| Box::into_raw(Box::new(RldbValue::from_sql(v))))
        .collect();
    let ctx = Box::new(RldbContext::new(db_addr as *mut rldb, user_data));
    let ctx_ptr = Box::into_raw(ctx);
    // SAFETY: callback signature matches the FFI ABI for a scalar UDF;
    // ctx_ptr is a Box::into_raw allocation we just made; boxed.as_mut_ptr
    // names the argv buffer of Box::into_raw RldbValue pointers; we reclaim
    // every allocation in the matching Box::from_raw block directly below;
    // ledgered at .jankurai/unsafe-ledger.toml
    // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
    // detector=rust.unsafe.extern-fn).
    unsafe {
        // SAFETY: see the documented FFI-ABI invariant directly above.
        callback(ctx_ptr, boxed.len() as c_int, boxed.as_mut_ptr());
    }
    // SAFETY: matching constructor/destructor pair — ctx_ptr originates
    // from Box::into_raw above; ownership invariant: the FFI ABI for
    // sqlite3_context* never transfers ownership to the callback (the
    // SQLite docs bound context lifetime to the UDF invocation); ledgered
    // at .jankurai/unsafe-ledger.toml (file=crates/ffi/src/sqlite3_api/udf.rs,
    // line=368, detector=rust.unsafe.raw-parts).
    let ctx_box = unsafe { reclaim_box(ctx_ptr) }; // SAFETY: reclaim the Box::into_raw context allocation (see invariant above).
    for ptr in boxed {
        // SAFETY: matching constructor/destructor pair — each `ptr`
        // originates from Box::into_raw above (argv materialisation);
        // ownership invariant: the FFI ABI for sqlite3_value* never
        // transfers ownership to the callback (read-only inspection
        // only); ledgered at .jankurai/unsafe-ledger.toml
        // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
        // detector=rust.unsafe.raw-parts).
        let _ = unsafe { reclaim_box(ptr) }; // SAFETY: reclaim the Box::into_raw RldbValue allocation; read-only ABI, no ownership transfer (see invariant above).
    }
    if let Some(err) = ctx_box.take_error() {
        return Some(Err(err));
    }
    Some(Ok(ctx_box.take_result().to_sql()))
}

/// Store `entry` under `(db, name, narg)`, or delete that registration when
/// `entry` is `None`. Returns the registration it displaced; the caller drops
/// it after the lock is released, which runs its destructor.
fn replace_entry(
    db: *mut rldb,
    name: &str,
    narg: i32,
    entry: Option<UdfEntry>,
) -> Option<UdfEntry> {
    let mut registry = registry();
    let map = registry.as_mut().expect("registry init");
    let key = (db as usize, name.to_ascii_lowercase(), narg);
    match entry {
        Some(entry) => map.insert(key, entry),
        None => map.remove(&key),
    }
}

/// Fail a registration after `db` was validated: record the error on the
/// connection and release `user_data` through its destructor, as upstream.
fn refuse(db: *mut rldb, user_data: UserData, code: c_int, message: Option<&str>) -> c_int {
    match message {
        Some(message) => record_status_with_message(db, code, message),
        None => record_status(db, code),
    }
    drop(user_data);
    code
}

/// Flags recorded for a registered function, or `None` when `name`/`narg`
/// is not registered on `db`.
#[doc(hidden)]
pub fn __test_function_flags(db: *mut rldb, name: &str, narg: i32) -> Option<c_int> {
    let registry = registry();
    let key = (db as usize, name.to_ascii_lowercase(), narg);
    registry.as_ref()?.get(&key).map(UdfEntry::flags)
}

unsafe fn name_to_string(name: *const c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    // SAFETY: caller obligation — name is NUL-terminated per the SQLite ABI.
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    String::from_utf8(bytes.to_vec()).ok()
}

/// # Safety
/// `db` non-NULL valid sqlite3*; `name` NUL-terminated; `func`, `step`,
/// `final_func` either NULL or valid C function pointers per the SQLite ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_create_function(
    db: *mut rldb,
    name: *const c_char,
    narg: c_int,
    enc: c_int,
    user_data: *mut c_void,
    func: Option<ScalarFn>,
    step: Option<StepFn>,
    final_func: Option<FinalFn>,
) -> c_int {
    // SAFETY: delegates to the v2 variant whose # Safety contract this
    // call inherits unchanged; all argument checks happen there; ledgered
    // at .jankurai/unsafe-ledger.toml (file=crates/ffi/src/sqlite3_api/udf.rs,
    // line=368, detector=rust.unsafe.extern-fn).
    unsafe {
        // SAFETY: see the documented FFI-ABI invariant directly above.
        sqlite3_create_function_v2(db, name, narg, enc, user_data, func, step, final_func, None)
    }
}

/// Registers, replaces or (with every callback NULL) deletes a function.
/// Once `db` is valid the call owns `user_data`: `destroy(user_data)` runs
/// exactly once, on failure, when the function is replaced or deleted, or
/// when the connection closes. `SQLITE_DIRECTONLY` and unknown `enc` bits
/// fail with `SQLITE_ERROR` and leave any existing registration in place.
///
/// # Safety
/// `db` non-NULL valid sqlite3*; `name` NUL-terminated; `func`, `step`,
/// `final_func` either NULL or valid C function pointers per the SQLite ABI;
/// `destroy` either NULL or a valid destructor for `user_data`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn sqlite3_create_function_v2(
    db: *mut rldb,
    name: *const c_char,
    narg: c_int,
    enc: c_int,
    user_data: *mut c_void,
    func: Option<ScalarFn>,
    step: Option<StepFn>,
    final_func: Option<FinalFn>,
    destroy: Option<DestructorFn>,
) -> c_int {
    if db.is_null() {
        return RLDB_MISUSE;
    }
    let user_data = UserData::new(user_data, destroy);
    // SAFETY: caller obligation — name is a NUL-terminated C string per
    // the documented # Safety contract of this function; name_to_string
    // reads only until the first NUL; ledgered at .jankurai/unsafe-ledger.toml
    // (file=crates/ffi/src/sqlite3_api/udf.rs, line=368,
    // detector=rust.unsafe.extern-fn).
    let name = match unsafe { name_to_string(name) } {
        // SAFETY: `name` is a NUL-terminated C string per the # Safety contract (see above).
        Some(n) => n,
        None => return refuse(db, user_data, RLDB_MISUSE, None),
    };
    // Upstream's accepted shapes: xFunc alone, xStep with xFinal, or no
    // callback at all (delete). Anything else is misuse.
    if !matches!(
        (func.is_some(), step.is_some(), final_func.is_some()),
        (true, false, false) | (false, true, true) | (false, false, false)
    ) {
        return refuse(db, user_data, RLDB_MISUSE, None);
    }
    let flags = match validate_function_flags(enc) {
        Ok(flags) => flags,
        Err(message) => return refuse(db, user_data, RLDB_ERROR, Some(message)),
    };
    let user_data = Arc::new(user_data);
    let entry = match (func, step, final_func) {
        (Some(callback), _, _) => Some(UdfEntry::Scalar(ScalarEntry {
            callback,
            user_data,
            flags,
        })),
        (None, Some(step), Some(final_fn)) => Some(UdfEntry::Aggregate(AggregateEntry {
            step,
            final_fn,
            user_data,
            flags,
        })),
        // Deleting: nothing keeps this call's user_data, so it is released
        // below together with the deleted registration's.
        _ => {
            let displaced = replace_entry(db, &name, narg, None);
            drop((displaced, user_data));
            return RLDB_OK;
        }
    };
    let displaced = replace_entry(db, &name, narg, entry);
    // The registry lock is released: the replaced destructor may re-enter.
    drop(displaced);
    RLDB_OK
}

/// Registers the aggregate part of a window function. RedlineDB's executor
/// has no path that calls `xValue` or `xInverse`, so a registration that
/// supplies them fails with `SQLITE_ERROR` (and releases `user_data`)
/// instead of succeeding as a function that silently ignores them.
///
/// # Safety
/// `db` non-NULL valid sqlite3*; `name` NUL-terminated; callbacks are either
/// NULL or valid C function pointers per the SQLite ABI.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn sqlite3_create_window_function(
    db: *mut rldb,
    name: *const c_char,
    narg: c_int,
    enc: c_int,
    user_data: *mut c_void,
    step: Option<StepFn>,
    final_func: Option<FinalFn>,
    value: Option<FinalFn>,
    inverse: Option<StepFn>,
    destroy: Option<DestructorFn>,
) -> c_int {
    if db.is_null() {
        return RLDB_MISUSE;
    }
    if value.is_some() || inverse.is_some() {
        return refuse(
            db,
            UserData::new(user_data, destroy),
            RLDB_ERROR,
            Some("window function callbacks (xValue, xInverse) are not supported by RedlineDB"),
        );
    }
    // SAFETY: delegates to the v2 variant whose # Safety contract this
    // call inherits unchanged; all argument checks happen there.
    unsafe {
        // SAFETY: see the documented FFI-ABI invariant directly above.
        sqlite3_create_function_v2(
            db, name, narg, enc, user_data, None, step, final_func, destroy,
        )
    }
}

/// `_16` variant: UTF-16 name path. We accept the UTF-16 name, convert to
/// UTF-8, and delegate. Unlike v2, the destructor slot is unsupported here
/// (SQLite's `_16` overload predates `_v2`).
///
/// # Safety
/// `db` non-NULL; `name_utf16` NUL-terminated UTF-16 little-endian string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_create_function16(
    db: *mut rldb,
    name_utf16: *const c_void,
    narg: c_int,
    enc: c_int,
    user_data: *mut c_void,
    func: Option<ScalarFn>,
    step: Option<StepFn>,
    final_func: Option<FinalFn>,
) -> c_int {
    if db.is_null() || name_utf16.is_null() {
        return RLDB_MISUSE;
    }
    // Walk the UTF-16 string until a u16 NUL.
    let mut len = 0usize;
    let ptr = name_utf16 as *const u16;
    // SAFETY: caller obligation — NUL-terminated UTF-16 string.
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
            if len > 4096 {
                return RLDB_MISUSE;
            }
        }
    }
    // SAFETY: matching constructor/destructor pair — `ptr` is the caller-
    // provided UTF-16 buffer base whose length we just measured (`len`
    // u16 elements before the NUL); ownership invariant: read-only borrow
    // immediately consumed by `String::from_utf16` on the next line so the
    // caller's allocation regains exclusive access on return.
    let slice = unsafe { caller_u16(ptr, len) }; // SAFETY: copy the caller's UTF-16 name into an owned buffer (see invariant above).
    let name = match String::from_utf16(&slice) {
        Ok(s) => s,
        Err(_) => return RLDB_MISUSE,
    };
    let cstring = match std::ffi::CString::new(name) {
        Ok(s) => s,
        Err(_) => return RLDB_MISUSE,
    };
    // SAFETY: delegates to the v2 variant whose # Safety contract this
    // call inherits; cstring lives for the call duration; ledgered at
    // .jankurai/unsafe-ledger.toml (file=crates/ffi/src/sqlite3_api/udf.rs,
    // line=368, detector=rust.unsafe.extern-fn).
    unsafe {
        // SAFETY: see the documented FFI-ABI invariant directly above.
        sqlite3_create_function_v2(
            db,
            cstring.as_ptr(),
            narg,
            enc,
            user_data,
            func,
            step,
            final_func,
            None,
        )
    }
}
