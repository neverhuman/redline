//! IDX-EPOCH: INTEGER and REAL share one index key space.
//!
//! RedlineDB 4.x gave INTEGER and REAL index keys separate type tags
//! (`crates/kernel/src/catalog/key.rs`), so an index ordered every INTEGER
//! before every REAL. Through an index `x < 2` lost `1.5`, `x = 2` lost a
//! stored `2.0`, `ORDER BY x LIMIT n` returned `2, 3` ahead of `1.5`, and a
//! UNIQUE index accepted both `1` and `1.0`. SQLite compares an INTEGER and a
//! REAL by numeric value (`sqlite3IntFloatCompare`) everywhere, including in
//! index b-trees. Every query here must give SQLite's answer both with
//! `INDEXED BY` and with `NOT INDEXED`.

use std::cmp::Ordering;

use redlinedb_kernel::catalog::{SortDir, ValueRef, compare_index_keys, encode_index_key};
use redlinedb_sql::SqlValue;

use crate::lab::{Lab, int, real};

/// Numerically distinct values, INTEGER and REAL interleaved, including
/// neighbours above 2^53 and the i64 edges.
const DISTINCT: &str = "(2),(3),(1.5),(1),(-1.5),(-1),(0.25),(0),(2.75),\
    (9007199254740993),(9007199254740992.0),(9007199254740994),(9007199254740995.0),\
    (9223372036854775807),(9223372036854775808.0),(1e300),(1e999),\
    ((-9223372036854775807-1)),(-9223372036854777856.0),(-1e300),(-1e999)";

/// Access paths every query runs through.
const ACCESS: &[&str] = &[
    "o INDEXED BY o_asc",
    "o INDEXED BY o_desc",
    "o NOT INDEXED",
    "o",
];

#[test]
fn index_order_is_numeric_across_integer_and_real() {
    let lab = Lab::new();
    lab.exec_both(&format!(
        "CREATE TABLE o(x); INSERT INTO o VALUES {DISTINCT}; \
         CREATE INDEX o_asc ON o(x); CREATE INDEX o_desc ON o(x DESC);"
    ));
    let queries = [
        "SELECT x FROM {access} ORDER BY x",
        "SELECT x FROM {access} ORDER BY x DESC",
        "SELECT x FROM {access} WHERE x > 1 ORDER BY x LIMIT 3",
        "SELECT x FROM {access} WHERE x < 2 ORDER BY x DESC LIMIT 4",
        "SELECT x FROM {access} WHERE x BETWEEN 1 AND 2.5 ORDER BY x",
        "SELECT x FROM {access} WHERE x >= 1.5 AND x < 3 ORDER BY x DESC",
        "SELECT x FROM {access} WHERE x > 9007199254740992 ORDER BY x",
        "SELECT x FROM {access} WHERE x < 9007199254740994.0 AND x > 3 ORDER BY x",
        "SELECT x FROM {access} WHERE x >= 9223372036854775807 ORDER BY x",
        "SELECT x FROM {access} WHERE x <= -9223372036854775808.0 ORDER BY x",
        "SELECT x, typeof(x) FROM {access} WHERE x > 0 ORDER BY x",
    ];
    for query in queries {
        for access in ACCESS {
            lab.assert_same(&query.replace("{access}", access), true);
        }
    }
}

#[test]
fn index_range_bounds_cover_integer_and_real() {
    let lab = Lab::new();
    lab.exec_both(&format!(
        "CREATE TABLE o(x); INSERT INTO o VALUES {DISTINCT},(2.0),(1.0),(0.0),(NULL),('t'); \
         CREATE INDEX o_asc ON o(x); CREATE INDEX o_desc ON o(x DESC);"
    ));
    let predicates = [
        "x < 2",
        "x <= 2",
        "x > 2",
        "x >= 2.0",
        "x < 2.5",
        "x > 1.5",
        "x BETWEEN 1.5 AND 3",
        "x BETWEEN 1 AND 2",
        "x > -1.5 AND x < 1",
        "x >= 9007199254740992 AND x <= 9007199254740993",
        "x > 9223372036854775807",
        "x < -9223372036854775808.0",
    ];
    for predicate in predicates {
        for access in ACCESS {
            lab.assert_same(
                &format!("SELECT x, typeof(x) FROM {access} WHERE {predicate}"),
                false,
            );
        }
    }
}

#[test]
fn open_index_ranges_skip_null_keys_and_respect_desc_keys() {
    // Found with this change: a range with no lower bound started at the
    // NULL keys, so `WHERE x < 5 ORDER BY x LIMIT 1` spent its row on a NULL
    // the predicate then rejected and returned nothing; a leading DESC key
    // took its value bounds in ASC byte order and returned no rows at all;
    // and the ordered-LIMIT walk ignored the key direction.
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE r(x, y); \
         INSERT INTO r VALUES (NULL, 1),(NULL, NULL),(1, NULL),(1, 2),(1, 1.5),(1, 3),\
         (2, NULL),(2, 1),(1.5, 0),(3, 'a'),(-1, x'00'); \
         CREATE INDEX r_x ON r(x); CREATE INDEX r_x_desc ON r(x DESC); \
         CREATE INDEX r_xy ON r(x, y);",
    );
    let queries = [
        "SELECT x FROM {access} WHERE x < 5 ORDER BY x LIMIT 1",
        "SELECT x FROM {access} WHERE x < 5 ORDER BY x DESC LIMIT 2",
        "SELECT x FROM {access} WHERE x <= 1.5 ORDER BY x LIMIT 2 OFFSET 1",
        "SELECT x FROM {access} WHERE x > 1 ORDER BY x LIMIT 2",
        "SELECT x FROM {access} WHERE x > 1 ORDER BY x DESC LIMIT 2",
        "SELECT x FROM {access} WHERE x BETWEEN 1 AND 2 ORDER BY x DESC LIMIT 3",
    ];
    for query in queries {
        for access in ["r INDEXED BY r_x", "r INDEXED BY r_x_desc", "r NOT INDEXED"] {
            lab.assert_same(&query.replace("{access}", access), true);
        }
    }
    for query in [
        "SELECT y FROM r INDEXED BY r_xy WHERE x = 1 AND y < 3 ORDER BY y LIMIT 1",
        "SELECT y FROM r INDEXED BY r_xy WHERE x = 1 AND y < 3 ORDER BY y",
        "SELECT y FROM r INDEXED BY r_xy WHERE x = 1 AND y > 1 ORDER BY y",
        "SELECT quote(y) FROM r INDEXED BY r_xy WHERE x = 2 AND y <= 1",
    ] {
        lab.assert_same(query, true);
    }
}

#[test]
fn index_equality_matches_integer_and_real() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE e(x, tag); \
         INSERT INTO e VALUES (2,'int'),(2.0,'real'),(1.5,'half'),(0,'zero'),(-0.0,'negzero'),\
         (9007199254740993,'big'),(9007199254740992.0,'bigreal'),(9007199254740992,'bigint'),\
         ((-9223372036854775807-1),'min'),(-9223372036854775808.0,'minreal'),\
         (9223372036854775807,'max'),(9223372036854775808.0,'twopow63'); \
         CREATE INDEX e_x ON e(x); CREATE INDEX e_x_tag ON e(x, tag);",
    );
    let predicates = [
        "x = 2",
        "x = 2.0",
        "x = 1.5",
        "x = 0",
        "x = -0.0",
        "x IN (2, 1.5)",
        "x IN (2.0, 0.0)",
        "x = 9007199254740992",
        "x = 9007199254740992.0",
        "x = 9007199254740993",
        "x = (-9223372036854775807-1)",
        "x = -9223372036854775808.0",
        "x = 9223372036854775807",
        "x = 9223372036854775808.0",
        "x = 2 AND tag > 'a'",
    ];
    for predicate in predicates {
        for access in [
            "e INDEXED BY e_x",
            "e INDEXED BY e_x_tag",
            "e NOT INDEXED",
            "e",
        ] {
            lab.assert_same(
                &format!("SELECT tag, count(*) FROM {access} WHERE {predicate} GROUP BY tag"),
                false,
            );
        }
    }
    for access in ["e INDEXED BY e_x", "e NOT INDEXED"] {
        let sql = format!("SELECT tag FROM {access} WHERE x = ?");
        for value in [
            int(2),
            real(2.0),
            int(9007199254740992),
            real(9007199254740992.0),
            int(9007199254740993),
            real(-9223372036854775808.0),
        ] {
            lab.assert_same_bound(&sql, &[value], false);
        }
    }
}

#[test]
fn unique_index_treats_equal_integer_and_real_as_duplicates() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE u(y); CREATE UNIQUE INDEX u_y ON u(y); INSERT INTO u VALUES (1);");
    lab.step_both("INSERT INTO u VALUES (1.0)");
    lab.step_both("INSERT INTO u VALUES (9007199254740992.0)");
    lab.step_both("INSERT INTO u VALUES (9007199254740993)");
    lab.step_both("INSERT INTO u VALUES (9007199254740992)");
    lab.assert_same("SELECT y, typeof(y) FROM u ORDER BY y", true);

    lab.exec_both("CREATE TABLE v(y); INSERT INTO v VALUES (3), (3.0);");
    lab.step_both("CREATE UNIQUE INDEX v_y ON v(y)");
    lab.assert_same("SELECT count(*) FROM v WHERE y = 3", true);
}

#[test]
fn covering_scans_keep_the_stored_storage_class() {
    // A whole number is one key whether it was stored as INTEGER or REAL,
    // so an index-only scan must take the storage class from the column's
    // affinity, or read the heap when the column has none.
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE a(n NUMERIC, r REAL, i INTEGER, b); \
         INSERT INTO a VALUES (1, 1, 1, 1), (2.0, 2.0, 2.0, 2.0), (2.5, 2.5, 2.5, 2.5), \
         (3, 3, 3, 3.0), (-4.0, -4, -4.0, -4); \
         CREATE INDEX a_n ON a(n); CREATE INDEX a_r ON a(r); CREATE INDEX a_i ON a(i); \
         CREATE INDEX a_b ON a(b DESC);",
    );
    for (column, index) in [("n", "a_n"), ("r", "a_r"), ("i", "a_i"), ("b", "a_b")] {
        for order in ["", " DESC"] {
            lab.assert_same(
                &format!(
                    "SELECT {column} FROM a INDEXED BY {index} WHERE {column} > -10 \
                     ORDER BY {column}{order}"
                ),
                true,
            );
            lab.assert_same(
                &format!(
                    "SELECT {column} FROM a INDEXED BY {index} WHERE {column} > 1 \
                     ORDER BY {column}{order} LIMIT 2"
                ),
                true,
            );
        }
    }
}

#[test]
fn covering_scan_of_a_nocase_key_returns_the_stored_text() {
    // A NOCASE key holds the text folded to lower case, so an index-only
    // scan cannot reproduce 'ABC' from it.
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(a TEXT); CREATE INDEX t_a ON t(a COLLATE NOCASE); \
         INSERT INTO t VALUES ('ABC'), ('abd'), ('Abe');",
    );
    lab.assert_same("SELECT a FROM t INDEXED BY t_a WHERE a > ''", false);
}

/// Pools of INTEGER and REAL values around every boundary of the key space.
fn numeric_pool() -> Vec<SqlValue> {
    let two63 = 9_223_372_036_854_775_808.0_f64;
    let two53 = 9_007_199_254_740_992_i64;
    let mut pool = vec![
        int(0),
        int(1),
        int(-1),
        int(i64::MAX),
        int(i64::MAX - 1),
        int(i64::MIN),
        int(i64::MIN + 1),
        real(0.0),
        real(-0.0),
        real(f64::INFINITY),
        real(f64::NEG_INFINITY),
        real(f64::MIN_POSITIVE),
        real(-f64::MIN_POSITIVE),
        real(5e-324),
        real(-5e-324),
        real(f64::MAX),
        real(f64::MIN),
        real(two63),
        real(-two63),
        real(f64::from_bits(two63.to_bits() - 1)),
        real(f64::from_bits((-two63).to_bits() - 1)),
        real(f64::from_bits(two63.to_bits() + 1)),
        real(f64::from_bits((-two63).to_bits() + 1)),
        real(0.5),
        real(-0.5),
        real(0.999_999_999_999_999_9),
        real(-0.999_999_999_999_999_9),
        real(1.0 - f64::EPSILON / 2.0),
        real(4_503_599_627_370_495.5),
        real(-4_503_599_627_370_495.5),
    ];
    for delta in -3..=3 {
        pool.push(int(two53 + delta));
        pool.push(int(-two53 + delta));
        pool.push(real((two53 + delta) as f64));
        pool.push(real(-(two53 + delta) as f64));
    }
    // Deterministic xorshift so a failure names a reproducible value.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..400 {
        let raw = next();
        pool.push(int(raw as i64));
        pool.push(int((raw % 2001) as i64 - 1000));
        let bits = f64::from_bits(next());
        if !bits.is_nan() {
            pool.push(real(bits));
        }
        pool.push(real(((next() % 8001) as f64 - 4000.0) / 8.0));
        pool.push(real((raw as i64) as f64));
    }
    pool
}

fn key(value: &SqlValue, dir: SortDir) -> Vec<u8> {
    let part = match value {
        SqlValue::Integer(v) => ValueRef::Integer(*v),
        SqlValue::Real(v) => ValueRef::Real(*v),
        other => panic!("not numeric: {other:?}"),
    };
    let mut buf = Vec::new();
    encode_index_key(&[part], &[dir], &mut buf).bytes
}

#[test]
fn index_key_order_matches_sqlite_value_order() {
    // SQLite's own comparison of two bound values is the oracle: bound
    // parameters have no affinity, so `?1 < ?2` compares INTEGER and REAL
    // exactly.
    let lab = Lab::new();
    let mut stmt = lab
        .sqlite
        .prepare("SELECT (?1 > ?2) - (?1 < ?2)")
        .expect("prepare oracle");
    let pool = numeric_pool();
    let mut checked = 0usize;
    for (i, a) in pool.iter().enumerate() {
        // Every value against a spread of partners, both directions.
        for b in pool.iter().skip(i % 7).step_by(11) {
            let bind = |v: &SqlValue| match v {
                SqlValue::Integer(x) => rusqlite::types::Value::Integer(*x),
                SqlValue::Real(x) => rusqlite::types::Value::Real(*x),
                _ => unreachable!(),
            };
            let sqlite: i64 = stmt
                .query_row([bind(a), bind(b)], |row| row.get(0))
                .expect("oracle row");
            let want = sqlite.cmp(&0);
            let asc = compare_index_keys(&key(a, SortDir::Asc), &key(b, SortDir::Asc));
            assert_eq!(asc, want, "ASC key order of {a:?} vs {b:?}");
            let desc = compare_index_keys(&key(a, SortDir::Desc), &key(b, SortDir::Desc));
            assert_eq!(desc, want.reverse(), "DESC key order of {a:?} vs {b:?}");
            if want == Ordering::Equal {
                assert_eq!(key(a, SortDir::Asc), key(b, SortDir::Asc), "{a:?} == {b:?}");
            }
            checked += 1;
        }
    }
    assert!(checked > 10_000, "only {checked} pairs checked");
}
