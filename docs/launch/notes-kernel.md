# Release notes: kernel (lane kernel)

Draft lines for the v5.0.0 CHANGELOG. The integrator owns `CHANGELOG.md`.

## Testing

- Kernel unit tests now pass when `cargo test` runs them as parallel threads
  of one process, as CI's `cargo test -p redlinedb-kernel` and
  `cargo test --workspace --lib` do. `index::mutate::insert_leaf` counted leaf
  rebuilds through the process-wide `observe` counters, so rebuilds made by
  tests running beside it failed its "direct path rebuilt the leaf" check.
  Test builds now also count each `observe` event on the thread that made it,
  and kernel unit tests read that count. The process-wide counters that
  `redlinedb-bench` reports are unchanged.
