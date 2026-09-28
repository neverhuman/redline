#![cfg(feature = "failpoints")]
//! An uncertain commit through the public crate (workplan R7).
//!
//! The WAL fsync fails after the commit record was written. The commit is
//! an I/O error that says the outcome is uncertain, later writes fail as
//! I/O errors naming the stopped WAL writer, and a reopen finds the row.
//!
//! `return(<db path>)` fails only this test's WAL; the fault fires on the
//! WAL writer thread, where no thread-local switch reaches.

use redlinedb::{Database, Durability, ErrorCode, OpenOptions};
use redlinedb_kernel::failpoints;

#[test]
fn commit_after_wal_fsync_failure_is_uncertain_io_error() {
    let root = tempfile::tempdir().expect("database root");
    let path = root.path().join("uncertain.redline");
    let options = OpenOptions {
        durability: Durability::Strict,
        ..OpenOptions::default()
    };

    {
        let database = Database::open_with_options(&path, options.clone()).expect("create");
        let mut connection = database.connect().expect("connect");
        connection
            .execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())
            .expect("create table");

        failpoints::cfg("wal::flush_error", &format!("return({})", path.display()))
            .expect("arm wal flush failure");
        let commit = connection.execute("INSERT INTO t VALUES (?, ?)", (1_i64, "uncertain"));
        let later = connection.execute("INSERT INTO t VALUES (?, ?)", (2_i64, "after"));
        failpoints::cfg("wal::flush_error", "off").expect("disarm wal flush failure");

        let commit = commit.expect_err("the commit's fsync failed");
        assert_eq!(commit.code(), ErrorCode::IoErr, "{commit}");
        assert!(
            commit.message().contains("commit outcome uncertain"),
            "{commit}"
        );
        let later = later.expect_err("the WAL writer stopped");
        assert_eq!(later.code(), ErrorCode::IoErr, "{later}");
        assert!(later.message().contains("wal writer failed"), "{later}");
        let visible: i64 = connection
            .query_row("SELECT COUNT(*) FROM t", ())
            .expect("count rows");
        assert_eq!(visible, 0, "an uncertain commit is not shown");
    }

    let database = Database::open_with_options(&path, options).expect("reopen");
    let mut connection = database.connect().expect("connect after reopen");
    let value: String = connection
        .query_row("SELECT v FROM t WHERE id = 1", ())
        .expect("the written commit record recovers");
    assert_eq!(value, "uncertain");
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM t", ())
        .expect("count rows after reopen");
    assert_eq!(rows, 1);
}
