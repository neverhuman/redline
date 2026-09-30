//! Unique point lookups agree with a scan and with SQLite (shard 10, #6).
//!
//! Commit f5714d855 answers `WHERE unique_key = value` from the unique index
//! instead of the routed full scan (now `exec::select_route_gate`). Its test
//! covered one INTEGER key. This matrix runs point lookups over every kind
//! of unique index -- plain, inline UNIQUE, NOCASE and RTRIM columns, a
//! NOCASE index on a BINARY column, composite, partial, expression, TEXT
//! affinity -- with literals and bound parameters of every storage class and
//! NULL, then again after deleting and moving keys, inside a transaction
//! that rolls back, from a statement prepared before the change, and from a
//! transaction that holds its snapshot while another connection commits.
//! Each answer must equal the `NOT INDEXED` scan and SQLite (the scan is
//! the reference where SQLite cannot take part, and for `s = ?` on an RTRIM
//! column, which RedlineDB compares without the column's collation whether
//! or not it uses the index).
//!
//! The matrix found one wrong answer: a `COLLATE NOCASE` index on a BINARY
//! column was probed with the raw constant, so `name = 'Gamma'` found
//! nothing. Such an index is no longer probed.

use std::sync::Arc;

use redlinedb_sql::{Connection, SqlValue, Step};

use crate::lab::{Lab, Outcome, int, null, real, text};

const SETUP: &str = "CREATE TABLE a(id INTEGER PRIMARY KEY, k INTEGER, v TEXT); \
    CREATE UNIQUE INDEX a_k ON a(k); \
    CREATE TABLE ai(id INTEGER PRIMARY KEY, k INTEGER UNIQUE, v TEXT); \
    CREATE TABLE b(id INTEGER PRIMARY KEY, name TEXT COLLATE NOCASE, v INTEGER); \
    CREATE UNIQUE INDEX b_name ON b(name); \
    CREATE TABLE c(id INTEGER PRIMARY KEY, name TEXT, v INTEGER); \
    CREATE UNIQUE INDEX c_nocase ON c(name COLLATE NOCASE); \
    CREATE TABLE d(id INTEGER PRIMARY KEY, s TEXT COLLATE RTRIM, v INTEGER); \
    CREATE UNIQUE INDEX d_s ON d(s); \
    CREATE TABLE e(id INTEGER PRIMARY KEY, x INTEGER, y TEXT, v INTEGER); \
    CREATE UNIQUE INDEX e_xy ON e(x, y); \
    CREATE TABLE f(id INTEGER PRIMARY KEY, k INTEGER, flag INTEGER, v INTEGER); \
    CREATE UNIQUE INDEX f_k ON f(k) WHERE flag = 1; \
    CREATE TABLE g(id INTEGER PRIMARY KEY, name TEXT, v INTEGER); \
    CREATE UNIQUE INDEX g_lower ON g(lower(name)); \
    CREATE TABLE h(id INTEGER PRIMARY KEY, t TEXT, v INTEGER); \
    CREATE UNIQUE INDEX h_t ON h(t); \
    INSERT INTO a VALUES (1, 5, 'five'), (2, 6, 'six'), (3, NULL, 'n1'), (4, NULL, 'n2'), \
        (5, -9223372036854775808, 'min'), (6, 9223372036854775807, 'max'); \
    INSERT INTO ai SELECT * FROM a; \
    INSERT INTO b VALUES (1, 'Alpha', 1), (2, 'beta', 2); \
    INSERT INTO c VALUES (1, 'Gamma', 1), (2, 'delta', 2); \
    INSERT INTO d VALUES (1, 'x  ', 1), (2, 'y', 2); \
    INSERT INTO e VALUES (1, 1, 'a', 1), (2, 1, 'b', 2), (3, 1, NULL, 3), (4, 1, NULL, 4), \
        (5, 2, 'a', 5); \
    INSERT INTO f VALUES (1, 5, 1, 1), (2, 5, 0, 2), (3, 5, 0, 3), (4, 6, 1, 4); \
    INSERT INTO g VALUES (1, 'Hello', 1), (2, 'world', 2); \
    INSERT INTO h VALUES (1, '5', 1), (2, '05', 2), (3, 'x', 3);";

/// One point lookup: `{t}` is the table, spelled with and without
/// `NOT INDEXED`, and also `INDEXED BY index` when SQLite can use the index
/// for it.
struct Probe {
    table: &'static str,
    index: Option<&'static str>,
    sql: &'static str,
    /// False only where RedlineDB is known to compare differently from
    /// SQLite with or without an index; the index must still give the scan's
    /// answer.
    sqlite: bool,
}

const PROBES: &[Probe] = &[
    Probe {
        table: "a",
        index: Some("a_k"),
        sql: "SELECT id, v FROM {t} WHERE k = ?1",
        sqlite: true,
    },
    Probe {
        table: "a",
        index: None,
        sql: "SELECT id, v FROM {t} WHERE ?1 = k",
        sqlite: true,
    },
    Probe {
        table: "ai",
        index: None,
        sql: "SELECT id, v FROM {t} WHERE k = ?1",
        sqlite: true,
    },
    Probe {
        table: "b",
        index: Some("b_name"),
        sql: "SELECT id FROM {t} WHERE name = ?1",
        sqlite: true,
    },
    Probe {
        table: "c",
        index: None,
        sql: "SELECT id FROM {t} WHERE name = ?1",
        sqlite: true,
    },
    Probe {
        table: "c",
        index: Some("c_nocase"),
        sql: "SELECT id FROM {t} WHERE name = ?1 COLLATE NOCASE",
        sqlite: true,
    },
    // RedlineDB does not apply a column's declared RTRIM collation to
    // `s = ?` (docs/sqlite-parity.md, Collations: partial), scan or index.
    // The index must give the scan's answer; the explicit COLLATE RTRIM
    // spelling below must also give SQLite's.
    Probe {
        table: "d",
        index: Some("d_s"),
        sql: "SELECT id FROM {t} WHERE s = ?1",
        sqlite: false,
    },
    Probe {
        table: "d",
        index: Some("d_s"),
        sql: "SELECT id FROM {t} WHERE s = ?1 COLLATE RTRIM",
        sqlite: true,
    },
    Probe {
        table: "e",
        index: Some("e_xy"),
        sql: "SELECT id FROM {t} WHERE x = 1 AND y = ?1",
        sqlite: true,
    },
    Probe {
        table: "e",
        index: Some("e_xy"),
        sql: "SELECT id FROM {t} WHERE x = ?1 AND y = 'a'",
        sqlite: true,
    },
    Probe {
        table: "f",
        index: Some("f_k"),
        sql: "SELECT id FROM {t} WHERE flag = 1 AND k = ?1",
        sqlite: true,
    },
    Probe {
        table: "f",
        index: None,
        sql: "SELECT id FROM {t} WHERE k = ?1",
        sqlite: true,
    },
    Probe {
        table: "g",
        index: Some("g_lower"),
        sql: "SELECT id FROM {t} WHERE lower(name) = ?1",
        sqlite: true,
    },
    Probe {
        table: "h",
        index: Some("h_t"),
        sql: "SELECT id FROM {t} WHERE t = ?1",
        sqlite: true,
    },
];

/// Bound values of every storage class, NULL, the i64 extremes, numeric
/// text, and the spellings the collations fold.
fn params() -> Vec<SqlValue> {
    vec![
        int(5),
        int(6),
        int(7),
        int(1),
        int(2),
        int(i64::MIN),
        int(i64::MAX),
        real(5.0),
        real(5.5),
        real(9_223_372_036_854_775_807.0),
        text("5"),
        text("05"),
        text("5.0"),
        text(" 5"),
        text("1"),
        text("a"),
        text("b"),
        text("x"),
        text("x "),
        text("y   "),
        text("ALPHA"),
        text("beta"),
        text("Beta "),
        text("GAMMA"),
        text("Gamma"),
        text("delta"),
        text("hello"),
        text("WORLD"),
        text("world"),
        null(),
    ]
}

/// The probe's SQL with the parameter spelled as a literal.
fn literal(value: &SqlValue) -> String {
    match value {
        SqlValue::Null => "NULL".to_owned(),
        SqlValue::Integer(v) => v.to_string(),
        SqlValue::Real(v) => format!("{v:?}"),
        SqlValue::Text(v) => format!("'{}'", v.replace('\'', "''")),
        SqlValue::Blob(_) => unreachable!("no blob parameters"),
    }
}

fn run_probe(lab: &Lab, probe: &Probe, value: &SqlValue) {
    let mut accesses = vec![
        format!("{} NOT INDEXED", probe.table),
        probe.table.to_owned(),
    ];
    if let Some(index) = probe.index {
        accesses.push(format!("{} INDEXED BY {index}", probe.table));
    }
    let params = std::slice::from_ref(value);
    let mut scan_answer = None;
    for access in &accesses {
        let bound = probe.sql.replace("{t}", access);
        let inline = bound.replace("?1", &literal(value));
        if probe.sqlite {
            lab.assert_same_bound(&bound, params, false);
            lab.assert_same(&inline, false);
            continue;
        }
        for (sql, params) in [(&bound, params), (&inline, &[][..])] {
            let answer = sorted(lab.rows_redline(sql, params));
            match &scan_answer {
                None => scan_answer = Some(answer),
                Some(scan) => assert_eq!(&answer, scan, "`{sql}` {params:?} differs from the scan"),
            }
        }
    }
}

/// An outcome as sorted debug strings, for an unordered comparison.
fn sorted(outcome: Outcome) -> Vec<String> {
    match outcome {
        Outcome::Rows(rows) => {
            let mut rows: Vec<String> = rows.iter().map(|row| format!("{row:?}")).collect();
            rows.sort();
            rows
        }
        Outcome::Err(err) => vec![format!("error: {err}")],
    }
}

fn run_matrix(lab: &Lab) {
    let params = params();
    for probe in PROBES {
        for value in &params {
            run_probe(lab, probe, value);
        }
    }
    for sql in [
        "SELECT id FROM {t} WHERE k IS NULL",
        "SELECT id FROM {t} WHERE k = NULL",
    ] {
        for access in ["a", "a NOT INDEXED"] {
            lab.assert_same(&sql.replace("{t}", access), false);
        }
    }
    for access in ["e", "e NOT INDEXED"] {
        lab.assert_same(
            &format!("SELECT id FROM {access} WHERE x = 1 AND y IS NULL"),
            false,
        );
    }
}

#[test]
fn unique_point_matrix_matches_scan_and_sqlite() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    run_matrix(&lab);
}

#[test]
fn unique_point_after_delete_update_and_rollback() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    // Deleted and moved keys.
    lab.exec_both(
        "DELETE FROM a WHERE k = 6; UPDATE a SET k = 7 WHERE k = 5; \
         DELETE FROM b WHERE name = 'ALPHA'; UPDATE c SET name = 'GAMMA' WHERE id = 1; \
         UPDATE d SET s = 'x' WHERE id = 1; DELETE FROM e WHERE x = 1 AND y = 'a'; \
         UPDATE f SET flag = 0 WHERE id = 1; UPDATE f SET flag = 1 WHERE id = 2; \
         UPDATE g SET name = 'HELLO' WHERE id = 1; UPDATE h SET t = '5.0' WHERE id = 1;",
    );
    run_matrix(&lab);
    // Changes inside a transaction are read through the index, and a
    // rollback takes them back out.
    lab.exec_both("BEGIN");
    lab.exec_both(
        "INSERT INTO a VALUES (10, 5, 'back'); DELETE FROM a WHERE k = 7; \
         UPDATE f SET flag = 0 WHERE id = 2; UPDATE f SET flag = 1 WHERE id = 3; \
         INSERT INTO g VALUES (3, 'World!', 3);",
    );
    run_matrix(&lab);
    lab.exec_both("ROLLBACK");
    run_matrix(&lab);
}

fn step_ids(stmt: &mut redlinedb_sql::Statement) -> Vec<SqlValue> {
    let mut out = Vec::new();
    while stmt.step().expect("step") == Step::Row {
        out.push(stmt.column_value(0).expect("value").clone());
    }
    out
}

fn fresh_ids(conn: &Arc<Connection>, sql: &str, value: &SqlValue) -> Vec<SqlValue> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    bind(&mut stmt, value);
    step_ids(&mut stmt)
}

fn bind(stmt: &mut redlinedb_sql::Statement, value: &SqlValue) {
    match value {
        SqlValue::Integer(v) => stmt.bind_i64(1, *v).expect("bind"),
        SqlValue::Text(v) => stmt.bind_text(1, v.clone()).expect("bind"),
        other => panic!("unexpected parameter {other:?}"),
    }
}

#[test]
fn unique_point_statement_prepared_before_changes() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    let cases: &[(&str, SqlValue)] = &[
        ("SELECT id FROM a WHERE k = ?1", int(5)),
        ("SELECT id FROM f WHERE flag = 1 AND k = ?1", int(5)),
        ("SELECT id FROM g WHERE lower(name) = ?1", text("hello")),
        ("SELECT id FROM e WHERE x = 1 AND y = ?1", text("a")),
    ];
    let mut held: Vec<_> = cases
        .iter()
        .map(|(sql, value)| {
            let mut stmt = lab.redline.prepare(sql).expect("prepare");
            bind(&mut stmt, value);
            let first = step_ids(&mut stmt);
            assert_eq!(first, fresh_ids(&lab.redline, &scan(sql), value), "{sql}");
            stmt
        })
        .collect();
    lab.exec_both(
        "DELETE FROM a WHERE k = 5; INSERT INTO a VALUES (11, 5, 'new'); \
         UPDATE f SET flag = 0 WHERE id = 1; UPDATE f SET flag = 1 WHERE id = 3; \
         UPDATE g SET name = 'other' WHERE id = 1; INSERT INTO g VALUES (4, 'HeLLo', 4); \
         UPDATE e SET y = 'z' WHERE id = 1; UPDATE e SET y = 'a' WHERE id = 3;",
    );
    for ((sql, value), stmt) in cases.iter().zip(held.iter_mut()) {
        stmt.reset().expect("reset");
        bind(stmt, value);
        let again = step_ids(stmt);
        assert_eq!(again, fresh_ids(&lab.redline, &scan(sql), value), "{sql}");
        let Outcome::Rows(rows) = lab.rows_sqlite(sql, std::slice::from_ref(value)) else {
            panic!("sqlite failed `{sql}`");
        };
        let want: Vec<SqlValue> = rows.into_iter().map(|mut row| row.remove(0)).collect();
        assert_eq!(again, want, "{sql} after the changes");
    }
}

/// `sql` with its table forced onto a scan.
fn scan(sql: &str) -> String {
    let (head, tail) = sql.split_once(" WHERE ").expect("where");
    format!("{head} NOT INDEXED WHERE {tail}")
}

#[test]
fn unique_point_in_a_held_snapshot_matches_the_scan() {
    let lab = Lab::new();
    lab.exec_both(SETUP);
    let reader = &lab.redline;
    let writer = lab.database.connect();
    reader.execute("BEGIN").expect("begin");
    let probes: &[(&str, SqlValue)] = &[
        ("SELECT id FROM a WHERE k = ?1", int(5)),
        ("SELECT id FROM a WHERE k = ?1", int(8)),
        ("SELECT id FROM f WHERE flag = 1 AND k = ?1", int(5)),
        ("SELECT id FROM g WHERE lower(name) = ?1", text("hello")),
    ];
    let before: Vec<_> = probes
        .iter()
        .map(|(sql, value)| fresh_ids(reader, sql, value))
        .collect();
    for ((sql, value), rows) in probes.iter().zip(&before) {
        assert_eq!(rows, &fresh_ids(reader, &scan(sql), value), "{sql}");
    }
    writer
        .execute(
            "UPDATE a SET k = 8 WHERE k = 5; UPDATE f SET flag = 0 WHERE id = 1; \
             UPDATE f SET flag = 1 WHERE id = 2; UPDATE g SET name = 'bye' WHERE id = 1;",
        )
        .expect("writer");
    // Whatever the reader's snapshot shows, the index and the scan show the
    // same rows.
    for (sql, value) in probes {
        assert_eq!(
            fresh_ids(reader, sql, value),
            fresh_ids(reader, &scan(sql), value),
            "{sql} inside the reader's transaction"
        );
    }
    reader.execute("COMMIT").expect("commit");
    lab.sqlite
        .execute_batch(
            "UPDATE a SET k = 8 WHERE k = 5; UPDATE f SET flag = 0 WHERE id = 1; \
             UPDATE f SET flag = 1 WHERE id = 2; UPDATE g SET name = 'bye' WHERE id = 1;",
        )
        .expect("sqlite replay");
    run_matrix(&lab);
}
