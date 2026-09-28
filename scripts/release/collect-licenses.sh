#!/usr/bin/env bash
# Collect the third-party licence texts and the dependency inventory for one
# release archive.
#
#   collect-licenses.sh --metadata <cargo-metadata.json> --share <dir>
#       [--bin <name>]... [--lib <name>]... [--npm-project <dir>] [--waivers <file>]
#
# The Cargo graph is the non-dev closure of the first-party packages whose bin
# (--bin) or cdylib/staticlib (--lib) targets the archive ships: dependency
# edges whose every dep_kinds[].kind is "dev" are not followed. Every package in
# it must be covered:
#   - first-party packages (no source) must declare Apache-2.0; the archive's
#     LICENSE and NOTICE cover them;
#   - every other package's licence texts are copied into
#     licenses/<name>-<version>/: its license-file (resolved against its
#     manifest directory), top-level LICENSE*, LICENCE*, COPYING*, NOTICE* and
#     UNLICENSE* files (any case), and license/, licenses/, licence/ and
#     licences/ directories. Upper-case LICENSE*, LICENCE*, COPYING*, NOTICE*
#     and UNLICENSE* files deeper in the package (bundled C sources, vendored
#     code, Unicode tables) are copied too, keeping their relative paths.
# With --npm-project, the production closure of that npm project
# (npm ls --omit=dev --all --parseable) is collected the same way into
# licenses/npm/<name>-<version>/.
# A package without its own licence text (a declared license-file that is
# missing, or no top-level text at all; nested files do not count) fails the
# run, listed by name, unless --waivers (default
# ops/release/license-waivers.toml) holds a [[waiver]] for that exact
# ecosystem, package and version.
#
# Writes <dir>/DEPENDENCIES.tsv (one header row, then one row per package:
# name, version, license, repository, ecosystem, source, license_texts,
# waiver), <dir>/sbom.cdx.json (the Cargo graph) and <dir>/licenses/.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
usage() { printf 'usage: %s --metadata <file> --share <dir> [--bin <name>]... [--lib <name>]... [--npm-project <dir>] [--waivers <file>]\n' "$0" >&2; exit 64; }
metadata='' share='' npm_project='' bins='' libs=''
waivers=$root/ops/release/license-waivers.toml
while [[ $# -gt 0 ]]; do
  [[ $# -ge 2 ]] || usage
  case $1 in
    --metadata) metadata=$2 ;;
    --share) share=$2 ;;
    --bin) bins+=$2$'\n' ;;
    --lib) libs+=$2$'\n' ;;
    --npm-project) npm_project=$2 ;;
    --waivers) waivers=$2 ;;
    *) usage ;;
  esac
  shift 2
done
[[ -n $metadata && -n $share && -n $bins$libs ]] || usage
[[ -f $waivers ]] || { printf 'licence waiver file not found: %s\n' "$waivers" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$share/licenses"
share=$(cd "$share" && pwd)

# Waivers: a TOML subset of [[waiver]] tables holding key = "string" pairs.
# Anything else is refused rather than guessed at.
awk '
  function die(msg) { printf "%s:%d: %s\n", FILENAME, FNR, msg > "/dev/stderr"; bad = 1; exit 1 }
  function flush(   k) {
    if (!open) return
    for (k in required) if (!(k in f)) die("[[waiver]] is missing " k)
    if (f["ecosystem"] != "cargo" && f["ecosystem"] != "npm") die("ecosystem must be \"cargo\" or \"npm\"")
    k = f["ecosystem"] " " f["package"] " " f["version"]
    if (k in seen) die("duplicate waiver for " k)
    seen[k] = 1
    printf "%s\t%s\t%s\n", f["ecosystem"], f["package"], f["version"]
    for (k in f) delete f[k]
    open = 0
  }
  BEGIN { required["ecosystem"]; required["package"]; required["version"]; required["license"]; required["reason"] }
  /^[[:space:]]*(#.*)?$/ { next }
  /^\[\[waiver\]\][[:space:]]*(#.*)?$/ { flush(); open = 1; next }
  /^[a-z]+[[:space:]]*=[[:space:]]*"[^"]*"[[:space:]]*(#.*)?$/ {
    if (!open) die("key outside a [[waiver]] table")
    key = $0; sub(/[[:space:]]*=.*$/, "", key)
    if (!(key in required)) die("unknown waiver key " key)
    if (key in f) die("duplicate key " key)
    value = $0; sub(/^[^"]*"/, "", value); sub(/".*$/, "", value)
    if (value == "" || value ~ /[\\\t]/) die(key " must be a non-empty string without tabs or escapes")
    f[key] = value
    next
  }
  { die("unsupported line; only [[waiver]] tables of key = \"string\" pairs are allowed") }
  END { if (bad) exit 1; flush() }
' "$waivers" > "$work/waivers.tsv"

# collect <package-dir> <dest> <declared-licence-file|->: copy the package's
# licence texts and set own=1 when the package has its own text (the declared
# file exists, or a top-level text or licence directory is non-empty). It is
# never called as a condition, so errexit still stops the run on a failed copy.
collect() {
  local dir=$1 dest=$2 declared=$3 path rel
  own=1
  mkdir -p "$dest"
  if [[ $declared != - ]]; then
    if [[ -f $dir/$declared ]]; then
      case $declared in
        /* | ../* | */../*) rel=$(basename "$declared") ;;
        *) rel=${declared#./} ;;
      esac
      mkdir -p "$(dirname "$dest/$rel")"
      cp "$dir/$declared" "$dest/$rel"
    else
      own=0
    fi
  fi
  while IFS= read -r -d '' path; do
    if [[ -d $path ]]; then cp -RL "$path" "$dest/"; elif [[ -f $path ]]; then cp "$path" "$dest/"; fi
  done < <(find "$dir" -mindepth 1 -maxdepth 1 \( \
    \( ! -type d \( -iname 'LICENSE*' -o -iname 'LICENCE*' -o -iname 'COPYING*' -o -iname 'NOTICE*' -o -iname 'UNLICENSE*' \) \) \
    -o \( -type d \( -iname license -o -iname licenses -o -iname licence -o -iname licences \) \) \) -print0)
  [[ -n $(find "$dest" -type f -size +0 -print -quit) ]] || own=0
  while IFS= read -r -d '' path; do
    [[ -f $path ]] || continue
    rel=${path#"$dir"/}
    mkdir -p "$(dirname "$dest/$rel")"
    cp "$path" "$dest/$rel"
  done < <(find "$dir" \( -name node_modules -o -name .git -o -name target \) -prune -o ! -type d -path "$dir/*/*" \
    \( -name 'LICENSE*' -o -name 'LICENCE*' -o -name 'COPYING*' -o -name 'NOTICE*' -o -name 'UNLICENSE*' \) -print0)
}

# cover <ecosystem> <name> <version> <package-dir> <dest-rel> <declared>:
# collect, else accept a waiver, else record the package as missing. Sets
# texts and waiver to the inventory's license_texts and waiver columns.
cover() {
  local ecosystem=$1 name=$2 version=$3 dir=$4 declared=$6
  texts=$5 waiver=-
  collect "$dir" "$share/$texts" "$declared"
  if [[ $own == 0 ]]; then
    if awk -F '\t' -v e="$ecosystem" -v n="$name" -v v="$version" \
      '$1 == e && $2 == n && $3 == v { found = 1 } END { exit !found }' "$work/waivers.tsv"; then
      waiver=waived
    else
      printf '%s %s %s (%s)\n' "$ecosystem" "$name" "$version" "$dir" >> "$work/missing"
    fi
  fi
  if [[ -z $(find "$share/$texts" -type f -print -quit) ]]; then
    rm -rf "${share:?}/$texts"
    texts=-
  fi
}

# The shipped Cargo graph.
lines_to_json() { printf '%s' "$1" | jq -R 'select(length > 0)' | jq -s .; }
jq --argjson bins "$(lines_to_json "$bins")" --argjson libs "$(lines_to_json "$libs")" '
  if .resolve == null then error("cargo metadata has no resolve graph (was it run with --no-deps?)") else . end
  | (.resolve.nodes
      | map({key: .id, value: [.deps[] | select((.dep_kinds | length) == 0 or any(.dep_kinds[]; .kind != "dev")) | .pkg]})
      | from_entries) as $graph
  | [.packages[] | select(.source == null)] as $local
  | ([$bins[] as $b | {artifact: ("bin " + $b), ids: [$local[] | select(any(.targets[]; any(.kind[]; . == "bin") and .name == $b)) | .id]}]
     + [$libs[] as $l | {artifact: ("lib " + $l), ids: [$local[] | select(any(.targets[]; any(.kind[]; . == "cdylib" or . == "staticlib") and .name == $l)) | .id]}]) as $artifacts
  | {unmatched: [$artifacts[] | select(.ids == []) | .artifact],
     ids: ({seen: {}, todo: [$artifacts[].ids[]]}
       | until(.todo == [];
           .todo[0] as $id
           | .todo |= .[1:]
           | if .seen[$id] then . else .seen[$id] = true | .todo += ($graph[$id] // []) end)
       | .seen)}
' "$metadata" > "$work/graph.json"
unmatched=$(jq -r '.unmatched[]' "$work/graph.json")
[[ -z $unmatched ]] || { printf 'no first-party Cargo target produces the shipped artifact(s):\n%s\n' "$unmatched" >&2; exit 1; }
# shellcheck disable=SC2016 # a jq program, not shell
shipped='(.packages | map(select($graph[0].ids[.id])) | unique_by(.id) | sort_by(.name, .version))'
jq -r --slurpfile graph "$work/graph.json" "$shipped"'[]
  | [.name, .version, (.license // (if .license_file then "SEE LICENSE IN " + .license_file else "UNKNOWN" end)), (.repository // "-"), (if .source == null then "workspace" else .source end),
     (.manifest_path | sub("/Cargo.toml$"; "")), (.license_file // "-")]
  | map(if . == "" then "-" else . end) | @tsv' "$metadata" > "$work/cargo.tsv"
jq --slurpfile graph "$work/graph.json" '{bomFormat: "CycloneDX", specVersion: "1.5", version: 1,
  components: [('"$shipped"')[] | {type: "library", name, version, purl: ("pkg:cargo/" + .name + "@" + .version),
    licenses: (if .license then [{expression: .license}] else [] end)}]}' "$metadata" > "$share/sbom.cdx.json"

inventory=$share/DEPENDENCIES.tsv
printf 'name\tversion\tlicense\trepository\tecosystem\tsource\tlicense_texts\twaiver\n' > "$inventory"
while IFS=$'\t' read -r name version license repository source dir declared; do
  if [[ $source == workspace ]]; then
    [[ $license == Apache-2.0 ]] || printf 'first-party cargo package %s %s declares %s, not Apache-2.0\n' "$name" "$version" "$license" >> "$work/missing"
    printf '%s\t%s\t%s\t%s\tcargo\tworkspace\tLICENSE\t-\n' "$name" "$version" "$license" "$repository" >> "$inventory"
    continue
  fi
  cover cargo "$name" "$version" "$dir" "licenses/$name-$version" "$declared"
  printf '%s\t%s\t%s\t%s\tcargo\t%s\t%s\t%s\n' "$name" "$version" "$license" "$repository" "$source" "$texts" "$waiver" >> "$inventory"
done < "$work/cargo.tsv"

if [[ -n $npm_project ]]; then
  project=$(cd "$npm_project" && pwd -P)
  (cd "$project" && npm ls --omit=dev --all --parseable) > "$work/npm-paths"
  # npm redacts UUID-like path segments in its output, so each dependency is
  # located by its node_modules path inside the project, not the printed prefix.
  [[ $(head -n 1 "$work/npm-paths") != */node_modules/* ]] || { printf 'npm ls did not start with the project\n' >&2; exit 1; }
  tail -n +2 "$work/npm-paths" | while IFS= read -r path; do
    [[ $path == */node_modules/* ]] || { printf 'unexpected npm ls path: %s\n' "$path" >&2; exit 1; }
    path=$project/node_modules/${path#*/node_modules/}
    jq -r --arg path "$path" '
      def spdx: if type == "object" then .type elif type == "array" then map(spdx) | join(" OR ") else . end;
      (((.license // .licenses) | spdx) // "UNKNOWN") as $license
      | [.name, .version, $license, ((.repository | if type == "object" then .url else . end) // "-"), $path]
      | map(if . == null or . == "" then "-" else tostring end) | @tsv' "$path/package.json"
  done | sort -u -t $'\t' -k1,1 -k2,2 > "$work/npm.tsv"
  while IFS=$'\t' read -r name version license repository dir; do
    [[ $name != *..* ]] || { printf 'refusing npm package name %s\n' "$name" >&2; exit 1; }
    declared=-
    [[ $license != 'SEE LICENSE IN '* ]] || declared=${license#SEE LICENSE IN }
    cover npm "$name" "$version" "$dir" "licenses/npm/$name-$version" "$declared"
    printf '%s\t%s\t%s\t%s\tnpm\tnpm\t%s\t%s\n' "$name" "$version" "$license" "$repository" "$texts" "$waiver" >> "$inventory"
  done < "$work/npm.tsv"
fi

if [[ -s $work/missing ]]; then
  printf 'licence collection failed; fix these packages, or waive a missing text in %s:\n' "$waivers" >&2
  cat "$work/missing" >&2
  exit 1
fi
printf 'Collected licences for %d packages into %s\n' "$(($(wc -l < "$inventory") - 1))" "$share"
