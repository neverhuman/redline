//! Q5-05: TEXT operands are SQLite values unless an expression says Postgres.
//!
//! `||`, `-`, `+`, `*`, `/` and `%` used to read JSON-, date- and
//! decimal-shaped TEXT as jsonb, dates and exact decimals in the default
//! SQLite dialect, `'1' + 2` was TEXT, and `'abc' + 1` failed with
//! `datatype mismatch`. Standard `CAST(x AS DATE)` kept the text. Every
//! query here must match bundled SQLite value for value and storage class
//! for storage class.

use crate::lab::{Lab, int, null, real, text};

#[test]
fn q5_05_concat_json_shaped_text() {
    let lab = Lab::new();
    lab.assert_rows("SELECT '[1]'||'[2]'", &[vec![text("[1][2]")]]);
    lab.assert_rows(
        "SELECT '{\"a\":1}' || '{\"b\":2}', '[1,2]' || '[3,4]', '1' || '[2]'",
        &[vec![
            text("{\"a\":1}{\"b\":2}"),
            text("[1,2][3,4]"),
            text("1[2]"),
        ]],
    );
}

#[test]
fn q5_05_minus_date_shaped_text() {
    let lab = Lab::new();
    lab.assert_rows(
        "SELECT '2025-01-02'-'2025-01-01', '2025-03-01' - '2025-01-01', '[1,2]'-0, \
         '{\"a\":1,\"b\":2}' - 'a'",
        &[vec![int(0), int(0), int(0), int(0)]],
    );
}

#[test]
fn q5_05_numeric_text_arith_typed() {
    let lab = Lab::new();
    lab.assert_rows(
        "SELECT '1'+2, typeof('1'+2), '1abc'+1, '12abc'+1, '1e2'+1, 'abc'+1, ' 5 '+0",
        &[vec![
            int(3),
            text("integer"),
            int(2),
            int(13),
            real(101.0),
            int(1),
            int(5),
        ]],
    );
    lab.assert_rows(
        "SELECT '1.5'*2, '1.'+0, '.5'+0, '1e'+0, '1e5x'+0, '1.x'+0, '-0'+0, 'inf'+0, 'nan'+0",
        &[vec![
            real(3.0),
            real(1.0),
            real(0.5),
            int(1),
            real(100_000.0),
            real(1.0),
            int(0),
            int(0),
            int(0),
        ]],
    );
    lab.assert_rows(
        "SELECT x'3132'+0, '9223372036854775807'+0, '9223372036854775808'+0, '0x10'+0",
        &[vec![
            int(12),
            int(i64::MAX),
            real(9_223_372_036_854_775_808.0),
            int(0),
        ]],
    );
    // Overflow on the INTEGER reading goes REAL, as for INTEGER operands.
    lab.assert_same("SELECT '9223372036854775807' + 1", true);
    lab.assert_rows(
        "SELECT -'5', -'abc', -'1.5', -x'35', typeof(-'5')",
        &[vec![int(-5), int(0), real(-1.5), int(-5), text("integer")]],
    );
    lab.assert_same("SELECT -'-9223372036854775808'", true);
}

#[test]
fn q5_05_text_div_integer() {
    let lab = Lab::new();
    lab.assert_rows(
        "SELECT '7'/'2', typeof('7'/'2'), '7'/2, 7/'2.0', '7'/'0', 'abc'/1",
        &[vec![
            int(3),
            text("integer"),
            int(3),
            real(3.5),
            null(),
            int(0),
        ]],
    );
}

#[test]
fn q5_05_text_modulo() {
    let lab = Lab::new();
    lab.assert_rows(
        "SELECT '7'%'4', 'abc'%'abd', '7.5'%'2', 'abc'%3",
        &[vec![int(3), null(), real(1.0), int(0)]],
    );
}

#[test]
fn q5_05_stored_columns() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE t(a TEXT, b TEXT);\
         INSERT INTO t VALUES('[1,2]','[1]'),('2025-01-02','2025-01-01'),('4000','50'),\
         ('{\"a\":1}','a'),('7','4'),('1.5','2'),('abc','abd');",
    );
    lab.assert_same(
        "SELECT a-b, a+b, a*b, a/b, a%b, a||b, typeof(a+b) FROM t ORDER BY rowid",
        true,
    );
    lab.assert_same("SELECT sum(a+b), total(a-b) FROM t", true);
    lab.assert_same("SELECT a FROM t WHERE a+0 > 1 ORDER BY rowid", true);
}

#[test]
fn q5_05_bound_params() {
    let lab = Lab::new();
    lab.assert_same_bound("SELECT ?1 + ?2, ?1 - ?2", &[text("1"), text("2")], true);
    lab.assert_same_bound(
        "SELECT ?1 - ?2, ?1 || ?2",
        &[text("2025-01-02"), text("2025-01-01")],
        true,
    );
    lab.assert_same_bound("SELECT ?1 || ?2", &[text("[1]"), text("[2]")], true);
    lab.assert_same_bound("SELECT ?1 % ?2", &[text("7"), text("4")], true);
    lab.assert_same_bound("SELECT ?1 * 2", &[text("1.5")], true);
}

#[test]
fn q5_05_cast_uses_sqlite_affinity_rules() {
    let lab = Lab::new();
    lab.assert_rows(
        "SELECT CAST('2025-01-02' AS DATE), typeof(CAST('2025-01-02' AS DATE)), \
         CAST('2025-01-02 10:00:00' AS TIMESTAMP), CAST('t' AS BOOLEAN), CAST('1' AS BOOLEAN)",
        &[vec![int(2025), text("integer"), int(2025), int(0), int(1)]],
    );
    lab.assert_rows(
        "SELECT CAST('1.0' AS NUMERIC), CAST('1e5x' AS NUMERIC), CAST('inf' AS NUMERIC), \
         CAST('1.5' AS NUMERIC), CAST(x'3132' AS NUMERIC), CAST(3.0 AS NUMERIC)",
        &[vec![
            int(1),
            int(100_000),
            int(0),
            real(1.5),
            int(12),
            real(3.0),
        ]],
    );
    lab.assert_same(
        "SELECT CAST('12' AS JSON), CAST('abc' AS UUID), CAST('9.5' AS MONEY), CAST('2' AS DECIMAL)",
        true,
    );
    lab.assert_same("SELECT CAST('7.5' AS POINT), CAST('7' AS CHARINT)", true);
}

#[test]
fn q5_05_pg_casts_keep_pg_semantics() {
    // `::` is a RedlineDB extension that SQLite does not parse, so these
    // pin RedlineDB's own answers: a Postgres cast gives the operator its
    // Postgres meaning in either dialect.
    let lab = Lab::new();
    let pg = |sql: &str, want: crate::lab::Outcome| {
        let got = lab.rows_redline(sql, &[]);
        assert_eq!(format!("{got:?}"), format!("{want:?}"), "{sql}");
    };
    use crate::lab::Outcome::Rows;
    pg(
        "SELECT 0.1::numeric + 0.2::numeric",
        Rows(vec![vec![text("0.3")]]),
    );
    pg("SELECT 1.5::numeric * 3", Rows(vec![vec![text("4.5")]]));
    pg(
        "SELECT '[1]'::jsonb || '[2]', '{\"a\":1,\"b\":2}'::jsonb - 'a'",
        Rows(vec![vec![text("[1, 2]"), text("{\"b\": 2}")]]),
    );
    pg(
        "SELECT '2025-01-02'::timestamp - '2025-01-01'::timestamp",
        Rows(vec![vec![text("1 day")]]),
    );
    // One cast is not enough for date subtraction: the other side is plain
    // TEXT, so this is SQLite arithmetic.
    pg(
        "SELECT '2025-01-02'::date - '2025-01-01'",
        Rows(vec![vec![int(0)]]),
    );
}
