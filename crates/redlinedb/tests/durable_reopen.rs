use redlinedb::Database;

#[test]
fn committed_autocommit_row_survives_checkpoint_and_reopen() {
    let root = tempfile::tempdir().expect("database root");
    let path = root.path().join("autocommit.redline");

    {
        let database = Database::create(&path).expect("create database");
        let mut connection = database.connect().expect("connect");
        connection
            .execute(
                "CREATE TABLE work_items(id TEXT PRIMARY KEY, version INTEGER)",
                (),
            )
            .expect("create table");
        connection
            .execute("INSERT INTO work_items VALUES (?, ?)", ("JRY-800", 1_i64))
            .expect("insert row");
        let version: i64 = connection
            .query_row("SELECT version FROM work_items WHERE id = ?", ("JRY-800",))
            .expect("read row before reopen");
        assert_eq!(version, 1);
        database.checkpoint().expect("checkpoint");
    }

    let reopened = Database::open(&path).expect("reopen database");
    let mut connection = reopened.connect().expect("connect after reopen");
    let full_scan_version: i64 = connection
        .query_row("SELECT version FROM work_items", ())
        .expect("read durable row with a full scan");
    assert_eq!(full_scan_version, 1);
    let version: i64 = connection
        .query_row("SELECT version FROM work_items WHERE id = ?", ("JRY-800",))
        .expect("read durable row");
    assert_eq!(version, 1);
}

#[test]
fn committed_transaction_survives_rollback_checkpoint_and_reopen() {
    let root = tempfile::tempdir().expect("database root");
    let path = root.path().join("transaction.redline");

    {
        let database = Database::create(&path).expect("create database");
        let mut connection = database.connect().expect("connect");
        connection
            .execute(
                "CREATE TABLE work_items(id TEXT PRIMARY KEY, version INTEGER)",
                (),
            )
            .expect("create table");
        connection.execute("BEGIN IMMEDIATE", ()).expect("begin");
        connection
            .execute("INSERT INTO work_items VALUES (?, ?)", ("JRY-800", 2_i64))
            .expect("insert row");
        connection.execute("COMMIT", ()).expect("commit");
        connection
            .execute("BEGIN IMMEDIATE", ())
            .expect("begin rollback probe");
        connection
            .execute(
                "UPDATE work_items SET version = 3 WHERE id = ?",
                ("JRY-800",),
            )
            .expect("update row");
        connection.execute("ROLLBACK", ()).expect("rollback");
        let version: i64 = connection
            .query_row("SELECT version FROM work_items WHERE id = ?", ("JRY-800",))
            .expect("read committed row before reopen");
        assert_eq!(version, 2);
        database.checkpoint().expect("checkpoint");
    }

    let reopened = Database::open(&path).expect("reopen database");
    let mut connection = reopened.connect().expect("connect after reopen");
    let full_scan_version: i64 = connection
        .query_row("SELECT version FROM work_items", ())
        .expect("read durable row with a full scan");
    assert_eq!(full_scan_version, 2);
    let version: i64 = connection
        .query_row("SELECT version FROM work_items WHERE id = ?", ("JRY-800",))
        .expect("read durable row");
    assert_eq!(version, 2);
}

/// Workplan R3: a rolled-back transaction's id must not be handed out again
/// after a clean close. Otherwise the next transaction that writes commits
/// under that id, and the following reopen replays the rolled-back rows as
/// committed.
#[test]
fn rollback_then_reopen_commit_does_not_resurrect_rows() {
    let root = tempfile::tempdir().expect("database root");
    let path = root.path().join("rollback.redline");

    {
        let database = Database::create(&path).expect("create database");
        let mut connection = database.connect().expect("connect");
        connection
            .execute("CREATE TABLE ghosts(id INTEGER PRIMARY KEY, note TEXT)", ())
            .expect("create ghosts");
        connection
            .execute("CREATE TABLE audit(id INTEGER PRIMARY KEY, note TEXT)", ())
            .expect("create audit");
        connection.execute("BEGIN IMMEDIATE", ()).expect("begin");
        connection
            .execute("INSERT INTO ghosts VALUES (1, 'rolled back')", ())
            .expect("insert ghost");
        connection.execute("ROLLBACK", ()).expect("rollback");
        let ghosts: i64 = connection
            .query_row("SELECT count(*) FROM ghosts", ())
            .expect("count ghosts before close");
        assert_eq!(ghosts, 0);
    }

    {
        let database = Database::open(&path).expect("first reopen");
        let mut connection = database.connect().expect("connect after first reopen");
        connection
            .execute("INSERT INTO audit VALUES (1, 'after reopen')", ())
            .expect("write after reopen");
    }

    let reopened = Database::open(&path).expect("second reopen");
    let mut connection = reopened.connect().expect("connect after second reopen");
    let ghosts: i64 = connection
        .query_row("SELECT count(*) FROM ghosts", ())
        .expect("count ghosts after second reopen");
    assert_eq!(ghosts, 0, "the rolled-back insert came back as committed");
    let audit: i64 = connection
        .query_row("SELECT count(*) FROM audit", ())
        .expect("count audit rows");
    assert_eq!(audit, 1);
}
