# Component boundary

The root GitHub checkout owns source history and release authority. Root
`subrepos.toml` describes six included components with independent Cargo workspaces.
`redline-proof` validates those paths, rejects Git dependencies and checkout escapes,
and routes family CI to the root dispatcher. An optional history check verifies
original source trees and recovered refs. It does not clone or update components,
promote consumer locks, or contact an alternate forge.
