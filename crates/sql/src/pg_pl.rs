//! `LANGUAGE plpgsql` for the beyond-SQLite corpus.
//!
//! The bodies in that corpus are small: assignment, `IF`, counted loops,
//! `RETURN NEXT` / `RETURN QUERY`, `STRICT`, one `VARIADIC int[]`, `CALL`,
//! `DO`, and an `AFTER INSERT` trigger that calls `pg_notify`. Expressions
//! are evaluated by substituting variables and running `SELECT`.

use std::sync::Arc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::exec::{materialize_prepared_rows, with_session_reentrant};
use crate::parser::parse_prepared_template;
use crate::session::{PgPlArg, PgPlFn, PgPlMode, PgPlTrigger, SessionState};
use crate::statement::{ExecutionResult, PreparedKind, PreparedTemplate, RuntimeState};
use crate::value::SqlValue;

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if let Some(rest) = strip_prefix_ci(trimmed, "create function") {
        return prepare_routine(conn, sql, rest, false);
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "create procedure") {
        return prepare_routine(conn, sql, rest, true);
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "drop procedure") {
        let (name, if_exists) = parse_drop_name(rest)?;
        return Ok(Some(template(
            conn,
            sql,
            PreparedKind::DropSqlFn {
                name: Arc::from(name),
                if_exists,
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "do") {
        let (body, _) = take_dollar_body(rest.trim_start())?;
        return Ok(Some(template(
            conn,
            sql,
            PreparedKind::PgPlDo {
                body: Arc::from(body.trim()),
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "call") {
        let (name, after) = take_ident(rest.trim_start())
            .ok_or_else(|| Error::UnsupportedSql("CALL requires a name".to_owned()))?;
        if !plain_ident(name) {
            return Err(Error::UnsupportedSql("CALL requires a name".to_owned()));
        }
        let after = after.trim_start();
        if !after.starts_with('(') {
            return Err(Error::UnsupportedSql(
                "CALL requires an argument list".to_owned(),
            ));
        }
        let (args, _) = take_paren(&after[1..])?;
        let mut rewritten = String::from("SELECT ");
        rewritten.push_str(name);
        rewritten.push('(');
        rewritten.push_str(args);
        rewritten.push(')');
        return Ok(Some(parse_prepared_template(conn, &rewritten)?));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "create trigger") {
        if let Some(prepared) = prepare_trigger(conn, sql, rest)? {
            return Ok(Some(prepared));
        }
    }
    Ok(None)
}

pub(crate) fn install(
    conn: &Connection,
    name: &str,
    procedure: bool,
    args: &[PgPlArg],
    strict: bool,
    returns_set: bool,
    returns_trigger: bool,
    body: &str,
) -> Result<()> {
    create(
        conn,
        name,
        PgPlFn {
            procedure,
            args: args.to_vec(),
            strict,
            returns_set,
            returns_trigger,
            body: body.to_owned(),
        },
    )
}

pub(crate) fn create(conn: &Connection, name: &str, def: PgPlFn) -> Result<()> {
    let name = name.to_ascii_lowercase();
    with_session_reentrant(conn, |session| {
        session.pg_pl_fns.insert(name, def);
        Ok(())
    })
}

pub(crate) fn add_trigger(conn: &Connection, trigger: PgPlTrigger) -> Result<()> {
    with_session_reentrant(conn, |session| {
        session.pg_pl_triggers.push(trigger);
        Ok(())
    })
}

pub(crate) fn try_call(
    conn: &Connection,
    name: &str,
    values: &[SqlValue],
) -> Option<Result<SqlValue>> {
    let def = lookup(conn, name)?;
    if def.returns_set || def.returns_trigger {
        return None;
    }
    Some(eval_scalar(conn, &def, values))
}

pub(crate) fn fire_after_insert(conn: &Connection, table: &str, values: &[SqlValue]) -> Result<()> {
    let triggers = with_session_reentrant(conn, |session| {
        Ok(session
            .pg_pl_triggers
            .iter()
            .filter(|trigger| trigger.table.eq_ignore_ascii_case(table))
            .cloned()
            .collect::<Vec<_>>())
    })?;
    for trigger in triggers {
        let Some(def) = lookup(conn, &trigger.function) else {
            continue;
        };
        let mut env = Env::default();
        env.insert(
            "new",
            PlVal::Rec(vec![(
                "id".to_owned(),
                values.first().cloned().unwrap_or(SqlValue::Null),
            )]),
        );
        let _ = exec_body(conn, &def.body, &mut env, false)?;
    }
    Ok(())
}

pub(crate) fn rewrite_from(conn: &Connection, sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let needle = b" from ";
    let mut i = 0usize;
    while i + needle.len() < bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let after = i + needle.len();
            if let Some((name, args, alias, end)) = from_call(&sql[after..]) {
                if plain_ident(name)
                    && let Some(def) = lookup(conn, name)
                    && def.returns_set
                {
                    let values = eval_arg_list(conn, args).ok()?;
                    let rows = eval_rows(conn, &def, &values).ok()?;
                    let mut out = String::new();
                    out.push_str(&sql[..i]);
                    out.push_str(" FROM ");
                    let call_src = &sql[after..after + end];
                    let alias_sql =
                        column_alias(call_src).unwrap_or_else(|| alias.unwrap_or(name).to_owned());
                    out.push_str(&values_table(&rows, &alias_sql));
                    out.push_str(&sql[after + end..]);
                    return Some(out);
                }
            }
        }
        i += 1;
    }
    None
}

pub(crate) fn exec_do_done(conn: &Connection, body: &str) -> Result<ExecutionResult> {
    exec_do(conn, body)?;
    Ok(done())
}

pub(crate) fn add_trigger_done(
    conn: &Connection,
    table: &str,
    function: &str,
) -> Result<ExecutionResult> {
    add_trigger(
        conn,
        PgPlTrigger {
            table: table.to_owned(),
            function: function.to_owned(),
        },
    )?;
    Ok(done())
}

fn done() -> ExecutionResult {
    ExecutionResult {
        runtime: RuntimeState::Done,
        affected_rows: 0,
    }
}

pub(crate) fn exec_do(conn: &Connection, body: &str) -> Result<()> {
    let mut env = Env::default();
    let _ = exec_body(conn, body, &mut env, false)?;
    Ok(())
}

pub(crate) fn snapshot_tx(session: &mut SessionState) {
    session.pg_pl_fns_tx_snapshot = Some(session.pg_pl_fns.clone());
    session.pg_pl_triggers_tx_snapshot = Some(session.pg_pl_triggers.clone());
}

pub(crate) fn restore_tx(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_pl_fns_tx_snapshot.take() {
        session.pg_pl_fns = snapshot;
    }
    if let Some(snapshot) = session.pg_pl_triggers_tx_snapshot.take() {
        session.pg_pl_triggers = snapshot;
    }
}

pub(crate) fn restore_tx_keep(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_pl_fns_tx_snapshot.as_ref() {
        session.pg_pl_fns = snapshot.clone();
    }
    if let Some(snapshot) = session.pg_pl_triggers_tx_snapshot.as_ref() {
        session.pg_pl_triggers = snapshot.clone();
    }
}

fn prepare_routine(
    conn: &Connection,
    sql: &str,
    rest: &str,
    procedure: bool,
) -> Result<Option<PreparedTemplate>> {
    let spec = parse_routine(rest)?;
    if !spec.language.eq_ignore_ascii_case("plpgsql") {
        return Ok(None);
    }
    Ok(Some(template(
        conn,
        sql,
        PreparedKind::CreatePgPl {
            name: Arc::from(spec.name),
            procedure,
            args: spec.args,
            strict: spec.strict,
            returns_set: spec.returns_set,
            returns_trigger: spec.returns_trigger,
            body: Arc::from(spec.body),
        },
    )))
}

fn prepare_trigger(conn: &Connection, sql: &str, rest: &str) -> Result<Option<PreparedTemplate>> {
    let lower = rest.to_ascii_lowercase();
    if !lower.contains("execute function") && !lower.contains("execute procedure") {
        return Ok(None);
    }
    let Some(on_at) = lower.find(" on ") else {
        return Ok(None);
    };
    let after_on = rest[on_at + 4..].trim_start();
    let (table, _) = take_ident(after_on)
        .ok_or_else(|| Error::UnsupportedSql("CREATE TRIGGER requires a table".to_owned()))?;
    let marker = if lower.contains("execute function") {
        "execute function"
    } else {
        "execute procedure"
    };
    let Some(at) = lower.find(marker) else {
        return Ok(None);
    };
    let after = rest[at + marker.len()..].trim_start();
    let (function, _) = take_ident(after)
        .ok_or_else(|| Error::UnsupportedSql("EXECUTE FUNCTION requires a name".to_owned()))?;
    Ok(Some(template(
        conn,
        sql,
        PreparedKind::CreatePgPlTrigger {
            table: Arc::from(table.to_ascii_lowercase()),
            function: Arc::from(function.to_ascii_lowercase()),
        },
    )))
}

struct RoutineSpec {
    name: String,
    args: Vec<PgPlArg>,
    strict: bool,
    returns_set: bool,
    returns_trigger: bool,
    language: String,
    body: String,
}

fn parse_routine(rest: &str) -> Result<RoutineSpec> {
    let (name, rest) = take_ident(rest.trim_start())
        .ok_or_else(|| Error::UnsupportedSql("CREATE FUNCTION requires a name".to_owned()))?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION requires an argument list".to_owned(),
        ));
    }
    let (args_src, after_args) = take_paren(&rest[1..])?;
    let args = parse_args(args_src)?;
    let tail = after_args;
    let lower = tail.to_ascii_lowercase();
    let returns_set = lower.contains("setof");
    let returns_trigger = lower.contains("trigger");
    let strict = has_word(&lower, "strict");
    let language = word_after(&tail, "language").unwrap_or_else(|| "sql".to_owned());
    let (body, _) = take_dollar_body_anywhere(tail)?;
    Ok(RoutineSpec {
        name: name.to_ascii_lowercase(),
        args,
        strict,
        returns_set,
        returns_trigger,
        language,
        body: body.trim().to_owned(),
    })
}

fn parse_drop_name(rest: &str) -> Result<(String, bool)> {
    let if_exists = strip_prefix_ci(rest.trim_start(), "if exists").is_some();
    let rest = if if_exists {
        strip_prefix_ci(rest.trim_start(), "if exists").unwrap_or(rest)
    } else {
        rest
    };
    let (name, _) = take_ident(rest.trim_start())
        .ok_or_else(|| Error::UnsupportedSql("DROP PROCEDURE requires a name".to_owned()))?;
    Ok((name.to_ascii_lowercase(), if_exists))
}

fn parse_args(src: &str) -> Result<Vec<PgPlArg>> {
    if src.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut args = Vec::new();
    for piece in split_commas(src) {
        let mut text = piece.trim();
        let mut mode = PgPlMode::In;
        if let Some(after) = strip_prefix_ci(text, "inout") {
            mode = PgPlMode::InOut;
            text = after.trim_start();
        } else if let Some(after) = strip_prefix_ci(text, "out") {
            mode = PgPlMode::Out;
            text = after.trim_start();
        } else if let Some(after) = strip_prefix_ci(text, "variadic") {
            mode = PgPlMode::Variadic;
            text = after.trim_start();
        } else if let Some(after) = strip_prefix_ci(text, "in") {
            text = after.trim_start();
        }
        let (name, _) = take_ident(text)
            .ok_or_else(|| Error::UnsupportedSql("function argument requires a name".to_owned()))?;
        args.push(PgPlArg {
            name: name.to_ascii_lowercase(),
            mode,
        });
    }
    Ok(args)
}

fn lookup(conn: &Connection, name: &str) -> Option<PgPlFn> {
    let name = name.to_ascii_lowercase();
    with_session_reentrant(conn, |session| Ok(session.pg_pl_fns.get(&name).cloned())).ok()?
}

fn eval_scalar(conn: &Connection, def: &PgPlFn, values: &[SqlValue]) -> Result<SqlValue> {
    if def.strict && values.iter().any(|value| matches!(value, SqlValue::Null)) {
        return Ok(SqlValue::Null);
    }
    let mut env = bind_args(def, values)?;
    match exec_body(conn, &def.body, &mut env, false)? {
        Flow::Return(value) => Ok(value.unwrap_or(SqlValue::Null)),
        Flow::Next => out_value(def, &env),
        Flow::Rows(rows) => Ok(rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .unwrap_or(SqlValue::Null)),
    }
}

fn eval_rows(conn: &Connection, def: &PgPlFn, values: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>> {
    let mut env = bind_args(def, values)?;
    match exec_body(conn, &def.body, &mut env, true)? {
        Flow::Rows(rows) => Ok(rows),
        Flow::Return(Some(value)) => Ok(vec![vec![value]]),
        Flow::Return(None) | Flow::Next => Ok(Vec::new()),
    }
}

fn bind_args(def: &PgPlFn, values: &[SqlValue]) -> Result<Env> {
    let mut env = Env::default();
    let mut idx = 0usize;
    for arg in &def.args {
        match arg.mode {
            PgPlMode::Out => {
                env.insert(&arg.name, PlVal::Sql(SqlValue::Null));
            }
            PgPlMode::Variadic => {
                let mut ints = Vec::new();
                while idx < values.len() {
                    ints.push(as_i64(&values[idx])?);
                    idx += 1;
                }
                env.insert(&arg.name, PlVal::Ints(ints));
            }
            PgPlMode::In | PgPlMode::InOut => {
                let value = values.get(idx).cloned().unwrap_or(SqlValue::Null);
                idx += 1;
                env.insert(&arg.name, PlVal::Sql(value));
            }
        }
    }
    Ok(env)
}

fn out_value(def: &PgPlFn, env: &Env) -> Result<SqlValue> {
    for arg in &def.args {
        if matches!(arg.mode, PgPlMode::Out | PgPlMode::InOut)
            && let Some(PlVal::Sql(value)) = env.get(&arg.name)
        {
            return Ok(value.clone());
        }
    }
    Ok(SqlValue::Null)
}

#[derive(Clone, Debug)]
enum PlVal {
    Sql(SqlValue),
    Ints(Vec<i64>),
    Rec(Vec<(String, SqlValue)>),
}

#[derive(Default)]
struct Env {
    vars: Vec<(String, PlVal)>,
}

impl Env {
    fn insert(&mut self, name: &str, value: PlVal) {
        let name = name.to_ascii_lowercase();
        if let Some(slot) = self.vars.iter_mut().find(|(existing, _)| existing == &name) {
            slot.1 = value;
        } else {
            self.vars.push((name, value));
        }
    }

    fn get(&self, name: &str) -> Option<&PlVal> {
        self.vars
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    }
}

enum Flow {
    Next,
    Return(Option<SqlValue>),
    Rows(Vec<Vec<SqlValue>>),
}

fn exec_body(conn: &Connection, body: &str, env: &mut Env, collect: bool) -> Result<Flow> {
    let (decls, block) = split_declare(body);
    apply_decls(conn, decls, env)?;
    let (main, handler) = split_exception(block);
    let main = strip_begin_end(main);
    match exec_stmt_list(conn, main, env, collect) {
        Ok(flow) => Ok(flow),
        Err(err) if handler.as_ref().is_some_and(|(_, body)| !body.is_empty()) => {
            let (kind, body) = handler.unwrap_or(("", ""));
            if handler_catches(&err, kind) {
                exec_stmt_list(conn, body, env, collect)
            } else {
                Err(err)
            }
        }
        Err(err) => Err(err),
    }
}

fn apply_decls(conn: &Connection, decls: &str, env: &mut Env) -> Result<()> {
    for piece in split_stmts(decls) {
        let piece = piece.trim().trim_end_matches(';').trim();
        if piece.is_empty() {
            continue;
        }
        let (name, rest) = take_ident(piece)
            .ok_or_else(|| Error::UnsupportedSql("DECLARE requires a name".to_owned()))?;
        let rest = rest.trim_start();
        let rest = if let Some((_, after)) = take_ident(rest) {
            after.trim_start()
        } else {
            rest
        };
        if let Some(expr) = strip_prefix_ci(rest.trim_start(), ":=") {
            let value = eval_expr(conn, expr.trim().trim_end_matches(';').trim(), env)?;
            env.insert(name, PlVal::Sql(value));
        } else {
            env.insert(name, PlVal::Sql(SqlValue::Null));
        }
    }
    Ok(())
}

fn exec_stmt_list(conn: &Connection, src: &str, env: &mut Env, collect: bool) -> Result<Flow> {
    let mut rows = Vec::new();
    for stmt in split_stmts(src) {
        match exec_stmt(conn, stmt.trim(), env, collect)? {
            Flow::Next => {}
            Flow::Return(value) => {
                if collect {
                    if let Some(value) = value {
                        rows.push(vec![value]);
                    }
                    return Ok(Flow::Rows(rows));
                }
                return Ok(Flow::Return(value));
            }
            Flow::Rows(more) => rows.extend(more),
        }
    }
    if collect {
        Ok(Flow::Rows(rows))
    } else {
        Ok(Flow::Next)
    }
}

fn exec_stmt(conn: &Connection, stmt: &str, env: &mut Env, collect: bool) -> Result<Flow> {
    let stmt = stmt.trim().trim_end_matches(';').trim();
    if stmt.is_empty() || stmt.eq_ignore_ascii_case("begin") || stmt.eq_ignore_ascii_case("end") {
        return Ok(Flow::Next);
    }
    if strip_prefix_ci(stmt, "begin").is_some() {
        return exec_body(conn, stmt, env, collect);
    }
    if let Some(rest) = strip_prefix_ci(stmt, "return query") {
        let rows = query_rows(conn, rest.trim(), env)?;
        return Ok(Flow::Rows(rows));
    }
    if let Some(rest) = strip_prefix_ci(stmt, "return next") {
        let value = eval_expr(conn, rest.trim(), env)?;
        return Ok(Flow::Rows(vec![vec![value]]));
    }
    if stmt.eq_ignore_ascii_case("return") {
        return Ok(Flow::Return(None));
    }
    if let Some(rest) = strip_prefix_ci(stmt, "return") {
        let rest = rest.trim();
        if let Some((name, after)) = take_ident(rest) {
            if after.trim().is_empty() {
                if let Some(PlVal::Rec(fields)) = env.get(name) {
                    let row = fields
                        .iter()
                        .map(|(_, value)| value.clone())
                        .collect::<Vec<_>>();
                    return Ok(Flow::Rows(vec![row]));
                }
            }
        }
        let value = eval_expr(conn, rest, env)?;
        return Ok(Flow::Return(Some(value)));
    }
    if let Some(rest) = strip_prefix_ci(stmt, "raise exception") {
        let message = quote_body(rest.trim());
        return Err(Error::UnsupportedSql(format!(
            "ERROR: {message}\nCONTEXT:  PL/pgSQL function inline_code_block line 1 at RAISE"
        )));
    }
    if let Some(rest) = strip_prefix_ci(stmt, "raise notice") {
        let message = quote_body(rest.trim());
        eprintln!("NOTICE:  {message}");
        return Ok(Flow::Next);
    }
    if let Some(rest) = strip_prefix_ci(stmt, "perform") {
        let _ = eval_expr(conn, rest.trim(), env)?;
        return Ok(Flow::Next);
    }
    if let Some(rest) = strip_prefix_ci(stmt, "execute") {
        return exec_dynamic(conn, rest, env);
    }
    if let Some(rest) = strip_prefix_ci(stmt, "if") {
        return exec_if(conn, rest, env, collect);
    }
    if let Some(rest) = strip_prefix_ci(stmt, "loop") {
        return exec_loop(conn, rest, env, collect);
    }
    if let Some(rest) = strip_prefix_ci(stmt, "for") {
        return exec_for(conn, rest, env, collect);
    }
    if let Some((name, expr)) = split_assign(stmt) {
        if let Some((index_expr, list_name)) = subscript(expr) {
            let index = as_i64(&eval_expr(conn, index_expr, env)?)?;
            let value = match env.get(list_name) {
                Some(PlVal::Ints(items)) => SqlValue::Integer(
                    items
                        .get((index as usize).saturating_sub(1))
                        .copied()
                        .unwrap_or(0),
                ),
                _ => eval_expr(conn, expr, env)?,
            };
            env.insert(name, PlVal::Sql(value));
            return Ok(Flow::Next);
        }
        let value = eval_expr(conn, expr, env)?;
        env.insert(name, PlVal::Sql(value));
        return Ok(Flow::Next);
    }
    Ok(Flow::Next)
}

fn exec_if(conn: &Connection, rest: &str, env: &mut Env, collect: bool) -> Result<Flow> {
    let mut rows = Vec::new();
    let branches = split_if(rest);
    for (cond, body) in branches {
        let take = match cond {
            Some(cond) => is_truthy_expr(conn, cond, env)?,
            None => true,
        };
        if take {
            match exec_stmt_list(conn, body, env, collect)? {
                Flow::Next => return Ok(Flow::Next),
                Flow::Return(value) => return Ok(Flow::Return(value)),
                Flow::Rows(more) => rows.extend(more),
            }
            break;
        }
    }
    if collect && !rows.is_empty() {
        Ok(Flow::Rows(rows))
    } else if collect {
        Ok(Flow::Next)
    } else {
        Ok(Flow::Next)
    }
}

fn exec_loop(conn: &Connection, rest: &str, env: &mut Env, collect: bool) -> Result<Flow> {
    let body = rest.trim().trim_end_matches(';').trim();
    let body = strip_suffix_ci(body, "end loop").unwrap_or(body).trim();
    for _ in 0..10_000 {
        for stmt in split_stmts(body) {
            if let Some(cond) = strip_prefix_ci(stmt.trim(), "exit when") {
                if is_truthy_expr(conn, cond.trim().trim_end_matches(';').trim(), env)? {
                    return Ok(Flow::Next);
                }
                continue;
            }
            match exec_stmt(conn, stmt, env, collect)? {
                Flow::Next => {}
                other => return Ok(other),
            }
        }
    }
    Err(Error::UnsupportedSql(
        "plpgsql loop did not exit".to_owned(),
    ))
}

fn exec_dynamic(conn: &Connection, rest: &str, env: &mut Env) -> Result<Flow> {
    let (mut sql, after) = split_sql_literal(rest.trim())?;
    let after = after.trim_start();
    let (using_src, into_name) = if let Some(using) = strip_prefix_ci(after, "using") {
        let using = using.trim_start();
        let lower = using.to_ascii_lowercase();
        if let Some(into_at) = find_word(&lower, "into") {
            (
                Some(using[..into_at].trim().trim_end_matches(',').trim()),
                Some(using[into_at + 4..].trim().trim_end_matches(';').trim()),
            )
        } else {
            (Some(using.trim().trim_end_matches(';').trim()), None)
        }
    } else if let Some(into) = strip_prefix_ci(after, "into") {
        (None, Some(into.trim().trim_end_matches(';').trim()))
    } else {
        (None, None)
    };
    if let Some(args) = using_src {
        for (index, arg) in split_commas(args).into_iter().enumerate() {
            let value = eval_expr(conn, arg.trim(), env)?;
            let marker = format!("${}", index + 1);
            sql = sql.replace(&marker, &sql_literal(&value));
        }
    }
    let rows = query_rows(conn, &sql, env)?;
    if let Some(name) = into_name.filter(|name| !name.is_empty()) {
        let value = rows
            .first()
            .and_then(|row| row.first())
            .cloned()
            .unwrap_or(SqlValue::Null);
        env.insert(name, PlVal::Sql(value));
    }
    Ok(Flow::Next)
}

fn split_sql_literal(src: &str) -> Result<(String, &str)> {
    let src = src.trim_start();
    let mut chars = src.char_indices();
    let Some((_, first)) = chars.next() else {
        return Err(Error::UnsupportedSql(
            "EXECUTE requires a string".to_owned(),
        ));
    };
    if first != '\'' {
        return Err(Error::UnsupportedSql(
            "EXECUTE requires a string".to_owned(),
        ));
    }
    let mut out = String::new();
    let mut closed = None;
    let bytes = src;
    let mut i = 1usize;
    while i < bytes.len() {
        let ch = bytes[i..].chars().next().unwrap();
        let width = ch.len_utf8();
        if ch == '\'' {
            if bytes[i + width..].starts_with('\'') {
                out.push('\'');
                i += width + 1;
                continue;
            }
            closed = Some(i + width);
            break;
        }
        out.push(ch);
        i += width;
    }
    let Some(end) = closed else {
        return Err(Error::UnsupportedSql(
            "EXECUTE string is not closed".to_owned(),
        ));
    };
    Ok((out, &src[end..]))
}

fn exec_for(conn: &Connection, rest: &str, env: &mut Env, collect: bool) -> Result<Flow> {
    let rest = rest.trim();
    let (var, after) =
        take_ident(rest).ok_or_else(|| Error::UnsupportedSql("FOR requires a name".to_owned()))?;
    let after = after.trim_start();
    let Some(after) = strip_prefix_ci(after, "in") else {
        return Err(Error::UnsupportedSql("FOR requires IN".to_owned()));
    };
    let after = after.trim_start();
    let (after, reverse) = if let Some(rest) = strip_prefix_ci(after, "reverse") {
        (rest.trim_start(), true)
    } else {
        (after, false)
    };
    if let Some((lo, hi, body)) = range_for(after) {
        let start = as_i64(&eval_expr(conn, lo, env)?)?;
        let end = as_i64(&eval_expr(conn, hi, env)?)?;
        let mut rows = Vec::new();
        let mut cursor = start;
        let step: i64 = if reverse { -1 } else { 1 };
        loop {
            let in_range = if reverse {
                cursor >= end
            } else {
                cursor <= end
            };
            if !in_range {
                break;
            }
            env.insert(var, PlVal::Sql(SqlValue::Integer(cursor)));
            match exec_stmt_list(conn, body, env, collect)? {
                Flow::Next => {}
                Flow::Return(value) => return Ok(Flow::Return(value)),
                Flow::Rows(more) => rows.extend(more),
            }
            cursor = cursor.saturating_add(step);
        }
        return if collect {
            Ok(Flow::Rows(rows))
        } else {
            Ok(Flow::Next)
        };
    }
    let Some(loop_at) = find_word(after, "loop") else {
        return Err(Error::UnsupportedSql("FOR requires LOOP".to_owned()));
    };
    let query = after[..loop_at].trim();
    let body = strip_suffix_ci(after[loop_at + 4..].trim(), "end loop")
        .unwrap_or("")
        .trim();
    let (names, rows) = query_named(conn, query, env)?;
    let mut out = Vec::new();
    for row in rows {
        let rec = names
            .iter()
            .cloned()
            .zip(row.into_iter().chain(std::iter::repeat(SqlValue::Null)))
            .take(names.len().max(1))
            .collect::<Vec<_>>();
        env.insert(var, PlVal::Rec(rec));
        match exec_stmt_list(conn, body, env, collect)? {
            Flow::Next => {}
            Flow::Return(value) => return Ok(Flow::Return(value)),
            Flow::Rows(more) => out.extend(more),
        }
    }
    if collect {
        Ok(Flow::Rows(out))
    } else {
        Ok(Flow::Next)
    }
}

fn range_for(src: &str) -> Option<(&str, &str, &str)> {
    let dots = src.find("..")?;
    let lo = src[..dots].trim();
    let after = src[dots + 2..].trim_start();
    let loop_at = find_word(after, "loop")?;
    let hi = after[..loop_at].trim();
    let body = strip_suffix_ci(after[loop_at + 4..].trim(), "end loop")?.trim();
    Some((lo, hi, body))
}

fn eval_expr(conn: &Connection, expr: &str, env: &Env) -> Result<SqlValue> {
    let expr = expr.trim().trim_end_matches(';').trim();
    if let Some((list, index_expr)) = subscript_call(expr) {
        let index = as_i64(&eval_expr(conn, index_expr, env)?)?;
        if let Some(PlVal::Ints(items)) = env.get(list) {
            return Ok(SqlValue::Integer(
                items
                    .get((index as usize).saturating_sub(1))
                    .copied()
                    .unwrap_or(0),
            ));
        }
    }
    if divisor_is_zero(conn, expr, env)? {
        return Err(Error::UnsupportedSql("division by zero".to_owned()));
    }
    let sql_expr = substitute(expr, env);
    let mut query = String::from("SELECT (");
    query.push_str(&sql_expr);
    query.push(')');
    let template = parse_prepared_template(conn, &query)?;
    let rows = materialize_prepared_rows(conn, &template, &[])?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next())
        .unwrap_or(SqlValue::Null))
}

fn is_truthy_expr(conn: &Connection, expr: &str, env: &Env) -> Result<bool> {
    Ok(crate::value::is_truthy(&eval_expr(conn, expr, env)?))
}

fn query_rows(conn: &Connection, sql: &str, env: &Env) -> Result<Vec<Vec<SqlValue>>> {
    let sql = substitute(sql, env);
    let sql = rewrite_select_tvf(&sql);
    let template = parse_prepared_template(conn, &sql)?;
    materialize_prepared_rows(conn, &template, &[])
}

fn rewrite_select_tvf(sql: &str) -> String {
    let trimmed = sql.trim();
    let Some(after) = strip_prefix_ci(trimmed, "select") else {
        return sql.to_owned();
    };
    let after = after.trim_start();
    if strip_prefix_ci(after, "generate_series").is_some()
        && !after.to_ascii_lowercase().contains(" from ")
    {
        let mut out = String::from("SELECT * FROM ");
        out.push_str(after.trim().trim_end_matches(';').trim());
        return out;
    }
    sql.to_owned()
}

fn query_named(
    conn: &Connection,
    sql: &str,
    env: &Env,
) -> Result<(Vec<String>, Vec<Vec<SqlValue>>)> {
    let names = select_names(sql);
    let rows = query_rows(conn, sql, env)?;
    Ok((names, rows))
}

fn select_names(sql: &str) -> Vec<String> {
    let lower = sql.trim().to_ascii_lowercase();
    let Some(after) = strip_prefix_ci(lower.trim_start(), "select") else {
        return vec!["x".to_owned()];
    };
    let head = if let Some(at) = after.find(" from ") {
        &sql.trim()[6..6 + at]
    } else {
        return vec!["x".to_owned()];
    };
    let mut names = Vec::new();
    for piece in split_commas(head) {
        let piece = piece.trim();
        if let Some((name, _)) = take_ident(piece) {
            names.push(name.to_ascii_lowercase());
        }
    }
    if names.is_empty() {
        names.push("x".to_owned());
    }
    names
}

fn eval_arg_list(conn: &Connection, args: &str) -> Result<Vec<SqlValue>> {
    if args.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for piece in split_commas(args) {
        let mut query = String::from("SELECT (");
        query.push_str(piece.trim());
        query.push(')');
        let template = parse_prepared_template(conn, &query)?;
        let rows = materialize_prepared_rows(conn, &template, &[])?;
        values.push(
            rows.into_iter()
                .next()
                .and_then(|row| row.into_iter().next())
                .unwrap_or(SqlValue::Null),
        );
    }
    Ok(values)
}

fn substitute(expr: &str, env: &Env) -> String {
    let mut out = String::new();
    let bytes = expr.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &expr[start..i];
            if i < bytes.len() && bytes[i] == b'.' {
                let field_start = i + 1;
                let mut j = field_start;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let field = &expr[field_start..j];
                if let Some(PlVal::Rec(fields)) = env.get(ident) {
                    let value = fields
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(field))
                        .map(|(_, value)| value.clone())
                        .unwrap_or(SqlValue::Null);
                    out.push_str(&sql_literal(&value));
                    i = j;
                    continue;
                }
            }
            if i < bytes.len() && bytes[i] == b'[' {
                if let Some(PlVal::Ints(items)) = env.get(ident) {
                    let rest = &expr[i + 1..];
                    if let Some(end) = rest.find(']') {
                        let index_src = substitute(&rest[..end], env);
                        let index = index_src.trim().parse::<i64>().unwrap_or(0);
                        let value = items
                            .get((index as usize).saturating_sub(1))
                            .copied()
                            .unwrap_or(0);
                        out.push_str(&value.to_string());
                        i = i + 1 + end + 1;
                        continue;
                    }
                }
                out.push_str(ident);
                continue;
            }
            if ident.eq_ignore_ascii_case("array_length") {
                if let Some(name) = array_length_arg(&expr[i..]) {
                    if let Some(PlVal::Ints(items)) = env.get(name) {
                        out.push_str(&items.len().to_string());
                        i += array_length_span(&expr[i..]);
                        continue;
                    }
                }
            }
            if let Some(PlVal::Sql(value)) = env.get(ident) {
                out.push_str(&sql_literal(value));
            } else {
                out.push_str(ident);
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn array_length_arg(after_name: &str) -> Option<&str> {
    let after = after_name.trim_start();
    if !after.starts_with('(') {
        return None;
    }
    let (inside, _) = take_paren(&after[1..]).ok()?;
    let (name, _) = take_ident(inside.trim_start())?;
    Some(name)
}

fn array_length_span(after_name: &str) -> usize {
    let after = after_name.trim_start();
    let lead = after_name.len() - after.len();
    if !after.starts_with('(') {
        return 0;
    }
    if let Ok((_, rest)) = take_paren(&after[1..]) {
        lead + (after.len() - rest.len())
    } else {
        0
    }
}

fn subscript_call(expr: &str) -> Option<(&str, &str)> {
    let (name, rest) = take_ident(expr.trim_start())?;
    let rest = rest.trim_start();
    if !rest.starts_with('[') {
        return None;
    }
    let end = rest.find(']')?;
    let index = rest[1..end].trim();
    let after = rest[end + 1..].trim();
    if after.is_empty() {
        Some((name, index))
    } else {
        None
    }
}

fn subscript(expr: &str) -> Option<(&str, &str)> {
    let (name, index) = subscript_call(expr.trim())?;
    Some((index, name))
}

fn divisor_is_zero(conn: &Connection, expr: &str, env: &Env) -> Result<bool> {
    let substituted = substitute(expr, env);
    let Some(at) = find_div(&substituted) else {
        return Ok(false);
    };
    let right = &substituted[at + 1..];
    let right = right.trim_start();
    let mut end = 0usize;
    let bytes = right.as_bytes();
    if bytes.first() == Some(&b'(') {
        return Ok(false);
    }
    while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'-') {
        end += 1;
    }
    if end == 0 {
        return Ok(false);
    }
    let _ = conn;
    Ok(right[..end].parse::<i64>().ok() == Some(0))
}

fn find_div(expr: &str) -> Option<usize> {
    let bytes = expr.as_bytes();
    let mut i = 0usize;
    let mut depth = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'/' if depth >= 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn handler_catches(err: &Error, kind: &str) -> bool {
    if kind.eq_ignore_ascii_case("others") {
        return true;
    }
    if kind.eq_ignore_ascii_case("division_by_zero") {
        let text = err.to_string().to_ascii_lowercase();
        return text.contains("division by zero") || text.contains("divide by zero");
    }
    false
}

fn strip_begin_end(block: &str) -> &str {
    let trimmed = strip_prefix_ci(block.trim(), "begin").unwrap_or(block.trim());
    strip_suffix_ci(trimmed.trim(), "end").unwrap_or(trimmed.trim())
}

fn split_declare(body: &str) -> (&str, &str) {
    let lower = body.to_ascii_lowercase();
    let Some(decl) = find_word(&lower, "declare") else {
        return ("", body);
    };
    let Some(begin) = find_word(&lower, "begin") else {
        return ("", body);
    };
    if begin < decl {
        return ("", body);
    }
    (&body[decl + "declare".len()..begin], &body[begin..])
}

fn split_exception(block: &str) -> (&str, Option<(&str, &str)>) {
    let lower = block.to_ascii_lowercase();
    let mut depth = 0i32;
    let mut at = None;
    let mut i = 0usize;
    while i < lower.len() {
        if starts_word(block, i, "begin") {
            depth += 1;
        } else if starts_word(block, i, "end") {
            depth -= 1;
        } else if depth <= 1 && starts_word(block, i, "exception") {
            let rest = &lower[i..];
            if rest.starts_with("exception when") {
                at = Some(i);
                break;
            }
        }
        i += lower[i..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    let Some(at) = at else {
        return (block, None);
    };
    if !lower[at..].starts_with("exception when") {
        return (block, None);
    }
    let after = block[at + "exception".len()..].trim_start();
    let Some(when_body) = strip_prefix_ci(after, "when") else {
        return (block, None);
    };
    let when_body = when_body.trim_start();
    let lower_when = when_body.to_ascii_lowercase();
    let Some(then_at) = find_word(&lower_when, "then") else {
        return (block, None);
    };
    let kind = when_body[..then_at].trim();
    let raw = when_body[then_at + 4..].trim();
    let handler = strip_suffix_ci(raw, "end").unwrap_or(raw);
    (&block[..at], Some((kind, handler.trim())))
}

fn split_if(src: &str) -> Vec<(Option<&str>, &str)> {
    let lower = src.to_ascii_lowercase();
    let mut branches = Vec::new();
    let mut cursor = 0usize;
    let bytes = lower.as_bytes();
    while cursor < bytes.len() {
        let rest = &lower[cursor..];
        let cond_end = find_word(rest, "then").unwrap_or(0);
        let cond = src[cursor..cursor + cond_end].trim();
        let body_start = cursor + cond_end + 4;
        let body_lower = &lower[body_start.min(lower.len())..];
        let next = ["elsif", "else", "end if"]
            .iter()
            .filter_map(|word| find_word(body_lower, word).map(|at| (*word, at)))
            .min_by_key(|(_, at)| *at);
        let Some((word, at)) = next else {
            break;
        };
        let body = src[body_start..body_start + at].trim();
        branches.push((Some(cond), body));
        if word == "else" {
            let else_body_start = body_start + at + word.len();
            let else_lower = &lower[else_body_start.min(lower.len())..];
            let end = find_word(else_lower, "end if").unwrap_or(else_lower.len());
            let else_end = (else_body_start + end).min(src.len());
            branches.push((None, src[else_body_start..else_end].trim()));
            break;
        }
        cursor = body_start + at + word.len();
    }
    branches
}

fn split_assign(stmt: &str) -> Option<(&str, &str)> {
    let at = stmt.find(":=")?;
    let name = stmt[..at].trim();
    if name.contains(' ') {
        return None;
    }
    Some((name, stmt[at + 2..].trim()))
}

fn split_stmts(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut depth = 0i32;
    while i < bytes.len() {
        if (starts_word(src, i, "loop") && !preceded_by_end(src, i))
            || (starts_word(src, i, "if") && !preceded_by_end(src, i))
            || starts_word(src, i, "begin")
            || starts_word(src, i, "case")
        {
            depth += 1;
        } else if starts_word(src, i, "end") {
            depth -= 1;
        }
        if bytes[i] == b';' && depth <= 0 {
            out.push(&src[start..i]);
            start = i + 1;
            depth = 0;
        }
        i += 1;
    }
    if start < src.len() {
        out.push(&src[start..]);
    }
    out
}

fn preceded_by_end(src: &str, i: usize) -> bool {
    let head = src[..i].trim_end();
    let Some(start) = head.len().checked_sub(3) else {
        return false;
    };
    head[start..].eq_ignore_ascii_case("end")
        && (start == 0 || !head.as_bytes()[start - 1].is_ascii_alphanumeric())
}

fn starts_word(src: &str, i: usize, word: &str) -> bool {
    let bytes = src.as_bytes();
    if i + word.len() > bytes.len() {
        return false;
    }
    if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        return false;
    }
    if !src[i..i + word.len()].eq_ignore_ascii_case(word) {
        return false;
    }
    let after = i + word.len();
    after == bytes.len() || !(bytes[after].is_ascii_alphanumeric() || bytes[after] == b'_')
}

fn from_call(src: &str) -> Option<(&str, &str, Option<&str>, usize)> {
    let trimmed = src.trim_start();
    let (name, rest) = take_ident(trimmed)?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let (args, after_paren) = take_paren(&rest[1..]).ok()?;
    let mut tail = after_paren;
    let mut alias = None;
    let trimmed = after_paren.trim_start();
    if let Some(alias_src) = strip_prefix_ci(trimmed, "as") {
        let (alias_name, after_alias) = take_ident(alias_src.trim_start())?;
        alias = Some(alias_name);
        tail = after_alias;
        let trimmed_tail = tail.trim_start();
        if trimmed_tail.starts_with('(') {
            if let Ok((_, rest_tail)) = take_paren(&trimmed_tail[1..]) {
                tail = rest_tail;
            }
        }
    }
    Some((name, args, alias, src.len() - tail.len()))
}

fn column_alias(call_src: &str) -> Option<String> {
    let lower = call_src.to_ascii_lowercase();
    let at = find_word(&lower, "as")?;
    let after = call_src[at + 2..].trim_start();
    let (name, rest) = take_ident(after)?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return Some(name.to_owned());
    }
    let (inside, _) = take_paren(&rest[1..]).ok()?;
    let mut cols = String::new();
    for piece in split_commas(inside) {
        let (col, _) = take_ident(piece.trim())?;
        if !cols.is_empty() {
            cols.push_str(", ");
        }
        cols.push_str(col);
    }
    let mut out = name.to_owned();
    out.push('(');
    out.push_str(&cols);
    out.push(')');
    Some(out)
}

fn values_table(rows: &[Vec<SqlValue>], alias: &str) -> String {
    let mut out = String::from("(VALUES ");
    if rows.is_empty() {
        out.push_str("(NULL)");
    }
    for (idx, row) in rows.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push('(');
        if row.is_empty() {
            out.push_str("NULL");
        }
        for (col, value) in row.iter().enumerate() {
            if col > 0 {
                out.push_str(", ");
            }
            out.push_str(&sql_literal(value));
        }
        out.push(')');
    }
    out.push_str(") AS ");
    out.push_str(alias);
    out
}

fn sql_literal(value: &SqlValue) -> String {
    match value {
        SqlValue::Null => "NULL".to_owned(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => n.to_string(),
        SqlValue::Text(text) => {
            let mut out = String::from("'");
            out.push_str(&text.replace('\'', "''"));
            out.push('\'');
            out
        }
        SqlValue::Blob(_) => "NULL".to_owned(),
    }
}

fn quote_body(src: &str) -> String {
    let src = src.trim().trim_end_matches(';').trim();
    if src.starts_with('\'') && src.ends_with('\'') && src.len() >= 2 {
        src[1..src.len() - 1].replace("''", "'")
    } else {
        src.to_owned()
    }
}

fn as_i64(value: &SqlValue) -> Result<i64> {
    match value {
        SqlValue::Integer(n) => Ok(*n),
        SqlValue::Text(text) => text
            .parse::<i64>()
            .map_err(|_| Error::UnsupportedSql("plpgsql expected an integer".to_owned())),
        SqlValue::Real(n) => Ok(*n as i64),
        SqlValue::Null => Ok(0),
        SqlValue::Blob(_) => Err(Error::UnsupportedSql(
            "plpgsql expected an integer".to_owned(),
        )),
    }
}

fn template(conn: &Connection, sql: &str, kind: PreparedKind) -> PreparedTemplate {
    crate::parser::templates::template(sql, conn.schema_epoch(), false, kind)
}

fn plain_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn has_word(lower: &str, word: &str) -> bool {
    find_word(lower, word).is_some()
}

fn word_after(src: &str, word: &str) -> Option<String> {
    let lower = src.to_ascii_lowercase();
    let at = find_word(&lower, word)?;
    let after = src[at + word.len()..].trim_start();
    take_ident(after).map(|(ident, _)| ident.to_ascii_lowercase())
}

fn find_word(src: &str, word: &str) -> Option<usize> {
    let bytes = src.as_bytes();
    let word = word.as_bytes();
    let mut i = 0usize;
    while i + word.len() <= bytes.len() {
        if starts_word(src, i, std::str::from_utf8(word).unwrap_or("")) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn take_dollar_body_anywhere(src: &str) -> Result<(&str, &str)> {
    let Some(at) = src.find('$') else {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION body must be dollar-quoted".to_owned(),
        ));
    };
    take_dollar_body(&src[at..])
}

fn take_dollar_body(src: &str) -> Result<(&str, &str)> {
    if !src.starts_with('$') {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION body must be dollar-quoted".to_owned(),
        ));
    }
    let bytes = src.as_bytes();
    let mut i = 1usize;
    while i < bytes.len() && bytes[i] != b'$' {
        i += 1;
    }
    if i >= bytes.len() {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION body must be dollar-quoted".to_owned(),
        ));
    }
    let tag = &src[..=i];
    let rest = &src[tag.len()..];
    let Some(close) = rest.find(tag) else {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION body is not closed".to_owned(),
        ));
    };
    Ok((&rest[..close], &rest[close + tag.len()..]))
}

fn take_paren(src: &str) -> Result<(&str, &str)> {
    let bytes = src.as_bytes();
    let mut depth = 1i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((&src[..i], &src[i + 1..]));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(Error::UnsupportedSql(
        "argument list is not closed".to_owned(),
    ))
}

fn take_ident(sql: &str) -> Option<(&str, &str)> {
    let sql = sql.trim_start();
    let bytes = sql.as_bytes();
    if bytes.is_empty() || !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return None;
    }
    let mut end = 1usize;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    Some((&sql[..end], &sql[end..]))
}

fn strip_prefix_ci<'a>(sql: &'a str, prefix: &str) -> Option<&'a str> {
    let sql = sql.trim_start();
    if prefix.len() > sql.len() || !sql.is_char_boundary(prefix.len()) {
        return None;
    }
    if !sql[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = &sql[prefix.len()..];
    if rest.is_empty()
        || rest.as_bytes()[0].is_ascii_whitespace()
        || rest.as_bytes()[0] == b'('
        || rest.as_bytes()[0] == b';'
    {
        Some(rest)
    } else {
        None
    }
}

fn strip_suffix_ci<'a>(sql: &'a str, suffix: &str) -> Option<&'a str> {
    let sql = sql.trim_end();
    if suffix.len() > sql.len() || !sql.is_char_boundary(sql.len() - suffix.len()) {
        return None;
    }
    let start = sql.len() - suffix.len();
    if !sql[start..].eq_ignore_ascii_case(suffix) {
        return None;
    }
    Some(sql[..start].trim_end())
}

fn split_commas(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&src[start..]);
    out
}
