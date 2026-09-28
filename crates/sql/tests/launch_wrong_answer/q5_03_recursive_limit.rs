//! Q5-03: an outer LIMIT caps a recursive CTE only when the outer query
//! reads the CTE row by row.
//!
//! The WS-A7 pushdown stopped the recursion after LIMIT + OFFSET rows for
//! any `SELECT ... FROM <cte> LIMIT n`, without looking at the SELECT list.
//! An aggregate, a window function or a subquery over the CTE then saw only
//! the first rows: `SELECT count(*) FROM c LIMIT 1` answered 1 for a
//! five-row CTE. Every query here must match bundled SQLite value for value.

use crate::lab::{Lab, int, real};

/// A recursive CTE of the five rows 1..=5.
const R5: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 5)";

fn r5(tail: &str) -> String {
    format!("{R5} {tail}")
}

#[test]
fn q5_03_count_star_limit_1() {
    let lab = Lab::new();
    lab.assert_rows(&r5("SELECT count(*) FROM c LIMIT 1"), &[vec![int(5)]]);
}

#[test]
fn q5_03_window_sum_over_all_rows() {
    let lab = Lab::new();
    lab.assert_rows(
        &r5("SELECT sum(x) OVER () FROM c LIMIT 1"),
        &[vec![int(15)]],
    );
}

#[test]
fn q5_03_sum_max_avg() {
    let lab = Lab::new();
    lab.assert_rows(
        &r5("SELECT sum(x), max(x), avg(x) FROM c LIMIT 1"),
        &[vec![int(15), int(5), real(3.0)]],
    );
}

#[test]
fn q5_03_row_number_desc_limit_2() {
    let lab = Lab::new();
    // Which two rows LIMIT keeps is not specified without an outer ORDER
    // BY (SQLite returns them in window order, RedlineDB in CTE order), so
    // check a property every row must have instead: over five rows, x plus
    // its descending row number is 6. Over the two rows the cap left it
    // was 3. (The CAST is there because arithmetic over a window result
    // answers REAL, a separate defect of the window combination evaluator.)
    lab.assert_rows(
        &r5("SELECT CAST(x + row_number() OVER (ORDER BY x DESC) AS INTEGER) FROM c LIMIT 2"),
        &[vec![int(6)], vec![int(6)]],
    );
}

#[test]
fn q5_03_scalar_subquery_count() {
    let lab = Lab::new();
    lab.assert_rows(
        &r5("SELECT x, (SELECT count(*) FROM c) FROM c LIMIT 1"),
        &[vec![int(1), int(5)]],
    );
}

#[test]
fn q5_03_frame_max_to_unbounded_following() {
    let lab = Lab::new();
    lab.assert_rows(
        &r5(
            "SELECT x, max(x) OVER (ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING) \
             FROM c LIMIT 1",
        ),
        &[vec![int(1), int(5)]],
    );
}

#[test]
fn q5_03_named_window() {
    let lab = Lab::new();
    lab.assert_rows(
        &r5("SELECT x, count(*) OVER w FROM c WINDOW w AS () LIMIT 1"),
        &[vec![int(1), int(5)]],
    );
}

#[test]
fn q5_03_sibling_cte_count() {
    let lab = Lab::new();
    lab.assert_rows(
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 5), \
         d(n) AS (SELECT count(*) FROM c) \
         SELECT x, (SELECT n FROM d) FROM c LIMIT 1",
        &[vec![int(1), int(5)]],
    );
}

#[test]
fn q5_03_case_and_function_projection() {
    let lab = Lab::new();
    // Not provably row-local by shape, so no cap; the answers still match.
    lab.assert_rows(
        &r5("SELECT CASE WHEN x > 0 THEN x END, abs(x) FROM c LIMIT 2"),
        &[vec![int(1), int(1)], vec![int(2), int(2)]],
    );
}

#[test]
fn q5_03_row_local_projection_keeps_limit_and_offset() {
    let lab = Lab::new();
    lab.assert_rows(
        &r5("SELECT x FROM c LIMIT 2 OFFSET 1"),
        &[vec![int(2)], vec![int(3)]],
    );
    lab.assert_rows(
        &r5("SELECT x * 2, -x, (x), CAST(x AS REAL), x || 'a' COLLATE NOCASE, c.x FROM c LIMIT 2"),
        &[
            vec![
                int(2),
                int(-1),
                int(1),
                real(1.0),
                crate::lab::text("1a"),
                int(1),
            ],
            vec![
                int(4),
                int(-2),
                int(2),
                real(2.0),
                crate::lab::text("2a"),
                int(2),
            ],
        ],
    );
    lab.assert_rows(&r5("SELECT * FROM c LIMIT 1"), &[vec![int(1)]]);
    lab.assert_rows(&r5("SELECT c.* FROM c LIMIT 1 OFFSET 4"), &[vec![int(5)]]);
}

/// With no terminating WHERE, a query that needs every row must stop at
/// the iteration limit with an error; it used to answer from a truncated
/// CTE. (SQLite would recurse forever here, so this is RedlineDB only.)
#[test]
fn q5_03_unbounded_recursion_with_aggregate_errors() {
    let lab = Lab::new();
    let sql = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) \
               SELECT count(*) FROM c LIMIT 1";
    match lab.rows_redline(sql, &[]) {
        crate::lab::Outcome::Err(err) => assert!(
            err.contains("exceeded 10000 iterations"),
            "unexpected error for `{sql}`: {err}"
        ),
        crate::lab::Outcome::Rows(rows) => {
            panic!("`{sql}` answered {rows:?} from a truncated recursion")
        }
    }
}

/// The row-local form keeps the pushdown: an unbounded recursion read
/// row by row still stops after LIMIT + OFFSET rows.
#[test]
fn q5_03_unbounded_recursion_row_local_limit_still_stops() {
    let lab = Lab::new();
    lab.assert_rows(
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) \
         SELECT x * 10 FROM c LIMIT 3 OFFSET 2",
        &[vec![int(30)], vec![int(40)], vec![int(50)]],
    );
}
