#!/usr/bin/env bash
# SQLite and PostgreSQL parity report dispatcher.
#
# Keeps the workflow YAML thin while preserving a local entrypoint for
# regenerating README/report artifacts from verified redline-testing evidence.
#
# Usage:
#   bash ops/ci/sqlite-parity-report.sh update
#   bash ops/ci/sqlite-parity-report.sh publish-pr

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

# shellcheck source=ops/ci/lib.sh
. "$repo_root/ops/ci/lib.sh"

report_branch="${REDLINEDB_SQLITE_PARITY_REPORT_BRANCH:-automation/sqlite-parity-report}"
base_branch="${REDLINEDB_SQLITE_PARITY_REPORT_BASE:-main}"
commit_message="${REDLINEDB_SQLITE_PARITY_REPORT_COMMIT_MESSAGE:-Update SQLite and PostgreSQL parity reports}"
pr_title="${REDLINEDB_SQLITE_PARITY_REPORT_PR_TITLE:-Update SQLite and PostgreSQL parity reports}"

report_paths=(
  README.md
  .github/parity-report-inputs.sha256
  .jankurai/repo-score.json
  .jankurai/repo-score.md
  .jankurai/score-history.jsonl
  .jankurai/score-history.csv
  assets/sqlite-parity-latency-gap.svg
  assets/sqlite-parity-performance-histogram.svg
  assets/sqlite-median-test-performance.svg
  benchmark-results/sqlite-parity/latest
  paper/data/loc_comparison.csv
  paper/sections/abstract.tex
  paper/sections/conclusion.tex
  paper/sections/evaluation.tex
  paper/sections/implementation.tex
  paper/sections/introduction.tex
)

ensure_jankurai() {
  if command -v jankurai >/dev/null 2>&1; then
    return 0
  fi
  mkdir -p .jankurai/sqlite-parity-report
  ci_install_jankurai_logged .jankurai/sqlite-parity-report/install.log
}

ensure_sqlite_parity_reference() {
  if [ -n "${REDLINEDB_SQLITE_PARITY_SQLITE_BIN:-}" ]; then
    return 0
  fi
  REDLINEDB_SQLITE_PARITY_SQLITE_BIN="$(bash scripts/sqlite/build-reference.sh)"
  export REDLINEDB_SQLITE_PARITY_SQLITE_BIN
}

run_update() {
  ensure_jankurai
  ensure_sqlite_parity_reference
  if command -v just >/dev/null 2>&1; then
    just sqlite-parity-report-update
  else
    bash scripts/just/run.sh sqlite-parity-report-update
  fi
  # The README block is published only from release evidence: the commit
  # being reported, a clean source tree, and a measured reference image.
  target/release/redline-testing check-postgres \
    --input target/redline-testing/beyond_sqlite.raw.jsonl \
    --baseline metadata/beyond_sqlite/postgres-regression.json --readme README.md \
    --expected-source-commit "$(git rev-parse HEAD)" --require-clean
}

ensure_publish_pr_tools() {
  if ! command -v gh >/dev/null 2>&1; then
    printf 'gh CLI is required for publish-pr\n' >&2
    return 127
  fi
  if [ -z "${GH_TOKEN:-}" ] && [ -z "${GITHUB_TOKEN:-}" ]; then
    printf 'GH_TOKEN or GITHUB_TOKEN is required for publish-pr\n' >&2
    return 1
  fi
}

configure_git_author() {
  git config user.name "${GIT_AUTHOR_NAME:-redlinedb-report-bot}"
  git config user.email "${GIT_AUTHOR_EMAIL:-41898282+github-actions[bot]@users.noreply.github.com}"
}

publish_pr() {
  ensure_publish_pr_tools
  # One active change: reuse the report PR, defer while any other PR is open.
  if gh pr list --state open --json headRefName --jq '.[].headRefName' | grep -vFx "$report_branch" >/dev/null; then
    printf 'Deferring report: another pull request is open.\n'
    return 0
  fi
  configure_git_author

  git fetch origin "$base_branch"
  if git ls-remote --exit-code --heads origin "$report_branch" >/dev/null 2>&1; then
    git fetch origin "$report_branch"
    git switch -C "$report_branch" "origin/${report_branch}"
    git merge --no-edit "origin/${base_branch}"
  else
    git switch -C "$report_branch" "origin/${base_branch}"
  fi

  # Generated reports, dates, charts, and README edits do not change these
  # inputs. In particular, merging this report cannot spawn another report.
  # The runner records the same hash as source_inputs_sha256 in its run
  # provenance; SOURCE_INPUT_PATHS in
  # subrepos/redline-testing/src/evidence/identity.rs must list these paths.
  local inputs
  inputs=$(git ls-tree -r HEAD -- Cargo.toml Cargo.lock rust-toolchain.toml crates subrepos metadata ops scripts \
    agent/audit-policy.toml .jankurai/audit-policy.toml .github/workflows/ci.yml \
    .github/workflows/sqlite-parity-report.yml | sha256sum | cut -d ' ' -f 1)
  if [ -f .github/parity-report-inputs.sha256 ] && [ "$(cat .github/parity-report-inputs.sha256)" = "$inputs" ]; then
    printf 'Parity product, corpus, oracle, and policy inputs are unchanged.\n'
    return 0
  fi
  export REDLINEDB_BENCH_GIT_SHA="$(git rev-parse HEAD)"
  run_update
  printf '%s\n' "$inputs" > .github/parity-report-inputs.sha256

  git add -- "${report_paths[@]}"

  if git diff --cached --quiet; then
    printf 'sqlite parity report outputs are already current\n'
    return 0
  fi

  git commit -m "$commit_message"
  git push origin "HEAD:refs/heads/${report_branch}"

  mkdir -p target/tmp
  pr_body="$(mktemp "$repo_root/target/tmp/sqlite-parity-report-pr.XXXXXX.md")"
  trap 'rm -f "$pr_body"' RETURN
  cat > "$pr_body" <<'BODY'
Automated SQLite and PostgreSQL corpus reports, generated from hash-verified evidence.
Known PostgreSQL failures remain failures and are checked against the reviewed regression policy.

Generated by `ops/ci/sqlite-parity-report.sh publish-pr`.
BODY

  pr_number="$(
    gh pr list \
      --head "$report_branch" \
      --base "$base_branch" \
      --state open \
      --json number \
      --jq '.[0].number // empty'
  )"
  if [ -n "$pr_number" ]; then
    gh api --method PATCH "repos/{owner}/{repo}/pulls/$pr_number" \
      -f base="$base_branch" -f title="$pr_title" -F body=@"$pr_body" >/dev/null
  else
    gh pr create \
      --base "$base_branch" \
      --head "$report_branch" \
      --title "$pr_title" \
      --body-file "$pr_body"
  fi
  pr_number=$(gh pr list --head "$report_branch" --base "$base_branch" --state open --json number --jq '.[0].number')
  # Token-created PRs may not emit pull_request workflows. Dispatch normal CI;
  # its report-merge job merges this exact head only after every gate passes.
  # Return immediately so this report job releases its self-hosted runner.
  gh workflow run ci.yml --ref "$report_branch"
}

command="${1:?command required: update or publish-pr}"
case "$command" in
  update)
    run_update
    ;;
  publish-pr)
    publish_pr
    ;;
  *)
    printf 'unknown sqlite parity report command: %s\n' "$command" >&2
    exit 1
    ;;
esac
