//! sqlparser accepts identity options only in INCREMENT-then-START order.
//! Postgres writes `START WITH n INCREMENT BY m`. Reorder the parenthetical
//! so the existing sequence-option parser can read both.

pub(crate) fn rewrite_identity_sequence_options(sql: &str) -> String {
    if !super::contains_ignore_ascii_case(sql, b"as identity") {
        return sql.to_owned();
    }
    // Literals and comments are blanked: only a code `AS IDENTITY` counts.
    let lower = crate::parser::code_scan::code_lowercase(sql);
    let bytes = sql.as_bytes();
    let lower_bytes = lower.as_bytes();
    let needle = b"as identity";
    let mut out = String::with_capacity(sql.len());
    let mut last = 0usize;
    let mut i = 0usize;
    while i + needle.len() <= lower_bytes.len() {
        let boundary = i == 0 || !lower_bytes[i - 1].is_ascii_alphanumeric();
        if boundary && &lower_bytes[i..i + needle.len()] == needle {
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'(' {
                if let Some(end) = matching_paren(bytes, j) {
                    let inner = &sql[j + 1..end - 1];
                    let reordered = reorder_sequence_options(inner);
                    if reordered != inner {
                        out.push_str(&sql[last..=j]);
                        out.push_str(&reordered);
                        last = end - 1;
                        i = end;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    if last == 0 {
        return sql.to_owned();
    }
    out.push_str(&sql[last..]);
    out
}

fn matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn reorder_sequence_options(inner: &str) -> String {
    let clauses = split_clauses(inner);
    if clauses.len() < 2 {
        return inner.to_owned();
    }
    let mut ranked: Vec<(u8, &str)> = clauses
        .iter()
        .map(|clause| (clause_rank(clause), clause.as_str()))
        .collect();
    let already = ranked.windows(2).all(|pair| pair[0].0 <= pair[1].0);
    if already {
        return inner.to_owned();
    }
    ranked.sort_by_key(|item| item.0);
    ranked
        .into_iter()
        .map(|(_, clause)| clause.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_clauses(inner: &str) -> Vec<String> {
    let lower = inner.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(len) = keyword_at(bytes, i) {
            starts.push(i);
            i += len;
            continue;
        }
        i += 1;
    }
    if starts.is_empty() {
        return vec![inner.to_owned()];
    }
    let mut clauses = Vec::new();
    if starts[0] > 0 && inner[..starts[0]].trim().is_empty() {
        // leading whitespace only
    } else if starts[0] > 0 {
        clauses.push(inner[..starts[0]].trim().to_owned());
    }
    for (idx, start) in starts.iter().copied().enumerate() {
        let end = starts.get(idx + 1).copied().unwrap_or(inner.len());
        clauses.push(inner[start..end].trim().to_owned());
    }
    clauses
}

fn keyword_at(bytes: &[u8], i: usize) -> Option<usize> {
    if i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    const KEYS: &[&[u8]] = &[
        b"no minvalue",
        b"no maxvalue",
        b"no cycle",
        b"increment",
        b"minvalue",
        b"maxvalue",
        b"start",
        b"cache",
        b"cycle",
    ];
    for key in KEYS {
        if i + key.len() <= bytes.len() && &bytes[i..i + key.len()] == *key {
            let end = i + key.len();
            if end == bytes.len() || !bytes[end].is_ascii_alphanumeric() {
                return Some(key.len());
            }
        }
    }
    None
}

fn clause_rank(clause: &str) -> u8 {
    let lower = clause.trim().to_ascii_lowercase();
    if lower.starts_with("increment") {
        0
    } else if lower.starts_with("minvalue") || lower.starts_with("no minvalue") {
        1
    } else if lower.starts_with("maxvalue") || lower.starts_with("no maxvalue") {
        2
    } else if lower.starts_with("start") {
        3
    } else if lower.starts_with("cache") {
        4
    } else if lower.starts_with("cycle") || lower.starts_with("no cycle") {
        5
    } else {
        6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_with_then_increment_is_reordered() {
        let sql = "CREATE TABLE t (id int GENERATED ALWAYS AS IDENTITY (START WITH 1000 INCREMENT BY 10), v text)";
        let out = rewrite_identity_sequence_options(sql);
        assert!(
            out.to_ascii_lowercase().find("increment").unwrap()
                < out.to_ascii_lowercase().find("start").unwrap(),
            "{out}"
        );
        assert!(out.contains("1000"));
        assert!(out.contains("10"));
    }
}
