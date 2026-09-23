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
  request and must not have authored or committed any commit on it. The steps and the
  credential paths are in Publication and review.
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

## Publication and review

Two logins, one job each. `/home/ubuntu/.local/bin/gh-role` loads the named existing
credential into the child `gh` process. It leaves the global active account alone and
it does not print the credential. A `GH_TOKEN` already exported in the parent shell is
cleared for that child. The git commit email does not choose the API user; a push that
uses the git helper still needs `gh-role` for `gh pr` and `gh api`.

| Role | Login | Use it for |
| --- | --- | --- |
| `writer` | `jepsontaylor` | New commits, pushes, `pr create`, `pr merge`. Approves only a pull request this login did not open and did not author or commit. |
| `reviewer` | `neverhuman` | `APPROVE` when that login did not open the pull request and did not author or commit any commit on it. |

New commits use `Jepson Taylor <130782313+jepsontaylor@users.noreply.github.com>`.

Credential files. Query identity with the commands below. Leave the file contents unread:
do not `cat` them, do not `source` them, and do not run `gh auth token` in a shell whose
output is kept. The writer file ends in `.env` and is still a raw token, not a shell script.

| Path | Mode | Role |
| --- | --- | --- |
| `/home/ubuntu/.local/bin/gh-role` | `0700` | Router. Arguments start with `writer` or `reviewer`. |
| `/home/ubuntu/.config/gh/hosts.yml` | `0600` | GitHub CLI account store. A user name in this file is not a working role until `gh-role` can show that login. |
| `/home/ubuntu/.config/jopedime/secrets/gh.env` | `0600` | Writer token consumed by the git helper. |
| `/home/ubuntu/.config/jopedime/bin/github-writer-credential.py` | `0700` | Git HTTPS `get` helper. It answers only for `neverhuman/RedlineDB` and `veox-ai/JopeDime`, as `jepsontaylor`. |
| `/etc/jope-runner/github-pat` | root-owned, not readable by `ubuntu` | Runner registration. It cannot approve a pull request. Leave it unchanged. |

`/home/ubuntu/.config/gh/hosts.yml` also names `jeryu` and `jepsont`. Neither has a
usable token (`gh auth token --user` fails). They are not reviewers. `jeppsontaylor`
(two p's) lives on the operator Mac and is not installed here. Do not copy it onto
this host. The two working logins are `jepsontaylor` and `neverhuman`.

Preflight, with the full path so a noninteractive shell does not pick up another `gh`:

```sh
/home/ubuntu/.local/bin/gh-role writer api user --jq .login
/home/ubuntu/.local/bin/gh-role reviewer api user --jq .login
/home/ubuntu/.local/bin/gh-role eligible reviewer neverhuman/RedlineDB <PR>
/home/ubuntu/.local/bin/gh-role preflight --repo neverhuman/RedlineDB
```

The first two logins are `jepsontaylor` and `neverhuman`. `eligible` prints who is
excluded and why. Exit 0 means that role may approve. `reviewer pr create` is refused.

Order for `neverhuman/RedlineDB`:

1. `just pr-ci` exits 0 on the commit that will be pushed. `just fast` is only the
   iteration lane. Push that one green head. A red local lane stays unpushed, so GitHub
   is not asked to re-run the same failure.
2. Open the pull request as the writer:
   `gh-role writer pr create --repo neverhuman/RedlineDB --body-file <prepared-body>`.
3. The reviewer reads the diff at the full head SHA. An eligible reviewer approves that
   SHA, after the push:

```sh
/home/ubuntu/.local/bin/gh-role reviewer api --method POST \
  repos/neverhuman/RedlineDB/pulls/<PR>/reviews \
  -f commit_id=<full sha> -f event=APPROVE -f body="$(cat <review-file>)"
```

The helper refuses the approval when the selected login opened the pull request or
authored or committed any commit. When both working logins are in that set, stop.
`jeryu` and `jepsont` are not a third reviewer. A commit whose `author.login` is null
(the `jekko` commits use `bot@jekko.ai`) is unassociated. Find out who produced it,
then add `--ack-unassociated` to the same command. The flag records that check. It
does not invent a login.

4. Rewriting author or committer so a login becomes eligible is not a review.
5. The writer merges after `RedlineDB/required` is success on that same SHA. No admin
   bypass:

```sh
/home/ubuntu/.local/bin/gh-role writer pr merge <PR> --repo neverhuman/RedlineDB \
  --rebase --delete-branch --match-head-commit <full sha>
```

The writer pushes. An eligible reviewer approves after that push, because a new push
dismisses the previous approval. `main` keeps one required approval, dismissal of stale
reviews, approval of the latest push, enforcement for admins, strict `RedlineDB/required`,
linear history, and a ban on force-push and deletion. JopeDime `main` protection still
returns HTTP 403 until a paid plan is active, so a review there is not yet enforced by
the server.

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
