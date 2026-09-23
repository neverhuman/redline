# RedlineDB Agent Router

`neverhuman/RedlineDB` is the sole source, review and release authority. Everything under
`subrepos/` is an ordinary vendored directory in this repository — never a submodule, never a
separate product repo. Preserve the independent nested Cargo workspaces. `GROK_GAPS.md` carries the
governing decisions and the dependency roadmap (task ids `EVID-*`, `SQL-*`, `ABI-*`, `PG-*`, …); it is
task-ordered, not a numbered PR sequence. Consumer deployment locks and databases are out of scope.

## Coordinate through the board, not through prose

1. When `bf` reaches a hub, read `bf board` and `bf board --to me` before a claim, an edit, or a handoff.
2. `bf claim <paths> -m "what and why"` before editing; a directory ends with `/`. Exit 2 means someone
   else holds it — that overlap is the only real lock, so resolve it rather than working around it.
3. `bf heartbeat <id>` at least every 30 minutes while you hold a claim.
4. `bf release <id> --proof '<command>'` after verification; the command runs and its exit code is recorded.
5. `bf note -m "…" --to <agent>`, `--re <id>`, or `--pr <url>` for anything you would otherwise have written
   in chat. An unaddressed note is a status line, not a handoff.
6. `bf` needs a local hub (`bulletfarm serve --local`). If none is reachable, append a timestamped
   claim to `AGENT_CHAT.md` before your first write, and record the release there when done. This
   checkout's `AGENT_CHAT.md` is a committed log, so that fallback is real. When the hub is up, the
   board is `bf`, and a hand edit of `AGENT_CHAT.md` is not a claim.
7. Announce the canonical checkout branch before its first write. One integrator controls branch changes.
   Do not `git checkout` another branch in this checkout while it is dirty or a claim is open. Other
   agents may read, review, and prepare acceptance cases. They do not move `HEAD`.
8. Do not kill another agent's session from a script.

## Every change is a small PR

- **Contract**: the body carries the claim id and `Acceptance: <test or command that exists in the diff>`.
  The acceptance command fails on the parent commit and passes on the head. Counts, scores and parity
  numbers are generated, never typed. A Postgres case id leaves
  `metadata/beyond_sqlite/postgres-regression.json` only in the commit whose raw result shows it passed.
- **Correct, do not reject**: a PR that misses its acceptance gets `REVIEW: changes <sha>` and is fixed on
  the same branch. Closing is only for a verified exact duplicate, with the salvaged content credited in
  the survivor. Merge waits for an eligible review of the exact head and for
  `RedlineDB/required` on that same head. The reviewer must not have opened the pull
  request and must not have authored or committed any commit on it. The steps are in
  `docs/testing.md`.
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
- **New behavior in a new file.** `crates/sql/src/exec/mod.rs`, `crates/sql/src/parser.rs`,
  `crates/sql/src/planner.rs`, `crates/sql/src/statement.rs`, and `crates/redlinedb/src/lib.rs` take a
  match arm or a call. The body of a statement, function, or dialect goes in a new module under the
  2,000-line cap, so two lanes are not both editing the same hot file.
- **The built reference is the oracle.** SQLite proof uses the binary from `scripts/sqlite/build-reference.sh`.
  A `sqlite3` on `PATH`, or an older tree under `target/sqlite-reference/` whose compile flags differ, is a
  diagnostic. Postgres proof uses the pinned 16.15 image and settings `160015|C|C|UTC`. A local run that
  disagrees with that oracle does not close a case. The 3.53.1 autoconf parser has no
  `UPDATE`/`DELETE` `ORDER BY` `LIMIT` grammar, so `-DSQLITE_ENABLE_UPDATE_DELETE_LIMIT` on the `cc`
  line does not make cases 00219 and 00220 succeed; see `docs/sqlite-parity.md`.
- **Write down a reference defect.** When a compile flag or a documented command does not change the
  reference shell, record that next to the flag. Do not hide it in a comment that the next agent will
  "fix" by flipping a default.

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
