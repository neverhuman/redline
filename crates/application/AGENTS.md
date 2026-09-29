# crates/application/AGENTS.md

<!-- jankurai generated adapter -->
<!-- jankurai agent request v1 sha256:REPLACE_WITH_HASH -->
Read `AGENTS.md` first. Use `.jankurai/JANKURAI_STANDARD.md` as the canonical jankurai standard.
When a user provides a paper, release, implementation, or handoff plan in the conversation, treat that plan as the controlling plan.
Owns `crates/application/`.
Forbidden: transport handlers, persistence code, and UI concerns.
Proof lane: `use-case / authz tests`.
If jankurai is installed, run `jankurai update --client-start --quiet` before work; do not apply updates unless the user asks.
