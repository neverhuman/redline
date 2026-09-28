//! Q5-01 follow-up: every compatibility rewrite leaves bracket- and
//! backtick-quoted names, comments and Postgres string forms alone.
//!
//! Several byte-wise passes -- GLOB, ON CONFLICT, jsonb `?`, STRICT /
//! WITHOUT ROWID, date arithmetic, ARRAY[..], postfix subscripts, the
//! schema-prefix strip and the statement splitter -- kept their own quote
//! tracker, which knew `'` and `"` but not `[..]`, backticks, comments,
//! `E'..'` or `$tag$..$tag$`. So `CREATE TABLE \`a GLOB b\`(x)` created
//! ` glob(b,a)`, an apostrophe in a comment (`/* it's */`) let a later
//! literal's text be rewritten as code, and `SELECT $$array[1,2]$$` answered
//! `json_array(1,2)`. The property tests of `q5_01_literals.rs` used only
//! `'..'`, `".."` and comments without a literal after them.

use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, Dialect, SqlValue, Step};

use crate::lab::{Lab, int, text};
use crate::q5_01_literals::{REWRITE_TRIGGER_PHRASES, quote, spellings};

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

fn same_columns(lab: &Lab, sql: &str) {
    lab.assert_same(sql, true);
    assert_eq!(
        redline_columns(&lab.redline, sql),
        sqlite_columns(lab, sql),
        "column names differ for `{sql}`"
    );
}

#[test]
fn q5_01_bracket_and_backtick_names_keep_every_phrase() {
    let lab = Lab::new();
    for phrase in REWRITE_TRIGGER_PHRASES {
        for value in spellings(phrase) {
            if !value.contains(']') {
                same_columns(&lab, &format!("SELECT 7 AS [{value}]"));
            }
            if !value.contains('`') {
                same_columns(&lab, &format!("SELECT 7 AS `{value}`"));
            }
        }
    }
}

#[test]
fn q5_01_quoted_table_names_are_stored_as_written() {
    let lab = Lab::new();
    for phrase in REWRITE_TRIGGER_PHRASES {
        let value = phrase.to_string();
        for (open, close) in [("[", "]"), ("`", "`"), ("\"", "\"")] {
            if value.contains(close) || value.contains(open) {
                continue;
            }
            let name = format!("{open}{value}{close}");
            lab.step_both(&format!("CREATE TABLE {name}(x)"));
            lab.assert_same(
                &format!(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name = {}",
                    quote(&value)
                ),
                true,
            );
            lab.step_both(&format!("DROP TABLE {name}"));
        }
    }
    lab.assert_same("SELECT count(*) FROM sqlite_master", true);
}

#[test]
fn q5_01_a_quote_in_a_comment_leaves_later_literals_alone() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE t(a TEXT)");
    for phrase in REWRITE_TRIGGER_PHRASES {
        for value in spellings(phrase) {
            let literal = quote(&value);
            let semi = quote(&format!("; {value}"));
            for sql in [
                format!("SELECT 7 /* it's */, {literal}"),
                format!("SELECT 7 -- it's\n, {literal}"),
                format!("SELECT 7 /* it's */, {semi}"),
            ] {
                lab.assert_same(&sql, true);
            }
            for insert in [
                format!("INSERT INTO t /* it's */ VALUES ({literal})"),
                format!("INSERT INTO t /* it's */ VALUES ({semi})"),
                format!("INSERT INTO t -- it's\n VALUES ({literal})"),
            ] {
                lab.step_both("DELETE FROM t");
                lab.step_both(&insert);
                lab.assert_same("SELECT a, length(a) FROM t", true);
            }
        }
    }
}

/// The witnesses from the review, each against SQLite.
#[test]
fn q5_01_review_witnesses() {
    let lab = Lab::new();
    for sql in [
        "CREATE TABLE `a GLOB b`(x)",
        "CREATE TABLE t([a GLOB 'b*'] INT)",
        "CREATE TABLE u(a TEXT)",
        "CREATE TABLE d(n TEXT)",
    ] {
        lab.step_both(sql);
    }
    lab.assert_same("SELECT name FROM sqlite_master ORDER BY name", true);
    same_columns(&lab, "SELECT name FROM pragma_table_info('t')");
    for sql in [
        "SELECT 7 AS [a glob 'b*']",
        "SELECT 7 AS [a ? 'k']",
        "SELECT 7 AS [without rowid, strict]",
        "SELECT 7 AS [d - '-2 months']",
        "SELECT 7 AS [insert into t select 1 on conflict (a) do nothing]",
        "SELECT 7 AS [select * from public.t]",
        "SELECT 2 AS `a glob 'b'`",
        "SELECT 1 AS [x - '-5 days']",
    ] {
        same_columns(&lab, sql);
    }
    // (SQLite names the first column `7 /* it's */`, its source text; the
    // rows are what this checks.)
    lab.assert_same("SELECT 7 /* it's */, 'x array[1,2] y', '(a)[1]'", true);
    lab.step_both("INSERT INTO u VALUES(1 -- it's\n|| 'array[2]')");
    lab.step_both("INSERT INTO d VALUES(/* it's */ 'ask public.relations')");
    lab.step_both("INSERT INTO u /* it's */ VALUES ('; SELECT a INTO b')");
    lab.assert_same("SELECT a FROM u ORDER BY rowid", true);
    lab.assert_same("SELECT n FROM d", true);
    lab.assert_same("SELECT name FROM sqlite_master ORDER BY name", true);
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

/// End to end the Postgres dialect accepts `E'..'` only with a backslash
/// escape and has no `$$..$$` string outside function bodies, so those forms
/// are checked as prefixes whose quotes the old trackers misread; the
/// rewrite-level check of every form is `parser::rewrite::quoted_form_tests`.
#[test]
fn q5_01_postgres_string_forms_keep_every_phrase() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let conn = postgres_conn(&dir);
    for phrase in REWRITE_TRIGGER_PHRASES {
        for value in spellings(phrase) {
            let literal = quote(&value);
            for prefix in ["E'\\''", "/* it's */ 'x'", "1 AS \"it's\""] {
                let sql = format!("SELECT {prefix}, {literal}");
                let rows = redline_rows(&conn, &sql);
                assert_eq!(rows.len(), 1, "{sql}");
                assert_eq!(rows[0][1], text(&value), "{sql}");
            }
            let sql = format!("SELECT 7 -- it's\n, {literal}");
            assert_eq!(
                redline_rows(&conn, &sql),
                vec![vec![int(7), text(&value)]],
                "{sql}"
            );
        }
    }
    assert_eq!(
        redline_rows(&conn, "SELECT /* it's */ 'select * from public.t'"),
        vec![vec![text("select * from public.t")]]
    );
}

#[test]
fn q5_01_attach_keeps_a_non_ascii_path() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let lab = Lab::open(&dir.path().join("main.db"));
    let target = dir.path().join("cöpy é.db");
    let path = target.to_str().expect("utf-8 path");
    lab.redline
        .execute(&format!("ATTACH {} AS x", quote(path)))
        .expect("attach");
    lab.redline
        .execute("CREATE TABLE x.t(a); INSERT INTO x.t VALUES (1);")
        .expect("write attached");
    assert!(
        target.exists(),
        "ATTACH wrote somewhere other than {target:?}"
    );
    let files: Vec<Vec<SqlValue>> = redline_rows(
        &lab.redline,
        "SELECT file FROM pragma_database_list WHERE name = 'x'",
    );
    assert_eq!(files, vec![vec![text(path)]]);
}

/// A non-ASCII character in a function body sliced a string inside the
/// character and aborted the process (release builds use panic = abort).
/// (RedlineDB does not accept `LANGUAGE sql` after the body; that form now
/// fails cleanly instead of aborting.)
#[test]
fn q5_01_function_body_with_non_ascii_text() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let conn = postgres_conn(&dir);
    let refused = conn.execute(
        "CREATE FUNCTION g(x text) RETURNS text AS $$ SELECT 'café ' || x $$ LANGUAGE sql",
    );
    assert!(refused.is_err());
    for (create, call) in [
        (
            "CREATE FUNCTION f(x text) RETURNS text LANGUAGE sql AS $$ SELECT 'café ' || x $$",
            "SELECT f('ü')",
        ),
        (
            "CREATE FUNCTION p(x text) RETURNS text AS $$ BEGIN RETURN 'café ' || x; END $$ \
             LANGUAGE plpgsql",
            "SELECT p('ü')",
        ),
    ] {
        conn.execute(create)
            .unwrap_or_else(|err| panic!("{create}: {err}"));
        assert_eq!(
            redline_rows(&conn, call),
            vec![vec![text("café ü")]],
            "{call}"
        );
    }
}
