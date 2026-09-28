//! Rows and CTE names a statement's binding produced, owned by that
//! statement (Q5-09).
//!
//! Binding materializes CTEs, derived tables, views and table-valued
//! functions into rows. A join reads them through a synthetic `TableDef`
//! whose relation id is looked up here, and a subquery bound while the
//! statement runs finds a CTE name here once the binder's scopes are gone.
//! These used to live in per-thread maps that outlived their statement:
//! ids restarted at 1 on every WITH, so a nested WITH overwrote the outer
//! CTE's rows, and a CTE name kept shadowing the real table for every later
//! statement on the thread, whatever its connection or database.
//!
//! Now the preparation of a statement captures what its binding registers
//! into a [`BindEnv`] that the statement owns; each step installs it, and
//! dropping the statement frees the rows. What is registered while no
//! preparation is capturing (a subquery or trigger body bound during a
//! step) lives in a scratch area cleared when the outermost statement call
//! on the thread returns.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use redlinedb_kernel::format::RelId;

use super::cte::CteDef;
use crate::value::SqlValue;

type Rows = Arc<Vec<Vec<SqlValue>>>;

/// What one statement's binding materialized.
#[derive(Default)]
pub(crate) struct BindEnv {
    rows: HashMap<u64, Rows>,
    /// CTE definitions by lowercased name.
    ctes: HashMap<String, CteDef>,
    /// A capture that CTE-name lookups do not look past: a view or trigger
    /// body is bound in a scope of its own.
    barrier: bool,
}

impl BindEnv {
    fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.ctes.is_empty()
    }

    fn cte(&self, name: &str) -> Option<CteDef> {
        self.ctes.get(&name.to_ascii_lowercase()).cloned()
    }
}

impl std::fmt::Debug for BindEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BindEnv")
            .field("relations", &self.rows.len())
            .field("ctes", &self.ctes.keys().collect::<Vec<_>>())
            .finish()
    }
}

thread_local! {
    /// Preparations in progress on this thread, innermost last.
    static CAPTURES: RefCell<Vec<BindEnv>> = const { RefCell::new(Vec::new()) };
    /// Environments of the statements executing on this thread.
    static INSTALLED: RefCell<Vec<Arc<BindEnv>>> = const { RefCell::new(Vec::new()) };
    /// Registrations made while nothing captures.
    static SCRATCH: RefCell<Option<BindEnv>> = const { RefCell::new(None) };
    /// Bumped by every bind-time materialization; see [`materializations`].
    static MATERIALIZATIONS: Cell<u64> = const { Cell::new(0) };
}

/// Synthetic relation ids come from one process-wide counter and are never
/// reused, so no two row sets can ever share an id.
static NEXT_SYNTHETIC_ID: AtomicU64 = AtomicU64::new(1);

/// A fresh synthetic relation id under `tag` (the CTE, view or cross-database
/// namespace in the top 16 bits).
pub(crate) fn next_synthetic_id(tag: u64) -> RelId {
    let n = NEXT_SYNTHETIC_ID.fetch_add(1, Ordering::Relaxed) & 0x0000_FFFF_FFFF_FFFF;
    RelId(tag | n)
}

/// How many bind-time materializations this thread has performed. A
/// preparation that changed it embeds rows read at bind time, so its
/// template must not be served from a statement cache.
pub(crate) fn materializations() -> u64 {
    MATERIALIZATIONS.with(Cell::get)
}

/// Record a bind-time materialization that embeds rows without registering
/// them (a view or CTE inlined as a row source).
pub(crate) fn note_materialization() {
    MATERIALIZATIONS.with(|count| count.set(count.get().wrapping_add(1)));
}

/// Run `f` on the innermost capture, or on the scratch area when nothing
/// captures.
fn with_target<T>(f: impl FnOnce(&mut BindEnv) -> T) -> T {
    let mut f = Some(f);
    let captured = CAPTURES.with(|captures| {
        captures
            .borrow_mut()
            .last_mut()
            .map(|env| (f.take().expect("unused"))(env))
    });
    match captured {
        Some(value) => value,
        None => SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            (f.take().expect("unused"))(scratch.get_or_insert_with(BindEnv::default))
        }),
    }
}

/// Search captures (innermost first), then installed statements (innermost
/// first), then scratch. With `stop_at_barrier`, a barrier capture ends the
/// search after it is searched.
fn find<T>(stop_at_barrier: bool, mut probe: impl FnMut(&BindEnv) -> Option<T>) -> Option<T> {
    let (found, stopped) = CAPTURES.with(|captures| {
        for env in captures.borrow().iter().rev() {
            if let Some(found) = probe(env) {
                return (Some(found), true);
            }
            if stop_at_barrier && env.barrier {
                return (None, true);
            }
        }
        (None, false)
    });
    if stopped {
        return found;
    }
    if let Some(found) = INSTALLED.with(|installed| {
        installed
            .borrow()
            .iter()
            .rev()
            .find_map(|env| probe(env.as_ref()))
    }) {
        return Some(found);
    }
    SCRATCH.with(|scratch| scratch.borrow().as_ref().and_then(&mut probe))
}

/// Publish the rows behind a synthetic relation.
pub(crate) fn register_rows(rel: RelId, rows: Rows) {
    note_materialization();
    with_target(|env| {
        env.rows.insert(rel.0, rows);
    });
}

/// Drop a synthetic relation (a recursive CTE's per-iteration working table).
pub(crate) fn deregister_rows(rel: RelId) {
    with_target(|env| {
        env.rows.remove(&rel.0);
    });
}

/// The rows behind a synthetic relation.
pub(crate) fn rows_for_relation(rel: RelId) -> Option<Rows> {
    // Relation ids are unique, so rows are visible across barriers.
    find(false, |env| env.rows.get(&rel.0).cloned())
}

/// Make a CTE name resolvable by subqueries bound after the binder's scope
/// for it is gone, for the rest of the owning statement.
pub(crate) fn register_cte(def: &CteDef) {
    note_materialization();
    with_target(|env| {
        env.ctes.insert(def.name.to_ascii_lowercase(), def.clone());
    });
}

/// A CTE the running or preparing statement defined, by name. Inside a
/// view or trigger body only the body's own CTEs are visible.
pub(crate) fn lookup_cte(name: &str) -> Option<CteDef> {
    find(true, |env| env.cte(name))
}

/// Forget scratch registrations. Called when the outermost statement call
/// on the thread (a prepare or a step) returns.
pub(crate) fn clear_scratch() {
    SCRATCH.with(|scratch| {
        scratch.borrow_mut().take();
    });
}

/// Captures what one preparation registers. Dropping it unfinished (an
/// error path) discards the capture.
pub(crate) struct Capture {
    depth: usize,
    finished: bool,
}

impl Capture {
    pub(crate) fn begin() -> Self {
        Self::push(BindEnv::default())
    }

    /// A capture for a view or trigger body: CTE names registered outside
    /// it are invisible inside it. Dropping it discards what it captured.
    pub(crate) fn barrier() -> Self {
        Self::push(BindEnv {
            barrier: true,
            ..BindEnv::default()
        })
    }

    fn push(env: BindEnv) -> Self {
        let depth = CAPTURES.with(|captures| {
            let mut captures = captures.borrow_mut();
            captures.push(env);
            captures.len()
        });
        Self {
            depth,
            finished: false,
        }
    }

    /// The captured environment; `None` when the binding registered nothing.
    pub(crate) fn finish(mut self) -> Option<Arc<BindEnv>> {
        self.finished = true;
        let env = CAPTURES.with(|captures| {
            let mut captures = captures.borrow_mut();
            debug_assert_eq!(captures.len(), self.depth, "captures must nest");
            captures.truncate(self.depth);
            captures.pop()
        })?;
        (!env.is_empty()).then(|| Arc::new(env))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if !self.finished {
            CAPTURES.with(|captures| captures.borrow_mut().truncate(self.depth - 1));
        }
    }
}

/// Installs a statement's environment for one step.
pub(crate) struct Installed {
    pushed: bool,
}

impl Installed {
    pub(crate) fn install(env: Option<&Arc<BindEnv>>) -> Self {
        let pushed = env.is_some();
        if let Some(env) = env {
            INSTALLED.with(|installed| installed.borrow_mut().push(Arc::clone(env)));
        }
        Self { pushed }
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        if self.pushed {
            INSTALLED.with(|installed| {
                installed.borrow_mut().pop();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(value: i64) -> Rows {
        Arc::new(vec![vec![SqlValue::Integer(value)]])
    }

    #[test]
    fn a_capture_owns_what_it_registers_and_installs_it_later() {
        let rel = next_synthetic_id(0xC7E0_0000_0000_0000);
        let capture = Capture::begin();
        register_rows(rel, rows(11));
        assert_eq!(rows_for_relation(rel), Some(rows(11)));
        let env = capture.finish().expect("registered rows");
        assert_eq!(rows_for_relation(rel), None, "owned by the statement");
        {
            let _step = Installed::install(Some(&env));
            assert_eq!(rows_for_relation(rel), Some(rows(11)));
        }
        assert_eq!(rows_for_relation(rel), None);
        // Nothing registered: no environment.
        assert!(Capture::begin().finish().is_none());
    }

    #[test]
    fn ids_are_never_reused_and_scratch_is_cleared() {
        let tag = 0xC1E0_0000_0000_0000;
        let (a, b) = (next_synthetic_id(tag), next_synthetic_id(tag));
        assert_ne!(a, b);
        register_rows(a, rows(1));
        assert_eq!(rows_for_relation(a), Some(rows(1)));
        clear_scratch();
        assert_eq!(rows_for_relation(a), None);
        // An abandoned capture discards its registrations.
        {
            let _capture = Capture::begin();
            register_rows(b, rows(2));
        }
        assert_eq!(rows_for_relation(b), None);
        assert!(CAPTURES.with(|captures| captures.borrow().is_empty()));
    }

    #[test]
    fn a_barrier_hides_outer_cte_names_but_not_rows() {
        let def = |name: &str| CteDef {
            name: Arc::from(name),
            columns: Arc::from(Vec::<String>::new()),
            rows: Arc::from(Vec::<Vec<SqlValue>>::new()),
            table_def: None,
        };
        let outer = Capture::begin();
        register_cte(&def("Outer"));
        let rel = next_synthetic_id(0xC7E0_0000_0000_0000);
        register_rows(rel, rows(3));
        assert!(lookup_cte("outer").is_some());
        {
            let _body = Capture::barrier();
            assert!(lookup_cte("outer").is_none());
            assert_eq!(rows_for_relation(rel), Some(rows(3)));
            register_cte(&def("inner"));
            assert!(lookup_cte("INNER").is_some());
        }
        assert!(lookup_cte("inner").is_none());
        assert!(lookup_cte("outer").is_some());
        drop(outer);
    }
}
