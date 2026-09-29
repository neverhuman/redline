//! Q5-10: declared column collations (NOCASE, RTRIM, a quoted collation
//! name) compare, sort, group and choose indexes as SQLite 3.53.1 does.

use super::*;

fn ints(conn: &Arc<Connection>, sql: &str) -> Vec<i64> {
    rows(conn, sql)
        .into_iter()
        .flat_map(|row| row.into_iter())
        .map(|value| match value {
            SqlValue::Integer(v) => v,
            other => panic!("{sql}: expected integers, got {other:?}"),
        })
        .collect()
}

fn texts(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    rows(conn, sql)
        .into_iter()
        .map(|row| text(&row[0]).to_owned())
        .collect()
}

#[test]
fn symmetric_declared_nocase() {
    let (_d, c) = open();
    c.execute("CREATE TABLE t(x TEXT COLLATE NOCASE)")
        .expect("create");
    c.execute("INSERT INTO t VALUES('a')").expect("insert");
    // Explicit left > explicit right > column left > column right: the
    // column's NOCASE applies with the column on either side.
    assert_eq!(
        ints(&c, "SELECT x='A', 'A'=x, x<'B', 'B'>x FROM t"),
        [1, 1, 1, 1]
    );
    assert_eq!(ints(&c, "SELECT count(*) FROM t WHERE 'A'=x"), [1]);
    // An explicit collation on either side wins over the column's.
    assert_eq!(
        ints(
            &c,
            "SELECT x='A' COLLATE BINARY, 'A' COLLATE BINARY = x FROM t"
        ),
        [0, 0]
    );
}

#[test]
fn quoted_column_collation() {
    let (_d, c) = open();
    c.execute("CREATE TABLE q(\"my col\" TEXT COLLATE NOCASE, [b,c] TEXT COLLATE NOCASE)")
        .expect("create");
    c.execute("INSERT INTO q VALUES('a', 'b')").expect("insert");
    assert_eq!(
        ints(
            &c,
            "SELECT \"my col\"='A', 'A'=\"my col\", [b,c]='B' FROM q"
        ),
        [1, 1, 1]
    );
}

#[test]
fn declared_rtrim_compare() {
    let (_d, c) = open();
    c.execute("CREATE TABLE r(x TEXT COLLATE RTRIM)")
        .expect("create");
    c.execute("INSERT INTO r VALUES('x')").expect("insert");
    assert_eq!(ints(&c, "SELECT x='x  ', 'x  '=x FROM r"), [1, 1]);
}

#[test]
fn nocase_order_group_distinct_union_in_between() {
    let (_d, c) = open();
    c.execute("CREATE TABLE n(x TEXT COLLATE NOCASE)")
        .expect("create");
    c.execute("INSERT INTO n VALUES('b'),('A'),('a'),('B')")
        .expect("insert");
    assert_eq!(
        texts(&c, "SELECT x FROM n ORDER BY x"),
        ["A", "a", "b", "B"]
    );
    assert_eq!(
        ints(&c, "SELECT count(*) FROM (SELECT x FROM n GROUP BY x)"),
        [2]
    );
    assert_eq!(
        ints(&c, "SELECT count(*) FROM (SELECT DISTINCT x FROM n)"),
        [2]
    );
    assert_eq!(ints(&c, "SELECT count(DISTINCT x) FROM n"), [2]);
    assert_eq!(
        ints(
            &c,
            "SELECT count(*) FROM (SELECT x FROM n UNION SELECT 'a')"
        ),
        [2]
    );
    assert_eq!(
        ints(
            &c,
            "SELECT count(*) FROM (SELECT x FROM n INTERSECT SELECT 'A')"
        ),
        [1]
    );
    assert_eq!(
        ints(
            &c,
            "SELECT count(*) FROM (SELECT x FROM n EXCEPT SELECT 'A')"
        ),
        [1]
    );
    assert_eq!(ints(&c, "SELECT count(*) FROM n WHERE x IN ('A')"), [2]);
    assert_eq!(
        ints(&c, "SELECT count(*) FROM n WHERE x BETWEEN 'A' AND 'A'"),
        [2]
    );
    assert_eq!(
        texts(
            &c,
            "SELECT CASE x WHEN 'A' THEN 'hit' ELSE 'miss' END FROM n WHERE x='a' LIMIT 1"
        ),
        ["hit"]
    );
    // DISTINCT and GROUP BY keep one of each case-folded value.
    assert_eq!(
        ints(
            &c,
            "SELECT count(*) FROM (SELECT DISTINCT x FROM n ORDER BY x DESC)"
        ),
        [2]
    );
}

#[test]
fn nocase_index_not_used_for_binary_eq() {
    let (_d, c) = open();
    c.execute("CREATE TABLE b(id INTEGER PRIMARY KEY, x TEXT)")
        .expect("create");
    c.execute("INSERT INTO b(x) VALUES('x'),('X'),('y')")
        .expect("insert");
    c.execute("CREATE INDEX bi ON b(x COLLATE NOCASE)")
        .expect("index");
    // BINARY equality: one row each way, whether or not the index is named.
    for sql in [
        "SELECT count(*) FROM b WHERE x='x'",
        "SELECT count(*) FROM b INDEXED BY bi WHERE x='x'",
        "SELECT count(*) FROM b NOT INDEXED WHERE x='x'",
        "SELECT count(*) FROM b WHERE x='X'",
        "SELECT count(*) FROM b INDEXED BY bi WHERE x='X'",
        "SELECT count(*) FROM b NOT INDEXED WHERE x='X'",
    ] {
        assert_eq!(ints(&c, sql), [1], "{sql}");
    }
    // NOCASE equality matches both, and may use the NOCASE index.
    for sql in [
        "SELECT count(*) FROM b WHERE x='X' COLLATE NOCASE",
        "SELECT count(*) FROM b INDEXED BY bi WHERE x='X' COLLATE NOCASE",
        "SELECT count(*) FROM b NOT INDEXED WHERE x COLLATE NOCASE='X'",
    ] {
        assert_eq!(ints(&c, sql), [2], "{sql}");
    }
    // Binary order, not the index's.
    assert_eq!(texts(&c, "SELECT x FROM b ORDER BY x"), ["X", "x", "y"]);
    // A declared-NOCASE column with a BINARY index: NOCASE equality must
    // not probe the BINARY keys.
    c.execute("CREATE TABLE c(id INTEGER PRIMARY KEY, x TEXT COLLATE NOCASE)")
        .expect("create c");
    c.execute("INSERT INTO c(x) VALUES('x'),('X')")
        .expect("insert c");
    c.execute("CREATE INDEX ci ON c(x COLLATE BINARY)")
        .expect("index c");
    assert_eq!(ints(&c, "SELECT count(*) FROM c WHERE x='x'"), [2]);
    assert_eq!(
        ints(
            &c,
            "SELECT count(*) FROM c INDEXED BY ci WHERE x='x' COLLATE BINARY"
        ),
        [1]
    );
}

#[test]
fn declared_collation_index_is_used_and_returns_stored_values() {
    let (_d, c) = open();
    c.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, x TEXT COLLATE NOCASE)")
        .expect("create");
    c.execute("INSERT INTO t(x) VALUES('b'),('A'),('a'),('B'),('Abc')")
        .expect("insert");
    c.execute("CREATE INDEX ti ON t(x)").expect("index");
    let plan = rows(&c, "EXPLAIN QUERY PLAN SELECT x FROM t WHERE x='A'")
        .iter()
        .map(|row| text(row.last().expect("detail column")).to_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plan.contains("USING INDEX ti"), "{plan}");
    // The index keys are case-folded; the rows come back as stored.
    assert_eq!(
        texts(&c, "SELECT x FROM t WHERE x='A' ORDER BY id"),
        ["A", "a"]
    );
    assert_eq!(ints(&c, "SELECT count(*) FROM t WHERE x > 'a'"), [3]);
    assert_eq!(
        texts(&c, "SELECT x FROM t WHERE x >= 'a' ORDER BY x LIMIT 3"),
        ["A", "a", "Abc"]
    );
    // A declared RTRIM column and its index find 'a ' for 'a'.
    c.execute("CREATE TABLE r(id INTEGER PRIMARY KEY, x TEXT COLLATE RTRIM)")
        .expect("create r");
    c.execute("INSERT INTO r(x) VALUES('a '),('a'),('b  ')")
        .expect("insert r");
    c.execute("CREATE INDEX ri ON r(x)").expect("index r");
    assert_eq!(ints(&c, "SELECT count(*) FROM r WHERE x='a'"), [2]);
    assert_eq!(texts(&c, "SELECT x || '|' FROM r WHERE x='b'"), ["b  |"]);
}
