#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# A private fixture must not inherit a pre-push hook's Git environment.
mapfile -t repository_variables < <(git rev-parse --local-env-vars)
unset "${repository_variables[@]}"
fixture="$work/repository with spaces"
mkdir -p "$fixture/ops/ci" "$fixture/scripts/release" "$work/packages"
cp "$root/ops/ci/package-build-custody.sh" "$fixture/ops/ci/"
cp "$root/scripts/release/package-layout.sh" "$fixture/scripts/release/"
git -C "$fixture" init --quiet
git -C "$fixture" -c user.name=Fixture -c user.email=fixture@example.invalid add .
git -C "$fixture" -c user.name=Fixture -c user.email=fixture@example.invalid commit --quiet -m fixture
sha=$(git -C "$fixture" rev-parse HEAD)
tree=$(git -C "$fixture" rev-parse 'HEAD^{tree}')
guard="$fixture/ops/ci/package-build-custody.sh"
reject() {
  local expected=$1; shift
  if bash "$guard" "$@" > "$work/rejection.log" 2>&1; then
    printf 'custody accepted invalid input: %s\n' "$*" >&2; exit 1
  else
    [[ $? == 1 ]]
  fi
  grep -Fq "$expected" "$work/rejection.log"
}
bash "$guard" checkout "$sha" "$tree"
reject 'requires full source SHA' checkout "${sha:0:8}" "$tree"
reject 'checkout differs' checkout 0000000000000000000000000000000000000000 "$tree"
reject 'checkout differs' checkout "$sha" 0000000000000000000000000000000000000000
printf '\n# modified fixture\n' >> "$fixture/ops/ci/package-build-custody.sh"
reject 'modified or untracked source inputs' checkout "$sha" "$tree"
git -C "$fixture" restore ops/ci/package-build-custody.sh
rm "$fixture/scripts/release/package-layout.sh"
reject 'modified or untracked source inputs' checkout "$sha" "$tree"
git -C "$fixture" restore scripts/release/package-layout.sh
touch "$fixture/untracked-source.txt"
reject 'modified or untracked source inputs' checkout "$sha" "$tree"
rm "$fixture/untracked-source.txt"
cp "$fixture/.git/index" "$work/index-backup"
printf 'invalid index\n' > "$fixture/.git/index"
reject 'cannot read package checkout status' checkout "$sha" "$tree"
cp "$work/index-backup" "$fixture/.git/index"
reject 'no package archives' archives "$work/packages" "$sha" "$tree"
mkdir -p "$work/archive/share/redlinedb"
jq -cn --arg sha "$sha" --arg tree "$tree" '{commit:$sha,source_tree:$tree}' \
  > "$work/archive/share/redlinedb/build-provenance.json"
tar -czf "$work/packages/redlinedb-v5.1.1-test-linux-arm64.tar.gz" -C "$work/archive" .
bash "$guard" archives "$work/packages" "$sha" "$tree"
reject 'archive provenance differs' archives "$work/packages" 0000000000000000000000000000000000000000 "$tree"
reject 'archive provenance differs' archives "$work/packages" "$sha" 0000000000000000000000000000000000000000
printf 'Package custody: clean checkout and archive pass; short SHA, wrong SHA/tree, changed or missing tracked input, untracked source, unreadable status, missing archive and stale archive SHA/tree refused.\n'
