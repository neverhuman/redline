# CI trust boundary

The repository is public, and anyone can open a pull request from a fork. CI
uses two kinds of runner:

- **GitHub-hosted** (`ubuntu-24.04`, and the `packages.yml` matrix): a fresh
  virtual machine for every job, discarded afterwards. `RedlineDB/required`,
  `lint`, `official-evidence-guard`, `typecheck`, `test`, `components`
  (testing, central, web, release-tools), `security` and `audit` run here for
  every event, so the required check reports even when the self-hosted
  runners are down or cannot reach github.com (CI-04).
- **Self-hosted** (`redline-xbabe1`, `redline-xbabe3`, labels
  `[self-hosted, Linux, X64]`): long-lived hosts for the heavy jobs:
  `preflight`, the `tests` shards, `parity`, `components (integration)` and
  the parity report bot. Every job runs as the
  runner's host user and shares `$RUNNER_TOOL_CACHE/redlinedb-cargo`
  (registry, git and advisory caches, cargo-installed tools) and the user's
  home directory. `ops/ci/apt-install.sh` calls `sudo` when a package is
  missing, and the parity Postgres service needs Docker. Anything one job
  leaves there, a later job may run, including the trusted jobs that hold
  write tokens: the parity report bot (release `publish` and `report-merge`
  run on hosted runners).

So code from a fork must never run on a self-hosted runner.

## Who is trusted

| Event | Class | Runs on |
| --- | --- | --- |
| `push` to `main`, release tags, `workflow_dispatch` | trusted | heavy jobs self-hosted, the rest hosted |
| `pull_request` from a branch of this repository | trusted | heavy jobs self-hosted, the rest hosted |
| `pull_request` from a fork (head repository is not this one, or was deleted) | untrusted | GitHub-hosted only |

Pushing a branch here needs write access, and write access can already change
the workflows, so a same-repository pull request is no less trusted than a
push.

The fork test used everywhere is:

```text
github.event_name == 'pull_request' &&
github.event.pull_request.head.repo.full_name != github.repository
```

## What the workflows enforce

`crates/bench/tests/ci_trust_boundary.rs` checks each of these rules against
the workflow files; actionlint (security lane) checks their syntax.

1. **Routing.** Every job in a workflow a pull request can start (`ci.yml` and
   the `packages.yml` it calls) either runs on hosted runners or uses
   `runs-on: ${{ (<fork test>) && 'ubuntu-24.04' || fromJSON('["self-hosted","Linux","X64"]') }}`.
   `crates/bench/tests/ci_workflow_routing.rs` pins which jobs are hosted and
   that `RedlineDB/required` needs every other job.
2. **Cargo home.** Each self-hosted job's first step picks `CARGO_HOME`: a
   fork job gets `$RUNNER_TEMP/cargo-home`, which the runner empties for every
   job; trusted jobs keep the shared `$RUNNER_TOOL_CACHE/redlinedb-cargo`, so
   they do not download the index again over the runners' unreliable links.
   Only trusted jobs reach that cache, so its `bin` on `PATH` is not writable
   by fork code. Hosted jobs keep the image's own `~/.cargo`.
3. **Pinned tools.** `ops/ci/install-nextest.sh` downloads one cargo-nextest
   release, checks it against a pinned SHA-256, installs it into
   `$RUNNER_TEMP/nextest-bin` and puts that first on `PATH`. It never uses a
   `cargo-nextest` that is already on `PATH`. Trusted jobs keep the verified
   archive in `$RUNNER_TOOL_CACHE/redlinedb-tools` and check its digest again
   on every use. The jankurai auditor installer (`ops/ci/install-github-tools.sh`)
   also checks pinned digests and keeps the verified archive in
   `$RUNNER_TOOL_CACHE/redlinedb-tools/<archive sha256>/` (the checkout clean
   empties `target/ci/tools`), checked again on every use; gitleaks is checked
   only against the checksum file published in its own release. Both
   downloads, and nextest's, retry (`--retry 5 --retry-all-errors
   --connect-timeout 20`). Self-hosted jobs get Rust from
   `ops/ci/ensure-rust.sh`, which checks the toolchain `rust-toolchain.toml`
   pins offline (`rustup run`, `rustup component list --installed`) and
   downloads only what is missing, with retries; hosted jobs use
   `dtolnay/rust-toolchain`.
4. **Tokens.** Every `actions/checkout` sets `persist-credentials: false`,
   except `sqlite-parity-report.yml`'s `publish-pr` job, which pushes the
   report branch and sets `persist-credentials: true`.
5. **Heavy lanes.** For a fork pull request the Postgres parity lane is skipped
   and the kernel test stage is left out of the `tests` matrix; both run only
   on the self-hosted runners. `RedlineDB/required` then fails with
   "Maintainer run required", so a fork pull request cannot pass the merge
   gate on hosted runs alone.
6. **Host check.** A pull request runs its own copy of the workflow files, so
   a fork can change `runs-on` back to the self-hosted labels. The workflow
   rules above cannot stop that. `ops/ci/runner-job-started.sh` is the
   runners' `job_started` hook: the host runs its own copy before every job
   and fails any job whose event carries a pull request from another
   repository, or whose event it cannot read. It must be deployed on each
   host (below); `ops/ci/tests/runner-job-started.sh` tests it.

## Merging a fork pull request

1. Approve the fork's workflow run (outside contributors need approval). It
   runs on hosted runners and ends with "Maintainer run required".
2. Review the whole diff, including `.github/`, `ops/`, `scripts/`, `build.rs`
   files, lockfiles and test fixtures. Running it on the self-hosted runners
   is the same decision as merging it.
3. Push exactly the reviewed commit to a branch here and open a pull request
   from it. That is a trusted run with every lane:

   ```bash
   gh pr view <N> --repo neverhuman/redline --json headRefOid --jq .headRefOid   # the reviewed commit
   git fetch origin "pull/<N>/head:ci/pr-<N>"
   git push origin "ci/pr-<N>"
   gh pr create --repo neverhuman/redline --head "ci/pr-<N>" --base main \
     --title "<title> (#<N>)" --body "Reviewed copy of #<N>."
   ```

4. Merge that pull request once `RedlineDB/required` passes, close the fork
   pull request with a link to it, and delete the branch. If the fork pushes
   again, review the new commits before repeating step 3.

## Maintainer-only host work

The workflows cannot do these; they are done on the hosts or in the
repository settings.

- **Deploy the hook** on every self-hosted host and check that `jq` is
  installed. Copy it to the path in each runner's `.env`
  (`ACTIONS_RUNNER_HOOK_JOB_STARTED`), for example
  `install -m 0755 ops/ci/runner-job-started.sh ~/actions-runners/job-started.sh`.
  `ops/ci/install-github-runner.sh` installs it for new runners.
- **Unprivileged runner user.** Run the runner service as a dedicated user
  with no `gh` tokens (`~/.config/gh/hosts.yml`), no `~/.git-credentials`, no
  SSH keys and no `sudo`. Preinstall the apt packages the workflows ask for
  (`build-essential pkg-config openssl sqlite3 zlib1g-dev postgresql-client
  mold`); `ops/ci/apt-install.sh` calls `sudo` only when one is missing.
  Membership of the `docker` group, which the parity service needs, is
  equivalent to root, so no credential may live on a host that runs parity.
- **Wipe the shared caches once** after this change reaches `main`:
  `$RUNNER_TOOL_CACHE/redlinedb-cargo`, `$RUNNER_TOOL_CACHE/redlinedb-tools`
  and tools installed by hand into the runner user's `~/.cargo/bin`. Earlier
  pull request jobs could write all of them.
- **Ephemeral runners.** If fork pull requests ever need self-hosted
  hardware, register those runners with `--ephemeral` on disposable machines
  under their own label, never on a host with trusted caches.
- **Release host.** Release `publish` and `verify-published` and
  `report-merge.yml` run on GitHub-hosted runners. Give the parity report job
  its own runner label on a dedicated host, and add an `environment: release`
  with required reviewers and tag-only deployments to `publish` once that
  environment exists (it is a repository setting).
- **Keep fork approval on** ("Require approval for all outside
  collaborators"), and branch protection on `main` requiring
  `RedlineDB/required` (see `docs/release.md`).

## Canary

Run this after deploying the hook, and again after any runner change.

1. From a throwaway fork, open a pull request that adds this first step to
   the `preflight` job in `ci.yml`:

   ```yaml
         - run: |
             echo "runner: $RUNNER_NAME ($RUNNER_ENVIRONMENT)"
             touch "$RUNNER_TOOL_CACHE/canary-$GITHUB_SHA" "$HOME/.canary-$GITHUB_SHA"
   ```

   Approve the run. The log must show `github-hosted`.
2. Push a second commit that also changes that job's `runs-on` to
   `[self-hosted, Linux, X64]`, and approve it. The job must fail before its
   first step with "refusing this pull_request job". Do not try this before
   the hook is deployed: the job would then run on the self-hosted runner.
3. On every self-hosted host, as the runner user (or in a trusted
   `workflow_dispatch` job on each runner), confirm that nothing was written:

   ```bash
   ls -d ~/actions-runners/*/_work/_tool/canary-* ~/.canary-* 2>/dev/null && echo "CANARY FOUND" || echo "clean"
   ```

   Any output other than `clean` means fork code reached a trusted runner:
   treat the host as compromised and rotate what it could read.
4. Close the pull request and delete the fork.

## Known limits

- The hosted path of the heavy jobs (preflight, the test shards except
  kernel, `components (integration)` with Playwright) runs only for fork pull
  requests and has not run on real CI yet; the first fork pull request, or
  the canary, is its first run. They may need longer timeouts there.
- `cargo-audit` and `cargo-deny` are built with
  `cargo install --locked --version …` on the hosted `security` and `audit`
  runners, so no shared cache supplies them.
- Until the host work above is done, a maintainer who approves a fork run
  that edits the workflows, on a host without the hook, lets that code run
  as the runner user with that user's credentials.
