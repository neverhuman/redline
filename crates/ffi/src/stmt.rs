//! Prepared statement lifecycle (prepare/step/reset/finalize/clear_bindings).

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::atomic::Ordering;

use redlinedb_sql::Step;

use crate::types::*;
use crate::util::{
    api, flatten_code, map_error, reclaim_box, record_status_with_message, sql_result,
};

/// # Safety
///
/// - `db` must be NULL or a live database handle.
/// - `sql` must be NULL or readable up to its first NUL byte when `nbytes` is
///   negative; when `nbytes >= 0` only the bytes before the first NUL or the
///   bound, whichever comes first, need to be readable (`nbytes == 0` reads
///   nothing).
/// - `out_stmt` must be NULL or valid for writing one pointer.
/// - `tail` must be NULL or valid for writing one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_prepare_v2(
    db: *mut rldb,
    sql: *const c_char,
    nbytes: c_int,
    out_stmt: *mut *mut rldb_stmt,
    tail: *mut *const c_char,
) -> c_int {
    flatten_code(api(|| {
        // SQLite contract: *out_stmt is NULL on every failure path, so clear
        // it before any validation can return early.
        if !out_stmt.is_null() {
            // SAFETY: `out_stmt` non-null (checked); per redlinedb.h it is a
            // writable rldb_stmt** owned by the caller for this call.
            unsafe { *out_stmt = ptr::null_mut() };
        }
        if db.is_null() || sql.is_null() || out_stmt.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: `db` non-null (checked); per redlinedb.h:95 from rldb_open
        // not yet closed; shared borrow scoped to api() closure (we bump
        // active_statements below which gates close).
        let db_ref = unsafe { &*db };
        let sql_text = read_bounded_sql(sql, nbytes)?;
        // sqlite3_prepare_v2 contract: parse only the FIRST statement in
        // `sql`, set `tail` to the byte after that statement (or to the NUL
        // terminator if it was the last). We route through
        // Connection::prepare_v2 which returns the unconsumed remainder.
        let (stmt_opt, remainder) = match db_ref.conn.clone().prepare_v2(&sql_text) {
            Ok(pair) => pair,
            Err(err) => {
                let msg = err.to_string();
                let code = map_error(err);
                record_status_with_message(db, code, &msg);
                return Err(code);
            }
        };
        let consumed_bytes = sql_text.len() - remainder.len();
        // `consumed_bytes` never exceeds the bytes scanned from `sql`, so the
        // tail stays within (or one past) the caller's input.
        // SAFETY: `tail` may be NULL (optional per C ABI); when non-null,
        // caller guarantees a writable *const c_char.
        unsafe {
            if !tail.is_null() {
                *tail = sql.wrapping_add(consumed_bytes);
            }
        }
        // Blank/comment-only input: *out_stmt stays NULL (cleared above).
        let Some(stmt) = stmt_opt else {
            return Ok(RLDB_OK);
        };
        // Preserve only the consumed prefix in `sql_text` so callers that
        // read `sqlite3_sql(stmt)` see the single statement, not the
        // multi-statement input.
        let head_text = &sql_text[..consumed_bytes];
        let mut boxed = Box::new(rldb_stmt {
            db,
            stmt,
            sql_text: CString::new(head_text).map_err(|_| RLDB_MISMATCH)?,
            column_names: Vec::new(),
            text_cache: Vec::new(),
            value_cache: Vec::new(),
        });
        for index in 0..boxed.stmt.column_count() {
            // A C string ends at its first NUL, so that is what a caller of
            // sqlite3_column_name can see. Truncate there rather than fail
            // the prepare (the engine can name a column after a folded
            // constant such as char(97, 0, 98)).
            let name = boxed.stmt.column_name(index);
            let name = name.split('\0').next().unwrap_or_default();
            boxed
                .column_names
                .push(CString::new(name).map_err(|_| RLDB_MISMATCH)?);
        }
        db_ref.active_statements.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `out_stmt` non-null (checked at top); per C ABI it is a
        // writable rldb_stmt**; Box::into_raw transfers ownership to caller
        // (paired with rldb_finalize's Box::from_raw).
        unsafe {
            *out_stmt = Box::into_raw(boxed);
        }
        Ok(RLDB_OK)
    }))
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_step(stmt: *mut rldb_stmt) -> c_int {
    flatten_code(api(|| {
        if stmt.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:97 from
        // rldb_prepare_v2 not yet finalized; single-thread ownership.
        let stmt_ref = unsafe { &mut *stmt };
        let db = stmt_ref.db;
        // SAFETY: stmt_ref.db recorded at prepare time and lives at least
        // as long as the statement (prepare bumped active_statements which
        // blocks rldb_close until finalize); reads atomic flag only.
        if unsafe { (*db).interrupted.load(Ordering::Relaxed) } {
            return Err(RLDB_INTERRUPT);
        }
        // Column text and value caches describe the previous row; drop them
        // whatever the outcome. Text is converted lazily by the accessors, so
        // a row whose TEXT holds an interior NUL steps normally.
        stmt_ref.text_cache.clear();
        stmt_ref.value_cache.clear();
        // Scope the udf/collation dispatcher to this connection for the
        // duration of step() so registered C callbacks can be looked up by
        // their `*mut sqlite3` connection identity.
        redlinedb_sql::udf::with_db(db as usize, || match stmt_ref.stmt.step() {
            Ok(Step::Row) => Ok(RLDB_ROW),
            Ok(Step::Done) => Ok(RLDB_DONE),
            Err(err) => {
                let msg = err.to_string();
                let code = map_error(err);
                record_status_with_message(db, code, &msg);
                Err(code)
            }
        })
    }))
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_reset(stmt: *mut rldb_stmt) -> c_int {
    flatten_code(api(|| {
        if stmt.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:98 from
        // rldb_prepare_v2 not yet finalized; single-thread ownership.
        let stmt = unsafe { &mut *stmt };
        sql_result(stmt.stmt.reset())?;
        stmt.text_cache.clear();
        stmt.value_cache.clear();
        Ok(RLDB_OK)
    }))
}

/// # Safety
///
/// `stmt` must be NULL or a live statement. After a successful finalize it is
/// dangling and must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_finalize(stmt: *mut rldb_stmt) -> c_int {
    flatten_code(api(|| {
        if stmt.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: matching constructor/destructor pair — `stmt` originates from
        // Box::into_raw(boxed) at rldb_prepare_v2 (crates/ffi/src/stmt.rs:103);
        // ownership invariant: the C caller may not free this pointer directly
        // per redlinedb.h:99; exclusive access because rldb_stmt is documented
        // as single-thread-owned in redlinedb.h:99; double-finalize guarded by
        // the null check above (caller must NULL stmt after rldb_finalize per
        // redlinedb.h:99); ledgered at .jankurai/unsafe-ledger.toml
        // (file=crates/ffi/src/stmt.rs, line=188, detector=rust.unsafe.raw-parts);
        // proof: crates/ffi/tests/safety_invariants.rs::oversize_sql_is_rejected_gracefully
        // and ::parameter_index_out_of_range_returns_range.
        let boxed = unsafe { reclaim_box(stmt) }; // SAFETY: reclaim the leaked Box; matching destructor for the Box::into_raw at prepare (see invariant above).
        // SAFETY: boxed.db is the *mut rldb recorded at prepare time;
        // rldb_close waits for active_statements==0 so the parent db is
        // still alive when we decrement here.
        unsafe {
            (*boxed.db)
                .active_statements
                .fetch_sub(1, Ordering::Relaxed);
        }
        Ok(RLDB_OK)
    }))
}

/// # Safety
///
/// `stmt` must be NULL or a live statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rldb_clear_bindings(stmt: *mut rldb_stmt) -> c_int {
    flatten_code(api(|| {
        if stmt.is_null() {
            return Err(RLDB_MISUSE);
        }
        // SAFETY: `stmt` non-null (checked); per redlinedb.h:100 from
        // rldb_prepare_v2 not yet finalized; single-thread ownership.
        let stmt = unsafe { &mut *stmt };
        stmt.stmt.clear_bindings();
        Ok(RLDB_OK)
    }))
}

/// Copy the SQL text the caller supplied, honouring the SQLite length rule:
/// a negative `nbytes` reads through the first NUL; a non-negative `nbytes`
/// reads at most that many bytes and still stops at the first NUL. Bytes are
/// read one at a time, so no read (and no slice) ever extends past the first
/// NUL or the bound, and `nbytes == 0` reads nothing.
fn read_bounded_sql(sql: *const c_char, nbytes: c_int) -> Result<String, c_int> {
    let mut bytes = Vec::new();
    while nbytes < 0 || bytes.len() < nbytes as usize {
        // The caller's buffer is readable up to the first NUL or the
        // non-negative bound, whichever comes first; the loop stops at both.
        // SAFETY: `sql` non-null (checked by the caller); this byte lies
        // before both the first NUL and the bound (sqlite3_prepare_v2 rule).
        let byte = unsafe { *sql.cast::<u8>().add(bytes.len()) };
        if byte == 0 {
            break;
        }
        bytes.push(byte);
    }
    String::from_utf8(bytes).map_err(|_| RLDB_MISMATCH)
}
