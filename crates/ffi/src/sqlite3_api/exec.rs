use std::ffi::c_char;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::Ordering;

use crate::rldb_exec;
use crate::types::{RLDB_OK, sqlite3};
use crate::util::record_status;

/// # Safety
///
/// - `db` must be NULL or a live database handle.
/// - `sql` must be NULL or point to a NUL-terminated string.
/// - `callback`, when set, must be sound to call with `ctx`; the row and
///   column-name arrays it receives are valid only during that call.
/// - `errmsg` must be NULL or valid for writing one pointer; a message written
///   there is owned by the caller and freed with `rldb_free` or `sqlite3_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqlite3_exec(
    db: *mut sqlite3,
    sql: *const c_char,
    callback: Option<
        extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
    >,
    ctx: *mut c_void,
    errmsg: *mut *mut c_char,
) -> c_int {
    // SAFETY: forwards the caller's arguments, so this function's `# Safety`
    // contract is the callee's.
    let rc = unsafe { rldb_exec(db, sql, callback, ctx, errmsg) };
    if rc == RLDB_OK {
        record_status(db, rc);
    } else if !db.is_null() {
        // Mirror the errmsg into last_message so sqlite3_errmsg(db) returns
        // the same explanation. We cannot read from `errmsg` (it's a caller
        // out-pointer); for now record the generic code only.
        // SAFETY: `db` non-null (checked); per redlinedb.h:175 from
        // sqlite3_open not yet closed; touches atomic last_code only.
        unsafe {
            (*db).last_code.store(rc, Ordering::Relaxed);
        }
    }
    rc
}
