//! Encoded index keys order INTEGER and REAL values the way SQLite orders
//! the values, across every boundary of the key space.

use super::*;

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
