//! Q5-01: compatibility rewrites never touch literals, identifiers or comments.
//!
//! Before the parser sees a statement, RedlineDB lowers some SQLite and
//! Postgres surface syntax by rewriting the SQL text: CTE `AS [NOT]
//! MATERIALIZED` hints, `NULL IS NOT 1`, a named `WINDOW win`, `OVERRIDING
//! SYSTEM VALUE`, `GROUP BY ROLLUP/CUBE`, `SELECT ... INTO`, `INTERVAL`,
//! `AT TIME ZONE` and `DROP IDENTITY`. Those passes searched the raw text, so
//! the same words inside a string literal, a quoted identifier or a comment
//! were rewritten as well: `SELECT 'AS MATERIALIZED'` answered `AS`, an
//! INSERT stored the corrupted value, `SELECT 1 AS materialized` failed to
//! parse, and non-ASCII text next to `interval ` was re-encoded byte by byte.
//! Every statement here must match bundled SQLite, value for value, storage
//! class for storage class and column name for column name.

use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, SqlValue, Step};

use crate::lab::{Lab, int, text};

/// Every phrase that switches on a pre-parse rewrite, spelled as it would
/// appear in code. Inside a literal, identifier or comment none of them may
/// change the statement.
const REWRITE_TRIGGER_PHRASES: &[&str] = &[
    "AS MATERIALIZED",
    "AS NOT MATERIALIZED",
    "as materialized x",
    "x AS MATERIALIZED (SELECT 1)",
    "y as not materialized (select 2)",
    "NULL IS NOT 1",
    "a window win as (b)",
    "sum(x) OVER win",
    " window win as (order by x) ",
    "overriding system value",
    "overriding user value",
    "insert into t overriding system value values (1)",
    "q group by rollup (a)",
    "... group by cube (c)",
    " group by grouping sets ((a),(b)) ",
    "select a into b from c",
    " into ",
    "interval '1 day'",
    "interval 5",
    "now() at time zone 'UTC'",
    "at time zone",
    "alter column c drop identity",
    "drop identity if exists",
    "create sequence s start with 1 increment by 1",
    "generated always as identity (start with 1 increment by 2)",
    "over (order by k exclude current row)",
    "exclude ties",
    "insert into t select 1 on conflict (a) do nothing",
    "on conflict do nothing",
    "a glob 'b*'",
    "a ? 'k'",
    "create index i on t using btree (a)",
    "using ",
    ") strict, without rowid",
    "without rowid, strict",
    "array[1,2]",
    "array_length(a, 1)",
    "array_agg(x)",
    "a && b",
    "(a)[1]",
    "x + '1 day'",
    "d - '-2 months'",
    "join lateral (select 1) s on true",
    ",lateral ",
    "x::regclass",
    "alter table t add column if not exists c",
    "t indexed by i",
    "t not indexed",
    "select * from pg_catalog.pg_namespace",
    "from pg_class where relname = 't'",
    "from pg_proc",
    "current_schema()",
    "set search_path to s",
    "to_tsvector('a') @@ to_tsquery('a')",
    "select * from public.t",
    "information_schema.tables",
];

/// Decorations around each phrase: as written, upper case, and surrounded by
/// multi-byte UTF-8 so a byte-wise rewrite cannot hide.
fn spellings(phrase: &str) -> Vec<String> {
    vec![
        phrase.to_owned(),
        phrase.to_ascii_uppercase(),
        format!("é {phrase} ü€"),
    ]
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn sqlite_columns(lab: &Lab, sql: &str) -> Vec<String> {
    let stmt = lab
        .sqlite
        .prepare(sql)
        .unwrap_or_else(|err| panic!("sqlite rejected `{sql}`: {err}"));
    stmt.column_names().into_iter().map(str::to_owned).collect()
}

fn redline_columns(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("redline rejected `{sql}`: {err}"));
    (0..stmt.column_count())
        .map(|index| stmt.column_name(index).to_owned())
        .collect()
}

/// Same rows (in order) and the same result column names as SQLite.
fn assert_same_with_columns(lab: &Lab, sql: &str) {
    lab.assert_same(sql, true);
    assert_eq!(
        redline_columns(&lab.redline, sql),
        sqlite_columns(lab, sql),
        "column names differ for `{sql}`"
    );
}

fn redline_rows(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("redline rejected `{sql}`: {err}"));
    let mut rows = Vec::new();
    while let Step::Row = stmt
        .step()
        .unwrap_or_else(|err| panic!("redline failed `{sql}`: {err}"))
    {
        rows.push(
            (0..stmt.column_count())
                .map(|index| stmt.column_value(index).expect("column").clone())
                .collect(),
        );
    }
    rows
}

#[test]
fn q5_01_materialized_words_in_literals_keep_their_text() {
    let lab = Lab::new();
    let sql = "SELECT 'AS MATERIALIZED','AS NOT MATERIALIZED','as materialized x'";
    lab.assert_rows(
        sql,
        &[vec![
            text("AS MATERIALIZED"),
            text("AS NOT MATERIALIZED"),
            text("as materialized x"),
        ]],
    );
    assert_same_with_columns(&lab, sql);
    assert_same_with_columns(
        &lab,
        "SELECT 'it''s As MaTeRiAlIzEd (', 'é AS NOT MATERIALIZED ü', \
         'AS  MATERIALIZED', 'AS\tNOT\nMATERIALIZED ('",
    );
}

#[test]
fn q5_01_insert_round_trip_survives_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("q5_01.db");
    let want = vec![
        vec![text("AS MATERIALIZED"), int(15)],
        vec![text("x AS NOT MATERIALIZED (y)"), int(25)],
    ];
    {
        let lab = Lab::open(&path);
        lab.exec_both("CREATE TABLE t(id INTEGER PRIMARY KEY, a TEXT)");
        lab.step_both("INSERT INTO t(a) VALUES('AS MATERIALIZED')");
        lab.step_both("INSERT INTO t(a) VALUES('x AS NOT MATERIALIZED (y)')");
        lab.assert_rows("SELECT a, length(a) FROM t ORDER BY id", &want);
    }
    let db = Database::open(&path, DbOptions::default()).expect("reopen");
    let conn = db.connect();
    assert_eq!(
        redline_rows(&conn, "SELECT a, length(a) FROM t ORDER BY id"),
        want,
        "values read back after reopen"
    );
}

#[test]
fn q5_01_alias_named_materialized_is_an_alias() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(c INTEGER)");
    lab.exec_both("INSERT INTO t VALUES (3), (1), (2)");
    assert_same_with_columns(&lab, "SELECT 1 AS materialized");
    assert_same_with_columns(&lab, "SELECT c AS materialized FROM t ORDER BY c");
    assert_same_with_columns(&lab, "SELECT c AS not_materialized FROM t ORDER BY c");
    assert_same_with_columns(
        &lab,
        "WITH x AS (SELECT c AS materialized FROM t) SELECT materialized FROM x",
    );
}

#[test]
fn q5_01_column_name_prefix_is_not_rewritten() {
    let lab = Lab::new();
    assert_same_with_columns(&lab, "SELECT 'a' AS materialized_col");
    assert_same_with_columns(&lab, "SELECT 'a' AS Materialized_x, 2 AS materializedy");
}

#[test]
fn q5_01_quoted_identifiers_keep_hint_words() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE \"AS MATERIALIZED\"(\"AS NOT MATERIALIZED\" TEXT)");
    lab.exec_both("INSERT INTO \"AS MATERIALIZED\" VALUES ('v')");
    assert_same_with_columns(
        &lab,
        "SELECT \"AS NOT MATERIALIZED\" FROM \"AS MATERIALIZED\"",
    );
    assert_same_with_columns(&lab, "SELECT 1 AS \"AS MATERIALIZED\"");
    assert_same_with_columns(
        &lab,
        "SELECT 1 AS [AS MATERIALIZED], 2 AS `as not materialized`",
    );
}

#[test]
fn q5_01_hint_after_comment_is_still_stripped() {
    let lab = Lab::new();
    lab.assert_rows(
        "WITH x AS /* hint */ MATERIALIZED (SELECT 1 AS v) SELECT v FROM x",
        &[vec![int(1)]],
    );
    lab.assert_rows(
        "WITH x AS -- hint\n NOT MATERIALIZED (SELECT 2 AS v) SELECT v FROM x",
        &[vec![int(2)]],
    );
    lab.assert_rows(
        "WITH x as materialized(SELECT 3 AS v), y AS NOT MATERIALIZED (SELECT v + 1 AS v FROM x) \
         SELECT v FROM y",
        &[vec![int(4)]],
    );
}

#[test]
fn q5_01_hint_and_literal_in_one_statement() {
    let lab = Lab::new();
    let sql = "WITH x AS MATERIALIZED (SELECT 'AS MATERIALIZED' AS v, 'AS NOT MATERIALIZED (' AS w) \
               SELECT v, w FROM x";
    lab.assert_rows(
        sql,
        &[vec![text("AS MATERIALIZED"), text("AS NOT MATERIALIZED (")]],
    );
    assert_same_with_columns(&lab, sql);
    lab.assert_rows(
        "SELECT 1 /* AS MATERIALIZED */ AS materialized -- AS NOT MATERIALIZED (\n",
        &[vec![int(1)]],
    );
}

#[test]
fn q5_01_bound_and_literal_agree() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(a TEXT)");
    lab.step_both("INSERT INTO t VALUES('AS MATERIALIZED')");
    let bound = [text("AS MATERIALIZED")];
    lab.assert_same_bound("SELECT count(*) FROM t WHERE a = ?1", &bound, true);
    lab.assert_rows(
        "SELECT count(*) FROM t WHERE a = 'AS MATERIALIZED'",
        &[vec![int(1)]],
    );

    // A named parameter next to the literal takes the same path.
    let mut stmt = lab
        .redline
        .prepare("SELECT :p = 'AS NOT MATERIALIZED', :p")
        .expect("prepare named");
    stmt.bind_named(":p", text("AS NOT MATERIALIZED"))
        .expect("bind named");
    assert!(matches!(stmt.step().expect("step"), Step::Row));
    assert_eq!(stmt.column_value(0).expect("col 0"), &int(1));
    assert_eq!(
        stmt.column_value(1).expect("col 1"),
        &text("AS NOT MATERIALIZED")
    );
}

#[test]
fn q5_01_other_rewrites_leave_literals_alone() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(x INTEGER)");
    lab.exec_both("INSERT INTO t VALUES (1), (2), (3)");
    for (sql, want) in [
        ("SELECT 'NULL IS NOT 1'", "NULL IS NOT 1"),
        ("SELECT 'a window win as (b)'", "a window win as (b)"),
        (
            "SELECT 'overriding system value'",
            "overriding system value",
        ),
        ("SELECT 'q group by rollup (a)'", "q group by rollup (a)"),
        ("SELECT 'q group by cube (c)'", "q group by cube (c)"),
        (
            "SELECT 'alter column c drop identity'",
            "alter column c drop identity",
        ),
    ] {
        lab.assert_rows(sql, &[vec![text(want)]]);
    }
    // The rewrite still applies to the code, and only to the code.
    lab.assert_rows(
        "SELECT NULL IS NOT 1, 'NULL IS NOT 1'",
        &[vec![int(1), text("NULL IS NOT 1")]],
    );
    assert_same_with_columns(
        &lab,
        "SELECT x, 'a window win as (b)', 'OVER win', sum(x) OVER win AS s FROM t \
         WINDOW win AS (ORDER BY x) ORDER BY x",
    );
}

#[test]
fn q5_01_rewrite_trigger_phrases_round_trip_sqlite() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(a TEXT)");
    for phrase in REWRITE_TRIGGER_PHRASES {
        for value in spellings(phrase) {
            let literal = quote(&value);
            lab.assert_rows(&format!("SELECT {literal}"), &[vec![text(&value)]]);
            lab.step_both("DELETE FROM t");
            lab.step_both(&format!("INSERT INTO t VALUES({literal})"));
            lab.assert_rows(
                "SELECT a, length(a) FROM t",
                &[vec![text(&value), int(value.chars().count() as i64)]],
            );
            assert_same_with_columns(&lab, &format!("SELECT 7 AS {}", quote_ident(&value)));
            lab.assert_rows(&format!("SELECT 7 -- {value}\n"), &[vec![int(7)]]);
            lab.assert_rows(&format!("SELECT 7 /* {value} */"), &[vec![int(7)]]);
        }
    }
}

fn postgres_conn(dir: &tempfile::TempDir) -> Arc<Connection> {
    let options = DbOptions {
        dialect: Some(Dialect::PostgresSubset),
        ..DbOptions::default()
    };
    Database::create(dir.path().join("pg.db"), options)
        .expect("create pg db")
        .connect()
}

#[test]
fn q5_01_rewrite_trigger_phrases_round_trip_postgres() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conn = postgres_conn(&dir);
    conn.execute("CREATE TABLE t(a TEXT)").expect("create");
    for phrase in REWRITE_TRIGGER_PHRASES {
        for value in spellings(phrase) {
            let literal = quote(&value);
            assert_eq!(
                redline_rows(&conn, &format!("SELECT {literal}")),
                vec![vec![text(&value)]],
                "postgres dialect literal `{literal}`"
            );
            conn.execute("DELETE FROM t").expect("delete");
            conn.execute(&format!("INSERT INTO t VALUES({literal})"))
                .unwrap_or_else(|err| panic!("insert {literal}: {err}"));
            assert_eq!(
                redline_rows(&conn, "SELECT a FROM t"),
                vec![vec![text(&value)]],
                "postgres dialect round trip of `{literal}`"
            );
            assert_eq!(
                redline_rows(&conn, &format!("SELECT 7 /* {value} */")),
                vec![vec![int(7)]],
                "postgres dialect comment around `{value}`"
            );
        }
    }
}

/// Byte-wise passes pushed each byte as its own `char`, so a statement that
/// woke one of them re-encoded every non-ASCII character: `'-'` woke the
/// date-arithmetic pass and INSERT stored `'cafÃ©'`, and the GLOB pass ran
/// on every statement and renamed `tëst` to `tÃ«st`.
#[test]
fn q5_01_non_ascii_text_survives_rewrites() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(a TEXT, b TEXT)");
    lab.step_both("INSERT INTO t VALUES('café', '-')");
    lab.step_both("INSERT INTO t VALUES('ü', '+1 day')");
    lab.assert_rows(
        "SELECT a, b, length(a) FROM t ORDER BY a",
        &[
            vec![text("café"), text("-"), int(4)],
            vec![text("ü"), text("+1 day"), int(1)],
        ],
    );
    lab.assert_rows(
        "SELECT 'x' || '-' || 'ü', 'é' || '+'",
        &[vec![text("x-ü"), text("é+")]],
    );
    assert_same_with_columns(&lab, "SELECT 'é' AS \"ö\", 'ä' GLOB 'ä*' AS g, 'x' AS ü");

    lab.exec_both("CREATE TABLE tëst(ä INTEGER)");
    lab.step_both("INSERT INTO tëst VALUES (1)");
    assert_same_with_columns(&lab, "SELECT * FROM tëst");
    lab.assert_rows(
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
        &[vec![text("t")], vec![text("tëst")]],
    );
}

/// A multi-byte character next to a pass's trigger word made the pass slice
/// the SQL inside that character. Under `panic = "abort"` (the release
/// profile) that aborted the process.
#[test]
fn q5_01_non_ascii_next_to_trigger_words_does_not_panic() {
    let lab = Lab::new();
    for sql in [
        "SELECT 'é interval '",
        "SELECT 'é at time zone '",
        "SELECT 'é array_length(', 'é array_agg('",
        "SELECT 'é GROUPING('",
    ] {
        assert_same_with_columns(&lab, sql);
    }
}

/// In SQLite `'\x41'` is the four-character text `\x41`; only the Postgres
/// dialect reads it as a hex bytea literal.
#[test]
fn q5_01_backslash_x_literal_is_text_in_sqlite() {
    let lab = Lab::new();
    lab.assert_rows(
        "SELECT '\\x41', typeof('\\x41'), length('\\x41')",
        &[vec![text("\\x41"), text("text"), int(4)]],
    );
}

#[test]
fn q5_01_vacuum_into_keeps_a_non_ascii_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let lab = Lab::open(&dir.path().join("src.db"));
    lab.exec_both("CREATE TABLE t(a)");
    let target = dir.path().join("cöpy é.db");
    let sql = format!(
        "VACUUM INTO {}",
        quote(target.to_str().expect("utf-8 path"))
    );
    lab.redline.execute(&sql).expect("vacuum into");
    assert!(
        target.exists(),
        "VACUUM INTO wrote somewhere other than {target:?}"
    );
}
