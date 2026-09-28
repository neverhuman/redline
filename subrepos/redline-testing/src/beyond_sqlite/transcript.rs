//! How both shells print a result row, and what the comparison claims.
//!
//! The PostgreSQL lane compares SQL-shell transcripts: `psql` in unaligned,
//! tuples-only mode against the `redlinedb` shell in `.mode list`. Until
//! PG-07 the two printed `|` between cells and the text `NULL` for a SQL
//! NULL, so the rows `('a|b', 'c')` and `('a', 'b|c')` printed the same
//! line, and so did NULL and the string `'NULL'`. Both shells now print the
//! ASCII unit separator between cells and a NULL marker wrapped in record
//! separators. Neither character occurs in the corpus (a test checks), so a
//! cell boundary or a NULL in the transcript is one the engine produced.
//!
//! This is still text: a value containing a newline and two rows print the
//! same, and the per-case normalizers act on cells without their SQL types.
//! The result is "normalized SQL-shell transcript agreement", not typed-row
//! or application parity. The typed, lossless comparison is PG-07 proper.

/// Printed between the cells of a row (ASCII unit separator).
pub const FIELD_SEPARATOR: char = '\u{1f}';

/// `FIELD_SEPARATOR` as a string, for `join`.
pub const SEPARATOR: &str = "\u{1f}";

/// Printed for a SQL NULL: `NULL` wrapped in ASCII record separators, so it
/// differs from the text `'NULL'` and from the empty string.
pub const NULL_MARKER: &str = "\u{1e}NULL\u{1e}";

/// Names the transcript framing and verdict rules a target record was
/// produced under. The gate refuses records from any other comparator.
pub const COMPARATOR_VERSION: &str = "redline-beyond-sqlite-comparator-v2";

/// The `redlinedb` shell preamble: list mode, no headers, this framing, and
/// case-sensitive `LIKE`.
///
/// `PRAGMA case_sensitive_like = 1` flips LIKE from its SQLite default
/// (ASCII-fold) to PostgreSQL's case-sensitive behaviour, which is what the
/// PostgreSQL oracle compares against (`LIKE_VS_ILIKE_ASCII`). ILIKE is
/// independent of this pragma and continues to fold case. Single quotes make
/// the shell take each argument verbatim.
pub fn target_preamble() -> String {
    format!(
        ".mode list\n.headers off\n.separator '{FIELD_SEPARATOR}'\n.nullvalue '{NULL_MARKER}'\nPRAGMA case_sensitive_like = 1;\n"
    )
}

/// The `psql` formatting options: unaligned, tuples only, and this framing.
pub fn psql_format_args() -> Vec<String> {
    vec![
        "-F".to_owned(),
        FIELD_SEPARATOR.to_string(),
        "-P".to_owned(),
        format!("null={NULL_MARKER}"),
        "-P".to_owned(),
        "format=unaligned".to_owned(),
        "-P".to_owned(),
        "tuples_only=on".to_owned(),
        "-P".to_owned(),
        format!("fieldsep={FIELD_SEPARATOR}"),
        "-P".to_owned(),
        "border=0".to_owned(),
    ]
}

/// Splits one transcript line into its cells.
pub fn cells(line: &str) -> impl Iterator<Item = &str> {
    line.split(FIELD_SEPARATOR)
}

/// Joins cells back into one transcript line.
#[cfg(test)]
pub fn join<S: AsRef<str>>(cells: &[S]) -> String {
    let mut line = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            line.push(FIELD_SEPARATOR);
        }
        line.push_str(cell.as_ref());
    }
    line
}

/// One row as either shell prints it under this framing.
#[cfg(test)]
pub fn render_row(cells: &[Option<&str>]) -> String {
    join(
        &cells
            .iter()
            .map(|cell| cell.unwrap_or(NULL_MARKER))
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value a shell argument carries after its own quoting rules.
    fn psql_option(name: &str) -> String {
        let args = psql_format_args();
        args.iter()
            .find_map(|arg| arg.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("psql sets {name}"))
            .to_owned()
    }

    fn preamble_argument(command: &str) -> String {
        target_preamble()
            .lines()
            .find_map(|line| line.strip_prefix(command))
            .unwrap_or_else(|| panic!("preamble sets {command}"))
            .trim()
            .strip_prefix('\'')
            .and_then(|arg| arg.strip_suffix('\''))
            .expect("single-quoted argument")
            .to_owned()
    }

    #[test]
    fn both_shells_print_the_same_framing() {
        assert_eq!(SEPARATOR, FIELD_SEPARATOR.to_string());
        assert_eq!(
            preamble_argument(".separator "),
            FIELD_SEPARATOR.to_string()
        );
        assert_eq!(preamble_argument(".nullvalue "), NULL_MARKER);
        assert_eq!(psql_option("fieldsep"), FIELD_SEPARATOR.to_string());
        assert_eq!(psql_option("null"), NULL_MARKER);
        let args = psql_format_args();
        let dash_f = args.iter().position(|arg| arg == "-F").expect("-F");
        assert_eq!(args[dash_f + 1], FIELD_SEPARATOR.to_string());
    }

    #[test]
    fn a_separator_inside_a_value_does_not_move_a_cell_boundary() {
        assert_ne!(
            render_row(&[Some("a|b"), Some("c")]),
            render_row(&[Some("a"), Some("b|c")])
        );
        assert_eq!(
            cells(&render_row(&[Some("a|b"), Some("c")])).collect::<Vec<_>>(),
            ["a|b", "c"]
        );
    }

    #[test]
    fn null_the_text_null_and_the_empty_string_differ() {
        let null = render_row(&[None]);
        let text = render_row(&[Some("NULL")]);
        let empty = render_row(&[Some("")]);
        assert_ne!(null, text);
        assert_ne!(null, empty);
        assert_ne!(text, empty);
        assert_ne!(render_row(&[None, Some("")]), render_row(&[Some(""), None]));
    }

    #[test]
    fn the_corpus_never_contains_the_framing_characters() {
        let manifest = super::super::oracle::MANIFEST;
        assert!(!manifest.contains(FIELD_SEPARATOR));
        assert!(!manifest.contains('\u{1e}'));
        // Escaped forms in the JSON text, and the SQL spellings of both.
        let lowered = manifest.to_ascii_lowercase();
        for spelling in [
            "\\u001f", "\\u001e", "chr(30)", "chr(31)", "char(30)", "char(31)",
        ] {
            assert!(!lowered.contains(spelling), "{spelling}");
        }
    }
}
