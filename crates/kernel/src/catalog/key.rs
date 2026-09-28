use std::cmp::Ordering;

use super::value::ValueRef;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum NullOrder {
    First,
    Last,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedIndexKey {
    pub bytes: Vec<u8>,
    pub contains_null: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexKeyDef {
    pub ordinal: u16,
    pub source: IndexKeySource,
    pub sort_dir: SortDir,
    pub null_order: NullOrder,
    /// Per-key collation name in UPPER-CASE (e.g. `"NOCASE"`, `"RTRIM"`).
    /// `None` means binary (byte-level) comparison — the default.
    /// Propagated from `CREATE INDEX … (col COLLATE name)`.
    pub collation: Option<Box<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexKeySource {
    Column {
        attnum: u16,
    },
    /// A6 SQL-D: expression index key. SQL stored verbatim from CREATE
    /// INDEX; SQL exec re-parses with `sqlparser` and evaluates against
    /// row values. `referenced_cols` lists ordinals the expression
    /// mentions so the index-update path re-emits keys only when an
    /// input column was touched.
    Expression {
        sql: Box<str>,
        referenced_cols: Vec<u16>,
    },
}

// Index-key part tags (index-format epoch 3). Byte order of the tags is the
// SQLite storage-class order: NULL < numeric < TEXT < BLOB.
//
// INTEGER and REAL share ONE numeric key space ordered by numeric value, the
// way SQLite compares them (`sqlite3IntFloatCompare`): 1 and 1.0 encode to the
// same bytes, 2 sorts after 1.5, and 9007199254740993 sorts after
// 9007199254740992.0. Epoch 2 (RedlineDB v4.x) gave INTEGER and REAL separate
// tags (0x10 and 0x20), so every INTEGER sorted before every REAL. These tags
// deliberately avoid 0x10 and 0x20 so an epoch-2 numeric key never decodes as
// an epoch-3 one.
const TAG_NULL: u8 = 0x00;
/// A REAL below -2^63 (including -inf): the part body is the sortable f64.
const TAG_NUM_BELOW: u8 = 0x18;
/// An INTEGER, or a REAL in [-2^63, 2^63): the body is the value's integer
/// part (truncated toward zero) as a sortable i64, then a fraction marker,
/// then (for a non-zero fraction only) the sortable f64 fraction.
const TAG_NUM: u8 = 0x19;
/// A REAL at or above 2^63 (including +inf and NaN): the sortable f64.
const TAG_NUM_ABOVE: u8 = 0x1a;
const TAG_TEXT: u8 = 0x30;
const TAG_BLOB: u8 = 0x40;

/// Fraction markers that follow the integer part of a `TAG_NUM` part. With
/// the same integer part, a negative fraction sorts first, then a whole
/// number, then a positive fraction. `trunc` is monotone and the fraction
/// `r - trunc(r)` lies in (-1, 1), so (integer part, fraction) ordered
/// lexicographically is the numeric order.
const FRAC_NEG: u8 = 0x01;
const FRAC_NONE: u8 = 0x02;
const FRAC_POS: u8 = 0x03;

const SIGN_BIT: u64 = 0x8000_0000_0000_0000;
/// 2^63 as an `f64`: the first REAL above every i64.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

pub fn encode_index_key(
    parts: &[ValueRef<'_>],
    dirs: &[SortDir],
    out: &mut Vec<u8>,
) -> EncodedIndexKey {
    debug_assert_eq!(parts.len(), dirs.len());
    out.clear();
    let legacy = super::key_epoch::v4_index_format_active();
    let mut contains_null = false;
    for (&value, &dir) in parts.iter().zip(dirs.iter()) {
        let start = out.len();
        contains_null |= matches!(value, ValueRef::Null);
        if legacy {
            super::key_epoch::encode_part_v4(value, out);
        } else {
            encode_part(value, out);
        }
        if dir == SortDir::Desc {
            for byte in &mut out[start..] {
                *byte = !*byte;
            }
        }
        out.push(0xff);
    }
    EncodedIndexKey {
        bytes: out.clone(),
        contains_null,
    }
}

pub fn compare_index_keys(left: &[u8], right: &[u8]) -> Ordering {
    left.cmp(right)
}

fn encode_part(value: ValueRef<'_>, out: &mut Vec<u8>) {
    match value {
        ValueRef::Null => out.push(TAG_NULL),
        ValueRef::Integer(v) => {
            out.push(TAG_NUM);
            out.extend_from_slice(&sortable_i64(v).to_be_bytes());
            out.push(FRAC_NONE);
        }
        ValueRef::Real(v) => encode_real(v, out),
        ValueRef::Text(v) => {
            out.push(TAG_TEXT);
            encode_bytes(v.as_bytes(), out);
        }
        ValueRef::Blob(v) => {
            out.push(TAG_BLOB);
            encode_bytes(v, out);
        }
    }
}

fn encode_real(v: f64, out: &mut Vec<u8>) {
    if v.is_nan() {
        // SQLite stores a NaN as NULL, so none should arrive here; keep a
        // canonical NaN after +inf rather than let its sign bit scatter it.
        out.push(TAG_NUM_ABOVE);
        out.extend_from_slice(&sortable_f64(f64::NAN).to_be_bytes());
    } else if v < -TWO_POW_63 {
        out.push(TAG_NUM_BELOW);
        out.extend_from_slice(&sortable_f64(v).to_be_bytes());
    } else if v >= TWO_POW_63 {
        out.push(TAG_NUM_ABOVE);
        out.extend_from_slice(&sortable_f64(v).to_be_bytes());
    } else {
        // Inside [-2^63, 2^63) truncation is exact in i64, and the fraction
        // is exact too: |v| >= 1 gives trunc(v) within a factor of two of v
        // (Sterbenz), and |v| < 1 gives trunc(v) == 0.
        let whole = v.trunc();
        let fraction = v - whole;
        out.push(TAG_NUM);
        out.extend_from_slice(&sortable_i64(whole as i64).to_be_bytes());
        if fraction == 0.0 {
            out.push(FRAC_NONE);
        } else {
            out.push(if fraction < 0.0 { FRAC_NEG } else { FRAC_POS });
            out.extend_from_slice(&sortable_f64(fraction).to_be_bytes());
        }
    }
}

fn sortable_i64(v: i64) -> u64 {
    (v as u64) ^ SIGN_BIT
}

fn sortable_f64(v: f64) -> u64 {
    let bits = if v == 0.0 { 0.0 } else { v }.to_bits();
    if bits & SIGN_BIT != 0 {
        !bits
    } else {
        bits ^ SIGN_BIT
    }
}

fn unsortable_f64(sortable: u64) -> f64 {
    let bits = if sortable & SIGN_BIT != 0 {
        sortable ^ SIGN_BIT
    } else {
        !sortable
    };
    f64::from_bits(bits)
}

fn encode_bytes(bytes: &[u8], out: &mut Vec<u8>) {
    for &byte in bytes {
        if byte == 0 {
            out.push(0);
            out.push(0xff);
        } else {
            out.push(byte);
        }
    }
    out.push(0);
    out.push(0);
}

/// A numeric index-key part decoded back into a number.
///
/// The key space does not record the storage class: INTEGER 2 and REAL 2.0
/// are the same key, so a whole number in the i64 range comes back as
/// `Whole` and the caller picks INTEGER or REAL (from the column's
/// affinity). A number with a fraction, or one outside the i64 range, can
/// only have been a REAL.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DecodedNumericKey {
    Whole(i64),
    Real(f64),
}

/// True when `tag` (already un-inverted for a DESC part) starts a numeric
/// index-key part.
pub fn is_numeric_key_tag(tag: u8) -> bool {
    matches!(tag, TAG_NUM_BELOW | TAG_NUM | TAG_NUM_ABOVE)
}

/// Decode the numeric part at the start of `part` (tag byte first, bytes
/// already un-inverted for a DESC part, part separator excluded). Returns the
/// number and the part's encoded length, or `None` when `part` does not hold
/// a whole numeric part.
pub fn decode_numeric_key_part(part: &[u8]) -> Option<(DecodedNumericKey, usize)> {
    let tag = *part.first()?;
    let word = |at: usize| -> Option<u64> {
        let bytes: [u8; 8] = part.get(at..at + 8)?.try_into().ok()?;
        Some(u64::from_be_bytes(bytes))
    };
    match tag {
        TAG_NUM_BELOW | TAG_NUM_ABOVE => {
            Some((DecodedNumericKey::Real(unsortable_f64(word(1)?)), 9))
        }
        TAG_NUM => {
            let whole = (word(1)? ^ SIGN_BIT) as i64;
            match *part.get(9)? {
                FRAC_NONE => Some((DecodedNumericKey::Whole(whole), 10)),
                FRAC_NEG | FRAC_POS => {
                    let fraction = unsortable_f64(word(10)?);
                    // |whole| < 2^52 whenever a fraction exists, so the sum
                    // reproduces the original REAL exactly.
                    Some((DecodedNumericKey::Real(whole as f64 + fraction), 18))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The encoded length of the numeric part at the start of `part`, reading
/// the fraction marker through `uninvert` (DESC parts store every byte
/// inverted). `None` when `part` is too short or not numeric.
pub fn numeric_key_part_len(part: &[u8], uninvert: impl Fn(u8) -> u8) -> Option<usize> {
    match uninvert(*part.first()?) {
        TAG_NUM_BELOW | TAG_NUM_ABOVE => Some(9),
        TAG_NUM => match uninvert(*part.get(9)?) {
            FRAC_NONE => Some(10),
            FRAC_NEG | FRAC_POS => Some(18),
            _ => None,
        },
        _ => None,
    }
}
