//! group_concat separators, NULL-only and empty input, GROUP BY, and the
//! string_agg alias.

use super::*;

#[test]
fn group_concat_basic_default_separator() {
    let (_d, c) = open();
    let setup = [
        "CREATE TABLE t(v TEXT)",
        "INSERT INTO t VALUES ('b'), ('c'), ('a')",
    ];
    for statement in setup {
        c.execute(statement).expect("setup");
    }
    assert_matches_sqlite(&c, &setup, "SELECT group_concat(v ORDER BY v) FROM t");
}

#[test]
fn group_concat_custom_separator() {
    let (_d, c) = open();
    let setup = [
        "CREATE TABLE t(v TEXT)",
        "INSERT INTO t VALUES ('z'), ('x'), ('y')",
    ];
    for statement in setup {
        c.execute(statement).expect("setup");
    }
    assert_matches_sqlite(
        &c,
        &setup,
        "SELECT group_concat(v, ' | ' ORDER BY v) FROM t",
    );
}

#[test]
fn group_concat_all_null_returns_null() {
    let (_d, c) = open();
    c.execute("CREATE TABLE t(v TEXT)").expect("create");
    c.execute("INSERT INTO t VALUES (NULL), (NULL)")
        .expect("insert");
    let v = q1(&c, "SELECT group_concat(v) FROM t");
    assert_eq!(v, SqlValue::Null);
}

#[test]
fn group_concat_empty_table_returns_null() {
    let (_d, c) = open();
    c.execute("CREATE TABLE t(v TEXT)").expect("create");
    let v = q1(&c, "SELECT group_concat(v) FROM t");
    assert_eq!(v, SqlValue::Null);
}

#[test]
fn group_concat_with_group_by() {
    let (_d, c) = open();
    setup_words(&c);
    assert_matches_sqlite(
        &c,
        SETUP_WORDS,
        "SELECT grp, group_concat(w ORDER BY w) FROM words WHERE w IS NOT NULL GROUP BY grp ORDER BY grp",
    );
}

// ── string_agg (alias) ────────────────────────────────────────────────────────

#[test]
fn string_agg_alias_works() {
    let (_d, c) = open();
    let setup = [
        "CREATE TABLE t(v TEXT)",
        "INSERT INTO t VALUES ('q'), ('p')",
    ];
    for statement in setup {
        c.execute(statement).expect("setup");
    }
    assert_matches_sqlite(&c, &setup, "SELECT string_agg(v, '-' ORDER BY v) FROM t");
}
