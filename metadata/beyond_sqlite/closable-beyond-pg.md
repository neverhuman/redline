# PostgreSQL compatibility backlog

All 265 executable cases are required. The historical 114-entry skip list is
planning history and does not reduce the CI denominator. The full requirement
inventory extends beyond this SQL-shell corpus, including wire clients and APIs.

`postgres-regression.json` records the remaining failing case IDs. The CI runner
runs every case twice against PostgreSQL 16.15 before comparing the target. It
rejects missing, duplicate, skipped, or unknown cases and unexpected oracle exits.
Negative fixtures explicitly declare their expected reference failure; a generic
target parser error does not count as correct negative-case behavior.

The generated `target/redline-testing/postgres-qualification.json` and
`postgres-progress.md` contain actual target totals and identities. Qualification
remains failed while any required case fails. The regression policy only permits
previously recorded failures and is ratcheted down as fixes land.

The first repair batch improves 120/265 to 127/265: UNNEST (including ordinality,
unequal arrays, and NULL padding), generate_series aliases, partial unique-index
backfill/enforcement, and two reviewed boolean result comparisons. No case was
removed or skipped. Materialized views, procedural SQL, namespaces, ranges,
notifications, replication interfaces, and other documented gaps remain open.
