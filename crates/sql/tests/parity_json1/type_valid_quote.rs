//! JSON1 parity for json_type, json_valid and json_quote.

use super::*;

// ---------------------------------------------------------------------------
// json_type
// ---------------------------------------------------------------------------

#[test]
fn parity_json_type_each_kind() {
    let pair = Pair::new();
    for (label, doc, path) in [
        ("object", r#"'{"x":1}'"#, "'$'"),
        ("array", "'[1,2]'", "'$'"),
        ("integer", "'1'", "'$'"),
        ("real", "'1.5'", "'$'"),
        ("text", r#"'"hi"'"#, "'$'"),
        ("null", "'null'", "'$'"),
        ("true", "'true'", "'$'"),
        ("false", "'false'", "'$'"),
    ] {
        let sql = format!("SELECT json_type({doc}, {path}) -- {label}");
        pair.assert_parity(&sql);
    }
}

// ---------------------------------------------------------------------------
// json_valid
// ---------------------------------------------------------------------------

#[test]
fn parity_json_valid_well_formed() {
    let pair = Pair::new();
    pair.assert_parity(r#"SELECT json_valid('{"a":1}')"#);
}

#[test]
fn parity_json_valid_malformed() {
    let pair = Pair::new();
    pair.assert_parity("SELECT json_valid('not-json')");
}

#[test]
fn parity_json_valid_null() {
    let pair = Pair::new();
    pair.assert_parity("SELECT json_valid(NULL)");
}

#[test]
fn parity_json_extract_set_official_shape() {
    let pair = Pair::new();
    pair.execute("CREATE TABLE docs(id INT PRIMARY KEY, doc TEXT)");
    pair.execute("INSERT INTO docs VALUES (1, json_object('a',40,'b',json_array(1,2,3)))");
    pair.execute(
        "INSERT INTO docs VALUES (2, json_set(json_object('a',0), '$.a', 41, '$.c', 'x'))",
    );
    pair.assert_parity(
        "SELECT id, json_extract(doc,'$.a'), json_type(doc,'$.b'), json_valid(doc) \
         FROM docs ORDER BY id",
    );
    pair.assert_parity("SELECT json_array_length(json_extract(doc,'$.b')) FROM docs WHERE id=1");
}

// ---------------------------------------------------------------------------
// json_quote
// ---------------------------------------------------------------------------

#[test]
fn parity_json_quote_text() {
    let pair = Pair::new();
    pair.assert_parity("SELECT json_quote('hello \"world\"')");
}

#[test]
fn parity_json_quote_integer() {
    let pair = Pair::new();
    pair.assert_parity("SELECT json_quote(42)");
}

#[test]
fn parity_json_quote_null_returns_json_null() {
    let pair = Pair::new();
    // SQLite's json_quote(NULL) returns the literal text "null" (the JSON
    // null), not SQL NULL. Asserting parity covers that surface.
    pair.assert_parity("SELECT json_quote(NULL)");
}
