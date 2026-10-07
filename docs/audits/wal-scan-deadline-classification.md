# WAL scan and bounded-run deadline classification

PR #49 supersedes #46 as the owner-directed, sole-writer publication of
its rebased WAL window implementation. The original measurements and
source history remain credited in that PR and custody bundles; no new
README performance figures are inferred from this change.

A bounded runner can observe a normally exited leader after its deadline
has fired, or lose bounded access to a stream or stdin writer left behind
by a descendant. A recorded deadline and exit code zero do not prove a
complete run inside the limit. Previously both that state and a leader
terminated by a signal were labelled `timeout`, with a diagnostic claiming
the process group was killed.

The runner now reports `deadline_exceeded` when the deadline fired but the
leader exited normally. Signal termination under a fired deadline retains
`timeout`. Both remain incomplete outcomes and fail before contract or
result comparison, even when both engines have the same output and exit
code zero. This classification does not establish why a deadline fired.

The deadline, output cap, process-group signalling, stream draining,
selection, repetitions, known-failure list and every official gate are
unchanged. RQL case 11513 remains enabled and must pass the complete
unfiltered official lane; its rejected sample is not converted to a pass.

The preserved sample recorded 149.22 seconds under a 60000 ms limit, exit 0 and stdout SHA256 `5ebc6d97664816d57c16d1b5d22ce6c4bdd1287bee95a9ce299fed1effd6c34a`. Direct Python CLI controls recorded `{"main_engine_control": {"otherFailures": 0, "passed": 10, "timeouts": 0}, "wal": {"otherFailures": 0, "passed": 10, "timeouts": 0}}`; these bypass the Rust capture runner, are not a whole-main gate, and cannot establish the cause of the failure. The WAL control binary matches the failed sample hash; its embedded source identity is unknown. Host I/O pressure was observed without changing peer processes or services.

Focused acceptance: `cargo test --locked --manifest-path subrepos/redline-testing/Cargo.toml --target-dir target --bin redline-testing deadline_classification`. The fixture records an expired deadline without sending a signal, checks normal zero/nonzero exits and signal termination, and verifies that every outcome stays incomplete. Parent control applies the same fixture to the actual main Group implementation, which must fail the normal-exit labels. Existing escaped-descendant tests retain process containment, deadlines and rejection.

## Documentation server readiness

The first CI run on the replacement PR rejected its published-server probe with `ECONNREFUSED`. The inherited startup loop stopped after one hundred retries spaced by 20 ms, even though the complete protocol check has a 15-second deadline. A transient refusal could therefore exhaust the attempt count before that deadline. The raw CI log alone does not establish why the server was not listening.

Readiness now retries only refused connections until the existing protocol deadline. Other connection errors and an exited child fail immediately. The 15-second timer, protocol magic, every SQL/transaction response assertion, socket teardown and owned-child cleanup are unchanged. A never-ready server remains a failure; no successful result is accepted after the deadline.

`server-readiness.test.mjs` exercises the actual verified published v5.1.1 server after 105 deterministic refused connections, then requires the complete protocol round trip. Separate controls reject a never-ready fixture at the protocol deadline, an exited fixture and an unexpected connection error. The fixtures are owned subprocesses, not other agents or services. `bash scripts/check-docs.sh`, called by shared preflight, runs these regressions before the normal published-release documentation proof. The same fixture must fail when run against the actual parent server-check implementation.
