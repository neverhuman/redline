# jankurai context-pack

<!-- jankurai generated adapter -->
<!-- jankurai agent request v1 sha256:REPLACE_WITH_HASH -->
Read `AGENTS.md` first. Use `.jankurai/JANKURAI_STANDARD.md` as the canonical jankurai standard.
When a user provides a paper, release, implementation, or handoff plan in the conversation, treat that plan as the controlling plan.
Use `jankurai context-pack . --changed <path> --max-tokens 6000 --out .jankurai/context-pack.json --md .jankurai/context-pack.md` to turn a bounded change set into a repo-aware context bundle.
Expected receipts: `.jankurai/context-pack.json`, `.jankurai/context-pack.md`.
Next command: `jankurai prove`.
Stop: the task is too broad, owner/test routing is unclear, or generated-zone work needs source regeneration first.
If jankurai is installed, run `jankurai update --client-start --quiet` before work; do not apply updates unless the user asks.
