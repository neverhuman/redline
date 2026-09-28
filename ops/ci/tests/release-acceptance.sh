#!/usr/bin/env bash
# Tests for the release acceptance manifest (CI-04/CI-06):
# ops/ci/release-acceptance.sh writes release-acceptance.v1.json from the
# checkout, the run's job list, the package archives and the receipt
# artifacts; scripts/release/verify-acceptance.sh refuses it unless every job
# succeeded, every package digest and provenance matches, the repository id,
# commit, tree and source-inputs hash are the checkout's, and (with
# --receipts) every receipt digest matches and the security receipt passed
# for the commit.
# No network or GitHub access: gh is a stand-in that serves a job list.
#
# Usage: bash ops/ci/tests/release-acceptance.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
generate=$root/ops/ci/release-acceptance.sh
verify=$root/scripts/release/verify-acceptance.sh
for script in "$generate" "$verify" "$root/ops/ci/source-inputs-sha256.sh"; do
  [[ -f $script ]] || { printf 'FAIL: %s does not exist\n' "$script" >&2; exit 1; }
done
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
# shellcheck source=scripts/release/package-layout.sh
. "$root/scripts/release/package-layout.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
tag=v5.0.0-rc.7
rustc_line='rustc 1.95.0 (59807616e 2026-04-14)'
mapfile -t receipts < <(sed 's/#.*//' "$root/ops/release/acceptance-receipts" | awk 'NF { print $1 }')
((${#receipts[@]} >= 5)) || { printf 'FAIL: ops/release/acceptance-receipts lists %d receipts\n' "${#receipts[@]}" >&2; exit 1; }

# --- the checkout: two source paths, target/ ignored, the tag at HEAD -------
checkout=$work/checkout
mkdir -p "$checkout/crates/fixture" "$checkout/ops"
printf '/target/\n' > "$checkout/.gitignore"
printf '[workspace]\n' > "$checkout/Cargo.toml"
printf 'fn main() {}\n' > "$checkout/crates/fixture/main.rs"
printf 'echo ops\n' > "$checkout/ops/tool.sh"
git_fixture() { git -C "$checkout" -c user.name=fixture -c user.email=fixture@example.invalid -c tag.gpgSign=false -c commit.gpgSign=false "$@"; }
git init --quiet "$checkout"
git_fixture add -A
git_fixture commit --quiet -m fixture
git_fixture tag -a -m "$tag" "$tag"
commit=$(git -C "$checkout" rev-parse HEAD)
tree=$(git -C "$checkout" rev-parse 'HEAD^{tree}')
inputs=$(cd "$checkout" && bash "$root/ops/ci/source-inputs-sha256.sh")

# packages [commit] [tree] [compiler of the last archive]: the twelve
# archives of $tag with checksum files.
packages() {
  local p_commit=${1:-$commit} p_tree=${2:-$tree} last_rust=${3:-$rustc_line} rust package platform dir share
  rm -rf "$checkout/target/packages" "$work/trees"
  mkdir -p "$checkout/target/packages"
  for package in redlinedb redline-web redline-testing; do
    for platform in linux-x86_64 linux-arm64 macos-x86_64 macos-arm64; do
      dir=$work/trees/$package-$platform
      rust=$rustc_line
      [[ $package-$platform != redline-testing-macos-arm64 ]] || rust=$last_rust
      share=$dir/$(package_share "$package")
      mkdir -p "$share"
      jq -cn --arg url "$REDLINE_REPO_URL" --argjson id "$REDLINE_REPO_ID" --arg tag "$tag" \
        --arg commit "$p_commit" --arg tree "$p_tree" --arg platform "$platform" --arg package "$package" \
        --arg rust "$rust" \
        '{schema:"redline.release-build/v2",repository_url:$url,repository_id:$id,tag:$tag,commit:$commit,source_tree:$tree,platform:$platform,package:$package,rust:$rust}' \
        > "$share/build-provenance.json"
      # Byte-identical when rebuilt, so each refusal below has one cause.
      tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner -cf - -C "$dir" . \
        | gzip -n > "$checkout/target/packages/$package-$tag-$platform.tar.gz"
      (cd "$checkout/target/packages" && sha256sum "$package-$tag-$platform.tar.gz" > "$package-$tag-$platform.tar.gz.sha256")
    done
  done
}

# receipt_files [security receipt sha] [result]: one directory per receipt.
receipt_files() {
  local sha=${1:-$commit} result=${2:-pass} name
  rm -rf "$checkout/target/acceptance/receipts"
  for name in "${receipts[@]}"; do
    mkdir -p "$checkout/target/acceptance/receipts/$name/logs"
    printf '%s evidence\n' "$name" > "$checkout/target/acceptance/receipts/$name/logs/run.log"
  done
  jq -n --arg sha "$sha" --arg result "$result" \
    '{schema:"redline.security-receipt.v1",candidate:{sha:$sha,dirty:false,shallow:false},result:$result}' \
    > "$checkout/target/acceptance/receipts/security-receipt/receipt.json"
}

# The run's job list as `gh api` prints it, while publish runs.
jq -n '{total_count: 5, jobs: [
    {name: "validate", status: "completed", conclusion: "success"},
    {name: "acceptance / preflight", status: "completed", conclusion: "success"},
    {name: "acceptance / tests (kernel)", status: "completed", conclusion: "success"},
    {name: "acceptance / RedlineDB/required", status: "completed", conclusion: "success"},
    {name: "publish", status: "in_progress", conclusion: null}]}' > "$work/jobs.json"

mkdir -p "$work/bin"
cat > "$work/bin/gh" <<SHIM
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "$work/gh.log"
[[ \$1 == api ]] || { echo "gh stand-in: unexpected call: \$*" >&2; exit 1; }
[[ ! -e "$work/gh-down" ]] || { echo "HTTP 503" >&2; exit 1; }
cat "$work/jobs.json"
SHIM
chmod +x "$work/bin/gh"

manifest=$checkout/target/acceptance/release-acceptance.v1.json
# run_generate <label> [VAR=value...]; sets $status.
run_generate() {
  local label=$1
  shift
  rm -f "$manifest" "$work/gh.log"
  status=0
  (cd "$checkout" && env PATH="$work/bin:$PATH" GH_TOKEN=fixture TAG=$tag \
    GITHUB_REPOSITORY="$REDLINE_REPO_SLUG" GITHUB_REPOSITORY_ID="$REDLINE_REPO_ID" \
    GITHUB_RUN_ID=4242 GITHUB_RUN_ATTEMPT=2 GITHUB_SERVER_URL=https://github.com "$@" \
    bash "$generate" target/packages target/acceptance/receipts "$manifest") > "$work/$label.log" 2>&1 || status=$?
}
# run_verify <label> <manifest> [extra args...]; sets $status.
run_verify() {
  local label=$1 file=$2
  shift 2
  status=0
  (cd "$checkout" && bash "$verify" "$file" --packages target/packages --receipts target/acceptance/receipts --tag "$tag" "$@") \
    > "$work/$label.log" 2>&1 || status=$?
}
expect_verified() {
  run_verify "$@"
  [[ $status == 0 ]] || fail "$1: refused: $(tail -n 3 "$work/$1.log")"
}
# expect_refused <label> <message> <manifest> [extra args...]
expect_refused() {
  local label=$1 message=$2
  shift 2
  run_verify "$label" "$@"
  [[ $status != 0 ]] || fail "$label: verified"
  grep -qF -- "$message" "$work/$label.log" || fail "$label: expected '$message', got: $(tail -n 3 "$work/$label.log")"
}
# mutate <label> <jq filter>: a copy of the good manifest.
mutate() {
  jq "$2" "$good" > "$work/$1.json"
  printf '%s' "$work/$1.json"
}
# rebound <label>: a copy of the good manifest with the digests of the
# archives now in target/packages, so only their contents differ.
rebound() {
  local digests
  digests=$(cd "$checkout/target/packages" && for f in *.tar.gz; do jq -n --arg n "$f" --arg s "$(sha256sum "$f" | cut -d' ' -f1)" '{($n): $s}'; done | jq -s 'add')
  mutate "$1" ".packages |= map(.sha256 = ${digests}[.name])"
}

# --- 1. the good path --------------------------------------------------------
packages
receipt_files
run_generate good
[[ $status == 0 ]] || { printf 'FAIL: good: generation failed: %s\n' "$(tail -n 5 "$work/good.log")" >&2; exit 1; }
grep -qx "api --paginate repos/$REDLINE_REPO_SLUG/actions/runs/4242/attempts/2/jobs?per_page=100" "$work/gh.log" \
  || fail "good: unexpected gh call: $(cat "$work/gh.log")"
good=$work/good.json
cp "$manifest" "$good"
check() { jq -e "$1" "$good" >/dev/null || fail "good: manifest fails $1"; }
check '.schema == "redline.release-acceptance/v1"'
check ".repository == {slug: \"$REDLINE_REPO_SLUG\", id: $REDLINE_REPO_ID}"
check ".tag == \"$tag\" and .commit == \"$commit\" and .tree == \"$tree\""
check ".source_inputs_sha256 == \"$inputs\" and .clean == true"
check ".rustc == \"$rustc_line\""
check '.run == {id: 4242, attempt: 2, url: "https://github.com/'"$REDLINE_REPO_SLUG"'/actions/runs/4242/attempts/2"}'
check '(.jobs | length) == 4 and all(.jobs[]; .conclusion == "success") and .pending_jobs == ["publish"]'
check '(.packages | length) == 12 and all(.packages[]; (.sha256 | test("^[0-9a-f]{64}$")))'
check "(.receipts | map(.name)) == $(printf '%s\n' "${receipts[@]}" | jq -R . | jq -sc .)"
check 'all(.receipts[]; (.sha256 | test("^[0-9a-f]{64}$")) and .files >= 1)'
archive=redline-web-$tag-macos-arm64.tar.gz
check ".packages[] | select(.name == \"$archive\") | .sha256 == \"$(sha256sum "$checkout/target/packages/$archive" | cut -d' ' -f1)\""
expect_verified good "$good"
status=0
(cd "$checkout" && bash "$verify" "$good" --packages target/packages) > "$work/no-receipts.log" 2>&1 || status=$?
[[ $status == 0 ]] || fail "good: refused without --receipts: $(tail -n 2 "$work/no-receipts.log")"

# --- 2. the generator refuses what it cannot bind ----------------------------
rm -rf "$checkout/target/acceptance/receipts/durability-evidence"
run_generate missing-receipt
[[ $status != 0 && ! -e $manifest ]] || fail "missing-receipt: wrote a manifest without durability-evidence"
grep -qF 'missing receipt artifact(s): durability-evidence' "$work/missing-receipt.log" \
  || fail "missing-receipt: unexpected output: $(tail -n 2 "$work/missing-receipt.log")"
receipt_files
run_generate foreign-repository GITHUB_REPOSITORY_ID=1240106851
[[ $status != 0 && ! -e $manifest ]] || fail "foreign-repository: wrote a manifest"
grep -qF 'refusing' "$work/foreign-repository.log" || fail "foreign-repository: $(tail -n 1 "$work/foreign-repository.log")"
# expect_generation_refused <label> <message> [VAR=value...]
expect_generation_refused() {
  local label=$1 message=$2
  shift 2
  run_generate "$label" "$@"
  [[ $status != 0 && ! -e $manifest ]] || fail "$label: wrote a manifest"
  grep -qF -- "$message" "$work/$label.log" || fail "$label: expected '$message', got: $(tail -n 2 "$work/$label.log")"
}
expect_generation_refused bad-tag 'v5.0 is not a release tag' TAG=v5.0
expect_generation_refused absent-tag 'tag v5.0.0-rc.8 is not in this checkout' TAG=v5.0.0-rc.8
elsewhere=$(git -C "$checkout" commit-tree -p HEAD -m elsewhere 'HEAD^{tree}')
git_fixture tag -a -m v5.0.0-rc.9 v5.0.0-rc.9 "$elsewhere"
expect_generation_refused moved-tag "tag v5.0.0-rc.9 names $elsewhere, but the checkout is at $commit" TAG=v5.0.0-rc.9
printf '%064d  %s\n' 0 "$archive" > "$checkout/target/packages/$archive.sha256"
expect_generation_refused crossed-checksum "$archive does not match its checksum file" GITHUB_RUN_ATTEMPT=2
rm "$checkout/target/packages/$archive.sha256"
expect_generation_refused no-checksum "$archive has no checksum file"
packages "$commit" "$tree" 'rustc 1.96.0 (0000000 2026-05-28)'
expect_generation_refused two-compilers 'the packages name different compilers'
packages
touch "$work/gh-down"
expect_generation_refused gh-down 'cannot list the jobs of run 4242 attempt 2'
rm "$work/gh-down"

# --- 3. the verifier refuses --------------------------------------------------
expect_refused failed-job 'acceptance / tests (kernel): failure' \
  "$(mutate failed-job '(.jobs[] | select(.name == "acceptance / tests (kernel)") | .conclusion) = "failure"')"
expect_refused no-required 'no RedlineDB/required job' \
  "$(mutate no-required '.jobs |= map(select(.name | endswith("RedlineDB/required") | not))')"
expect_refused pending 'still running: acceptance / parity' \
  "$(mutate pending '.pending_jobs += ["acceptance / parity"]')"
expect_refused schema 'schema' "$(mutate schema '.schema = "redline.release-acceptance/v0"')"
expect_refused repository 'repository' "$(mutate repository '.repository.id = 1240106851')"
expect_refused tree "tree is $(printf '%040d' 0) in the manifest, $tree in the commit" \
  "$(mutate tree ".tree = \"$(printf '%040d' 0)\"")"
expect_refused bad-tag 'is not a release tag' "$(mutate bad-tag '.tag = "v5.0"')" --tag v5.0
expect_refused bad-commit 'is not a SHA-1' "$(mutate bad-commit '.commit = "abc"')"
expect_refused bad-rustc "rustc is 'gcc'" "$(mutate bad-rustc '.rustc = "gcc"')"
expect_refused other-run 'is not a run of' "$(mutate other-run '.run.url = "https://github.com/someone/redline/actions/runs/4242/attempts/2"')"
expect_refused no-jobs 'no jobs recorded' "$(mutate no-jobs '.jobs = []')"
printf 'not json\n' > "$work/not-json.json"
expect_refused not-json 'not a JSON manifest' "$work/not-json.json"
expect_refused dirty 'clean' "$(mutate dirty '.clean = false')"
expect_refused inputs 'source_inputs_sha256' "$(mutate inputs ".source_inputs_sha256 = \"$(printf '%064d' 0)\"")"
expect_refused dropped-package "missing: $archive" "$(mutate dropped-package ".packages |= map(select(.name != \"$archive\"))")"
expect_refused other-tag 'does not match --tag' "$good" --tag v5.0.0-rc.8
expect_refused dropped-receipt 'receipts' "$(mutate dropped-receipt '.receipts |= .[1:]')"

printf 'tampered' >> "$checkout/target/packages/$archive"
expect_refused tampered-package "$archive: sha256" "$good"
packages
printf '%064d  %s\n' 0 "$archive" > "$checkout/target/packages/$archive.sha256"
expect_refused crossed-checksum "$archive: its checksum file says $(printf '%064d' 0)" "$good"
packages
rm "$checkout/target/packages/$archive"
expect_refused absent-package "$archive: not in target/packages" "$good"
packages
cp "$checkout/target/packages/$archive" "$checkout/target/packages/redlinedb-$tag-windows-x86_64.tar.gz"
expect_refused extra-package 'not in the manifest: redlinedb-'"$tag"'-windows-x86_64.tar.gz' "$good"
packages "$(printf '%040d' 1)"
expect_refused other-commit 'provenance names' "$(rebound other-commit)"
packages

# rebound_receipt <label> <name>: a copy of the good manifest with the digest
# of the receipt directory as it is now.
rebound_receipt() {
  local digest
  digest=$(cd "$checkout/target/acceptance/receipts/$2" && find . -type f | LC_ALL=C sort \
    | while IFS= read -r f; do sha256sum "$f"; done | sha256sum | cut -d' ' -f1)
  mutate "$1" "(.receipts[] | select(.name == \"$2\") | .sha256) = \"$digest\""
}
printf 'edited\n' >> "$checkout/target/acceptance/receipts/audit-family/logs/run.log"
expect_refused tampered-receipt 'audit-family: sha256' "$good"
receipt_files
rm -r "$checkout/target/acceptance/receipts/durability-evidence"
expect_refused absent-receipt 'receipt durability-evidence: not in target/acceptance/receipts' "$good"
receipt_files "$(printf '%040d' 2)"
expect_refused receipt-other-commit 'receipt.json is for' "$(rebound_receipt receipt-other-commit security-receipt)"
receipt_files "$commit" fail
expect_refused receipt-failed 'security-receipt: result fail' "$(rebound_receipt receipt-failed security-receipt)"
receipt_files
rm "$checkout/target/acceptance/receipts/security-receipt/receipt.json"
expect_refused receipt-absent 'security-receipt: no receipt.json' "$(rebound_receipt receipt-absent security-receipt)"
receipt_files

# The checkout's tag must name the commit.
git_fixture tag -d "$tag" > /dev/null
expect_refused untagged "tag $tag names nothing in this checkout" "$good"
git_fixture tag -a -m "$tag" "$tag"

# The verifier runs in a checkout of the tagged commit.
git_fixture commit --quiet --allow-empty -m later
expect_refused moved-head 'check out' "$good"

[[ $failures == 0 ]] || { printf '%d release acceptance check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Release acceptance tests passed.\n'
