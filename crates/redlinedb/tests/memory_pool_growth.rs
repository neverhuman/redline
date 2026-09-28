//! An in-memory database keeps working once its pages outgrow the buffer
//! pool. Its kernel engine is volatile: no WAL, no recovery, nothing to
//! checkpoint. Its pages used to stay pinned in the pool by their logged
//! LSNs, so the first page past the pool failed every insert with "no
//! unpinned frame available for eviction".

use redlinedb::{Database, OpenOptions};
use redlinedb_kernel::format::DEFAULT_PAGE_SIZE;

/// Two rows to a 16 KiB page.
const BODY_BYTES: usize = 7000;
/// Rows per transaction; one long transaction slows every insert down.
const BATCH: i64 = 64;
const POOL_PAGES: usize = 64;

#[test]
fn an_in_memory_database_holds_more_than_its_buffer_pool() {
    // A 64-page pool, a quarter of the default in-memory one, keeps the
    // test short; the default pool fails the same way, only later.
    let mut options = OpenOptions::default().with_lean_ephemeral(false);
    options.memory.cache_bytes = POOL_PAGES * DEFAULT_PAGE_SIZE;
    let database = Database::create_in_memory(options).expect("in-memory database");
    assert_eq!(database.buffer_pool_pages(), POOL_PAGES);
    let pool_bytes = POOL_PAGES * DEFAULT_PAGE_SIZE;
    // Three times the pool in row bodies alone, with an index over them.
    let rows = (3 * pool_bytes / BODY_BYTES) as i64;
    let mut connection = database.connect().expect("connect");
    connection
        .execute(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, tag TEXT NOT NULL, body TEXT NOT NULL)",
            (),
        )
        .expect("create table");
    connection
        .execute("CREATE INDEX t_tag ON t(tag)", ())
        .expect("create index");
    let tag = |id: i64| format!("tag-{id:08}");
    let body = |id: i64| format!("{id:08}{}", "x".repeat(BODY_BYTES - 8));
    for batch in (0..rows).step_by(BATCH as usize) {
        connection.execute("BEGIN", ()).expect("begin");
        for id in batch..(batch + BATCH).min(rows) {
            connection
                .execute(
                    "INSERT INTO t(id, tag, body) VALUES(?1, ?2, ?3)",
                    (id, tag(id), body(id)),
                )
                .unwrap_or_else(|err| panic!("insert {id} of {rows}: {err}"));
        }
        connection.execute("COMMIT", ()).expect("commit");
    }

    let count: i64 = connection
        .query_row("SELECT count(*) FROM t", ())
        .expect("count");
    assert_eq!(count, rows);
    for id in [0, rows / 3, rows / 2, rows - 1] {
        let found: String = connection
            .query_row("SELECT body FROM t WHERE tag = ?1", (tag(id),))
            .expect("index lookup");
        assert_eq!(found, body(id));
    }
    let check: String = connection
        .query_row("PRAGMA integrity_check", ())
        .expect("integrity check");
    assert_eq!(check, "ok");
}
