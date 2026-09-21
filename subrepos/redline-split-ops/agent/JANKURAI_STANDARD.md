# Release-tooling standard

Use the governed Jankurai binary and root GitHub `RedlineDB/required` check.
Root `subrepos.toml` describes the six included component workspaces. Source,
review and release authority is `neverhuman/RedlineDB`.

Rust implementation lives below `tools/redline-proof/`; process tests live in
`tests/`. Shell files launch that controller or root CI lanes. Validation rejects
wrong-owner remotes, obsolete forge activation files and executable old routes.

Run the component checks documented in `docs/testing.md`, then root acceptance.
Historical receipts under `release-evidence/` retain their original identities;
they are not current acceptance evidence and never authorize deployment.
