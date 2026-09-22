# Review of enum boundaries

HLT-046-UNNECESSARY-VARIETY ("divergent enum definitions") reports seven
pairs. All seven were inspected individually; each is separation by design,
not accidental duplication. The detector matches on enum NAME across
modules and cannot see the distinction. It carries no score weight - the
Code-shape dimension records "copy-code advisory classes found: N
(advisory only, no score impact)".

NOTE: listing the rule in `disabled_rules` in `agent/audit-policy.toml` does
NOT remove these from
the findings list - verified against auditor 1.6.11, where adding either
"HLT-046-UNNECESSARY-VARIETY" or its check id
"HLT-046-UNNECESSARY-VARIETY:copy-code" left soft_findings unchanged at 10
(and an unrecognised entry raised it to 11). The pairs are documented here
instead of suppressed, so the next reader does not re-derive the analysis:

  * `Error` - crates/kernel/src/error.rs (Io, InvalidChecksum: storage
    faults) vs crates/sql/src/error.rs, which already wraps the former in a
    `Kernel(#[from] KernelError)` variant. Merging collapses the kernel/sql
    layering and creates a dependency cycle.
  * `Op` - crates/kernel/src/json/path_bytecode.rs (Root, LoadObjKey,
    LoadArrIdx: a JSON-path VM) vs crates/sql/src/exec/expr/program.rs
    (LoadConstI64, LoadCol, LoadBinding: the scalar-expression VM). Two
    unrelated instruction sets sharing a two-letter name.
  * `Step` - crates/redlinedb/src/iter.rs `Step<'a>` borrows its row for the
    public cursor API; crates/sql/src/statement.rs `Step` is the
    non-borrowing internal signal. The lifetime difference is the point.
  * `JoinKind` - crates/sql/src/planner.rs is the PHYSICAL algorithm
    (NestedLoop, IndexNestedLoop, Hash, Cross); crates/sql/src/statement.rs
    is the LOGICAL join type (Inner, Left, Right, Full). Different axes.
  * `AccessPath` - crates/sql/src/planner.rs is the legacy enum that
    crates/sql/src/planner/access.rs deliberately lowers back to (see its
    comment "lower back to the legacy `super::AccessPath` enum"), while
    crates/sql/src/planner/access_path.rs is the formal IR behind the R2-C
    planner gate. The gated migration keeps both on purpose.
  * `AggKind` - crates/sql/src/exec/morsel/hash_agg.rs is the columnar
    morsel aggregator over `Morsel<'arena>`; crates/sql/src/exec/vec/hash_agg.rs
    is the row-oriented aggregator that spills at `work_mem_bytes`. Each
    enum states what its own engine supports.
  * `Cell` - crates/bench/src/fuzz/normalize.rs (redlinedb-bench) vs the
    `pub(crate)` renderer in crates/cli/src/render.rs. The variants match,
    but the crates are independent (cli has no bench dependency); sharing
    the type would couple the CLI to the benchmark harness.

Re-inspect before adding a pair to this list.

