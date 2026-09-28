//! A persistent database whose dirty pages outgrow its buffer pool keeps
//! accepting writes from many connections at once.
//!
//! Eviction may write a page on its own only when crash recovery can rebuild
//! it alone, so a pool full of dirty logged pages needs a checkpoint to make
//! room. The SQL layer now lets the pool ask for one. Before, such a pool
//! failed the insert with "no unpinned frame available for eviction".

use std::thread;

use redlinedb::{Database, Durability, OpenOptions};
use redlinedb_kernel::format::DEFAULT_PAGE_SIZE;

const POOL_PAGES: usize = 32;
const WRITERS: i64 = 4;
const ROWS_PER_WRITER: i64 = 120;
/// Two rows to a 16 KiB page: 480 rows fill 240 pages, many times the pool.
const BODY_BYTES: usize = 7000;

fn options() -> OpenOptions {
    let mut options = OpenOptions::default().with_durability(Durability::Normal);
    options.memory.cache_bytes = POOL_PAGES * DEFAULT_PAGE_SIZE;
    options
}

fn body(id: i64) -> String {
    format!("{id:08}{}", "x".repeat(BODY_BYTES - 8))
}

fn assert_rows(database: &Database) {
    let mut connection = database.connect().expect("connect");
    let count: i64 = connection
        .query_row("SELECT count(*) FROM t", ())
        .expect("count");
    assert_eq!(count, WRITERS * ROWS_PER_WRITER);
    for id in [0, 1_000 + 7, 2_000 + ROWS_PER_WRITER - 1, 3_000 + 60] {
        let found: String = connection
            .query_row("SELECT body FROM t WHERE tag = ?1", (format!("tag-{id}"),))
            .expect("index lookup");
        assert_eq!(found, body(id), "row {id}");
    }
    let check: String = connection
        .query_row("PRAGMA integrity_check", ())
        .expect("integrity check");
    assert_eq!(check, "ok");
}

#[test]
fn many_connections_write_past_the_buffer_pool_and_reopen() {
    let root = tempfile::tempdir().expect("database root");
    let path = root.path().join("pressure.redline");
    {
        let database = Database::open_with_options(&path, options().with_create(true))
            .expect("create database");
        assert_eq!(database.buffer_pool_pages(), POOL_PAGES);
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
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let database = database.clone();
                thread::spawn(move || {
                    let mut connection = database.connect().expect("connect writer");
                    for i in 0..ROWS_PER_WRITER {
                        let id = writer * 1_000 + i;
                        connection
                            .execute(
                                "INSERT INTO t(id, tag, body) VALUES(?1, ?2, ?3)",
                                (id, format!("tag-{id}"), body(id)),
                            )
                            .unwrap_or_else(|err| panic!("writer {writer} row {i}: {err}"));
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("writer panicked");
        }
        assert_rows(&database);
    }

    let reopened = Database::open_with_options(&path, options()).expect("reopen");
    assert_rows(&reopened);
}
