//! Pre-parse SQL rewrites that lower SQLite/Postgres surface syntax into
//! shapes `sqlparser` and the executor already understand.

mod dml_limit;
mod identity_opts;
mod pg_ddl;
mod pg_empty;
mod pg_types;
mod scan;
mod sqlite_shape;
mod virtual_table;

pub(crate) use dml_limit::*;
pub(crate) use identity_opts::*;
pub(crate) use pg_ddl::*;
pub(crate) use pg_empty::*;
pub(crate) use pg_types::*;
pub(crate) use scan::*;
pub(crate) use sqlite_shape::*;
pub(crate) use virtual_table::*;

use super::code_mask::{KeepBrackets, rewrite_code_only};
use super::code_scan::{replace_code, sql_code_contains_ci};
use crate::value::postgres_result_dialect;

/// Lower SQLite and Postgres surface syntax the parser lacks. A pass that
/// searches for its trigger words gates on [`sql_code_contains_ci`] and
/// matches code only, so the same words in a literal, quoted identifier or
/// comment are never rewritten (Q5-01). A pass that scans the text with its
/// own quote tracking runs through [`rewrite_code_only`], which hides quoted
/// names, comments and Postgres string forms from it.
pub(crate) fn rewrite_sqlite_compat_syntax(sql: &str) -> String {
    let mut out = sql.to_owned();
    if sql_code_contains_ci(&out, b" window win as ")
        && let Some(spec) = extract_named_window_spec(&out, "win")
    {
        out = replace_code(&out, "OVER win", &format!("OVER ({spec})"));
        out = strip_window_clause(&out, "win");
    }
    if has_window_exclude(&out) {
        out = rewrite_window_exclude(&out);
    }
    if contains_on_conflict_clause(&out) {
        out = rewrite_code_only(&out, KeepBrackets::No, |sql| {
            rewrite_on_conflict_clauses(&wrap_insert_select_with_upsert(sql))
        });
    }
    if contains_ignore_ascii_case(&out, b"glob") {
        out = rewrite_code_only(&out, KeepBrackets::No, rewrite_glob_to_function);
    }
    if out.contains("NULL IS NOT 1") {
        out = replace_code(&out, "NULL IS NOT 1", "NULL IS DISTINCT FROM 1");
    }
    if has_jsonb_question_op(&out) {
        out = rewrite_code_only(&out, KeepBrackets::Array, rewrite_jsonb_question_ops);
    }
    if sql_code_contains_ci(&out, b"using ") {
        out = strip_create_index_using_clause(&out);
    }
    if contains_ignore_ascii_case(&out, b"strict") {
        out = rewrite_code_only(&out, KeepBrackets::No, rewrite_strict_without_rowid_combo);
    }
    out = rewrite_create_sequence_options_order(&out);
    out = rewrite_identity_sequence_options(&out);
    if sql_code_contains_ci(&out, b"drop identity") {
        out = rewrite_alter_column_drop_identity(&out);
    }
    if sql_code_contains_ci(&out, b"overriding") {
        out = rewrite_overriding_system_value(&out);
    }
    if has_pg_array_literal(&out) {
        out = rewrite_code_only(&out, KeepBrackets::Array, rewrite_pg_array_literal);
    }
    // `'\x41'` is the text `\x41` in SQLite; only Postgres reads hex bytea.
    if postgres_result_dialect() && has_pg_bytea_literal(&out) {
        out = rewrite_pg_bytea_literal(&out);
    }
    if contains_ignore_ascii_case(&out, b"array_length(") {
        out = rewrite_array_length_function(&out);
    }
    if contains_ignore_ascii_case(&out, b"array_agg(") {
        out = rewrite_array_agg_function(&out);
    }
    if out.contains("&&") {
        out = rewrite_pg_array_overlap(&out);
    }
    if has_postfix_index(&out) {
        out = rewrite_code_only(&out, KeepBrackets::Subscripts, rewrite_postfix_index);
    }
    if sql_code_contains_ci(&out, b"at time zone") {
        out = rewrite_at_time_zone(&out);
    }
    if sql_code_contains_ci(&out, b"interval ") {
        out = rewrite_pg_interval_literal(&out);
    }
    if out.contains("'+") || out.contains("'-") {
        out = rewrite_code_only(&out, KeepBrackets::No, rewrite_date_arith_with_modifier);
    }
    if sql_code_contains_ci(&out, b" into ") {
        out = rewrite_select_into_to_ctas(&out);
    }
    if sql_code_contains_ci(&out, b" group by rollup ")
        || sql_code_contains_ci(&out, b" group by cube ")
    {
        out = rewrite_rollup_cube_to_grouping_sets(&out);
    }
    if sql_code_contains_ci(&out, b" group by grouping sets ") {
        out = rewrite_grouping_sets_to_union_all(&out);
    }
    if contains_ignore_ascii_case(&out, b" join lateral ")
        || contains_ignore_ascii_case(&out, b",lateral ")
    {
        out = rewrite_join_lateral_to_subquery(&out);
    }
    if dml_order_limit_rewrite_enabled()
        && contains_ignore_ascii_case(&out, b"limit ")
        && (contains_ignore_ascii_case(&out, b"delete ")
            || contains_ignore_ascii_case(&out, b"update "))
    {
        out = rewrite_dml_order_limit_to_subquery(&out);
    }
    out
}

#[cfg(test)]
#[path = "quoted_form_tests.rs"]
mod quoted_form_tests;
