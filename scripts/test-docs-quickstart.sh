#!/usr/bin/env bash
# Run the documented quick start the way a reader does. Every ```bash quickstart
# block of docs/install.md, README.md and docs/manual/02-start-here.md runs
# in order, in one shell per
# document, in a private prefix with inherited HOME and PATH=/usr/bin:/bin,
# against this platform's
# candidate archive: a file-transport curl serves the installer and release
# URLs from the packages directory, and cargo, rustc, cc, node, npm, just and
# rtk fail if called. The `# prints: <line>` comments of a block are its exact
# standard output. The documents name the release tag; a candidate built for
# another tag (an rc or dev build) is installed under its own tag instead.
#
# Checked without a package (also by --static): README.md's ```rust readme
# block is crates/redlinedb/examples/readme.rs verbatim, and the active
# documentation carries no retired install advice.
#
#   scripts/test-docs-quickstart.sh [--static] [packages dir]
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
static=false
[[ ${1:-} != --static ]] || { static=true; shift; }
failures=0
fail() { printf 'FAIL: %s\n' "$*" >&2; failures=$((failures + 1)); }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
documents=(docs/install.md README.md docs/manual/02-start-here.md)

# block <file> <info string>: the fenced blocks opened by exactly that info
# string, one file each: $work/blocks/<n>.
blocks() {
  rm -rf "$work/blocks"
  mkdir -p "$work/blocks"
  awk -v info="$2" -v dir="$work/blocks" '
    !open && $0 == "```" info { open = 1; n++; file = dir "/" n; printf "" > file; next }
    open && /^```[[:space:]]*$/ { open = 0; close(file); next }
    open { print > file }
  ' "$1"
}

# --- Retired advice stays out of the active documentation. -------------------
# Migration records, archived pages and release-note drafts (docs/launch/,
# like CHANGELOG.md) describe what was removed, so they may quote it. The old
# repository name is split so this file passes the same scans.
retired="jankurai init|VERSION=v1\\.0\\.1|/usr/local/bin/sqlite3|neverhuman(bot)?/redline""db|redline/main/(scripts/)?install\\.sh"
hits=$(find README.md CONTRIBUTING.md SECURITY.md docs -name '*.md' -not -path 'docs/migration/*' -not -path 'docs/archive/*' \
  -not -path 'docs/launch/*' -print0 |
  xargs -0 grep -n -i -E "$retired" || true)
[[ -z $hits ]] || fail "active docs carry retired install advice (a jankurai scaffold command, the v1.0.1 installer, a sqlite3 alias, the old repository or an installer from main):
$hits"

# --- The README's Rust example is the example cargo builds. -------------------
blocks README.md 'rust readme'
if [[ ! -f $work/blocks/1 || -f $work/blocks/2 ]]; then
  fail 'README.md must have exactly one ```rust readme block'
elif ! cmp -s "$work/blocks/1" crates/redlinedb/examples/readme.rs; then
  fail "README.md's rust readme block differs from crates/redlinedb/examples/readme.rs:
$(diff "$work/blocks/1" crates/redlinedb/examples/readme.rs || true)"
fi

if "$static"; then
  [[ $failures == 0 ]] || { printf '%d documentation check(s) failed\n' "$failures" >&2; exit 1; }
  printf 'Documentation static checks passed.\n'
  exit 0
fi

# --- Run each document's quick start against the candidate archive. ----------
packages=$(cd "${1:-${OUTPUT_DIR:-$root/target/packages}}" && pwd)
archives=("$packages"/redlinedb-v*.tar.gz)
[[ ${#archives[@]} == 1 && -f ${archives[0]} ]] || { printf 'expected one native core archive in %s\n' "$packages" >&2; exit 1; }
tag=$(tar -xzOf "${archives[0]}" ./share/redlinedb/VERSION)
# shellcheck source=scripts/release/test-shims.sh
. "$root/scripts/release/test-shims.sh"
write_no_toolchain "$work/no-toolchain"
write_release_transport "$work/transport"

for document in "${documents[@]}"; do
  blocks "$document" 'bash quickstart'
  count=$(find "$work/blocks" -type f | wc -l | tr -d ' ')
  [[ $count -gt 0 ]] || { fail "$document has no \`\`\`bash quickstart blocks"; continue; }
  documented=$(cat "$work/blocks"/* | grep -oE 'v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?' | sort -u || true)
  [[ $(printf '%s' "$documented" | grep -c . || true) -le 1 ]] || { fail "$document's quick start names several tags: $documented"; continue; }
  grep -qE "raw\\.githubusercontent\\.com/neverhuman/redline/${documented:-v}[^/]*/install\\.sh" "$work/blocks"/* ||
    fail "$document's quick start does not fetch install.sh from a release tag"
  grep -lF -- "-batch :memory: 'SELECT 1;'" "$work/blocks"/* > "$work/select-one" || fail "$document's quick start has no SELECT 1"
  name=${document//\//-}
  run=$work/$name
  mkdir -p "$run/prefix" "$run/tmp" "$run/out"
  script=$run/quickstart.sh
  printf 'cd "$DOCS_CHECK_WORK"\n' > "$script"
  for ((n = 1; n <= count; n++)); do
    # A candidate for another tag is installed under that tag.
    if [[ -n $documented && $documented != "$tag" ]]; then
      sed "s/${documented//./\\.}/$tag/g" "$work/blocks/$n" > "$run/block-$n"
    else
      cp "$work/blocks/$n" "$run/block-$n"
    fi
    # Redirect the documented default prefix into this private fixture without
    # replacing HOME or letting an example touch the caller's installation.
    sed 's|$HOME/.local|$DOCS_CHECK_PREFIX|g' "$run/block-$n" > "$run/executable-$n"
    printf 'echo %d > "%s/at"\n{\n' "$n" "$run" >> "$script"
    cat "$run/executable-$n" >> "$script"
    printf '\n} > "%s/out/%d"\n' "$run" "$n" >> "$script"
  done
  if ! env -i HOME="$HOME" TMPDIR="$run/tmp" PATH="$work/no-toolchain:$work/transport:/usr/bin:/bin" \
    PREFIX="$run/prefix" DOCS_CHECK_PREFIX="$run/prefix" DOCS_CHECK_WORK="$run" \
    RELEASE_PACKAGES="$packages" RELEASE_TAG="$tag" RELEASE_INSTALLER="$root/install.sh" \
    bash -euo pipefail "$script" > "$run/stdout" 2> "$run/stderr"; then
    fail "$document quick start block $(cat "$run/at") failed: $(tail -n 5 "$run/stderr")"
    continue
  fi
  for ((n = 1; n <= count; n++)); do
    grep -q '^# prints: ' "$work/blocks/$n" || continue
    expected=$(sed -n 's/^# prints: //p' "$run/block-$n")
    actual=$(cat "$run/out/$n")
    [[ $actual == "$expected" ]] || fail "$document quick start block $n printed '$actual', the document says '$expected'"
  done
  while IFS= read -r block; do
    [[ $(sed -n 's/^# prints: //p' "$block") == 1 ]] || fail "$document: the SELECT 1 block must document '# prints: 1'"
  done < "$work/select-one"
  printf '%s: %d quick start blocks ran against %s.\n' "$document" "$count" "${archives[0]##*/}"
done

[[ $failures == 0 ]] || { printf '%d documentation check(s) failed\n' "$failures" >&2; exit 1; }
printf 'Documented quick starts ran without a development toolchain and printed what the documents say.\n'
