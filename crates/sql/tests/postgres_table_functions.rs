use redlinedb_sql::{Database, DbOptions, SqlValue, Step};

fn query(sql: &str) -> Vec<Vec<SqlValue>> {
    let db = Database::create_in_memory(DbOptions::default()).unwrap();
    let conn = db.connect();
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    let mut rows = Vec::new();
    while stmt.step().unwrap() == Step::Row {
        rows.push(
            (0..stmt.column_count())
                .map(|i| stmt.column_value(i).unwrap().clone())
                .collect(),
        );
    }
    rows
}

#[test]
fn unnest_preserves_order_nulls_and_pads_shorter_arrays() {
    use redlinedb_kernel::catalog::OwnedValue::{Integer as I, Null as N};
    assert_eq!(
        query("SELECT * FROM unnest(ARRAY[3,NULL,1], ARRAY[8]) AS u(a,b)"),
        vec![vec![I(3), I(8)], vec![N, N], vec![I(1), N]]
    );
    assert!(query("SELECT * FROM unnest(ARRAY[]::int[])").is_empty());
    assert!(query("SELECT * FROM unnest(NULL::int[])").is_empty());
}

#[test]
fn ordinality_and_aliases_work_in_single_sources_and_joins() {
    use redlinedb_kernel::catalog::OwnedValue::Integer as I;
    assert_eq!(
        query("SELECT val, ord FROM unnest(ARRAY[9,7]) WITH ORDINALITY AS u(val,ord) ORDER BY ord"),
        vec![vec![I(9), I(1)], vec![I(7), I(2)]]
    );
    assert_eq!(
        query(
            "SELECT s.x,u.val,u.ord FROM generate_series(1,2) s(x) CROSS JOIN unnest(ARRAY[9,7]) WITH ORDINALITY u(val,ord) ORDER BY s.x,u.ord"
        ),
        vec![
            vec![I(1), I(9), I(1)],
            vec![I(1), I(7), I(2)],
            vec![I(2), I(9), I(1)],
            vec![I(2), I(7), I(2)]
        ]
    );
    assert_eq!(
        query("SELECT g FROM generate_series(1,2) g"),
        vec![vec![I(1)], vec![I(2)]]
    );
    assert_eq!(
        query("SELECT value FROM generate_series(1,2)"),
        vec![vec![I(1)], vec![I(2)]]
    );
}

#[test]
fn series_handles_null_direction_and_integer_boundaries() {
    use redlinedb_kernel::catalog::OwnedValue::Integer as I;
    assert!(query("SELECT * FROM generate_series(NULL,2)").is_empty());
    assert!(query("SELECT * FROM generate_series(3,1)").is_empty());
    assert_eq!(
        query("SELECT * FROM generate_series(5,1,-2)"),
        vec![vec![I(5)], vec![I(3)], vec![I(1)]]
    );
    assert_eq!(
        query("SELECT * FROM generate_series(9223372036854775806,9223372036854775807)"),
        vec![vec![I(i64::MAX - 1)], vec![I(i64::MAX)]]
    );
    assert_eq!(
        query("SELECT * FROM generate_series(-9223372036854775807,-9223372036854775808,-1)"),
        vec![vec![I(i64::MIN + 1)], vec![I(i64::MIN)]]
    );
}

#[test]
fn series_budget_and_bad_arguments_fail_before_materializing() {
    let db = Database::create_in_memory(DbOptions::default()).unwrap();
    let conn = db.connect();
    for sql in [
        "SELECT * FROM generate_series(1,2,0)",
        "SELECT * FROM generate_series(0,9223372036854775807)",
        "SELECT * FROM unnest(42)",
        "SELECT * FROM unnest(ARRAY[1]) AS u(a,b)",
    ] {
        assert!(conn.prepare(sql).is_err(), "must reject {sql}");
    }
}
