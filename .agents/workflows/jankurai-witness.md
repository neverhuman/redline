# jankurai witness

<!-- jankurai generated adapter -->
<!-- jankurai agent request v1 sha256:REPLACE_WITH_HASH -->
Read `AGENTS.md` first. Use `.jankurai/JANKURAI_STANDARD.md` as the canonical jankurai standard.
When a user provides a paper, release, implementation, or handoff plan in the conversation, treat that plan as the controlling plan.
Use `jankurai witness . --changed-from origin/main --baseline .jankurai/baselines/main.repo-score.json --out .jankurai/merge-witness.json --md .jankurai/merge-witness.md` to compare the current branch against the accepted baseline.
Expected receipts: `.jankurai/merge-witness.json`, `.jankurai/merge-witness.md`.
Next command: `jankurai repair-plan`.
Stop: changed-path routing, generated-zone touches, baseline score delta, or proof coverage cannot be justified.
If jankurai is installed, run `jankurai update --client-start --quiet` before work; do not apply updates unless the user asks.
