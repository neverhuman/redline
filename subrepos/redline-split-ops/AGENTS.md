# Release tooling

This component belongs to [neverhuman/redline](https://github.com/neverhuman/redline).
Root `AGENTS.md` and GitHub workflows govern all development and publication.
Keep the independent Cargo workspace and edit only in the canonical checkout.

`./redlinectl doctor`, `./redlinectl validate`, and `./redlinectl family-ci`
use root `subrepos.toml`. `validate --history` additionally verifies imported
source identities and custody refs. Commands require the complete checkout or
source archive; there is no split-repository or alternate-forge fallback.

New controller implementation and tests are Rust. Historical receipts under
`release-evidence/` retain their original bytes and are not release authority.
