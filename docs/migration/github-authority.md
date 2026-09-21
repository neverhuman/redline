# GitHub authority cutover

Source, pull requests, checks and releases for all six RedlineDB components use
https://github.com/neverhuman/RedlineDB. Root `subrepos.toml` is the component
inventory; component Cargo workspaces remain independent.

The 2026-09-21 owner instruction supersedes earlier plans to restore the former
forge or merge its six retirement PRs. Those proposals are rejected as obsolete:
central #1, core #1, split-ops #1, testing #2, web #1 and hub #3. No connection to
that forge is a development, validation or release prerequisite. Their remote
states are not claimed to have changed.

## Removed activation paths

The root and component forge policies and secondary CI files are retired. Agent
guidance and release instructions route to GitHub. The installer uses
`REDLINEDB_INSTALL_DIR` or `PREFIX`; the old forge installation variable is ignored.
Signing and artifact entrypoints route to root checks and GitHub attestations.
The old deployment-telemetry launcher returns an explicit unsupported status.

`redline-proof` uses only the complete single checkout or source archive. It
rejects obsolete split publication commands, validates all six component paths,
checks fetch and push remotes against the canonical GitHub repository, and
rejects reintroduced forge policies and executable routes. Root CI's release-tools
lane runs these tests and validation. An explicit invalid `REDLINE_REPO_ROOT`
fails rather than selecting an unrelated ancestor checkout.

Historical schema names and application compatibility fixtures remain test data.
Historical audit/consumer receipts retain their original bytes and hashes. The
former split manifest and lock moved to
`subrepos/redline-split-ops/release-evidence/retired-forge/`; its `custody.json`
records exact identities. The obsolete controller and its tests are recoverable
from source commit `631db4e0e34c639d2eff603a25f2dcc9f041251e`. Current validation
does not use those historical receipts as release authority.

## Gate 0 dispositions and custody

- RedlineDB #75: closed unmerged after preserving report head
  `5d69bdfee0ae24252bf2df134ed59a23213fba6c`, tree, patch and PR metadata.
  The report workflow is disabled pending the separate report-loop repair.
- redline-testing #3 and #4: closed as superseded. Their heads
  `0adf38a1f4d961cfd1337ef499d1e02562712b71` and
  `eb74f2fb9dc9c3379d4103c078673beea622af93` are verified ancestors of imported
  testing source `d25d6a9cfef3e117c9d5e35f9337fe5dec41f7d3`.
- Local safety commit `1cb2d9992950436e5fccee6cf89ffd051f69668e` is preserved
  on its original branch and in a full-ref bundle, with independent exact-commit,
  exact-tree and connectivity verification. Its seven salvage stages remain open.
- The standalone testing changes and pre-existing untracked audit/planning files
  are archived with per-file hashes. Planning inputs moved outside active source.
- Standalone testing and all six migration cutover checkout remotes now use the
  canonical GitHub repository. Original configurations are held in custody;
  archival checkouts are not development roots.

Host custody: `/home/ubuntu/redline-custody/gate0-20260921T204356Z/`.
The independent verification store is the reusable `verification.git` beside it.
No installed consumer database or immutable published release was changed.

This cutover does not establish full SQL, ABI, file or PostgreSQL compatibility.
The broader compatibility and quality program remains unqualified.
