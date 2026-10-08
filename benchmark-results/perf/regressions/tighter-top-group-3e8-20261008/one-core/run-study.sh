#!/usr/bin/env bash
set -euo pipefail
base=/dev/shm/redline-tight-top-group-20261008/one-core
cd "$base"
exec 9>"$base/owner.lock"
flock -n 9
set -o noclobber
printf '%s\n' "$$" > coordinator.started
node=/home/ubuntu/.nvm/versions/node/v22.22.2/bin/node
"$node" --version > node-version.txt
sha256sum inputs/release/redline-scoreboard inputs/main/redline-scoreboard > binary-hashes-before.txt
for session in 1 2; do
  mkdir -p "$base/session-$session/work"
  set +e
  "$node" "$base/tighter-compare.mjs" "$base/plan-session-$session.json" > "$base/session-$session/measure.log" 2>&1
  result=$?
  set -e
  printf '%s\n' "$result" > "$base/session-$session/measure.exit"
  if [ "$result" -ne 0 ]; then
    printf '%s\n' "$result" > coordinator.exit
    exit "$result"
  fi
  if [ "$session" -eq 1 ]; then sleep 60; fi
done
sha256sum inputs/release/redline-scoreboard inputs/main/redline-scoreboard > binary-hashes-after.txt
cmp binary-hashes-before.txt binary-hashes-after.txt
printf '0\n' > coordinator.exit
