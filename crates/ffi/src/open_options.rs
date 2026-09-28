//! Validation of the caller's `rldb_config` for `rldb_open_v2`.
//!
//! `struct_size` is the version of the struct: it is read first, and no byte
//! past it is read. A shorter struct (from an older header) leaves the later
//! fields at their defaults; a longer one (from a newer header) is accepted
//! only when every byte past the fields this library knows is zero, because a
//! non-zero byte there asks for something this library cannot provide. The
//! rules are in `contracts/c-abi/redlinedb.h` next to `rldb_config`.

use std::mem::{offset_of, size_of};
use std::os::raw::c_int;
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use redlinedb_kernel::engine::CommitDurability;
use redlinedb_sql::{Connection, DbOptions};

use crate::types::*;
use crate::util::{caller_buffer, sql_result};

/// Smallest accepted `struct_size`: the `struct_size` field alone.
const MIN_STRUCT_SIZE: usize = size_of::<u32>();
/// Largest accepted `struct_size`. It leaves room for later versions of the
/// struct; anything larger is taken to be an uninitialised field, not a size.
const MAX_STRUCT_SIZE: usize = 4096;
/// Size of the struct this library was built with.
const KNOWN_SIZE: usize = size_of::<rldb_config>();

/// Sizes below `KNOWN_SIZE` at which an older, shorter struct may end: after
/// each field, and after the padding that aligns `cache_bytes`. Any other
/// shorter size would cut a field in half.
const FIELD_ENDS: [usize; 8] = [
    offset_of!(rldb_config, flags),
    offset_of!(rldb_config, durability),
    offset_of!(rldb_config, durability) + size_of::<u32>(),
    offset_of!(rldb_config, cache_bytes),
    offset_of!(rldb_config, work_mem_bytes),
    offset_of!(rldb_config, max_spill_bytes),
    offset_of!(rldb_config, statement_cache_capacity),
    offset_of!(rldb_config, busy_timeout_ms),
];

/// `durability` values. 0 keeps the default, which is Strict.
pub(crate) const RLDB_DURABILITY_DEFAULT: u32 = 0;
pub(crate) const RLDB_DURABILITY_STRICT: u32 = 1;
pub(crate) const RLDB_DURABILITY_NORMAL: u32 = 2;

/// Read and check `*config`, then build the open options from it.
///
/// Returns `RLDB_MISUSE` when `struct_size` is below 4, above 4096, or ends
/// inside a field; when `flags` is not zero; when `durability` is not one of
/// the `RLDB_DURABILITY_*` values; or when a byte past the known fields is not
/// zero. A zero field keeps the `DbOptions::default()` value.
///
/// # Safety
///
/// `config` must be non-NULL, readable for 4 bytes, and readable for
/// `struct_size` bytes. It need not be aligned.
pub(crate) unsafe fn db_options_from_config(
    config: *const rldb_config,
) -> Result<DbOptions, c_int> {
    // SAFETY: the caller guarantees the first four bytes are readable; the
    // read makes no alignment assumption.
    let declared = unsafe { ptr::read_unaligned(config.cast::<u32>()) } as usize;
    if !(MIN_STRUCT_SIZE..=MAX_STRUCT_SIZE).contains(&declared) {
        return Err(RLDB_MISUSE);
    }
    if declared < KNOWN_SIZE && !FIELD_ENDS.contains(&declared) {
        return Err(RLDB_MISUSE);
    }
    // SAFETY: the caller guarantees `struct_size` (= `declared`, at most
    // 4096) bytes are readable; the helper copies exactly those bytes into
    // an owned buffer and makes no alignment assumption.
    let supplied = unsafe { caller_buffer(config.cast::<u8>(), declared) };
    if supplied.iter().skip(KNOWN_SIZE).any(|&byte| byte != 0) {
        return Err(RLDB_MISUSE);
    }
    let mut known = [0u8; KNOWN_SIZE];
    let copied = declared.min(KNOWN_SIZE);
    known[..copied].copy_from_slice(&supplied[..copied]);
    options_from_fields(&known)
}

/// Make `PRAGMA synchronous` report the durability the connection was opened
/// with. The engine takes its commit durability from the open options, but a
/// connection's `synchronous` level starts at FULL whatever the engine uses.
/// Setting it through the PRAGMA keeps the two in step: NORMAL is the level
/// that maps to `CommitDurability::Normal`.
pub(crate) fn report_durability(
    conn: &Arc<Connection>,
    durability: CommitDurability,
) -> Result<(), c_int> {
    if durability == CommitDurability::Normal {
        sql_result(conn.execute("PRAGMA synchronous = NORMAL"))?;
    }
    Ok(())
}

fn options_from_fields(bytes: &[u8; KNOWN_SIZE]) -> Result<DbOptions, c_int> {
    let flags = read_u32(bytes, offset_of!(rldb_config, flags));
    if flags != 0 {
        return Err(RLDB_MISUSE);
    }
    let mut options = DbOptions::default();
    options.engine.commit_durability = match read_u32(bytes, offset_of!(rldb_config, durability)) {
        RLDB_DURABILITY_DEFAULT => options.engine.commit_durability,
        RLDB_DURABILITY_STRICT => CommitDurability::Strict,
        RLDB_DURABILITY_NORMAL => CommitDurability::Normal,
        _ => return Err(RLDB_MISUSE),
    };
    let cache_bytes = read_u64(bytes, offset_of!(rldb_config, cache_bytes));
    if cache_bytes != 0 {
        let page_size = options.engine.page_size.max(1);
        options.engine.buffer_pool_pages = (saturating_usize(cache_bytes) / page_size).max(16);
    }
    let work_mem = read_u64(bytes, offset_of!(rldb_config, work_mem_bytes));
    if work_mem != 0 {
        options.query_memory.work_mem_bytes = saturating_usize(work_mem);
    }
    let max_spill = read_u64(bytes, offset_of!(rldb_config, max_spill_bytes));
    if max_spill != 0 {
        options.query_memory.max_spill_bytes = saturating_usize(max_spill);
    }
    let statement_cache = read_u32(bytes, offset_of!(rldb_config, statement_cache_capacity));
    if statement_cache != 0 {
        options.statement_cache_capacity = statement_cache as usize;
    }
    let busy_timeout_ms = read_u32(bytes, offset_of!(rldb_config, busy_timeout_ms));
    if busy_timeout_ms != 0 {
        // Both lock waits, as rldb_busy_timeout sets them: the unique-key
        // lock table and the engine's row locks.
        let timeout = Duration::from_millis(u64::from(busy_timeout_ms));
        options.busy_timeout = timeout;
        options.engine.busy_timeout = timeout;
    }
    Ok(options)
}

fn read_u32(bytes: &[u8; KNOWN_SIZE], offset: usize) -> u32 {
    let mut field = [0u8; 4];
    field.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_ne_bytes(field)
}

fn read_u64(bytes: &[u8; KNOWN_SIZE], offset: usize) -> u64 {
    let mut field = [0u8; 8];
    field.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_ne_bytes(field)
}

fn saturating_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[cfg(test)]
#[path = "open_options_tests.rs"]
mod tests;
