//! Lane SQL-D phase 10: text collations (BINARY / NOCASE / RTRIM).
//!
//! SQLite supports three built-in collations. We model them here so the
//! executor can apply a collation when an explicit `COLLATE` clause appears
//! or a column-level collation is declared. Comparisons fall back to the
//! standard byte-wise compare in the absence of any collation.
//!
//! Custom collations registered through `sqlite3_create_collation*` flow
//! through the `Custom { name }` variant and dispatch via
//! `crate::udf::call_registered_collation` at compare time.

use std::cmp::Ordering;

use crate::value::SqlValue;

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub enum Collation {
    #[default]
    Binary,
    NoCase,
    RTrim,
    Uint,
    /// Postgres `en-x-icu` / `und-x-icu`: case-insensitive primary order,
    /// lowercase before uppercase on a tie.
    PgIcu,
    /// Externally registered collation looked up by name through the FFI
    /// collation registry at compare time.
    Custom(String),
}

impl Collation {
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().trim_matches('"');
        match name.to_ascii_uppercase().as_str() {
            "BINARY" | "" | "C" | "POSIX" => Some(Self::Binary),
            "NOCASE" => Some(Self::NoCase),
            "RTRIM" => Some(Self::RTrim),
            "UINT" => Some(Self::Uint),
            "EN-X-ICU" | "UND-X-ICU" => Some(Self::PgIcu),
            _ => Some(Self::Custom(name.to_owned())),
        }
    }

    /// True if `name` is a built-in collation or has been registered
    /// through the custom-collation FFI. SQLite rejects DDL and
    /// `ORDER BY` references to unknown collations at compile time
    /// (`no such collation sequence: NAME`) — call this from the
    /// parser to mirror that error.
    pub fn is_known(name: &str) -> bool {
        let name = name.trim().trim_matches('"');
        if matches!(
            name.to_ascii_uppercase().as_str(),
            "BINARY" | "" | "C" | "POSIX" | "NOCASE" | "RTRIM" | "UINT" | "EN-X-ICU" | "UND-X-ICU"
        ) {
            return true;
        }
        if session_collation_level(name).is_some() {
            return true;
        }
        crate::udf::call_registered_collation(crate::udf::current_db(), name, "", "").is_some()
    }

    pub fn compare_text(&self, a: &str, b: &str) -> Ordering {
        match self {
            Self::Binary => a.cmp(b),
            Self::NoCase => {
                let mut ai = a.bytes();
                let mut bi = b.bytes();
                loop {
                    match (ai.next(), bi.next()) {
                        (Some(x), Some(y)) => {
                            let xa = x.to_ascii_lowercase();
                            let ya = y.to_ascii_lowercase();
                            match xa.cmp(&ya) {
                                Ordering::Equal => continue,
                                non_eq => return non_eq,
                            }
                        }
                        (None, None) => return Ordering::Equal,
                        (None, _) => return Ordering::Less,
                        (_, None) => return Ordering::Greater,
                    }
                }
            }
            Self::RTrim => a.trim_end_matches(' ').cmp(b.trim_end_matches(' ')),
            Self::Uint => compare_uint_text(a, b),
            Self::PgIcu => compare_pg_icu(a, b),
            Self::Custom(name) => {
                if let Some(level) = session_collation_level(name) {
                    return compare_pg_level(level, a, b);
                }
                match crate::udf::call_registered_collation(crate::udf::current_db(), name, a, b) {
                    Some(ord) => ord,
                    None => a.cmp(b),
                }
            }
        }
    }

    /// Key used by `ORDER BY`. Byte order of this string matches
    /// [`Self::compare_text`].
    pub fn sort_text(&self, text: &str) -> String {
        match self {
            Self::NoCase => text.to_ascii_lowercase(),
            Self::RTrim => text.trim_end_matches(' ').to_owned(),
            Self::PgIcu => icu_sort_key(text),
            Self::Custom(name) => match session_collation_level(name) {
                Some(1) => fold_level1(text),
                Some(_) => icu_sort_key(text),
                None => text.to_owned(),
            },
            Self::Binary | Self::Uint => text.to_owned(),
        }
    }

    pub fn compare_values(&self, left: &SqlValue, right: &SqlValue) -> Option<Ordering> {
        match (left, right) {
            (SqlValue::Text(a), SqlValue::Text(b)) => Some(self.compare_text(a, b)),
            _ => None,
        }
    }
}

fn session_collation_level(name: &str) -> Option<u8> {
    let conn = crate::exec::current_connection()?;
    let folded = name.to_ascii_lowercase();
    crate::exec::with_session_reentrant(conn, |session| {
        Ok(session.pg_collations.get(&folded).copied())
    })
    .ok()
    .flatten()
}

fn compare_pg_icu(a: &str, b: &str) -> Ordering {
    icu_sort_key(a).cmp(&icu_sort_key(b))
}

fn compare_pg_level(level: u8, a: &str, b: &str) -> Ordering {
    if level == 1 {
        fold_level1(a).cmp(&fold_level1(b))
    } else {
        a.to_lowercase().cmp(&b.to_lowercase())
    }
}

fn icu_sort_key(text: &str) -> String {
    let mut key = text.to_lowercase();
    key.push('\0');
    if text.chars().any(|ch| ch.is_uppercase()) {
        key.push('1');
    } else {
        key.push('0');
    }
    key
}

fn fold_level1(text: &str) -> String {
    text.chars()
        .map(|ch| match ch.to_lowercase().next().unwrap_or(ch) {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

fn compare_uint_text(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(da), Some(db)) if da.is_ascii_digit() && db.is_ascii_digit() => {
                let a_num = take_uint(&mut ai);
                let b_num = take_uint(&mut bi);
                match a_num.cmp(&b_num) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
            (Some(_), Some(_)) => {
                let ca = ai.next().unwrap();
                let cb = bi.next().unwrap();
                match ca.cmp(&cb) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
        }
    }
}

fn take_uint(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> (usize, usize) {
    let mut value: usize = 0;
    let mut digits = 0usize;
    while let Some(ch) = chars.peek().copied() {
        if !ch.is_ascii_digit() {
            break;
        }
        value = value
            .saturating_mul(10)
            .saturating_add(ch.to_digit(10).unwrap_or(0) as usize);
        digits += 1;
        chars.next();
    }
    (value, digits)
}
