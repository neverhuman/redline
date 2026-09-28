//! Q5-06/S9-06 follow-up: sliding `sum()` / `total()` / `avg()` frames.
//!
//! Two defects in the sliding-window accumulator:
//! - Its ROWS schedules looped once per row of the FOLLOWING offset, so
//!   `ROWS BETWEEN CURRENT ROW AND 9223372036854775807 FOLLOWING` over three
//!   rows spun for about 2^63 iterations, and a start offset near 2^63
//!   overflowed `k + start` (a panic in debug builds).
//! - RANGE and GROUPS frames always removed the rows that left the frame
//!   before adding the ones that entered it. SQLite adds first for a RANGE
//!   frame whose bounds are both PRECEDING or both FOLLOWING, so it reports
//!   `integer overflow` where RedlineDB summed on.
//!
//! RANGE offsets are compared over dense, distinct ORDER BY keys (1, 2,
//! 3, ...), where a RANGE offset of n spans the same rows as n rows.

use std::time::{Duration, Instant};

use crate::lab::{Lab, null};

const MAX: &str = "9223372036854775807";
const BIG: &str = "9223372036854775807";

/// Tables `name(o, x)` with `o` = 1, 2, 3, ... and the given `x` values.
fn load(lab: &Lab, name: &str, values: &[&str]) {
    lab.exec_both(&format!("CREATE TABLE {name}(o INTEGER, x)"));
    for (i, value) in values.iter().enumerate() {
        lab.exec_both(&format!("INSERT INTO {name} VALUES ({}, {value})", i + 1));
    }
}

fn tables(lab: &Lab) -> Vec<&'static str> {
    load(lab, "w_over", &[MAX, "1", "5", "-3"]);
    load(
        lab,
        "w_cancel",
        &["1", MAX, "-9223372036854775807", "4", "5"],
    );
    load(lab, "w_real", &["1.5", MAX, "1", "2"]);
    load(lab, "w_ints", &["1", "2", "3", "4", "5", "6"]);
    load(lab, "w_one", &["7"]);
    vec!["w_over", "w_cancel", "w_real", "w_ints", "w_one"]
}

fn compare(lab: &Lab, table: &str, frame: &str) {
    for func in ["sum(x)", "total(x)", "avg(x)", "count(x)"] {
        lab.assert_same(
            &format!("SELECT o, {func} OVER (ORDER BY o {frame}) FROM {table} ORDER BY o"),
            true,
        );
    }
}

#[test]
fn range_and_groups_frames_follow_sqlite_order() {
    let lab = Lab::new();
    let frames = [
        "RANGE BETWEEN 1 PRECEDING AND 1 PRECEDING",
        "RANGE BETWEEN 2 PRECEDING AND 1 PRECEDING",
        "RANGE BETWEEN 1 FOLLOWING AND 1 FOLLOWING",
        "RANGE BETWEEN 1 FOLLOWING AND 2 FOLLOWING",
        "RANGE BETWEEN 1 PRECEDING AND 1 FOLLOWING",
        "RANGE BETWEEN 1 PRECEDING AND CURRENT ROW",
        "RANGE BETWEEN CURRENT ROW AND 1 FOLLOWING",
        "RANGE BETWEEN CURRENT ROW AND CURRENT ROW",
        "GROUPS BETWEEN 1 PRECEDING AND 1 PRECEDING",
        "GROUPS BETWEEN 2 PRECEDING AND 1 PRECEDING",
        "GROUPS BETWEEN 1 FOLLOWING AND 1 FOLLOWING",
        "GROUPS BETWEEN 1 FOLLOWING AND 2 FOLLOWING",
        "GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING",
        "GROUPS BETWEEN CURRENT ROW AND CURRENT ROW",
    ];
    for table in tables(&lab) {
        for frame in frames {
            compare(&lab, table, frame);
        }
    }
    // The witnesses: SQLite adds before it removes, so the overflow shows.
    lab.assert_error(
        "SELECT sum(x) OVER (ORDER BY o RANGE BETWEEN 1 FOLLOWING AND 1 FOLLOWING) FROM w_over",
        "integer overflow",
    );
    lab.assert_error(
        "SELECT sum(x) OVER (ORDER BY o RANGE BETWEEN 1 PRECEDING AND 1 PRECEDING) FROM w_over",
        "integer overflow",
    );
    // Peer rows: CURRENT ROW spans the peers.
    lab.exec_both(&format!(
        "CREATE TABLE w_peers(o INTEGER, x); \
         INSERT INTO w_peers VALUES (1, 1), (1, {MAX}), (2, 1), (2, -5), (3, 2), (4, 3), \
         (4, 4), (4, 5), (5, 6);"
    ));
    for frame in [
        "RANGE BETWEEN CURRENT ROW AND CURRENT ROW",
        "GROUPS BETWEEN CURRENT ROW AND CURRENT ROW",
        "GROUPS BETWEEN 1 PRECEDING AND 1 PRECEDING",
        "GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW",
        "GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING",
        "GROUPS BETWEEN 1 FOLLOWING AND 1 FOLLOWING",
        "GROUPS BETWEEN 1 FOLLOWING AND 2 FOLLOWING",
        "GROUPS BETWEEN 1 PRECEDING AND UNBOUNDED FOLLOWING",
        "GROUPS BETWEEN 1 FOLLOWING AND UNBOUNDED FOLLOWING",
    ] {
        for func in ["sum(x)", "total(x)", "count(x)", "avg(x)"] {
            lab.assert_same(
                &format!("SELECT o, x, {func} OVER (ORDER BY o {frame}) FROM w_peers"),
                false,
            );
        }
    }
}

/// Each query must answer promptly; a schedule that walks the offset would
/// run for centuries.
fn prompt(lab: &Lab, sql: &str) {
    let began = Instant::now();
    lab.assert_same(sql, true);
    assert!(
        began.elapsed() < Duration::from_secs(20),
        "`{sql}` took {:?}",
        began.elapsed()
    );
}

#[test]
fn offsets_near_2_63_answer_at_once() {
    let lab = Lab::new();
    let tables = tables(&lab);
    let frames = [
        format!("ROWS BETWEEN CURRENT ROW AND {BIG} FOLLOWING"),
        format!("ROWS BETWEEN 1 PRECEDING AND {BIG} FOLLOWING"),
        format!("ROWS BETWEEN {BIG} PRECEDING AND {BIG} FOLLOWING"),
        format!("ROWS BETWEEN {BIG} PRECEDING AND CURRENT ROW"),
        format!("ROWS BETWEEN {BIG} PRECEDING AND 1 PRECEDING"),
        format!("ROWS BETWEEN {BIG} PRECEDING AND {BIG} PRECEDING"),
        format!("ROWS BETWEEN {BIG} FOLLOWING AND UNBOUNDED FOLLOWING"),
        format!("ROWS BETWEEN {BIG} PRECEDING AND UNBOUNDED FOLLOWING"),
        format!("RANGE BETWEEN {BIG} PRECEDING AND {BIG} FOLLOWING"),
        format!("GROUPS BETWEEN {BIG} PRECEDING AND {BIG} FOLLOWING"),
        format!("GROUPS BETWEEN CURRENT ROW AND {BIG} FOLLOWING"),
    ];
    for table in &tables {
        for frame in &frames {
            for func in ["sum(x)", "total(x)", "avg(x)", "count(x)"] {
                prompt(
                    &lab,
                    &format!("SELECT o, {func} OVER (ORDER BY o {frame}) FROM {table} ORDER BY o"),
                );
            }
        }
    }
}

/// `n FOLLOWING AND m FOLLOWING` with offsets past the partition: SQLite
/// spins on these too, so they are checked against the answer itself:
/// every frame is empty, or holds the rows from `a` on.
#[test]
fn following_following_offsets_past_the_partition() {
    let lab = Lab::new();
    load(&lab, "w_ints", &["1", "2", "3", "4", "5", "6"]);
    for frame in [
        format!("ROWS BETWEEN {BIG} FOLLOWING AND {BIG} FOLLOWING"),
        format!("ROWS BETWEEN 1000000000 FOLLOWING AND {BIG} FOLLOWING"),
    ] {
        let sql = format!("SELECT sum(x) OVER (ORDER BY o {frame}), o FROM w_ints ORDER BY o");
        let began = Instant::now();
        match lab.rows_redline(&sql, &[]) {
            crate::lab::Outcome::Rows(rows) => {
                let sums: Vec<_> = rows.into_iter().map(|row| row[0].clone()).collect();
                assert_eq!(sums, vec![null(); 6], "{sql}");
            }
            crate::lab::Outcome::Err(err) => panic!("{sql}: {err}"),
        }
        assert!(began.elapsed() < Duration::from_secs(20), "{sql}");
    }
    // A short lag with a far end: rows from `a` on.
    let sql = format!(
        "SELECT sum(x) OVER (ORDER BY o ROWS BETWEEN 2 FOLLOWING AND {BIG} FOLLOWING), o \
         FROM w_ints ORDER BY o"
    );
    match lab.rows_redline(&sql, &[]) {
        crate::lab::Outcome::Rows(rows) => {
            let sums: Vec<_> = rows.into_iter().map(|row| row[0].clone()).collect();
            let want: Vec<redlinedb_sql::SqlValue> = [18, 15, 11, 6]
                .into_iter()
                .map(crate::lab::int)
                .chain([null(), null()])
                .collect();
            assert_eq!(sums, want, "{sql}");
        }
        crate::lab::Outcome::Err(err) => panic!("{sql}: {err}"),
    }
}
