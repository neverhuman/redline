# RedlineDB Agent Router

`neverhuman/RedlineDB` is the sole source, review and release authority. Everything under
`subrepos/` is an ordinary vendored directory in this repository — never a submodule, never a
separate product repo. Preserve the independent nested Cargo workspaces. `GROK_GAPS.md` carries the
governing decisions and the dependency roadmap (task ids `EVID-*`, `SQL-*`, `ABI-*`, `PG-*`, …); it is
task-ordered, not a numbered PR sequence. Consumer deployment locks and databases are out of scope.

## Coordinate through the board, not through prose

1. `bf claim <paths> -m "what and why"` before editing; a directory ends with `/`. Exit 2 means someone
   else holds it — that overlap is the only real lock, so resolve it rather than working around it.
2. `bf heartbeat <id>` at least every 30 minutes while you hold a claim.
3. `bf release <id> --proof '<command>'` after verification; the command runs and its exit code is recorded.
4. `bf note` for anything you would otherwise have written in chat.
5. `bf` needs a local hub (`bulletfarm serve --local`). If none is reachable, append a timestamped,
   signed claim to `AGENT_CHAT.md` before your first write, and record the release there when done.
6. Announce the canonical checkout branch before its first write. One integrator controls branch changes;
   others may read, review and prepare acceptance cases concurrently.

## Every change is a small PR

- **Contract**: the body carries the claim id and `Acceptance: <test or command that exists in the diff>`.
  Counts, scores and parity numbers are generated, never typed.
- **Correct, do not reject**: a PR that misses its acceptance gets `REVIEW: changes <sha>` and is fixed on
  the same branch. Closing is only for a verified exact duplicate, with the salvaged content credited in
  the survivor. Aim for one independent read before merge; self-merge is permitted once
  `RedlineDB/required` is green, which is the only required context.
- **Parallelism**: at most four open PRs, each one lane, each rebased on a freshly fetched `origin/main`
  (protection is strict, so this is mandatory), each disjoint from the others on the hot files —
  `crates/sql/src/exec/**`, `crates/sql/src/parser*`, `crates/sql/src/{planner,statement}.rs`,
  `crates/redlinedb/src/lib.rs`, `scripts/just/run.sh`, `ops/ci/fast.sh`, `.github/workflows/ci.yml`.
  `.jankurai/` reports, `benchmark-results/**`, `assets/*.svg`, README auto-blocks, `Cargo.lock` and
  `CHANGELOG.md` are rebase-trivial and never block a claim.
- **Proof before merge**: `just fast` is the default lane; `just pr-ci` (= `just required`) is the CI
  mirror. It does not run `scripts/guard-official-evidence.sh`, and local parity has no Postgres oracle
  unless `REDLINE_TESTING_POSTGRES_URL` is set. `ops/git-hooks/pre-push` is the enforced gate.
- **Zero stale PRs**, not zero open PRs: anything older than one working day is fixed or merged.

## Workspace boundary

- Mutate source only in the claimed canonical checkout. Never a sibling clone, archive, backup, resolved
  symlink target, or duplicate root.
- Never create or register a Git worktree anywhere, including under `/tmp`. Exact-SHA isolation may use an
  automatically removed standalone `git clone --no-local` sandbox; it must never be registered as a
  worktree or become a second persistent source of truth.
- Before edits, report `pwd`, `git rev-parse --show-toplevel`, and `git status --short --branch`.
- Preserve source history with Git refs and verified bundles, not copied directories.

Mission: keep invariants local, edit the smallest lawful surface, and preserve raw evidence.

Start here (the `agent/` and `.jankurai/` copies are not mirrors; each has live readers):
- `agent/owner-map.json` · `agent/test-map.json` — what the auditor fingerprints; register new paths here
- `.jankurai/proof-lanes.toml` — the full lane list; `scripts/guard-official-evidence.sh` reads this copy
- `agent/generated-zones.toml`
- `.jankurai/unsafe-ledger.toml`
- `docs/audit-rubric.md` · `docs/language-bad-behavior.md` · `docs/testing.md`
- `docs/architecture.md` · `docs/boundaries.md`

Rules:
- Prefer package-scoped edits over workspace-wide edits.
- Never hand-edit paths listed in `agent/generated-zones.toml`.
- Keep active source files under 2,000 LOC; split or archive anything larger.
- Do not compress away exit codes, failing test names, panic text, spans, advisory IDs, seeds, raw-log paths, or raw-log hashes.
- Treat `just fast` as the default proof lane, then widen only when the edit crosses contract, security, or concurrency boundaries.
- Do not record rationale in `agent/audit-policy.toml`. Editing it changes the policy fingerprint, and
  `jankurai audit --mode ratchet` then fails on `policy_changed` whatever the score. Rationale goes in `docs/`.
- Never let an audit write into the tree: pass `--json`/`--md` to a scratch path with `--no-score-history`.
  A stray `.jankurai/repo-score.md` is a generated-zone violation on the next run.
- A skip is not a pass. A suite reporting `passed` with skipped cases has measured nothing; report skip
  counts beside pass counts.
- Evidence that matters gets committed. `target/` is gitignored and its contents do not survive.

<!-- jankurai merge marker: review and merge canonical guidance for AGENTS.md -->
