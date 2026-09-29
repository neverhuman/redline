//! Corpus plpgsql bodies: assignment, branches, loops, set-returning calls,
//! procedures, DO, and an AFTER INSERT trigger.

use redlinedb_sql::{Connection, Database, DbOptions};
use std::sync::Arc;
use tempfile::tempdir;

#[path = "parity_pg_pl/statements.rs"]
mod statements;

use statements::{error_of, exec, rows, script};

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db = Database::create(&dir.path().join("pl.db"), DbOptions::default()).expect("db");
    (dir, db.connect())
}

#[test]
fn plpgsql_corpus_shapes() {
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();

    script(
        &conn,
        "CREATE FUNCTION bsp_double_plpgsql(n int) RETURNS int LANGUAGE plpgsql AS $$
DECLARE r int;
BEGIN r := n * 2; RETURN r; END;
$$",
    );
    assert_eq!(rows(&conn, "SELECT bsp_double_plpgsql(21)"), "42");

    script(
        &conn,
        "CREATE PROCEDURE bsp_proc_inc(INOUT n int) LANGUAGE plpgsql AS $$
BEGIN n := n + 1; END;
$$",
    );
    assert_eq!(rows(&conn, "CALL bsp_proc_inc(41)"), "42");

    script(
        &conn,
        "CREATE FUNCTION bsp_sign(n int) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
  IF n > 0 THEN RETURN 'pos';
  ELSIF n < 0 THEN RETURN 'neg';
  ELSE RETURN 'zero';
  END IF;
END;
$$",
    );
    assert_eq!(
        rows(&conn, "SELECT bsp_sign(5), bsp_sign(-3), bsp_sign(0)"),
        "pos|neg|zero"
    );

    script(
        &conn,
        "CREATE FUNCTION bsp_loop_count(target int) RETURNS int LANGUAGE plpgsql AS $$
DECLARE i int := 0;
BEGIN
  LOOP EXIT WHEN i >= target; i := i + 1; END LOOP;
  RETURN i;
END;
$$",
    );
    assert_eq!(rows(&conn, "SELECT bsp_loop_count(7)"), "7");

    script(
        &conn,
        "CREATE FUNCTION bsp_for_range_sum(n int) RETURNS int LANGUAGE plpgsql AS $$
DECLARE s int := 0;
BEGIN
  FOR i IN 1..n LOOP s := s + i; END LOOP;
  RETURN s;
END;
$$",
    );
    assert_eq!(rows(&conn, "SELECT bsp_for_range_sum(10)"), "55");

    script(
        &conn,
        "CREATE FUNCTION bsp_for_query_concat() RETURNS text LANGUAGE plpgsql AS $$
DECLARE acc text := ''; r record;
BEGIN
  FOR r IN SELECT x FROM (VALUES ('a'),('b'),('c')) AS t(x) ORDER BY x LOOP
    acc := acc || r.x;
  END LOOP;
  RETURN acc;
END;
$$",
    );
    assert_eq!(rows(&conn, "SELECT bsp_for_query_concat()"), "abc");

    exec(
        &conn,
        "DO $$ BEGIN RAISE NOTICE 'hello from plpgsql'; END $$",
    );
    assert_eq!(rows(&conn, "SELECT 'after-notice'::text"), "after-notice");

    let err = conn
        .prepare("DO $$ BEGIN RAISE EXCEPTION 'boom'; END $$")
        .err()
        .map(|err| err.to_string())
        .or_else(|| {
            let mut stmt = conn
                .prepare("DO $$ BEGIN RAISE EXCEPTION 'boom'; END $$")
                .expect("prepare do");
            match stmt.step() {
                Err(err) => Some(err.to_string()),
                Ok(_) => None,
            }
        });
    let err = err.expect("raise must fail");
    assert!(err.to_ascii_lowercase().contains("boom"), "{err}");

    script(
        &conn,
        "CREATE FUNCTION bsp_safe_div(a int, b int) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
  RETURN (a / b)::text;
EXCEPTION WHEN division_by_zero THEN
  RETURN 'caught';
END;
$$",
    );
    assert_eq!(
        rows(&conn, "SELECT bsp_safe_div(10, 2), bsp_safe_div(10, 0)"),
        "5|caught"
    );

    script(
        &conn,
        "CREATE FUNCTION bsp_return_next(n int) RETURNS SETOF int LANGUAGE plpgsql AS $$
BEGIN
  FOR i IN 1..n LOOP RETURN NEXT i * 10; END LOOP;
  RETURN;
END;
$$",
    );
    assert_eq!(
        rows(&conn, "SELECT * FROM bsp_return_next(3) ORDER BY 1"),
        "10\n20\n30"
    );

    script(
        &conn,
        "CREATE FUNCTION bsp_return_query() RETURNS SETOF int LANGUAGE plpgsql AS $$
BEGIN RETURN QUERY SELECT generate_series(1, 4); END;
$$",
    );
    assert_eq!(
        rows(&conn, "SELECT * FROM bsp_return_query() ORDER BY 1"),
        "1\n2\n3\n4"
    );

    script(
        &conn,
        "CREATE PROCEDURE bsp_proc_out(IN a int, OUT b int) LANGUAGE plpgsql AS $$
BEGIN b := a * 100; END;
$$",
    );
    assert_eq!(rows(&conn, "CALL bsp_proc_out(7, NULL)"), "700");

    script(
        &conn,
        "CREATE FUNCTION bsp_fact(n int) RETURNS bigint LANGUAGE plpgsql AS $$
BEGIN
  IF n <= 1 THEN RETURN 1; END IF;
  RETURN n::bigint * bsp_fact(n - 1);
END;
$$",
    );
    assert_eq!(
        rows(&conn, "SELECT bsp_fact(0), bsp_fact(1), bsp_fact(6)"),
        "1|1|720"
    );

    script(
        &conn,
        "CREATE FUNCTION bsp_sumv(VARIADIC vs int[]) RETURNS int LANGUAGE plpgsql AS $$
DECLARE s int := 0;
BEGIN
  FOR i IN 1..coalesce(array_length(vs, 1), 0) LOOP s := s + vs[i]; END LOOP;
  RETURN s;
END;
$$",
    );
    assert_eq!(rows(&conn, "SELECT bsp_sumv(1,2,3,4,5)"), "15");

    script(
        &conn,
        "CREATE FUNCTION bsp_strict_inc(n int) RETURNS int LANGUAGE plpgsql STRICT AS $$
BEGIN RETURN n + 1; END;
$$",
    );
    assert_eq!(
        rows(&conn, "SELECT bsp_strict_inc(5), bsp_strict_inc(NULL)"),
        "6|NULL"
    );

    script(
        &conn,
        "CREATE FUNCTION bsp_setof_pair(n int) RETURNS SETOF record LANGUAGE plpgsql AS $$
BEGIN
  FOR i IN 1..n LOOP RETURN QUERY SELECT i, (i*i)::int; END LOOP;
  RETURN;
END;
$$",
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT * FROM bsp_setof_pair(3) AS t(a int, b int) ORDER BY a"
        ),
        "1|1\n2|4\n3|9"
    );

    // PG-01: nothing delivers a notification, so `PERFORM pg_notify` fails
    // the function, and the trigger that runs it, instead of doing nothing.
    script(
        &conn,
        "CREATE FUNCTION beyond_notif_fn() RETURNS int AS $$ BEGIN PERFORM pg_notify('beyond_no_listener', 'from-fn'); RETURN 7; END; $$ LANGUAGE plpgsql",
    );
    let err = error_of(&conn, "SELECT beyond_notif_fn()");
    assert!(err.contains("unsupported capability: pg_notify"), "{err}");

    exec(&conn, "CREATE TABLE beyond_trig_t(id int)");
    exec(
        &conn,
        "CREATE FUNCTION beyond_trig_fn() RETURNS trigger AS $$ BEGIN PERFORM pg_notify('beyond_no_listener', 'trig'); RETURN NEW; END; $$ LANGUAGE plpgsql",
    );
    exec(
        &conn,
        "CREATE TRIGGER beyond_trig_trg AFTER INSERT ON beyond_trig_t FOR EACH ROW EXECUTE FUNCTION beyond_trig_fn()",
    );
    let err = error_of(&conn, "INSERT INTO beyond_trig_t VALUES (1)");
    assert!(err.contains("unsupported capability: pg_notify"), "{err}");
    assert_eq!(rows(&conn, "SELECT count(*) FROM beyond_trig_t"), "0");
}

#[test]
fn plpgsql_reverse_execute_and_others() {
    unsafe { std::env::set_var("REDLINEDB_RESULT_DIALECT", "postgres") };
    let (_dir, conn) = open();
    script(
        &conn,
        "CREATE FUNCTION rev_sum() RETURNS int LANGUAGE plpgsql AS $$
DECLARE i int; s int := 0;
BEGIN
  FOR i IN REVERSE 3..1 LOOP
    s := s + i;
  END LOOP;
  RETURN s;
END $$",
    );
    assert_eq!(rows(&conn, "SELECT rev_sum()"), "6");
    script(
        &conn,
        "CREATE FUNCTION twice(n int) RETURNS int LANGUAGE plpgsql AS $$
DECLARE r int;
BEGIN
  EXECUTE 'SELECT $1 * 2' USING n INTO r;
  RETURN r;
END $$",
    );
    assert_eq!(rows(&conn, "SELECT twice(21)"), "42");
    script(
        &conn,
        "CREATE FUNCTION catch_inner() RETURNS text LANGUAGE plpgsql AS $$
BEGIN
  BEGIN
    RAISE EXCEPTION 'inner';
  EXCEPTION WHEN OTHERS THEN
    RETURN 'caught';
  END;
  RETURN 'missed';
END $$",
    );
    assert_eq!(rows(&conn, "SELECT catch_inner()"), "caught");
}
