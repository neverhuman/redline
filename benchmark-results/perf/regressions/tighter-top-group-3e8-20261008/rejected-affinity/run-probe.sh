#!/usr/bin/env bash
set -euo pipefail
root=/dev/shm/redline-tight-top-group-20261008/probe-interference
cd "$root"
exec 9>owner.lock
flock -n 9
test ! -e coordinator.started
date -u +%FT%TZ > coordinator.started
sha256sum probe-interference.mjs probe-interference-plan.json abba-summary.mjs > sources-before.txt
sha256sum ../one-core/inputs/release/redline-scoreboard ../one-core/inputs/main/redline-scoreboard > binaries-before.txt
set +e
/home/ubuntu/.nvm/versions/node/v22.22.2/bin/node probe-interference.mjs probe-interference-plan.json > measure.log 2>&1
result=$?
set -e
printf '%s\n' "$result" > measure.exit
sha256sum probe-interference.mjs probe-interference-plan.json abba-summary.mjs > sources-after.txt
sha256sum ../one-core/inputs/release/redline-scoreboard ../one-core/inputs/main/redline-scoreboard > binaries-after.txt
cmp sources-before.txt sources-after.txt || result=1
cmp binaries-before.txt binaries-after.txt || result=1
printf '%s\n' "$result" > coordinator.exit
exit "$result"
