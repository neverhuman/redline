//! NTILE, PERCENT_RANK and CUME_DIST, LAG and LEAD, and FIRST_VALUE,
//! LAST_VALUE and NTH_VALUE parity against rusqlite.

use super::*;

#[test]
fn ntile_distributes_buckets() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(v INTEGER)");
    lab.execute("INSERT INTO t VALUES (1), (2), (3), (4), (5), (6), (7)");
    lab.assert_match("SELECT v, NTILE(3) OVER (ORDER BY v) AS bucket FROM t ORDER BY v");
}

#[test]
fn percent_rank_and_cume_dist_with_ties() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(grp TEXT, v INTEGER)");
    lab.execute("INSERT INTO t VALUES ('A',1),('A',1),('A',2),('A',4),('B',5),('B',5)");
    lab.assert_match(
        "SELECT grp, v, \
            PERCENT_RANK() OVER (PARTITION BY grp ORDER BY v) AS pr, \
            CUME_DIST() OVER (PARTITION BY grp ORDER BY v) AS cd \
         FROM t ORDER BY grp, v",
    );
}

// ── LAG / LEAD ─────────────────────────────────────────────────────────────

#[test]
fn lag_and_lead_with_default() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(v INTEGER)");
    lab.execute("INSERT INTO t VALUES (1), (2), (3), (4)");
    lab.assert_match(
        "SELECT v, LAG(v, 1, -1) OVER (ORDER BY v) AS prev, \
                LEAD(v, 1, -1) OVER (ORDER BY v) AS next \
         FROM t ORDER BY v",
    );
}

#[test]
fn lag_with_partition() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(grp TEXT, v INTEGER)");
    lab.execute("INSERT INTO t VALUES ('A',1),('A',2),('A',3),('B',10),('B',20)");
    lab.assert_match(
        "SELECT grp, v, LAG(v) OVER (PARTITION BY grp ORDER BY v) AS prev \
         FROM t ORDER BY grp, v",
    );
}

// ── FIRST_VALUE / LAST_VALUE / NTH_VALUE ───────────────────────────────────

#[test]
fn first_value_last_value() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(grp TEXT, v INTEGER)");
    lab.execute("INSERT INTO t VALUES ('A',1),('A',2),('A',3),('B',10),('B',20)");
    lab.assert_match(
        "SELECT grp, v, \
            FIRST_VALUE(v) OVER (PARTITION BY grp ORDER BY v ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS fv, \
            LAST_VALUE(v)  OVER (PARTITION BY grp ORDER BY v ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS lv \
         FROM t ORDER BY grp, v",
    );
}

#[test]
fn nth_value_basic() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(v INTEGER)");
    lab.execute("INSERT INTO t VALUES (10), (20), (30), (40)");
    lab.assert_match(
        "SELECT v, NTH_VALUE(v, 2) OVER (ORDER BY v ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS nth \
         FROM t ORDER BY v",
    );
}
