//! The `rldb_config` -> `DbOptions` mapping, compared field by field with
//! the options each config must produce.

use std::mem::{offset_of, size_of};
use std::time::Duration;

use redlinedb_kernel::engine::CommitDurability;
use redlinedb_sql::DbOptions;

use super::*;

fn config() -> rldb_config {
    rldb_config {
        struct_size: size_of::<rldb_config>() as u32,
        flags: 0,
        durability: 0,
        cache_bytes: 0,
        work_mem_bytes: 0,
        max_spill_bytes: 0,
        statement_cache_capacity: 0,
        busy_timeout_ms: 0,
    }
}

fn options(config: &rldb_config) -> Result<DbOptions, c_int> {
    // SAFETY: `config` is a live, fully initialised rldb_config whose
    // struct_size never exceeds its size in these tests.
    unsafe { db_options_from_config(config) }
}

fn page_size() -> usize {
    DbOptions::default().engine.page_size
}

#[test]
fn all_zero_fields_keep_every_default() {
    assert_eq!(options(&config()), Ok(DbOptions::default()));
}

#[test]
fn every_non_zero_field_reaches_its_option() {
    let config = rldb_config {
        durability: RLDB_DURABILITY_NORMAL,
        cache_bytes: 64 * page_size() as u64,
        work_mem_bytes: 12_345,
        max_spill_bytes: 67_890,
        statement_cache_capacity: 7,
        busy_timeout_ms: 1_234,
        ..config()
    };
    let mut expected = DbOptions::default();
    expected.engine.commit_durability = CommitDurability::Normal;
    expected.engine.buffer_pool_pages = 64;
    expected.query_memory.work_mem_bytes = 12_345;
    expected.query_memory.max_spill_bytes = 67_890;
    expected.statement_cache_capacity = 7;
    // The busy timeout bounds both lock waits, as rldb_busy_timeout does.
    expected.busy_timeout = Duration::from_millis(1_234);
    expected.engine.busy_timeout = Duration::from_millis(1_234);
    assert_eq!(options(&config), Ok(expected));
}

#[test]
fn durability_values_map_and_unknown_ones_are_refused() {
    let with = |durability| {
        options(&rldb_config {
            durability,
            ..config()
        })
    };
    let strict = with(RLDB_DURABILITY_STRICT).expect("strict");
    assert_eq!(strict.engine.commit_durability, CommitDurability::Strict);
    let normal = with(RLDB_DURABILITY_NORMAL).expect("normal");
    assert_eq!(normal.engine.commit_durability, CommitDurability::Normal);
    assert_eq!(with(3), Err(RLDB_MISUSE));
    assert_eq!(
        options(&rldb_config {
            flags: 1,
            ..config()
        }),
        Err(RLDB_MISUSE)
    );
}

#[test]
fn cache_bytes_has_a_sixteen_page_floor_and_saturates() {
    let pages = |cache_bytes| {
        options(&rldb_config {
            cache_bytes,
            ..config()
        })
        .expect("options")
        .engine
        .buffer_pool_pages
    };
    assert_eq!(pages(1), 16);
    assert_eq!(pages(16 * page_size() as u64 - 1), 16);
    assert_eq!(pages(17 * page_size() as u64), 17);
    assert_eq!(pages(u64::MAX), usize::MAX / page_size());
}

#[test]
fn a_shorter_struct_ignores_the_bytes_past_its_size() {
    // Every field non-zero, then cut the struct at each field boundary: the
    // fields past struct_size must keep their defaults.
    let full = rldb_config {
        durability: RLDB_DURABILITY_NORMAL,
        cache_bytes: 64 * page_size() as u64,
        work_mem_bytes: 12_345,
        max_spill_bytes: 67_890,
        statement_cache_capacity: 7,
        busy_timeout_ms: 1_234,
        ..config()
    };
    let cut = |size: usize| {
        options(&rldb_config {
            struct_size: size as u32,
            ..full
        })
        .expect("options")
    };
    let defaults = DbOptions::default();
    let at_cache = cut(offset_of!(rldb_config, cache_bytes));
    assert_eq!(
        at_cache.engine.commit_durability,
        CommitDurability::Normal,
        "durability is inside a struct cut at cache_bytes"
    );
    assert_eq!(
        at_cache.engine.buffer_pool_pages,
        defaults.engine.buffer_pool_pages
    );
    let at_work_mem = cut(offset_of!(rldb_config, work_mem_bytes));
    assert_eq!(at_work_mem.engine.buffer_pool_pages, 64);
    assert_eq!(at_work_mem.query_memory, defaults.query_memory);
    let at_spill = cut(offset_of!(rldb_config, max_spill_bytes));
    assert_eq!(at_spill.query_memory.work_mem_bytes, 12_345);
    assert_eq!(
        at_spill.query_memory.max_spill_bytes,
        defaults.query_memory.max_spill_bytes
    );
    let at_cache_capacity = cut(offset_of!(rldb_config, statement_cache_capacity));
    assert_eq!(at_cache_capacity.query_memory.max_spill_bytes, 67_890);
    assert_eq!(
        at_cache_capacity.statement_cache_capacity,
        defaults.statement_cache_capacity
    );
    let at_busy = cut(offset_of!(rldb_config, busy_timeout_ms));
    assert_eq!(at_busy.statement_cache_capacity, 7);
    assert_eq!(at_busy.busy_timeout, defaults.busy_timeout);
    assert_eq!(at_busy.engine.busy_timeout, defaults.engine.busy_timeout);
    // A struct cut inside a field is refused.
    assert_eq!(
        options(&rldb_config {
            struct_size: offset_of!(rldb_config, work_mem_bytes) as u32 + 4,
            ..full
        }),
        Err(RLDB_MISUSE)
    );
}
