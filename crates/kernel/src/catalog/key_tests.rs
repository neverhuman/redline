//! Index-key encoding (index-format epoch 3): one numeric key space for
//! INTEGER and REAL, ordered by value as SQLite orders them.

use std::cmp::Ordering;

use super::key::{
    DecodedNumericKey, SortDir, compare_index_keys, decode_numeric_key_part, encode_index_key,
    is_numeric_key_tag, numeric_key_part_len,
};
use super::key_epoch::with_v4_index_format_for_tests;
use super::value::ValueRef;

const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;
const TWO_POW_53: i64 = 9_007_199_254_740_992;

fn key(value: ValueRef<'_>, dir: SortDir) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_index_key(&[value], &[dir], &mut buf).bytes
}

/// Values in SQLite's sort order; each inner group holds values SQLite
/// compares equal.
fn ordered_groups() -> Vec<Vec<ValueRef<'static>>> {
    use ValueRef::{Blob, Integer, Null, Real, Text};
    vec![
        vec![Null],
        vec![Real(f64::NEG_INFINITY)],
        vec![Real(f64::MIN)],
        vec![Real(-1e300)],
        vec![Real(f64::from_bits((-TWO_POW_63).to_bits() + 1))],
        vec![Integer(i64::MIN), Real(-TWO_POW_63)],
        vec![Integer(i64::MIN + 1)],
        vec![Integer(-TWO_POW_53 - 1)],
        vec![Integer(-TWO_POW_53), Real(-TWO_POW_53 as f64)],
        vec![Real(-4_503_599_627_370_495.5)],
        vec![Integer(-2), Real(-2.0)],
        vec![Real(-1.5)],
        vec![Integer(-1), Real(-1.0)],
        vec![Real(-0.999_999_999_999_999_9)],
        vec![Real(-0.5)],
        vec![Real(-5e-324)],
        vec![Integer(0), Real(0.0), Real(-0.0)],
        vec![Real(5e-324)],
        vec![Real(f64::MIN_POSITIVE)],
        vec![Real(0.25)],
        vec![Real(1.0 - f64::EPSILON / 2.0)],
        vec![Integer(1), Real(1.0)],
        vec![Real(1.5)],
        vec![Integer(2), Real(2.0)],
        vec![Real(2.75)],
        vec![Integer(3)],
        vec![Real(4_503_599_627_370_495.5)],
        vec![Integer(TWO_POW_53), Real(TWO_POW_53 as f64)],
        vec![Integer(TWO_POW_53 + 1)],
        vec![Integer(TWO_POW_53 + 2), Real((TWO_POW_53 + 2) as f64)],
        vec![Integer(i64::MAX - 1)],
        vec![Integer(i64::MAX)],
        vec![Real(TWO_POW_63)],
        vec![Real(f64::from_bits(TWO_POW_63.to_bits() + 1))],
        vec![Real(1e300)],
        vec![Real(f64::MAX)],
        vec![Real(f64::INFINITY)],
        vec![Text("")],
        vec![Text("\0")],
        vec![Text("a")],
        vec![Text("a\0")],
        vec![Text("b")],
        vec![Blob(b"")],
        vec![Blob(b"\0")],
        vec![Blob(b"\x01")],
    ]
}

#[test]
fn key_order_follows_sqlite_value_order() {
    let groups = ordered_groups();
    for dir in [SortDir::Asc, SortDir::Desc] {
        for (i, left_group) in groups.iter().enumerate() {
            for (j, right_group) in groups.iter().enumerate() {
                for &left in left_group {
                    for &right in right_group {
                        let want = match dir {
                            SortDir::Asc => i.cmp(&j),
                            SortDir::Desc => j.cmp(&i),
                        };
                        let got = compare_index_keys(&key(left, dir), &key(right, dir));
                        assert_eq!(got, want, "{dir:?}: {left:?} vs {right:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn equal_numbers_share_one_key() {
    for group in ordered_groups() {
        for dir in [SortDir::Asc, SortDir::Desc] {
            let first = key(group[0], dir);
            for &value in &group[1..] {
                assert_eq!(
                    key(value, dir),
                    first,
                    "{dir:?}: {:?} vs {value:?}",
                    group[0]
                );
            }
        }
    }
}

#[test]
fn composite_keys_order_by_the_first_differing_part() {
    let mut buf = Vec::new();
    let dirs = [SortDir::Asc, SortDir::Asc];
    let whole = encode_index_key(
        &[ValueRef::Integer(5), ValueRef::Text("z")],
        &dirs,
        &mut buf,
    );
    let fraction = encode_index_key(&[ValueRef::Real(5.5), ValueRef::Text("a")], &dirs, &mut buf);
    let below = encode_index_key(
        &[ValueRef::Real(4.5), ValueRef::Text("zz")],
        &dirs,
        &mut buf,
    );
    let same = encode_index_key(&[ValueRef::Real(5.0), ValueRef::Text("z")], &dirs, &mut buf);
    assert_eq!(
        compare_index_keys(&whole.bytes, &fraction.bytes),
        Ordering::Less
    );
    assert_eq!(
        compare_index_keys(&below.bytes, &whole.bytes),
        Ordering::Less
    );
    assert_eq!(whole.bytes, same.bytes);
}

#[test]
fn numeric_parts_decode_to_their_value() {
    for group in ordered_groups() {
        for value in group {
            let expected = match value {
                ValueRef::Integer(v) => DecodedNumericKey::Whole(v),
                ValueRef::Real(v) if v.fract() == 0.0 && (-TWO_POW_63..TWO_POW_63).contains(&v) => {
                    DecodedNumericKey::Whole(v as i64)
                }
                ValueRef::Real(v) => DecodedNumericKey::Real(v),
                _ => continue,
            };
            for dir in [SortDir::Asc, SortDir::Desc] {
                let bytes = key(value, dir);
                let uninvert = |byte: u8| if dir == SortDir::Desc { !byte } else { byte };
                assert!(is_numeric_key_tag(uninvert(bytes[0])));
                let len = numeric_key_part_len(&bytes, uninvert).expect("numeric length");
                assert_eq!(len + 1, bytes.len(), "part then one separator");
                let part: Vec<u8> = bytes[..len].iter().map(|&b| uninvert(b)).collect();
                let (decoded, decoded_len) = decode_numeric_key_part(&part).expect("decodes");
                assert_eq!(decoded_len, len);
                match (decoded, expected) {
                    (DecodedNumericKey::Real(a), DecodedNumericKey::Real(b)) => {
                        assert_eq!(a.to_bits(), b.to_bits(), "{value:?}")
                    }
                    (a, b) => assert_eq!(a, b, "{value:?}"),
                }
            }
        }
    }
}

#[test]
fn v4_format_hook_writes_the_old_tags_on_this_thread_only() {
    let int = key(ValueRef::Integer(1), SortDir::Asc);
    let real = key(ValueRef::Real(1.5), SortDir::Asc);
    assert!(![0x10, 0x20].contains(&int[0]) && ![0x10, 0x20].contains(&real[0]));
    with_v4_index_format_for_tests(|| {
        assert_eq!(key(ValueRef::Integer(1), SortDir::Asc)[0], 0x10);
        assert_eq!(key(ValueRef::Real(1.5), SortDir::Asc)[0], 0x20);
        // Epoch 2 put every INTEGER before every REAL.
        assert_eq!(
            compare_index_keys(
                &key(ValueRef::Integer(2), SortDir::Asc),
                &key(ValueRef::Real(1.5), SortDir::Asc)
            ),
            Ordering::Less
        );
        std::thread::spawn(|| assert_eq!(key(ValueRef::Integer(1), SortDir::Asc)[0], 0x19))
            .join()
            .expect("other thread");
        assert_eq!(
            crate::index::current_index_version(),
            crate::index::V4_INDEX_VERSION
        );
    });
    assert_eq!(key(ValueRef::Integer(1), SortDir::Asc), int);
    assert_eq!(
        crate::index::current_index_version(),
        crate::index::INDEX_VERSION
    );
}
