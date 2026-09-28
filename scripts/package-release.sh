#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
: "${TAG:?TAG is required}"
# Release authority. In GitHub Actions GITHUB_REPOSITORY_ID names the repository
# running the build; only the canonical one may produce release archives.
# shellcheck source=ops/release/authority.env
. ops/release/authority.env
repository_id=${GITHUB_REPOSITORY_ID:-$REDLINE_REPO_ID}
if [[ $repository_id != "$REDLINE_REPO_ID" ]]; then
  printf 'refusing to package for repository id %s: RedlineDB releases are built only in %s (id %s)\n' \
    "$repository_id" "$REDLINE_REPO_SLUG" "$REDLINE_REPO_ID" >&2
  exit 1
fi
case "$(uname -s)/$(uname -m)" in
  Linux/x86_64) platform=linux-x86_64 ;;
  Linux/aarch64|Linux/arm64) platform=linux-arm64 ;;
  Darwin/x86_64) platform=macos-x86_64 ;;
  Darwin/arm64) platform=macos-arm64 ;;
  *) printf 'unsupported packaging platform\n' >&2; exit 1 ;;
esac
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target}
output=${OUTPUT_DIR:-$root/target/packages}
mkdir -p "$output"
output=$(cd "$output" && pwd)
./scripts/build-from-source.sh --all
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
for package in redlinedb redline-web redline-testing; do
  mkdir -p "$stage/$package/bin" "$stage/$package/share/redlinedb"
  cp LICENSE NOTICE "$stage/$package/share/redlinedb/"
  printf '%s\n' "$TAG" > "$stage/$package/share/redlinedb/VERSION"
done
./scripts/install-from-source.sh --tree "$stage/redlinedb"
install -m 644 contracts/c-abi/sqlite3.h "$stage/redlinedb/include/"
install -m 755 "$CARGO_TARGET_DIR/release/redline-web" "$stage/redline-web/bin/"
install -m 755 "$CARGO_TARGET_DIR/release/redline-testing" "$CARGO_TARGET_DIR/release/redlinedb-client-smoke" "$stage/redline-testing/bin/"
cp -R subrepos/redline-testing/{corpus,metadata,schemas,templates} "$stage/redline-testing/share/redlinedb/"
commit=$(git rev-parse HEAD)
source_tree=$(git rev-parse 'HEAD^{tree}')
# Licence collection follows the dependency graph of this build's platform.
host=$(rustc -vV | sed -n 's/^host: //p')
for package in redlinedb redline-web redline-testing; do
  case "$package" in
    redlinedb) manifest=Cargo.toml ;;
    *) manifest=subrepos/$package/Cargo.toml ;;
  esac
  cargo metadata --locked --format-version 1 --filter-platform "$host" --manifest-path "$manifest" > "$stage/metadata.json"
  if [[ $package == redline-testing ]]; then
    cargo metadata --locked --format-version 1 --filter-platform "$host" --manifest-path subrepos/redline-central/Cargo.toml > "$stage/client-metadata.json"
    jq -s '{packages: ([.[].packages[]] | unique_by(.id)),
      resolve: {nodes: ([.[].resolve.nodes[]] | group_by(.id) | map({id: .[0].id, deps: [.[].deps[]]}))}}' \
      "$stage/metadata.json" "$stage/client-metadata.json" > "$stage/combined-metadata.json"
    mv "$stage/combined-metadata.json" "$stage/metadata.json"
  fi
  # The graph's roots are the artifacts actually staged: every bin/ file and
  # each lib/lib<name>.* library.
  roots=()
  for file in "$stage/$package"/bin/* "$stage/$package"/lib/lib*; do
    [[ -f $file ]] || continue
    case $file in
      */bin/*) roots+=(--bin "${file##*/}") ;;
      *) name=${file##*/lib}; roots+=(--lib "${name%%.*}") ;;
    esac
  done
  npm_project=()
  [[ $package != redline-web ]] || npm_project=(--npm-project subrepos/redline-web/apps/web)
  bash scripts/release/collect-licenses.sh --metadata "$stage/metadata.json" --share "$stage/$package/share/redlinedb" \
    "${roots[@]}" ${npm_project[@]+"${npm_project[@]}"}
  # One compact line: install.sh and publish-github-release.sh match
  # "repository_id":<id> and "tag":"<tag>" in it without a JSON parser.
  jq -cn --arg url "$REDLINE_REPO_URL" --argjson id "$REDLINE_REPO_ID" --arg tag "$TAG" --arg commit "$commit" \
    --arg tree "$source_tree" --arg platform "$platform" --arg package "$package" --arg rust "$(rustc --version)" \
    '{schema:"redline.release-build/v2",repository_url:$url,repository_id:$id,tag:$tag,commit:$commit,source_tree:$tree,platform:$platform,package:$package,rust:$rust}' \
    > "$stage/$package/share/redlinedb/build-provenance.json"
  if [[ $package == redline-web ]]; then
    npm --prefix subrepos/redline-web/apps/web sbom --sbom-format cyclonedx > "$stage/$package/share/redlinedb/frontend-sbom.cdx.json"
  fi
  # Archives hold regular files and directories only; no symlinks or devices.
  if [[ -n $(find "$stage/$package" ! -type f ! -type d -print -quit) ]]; then
    printf 'refusing to package non-regular files:\n' >&2
    find "$stage/$package" ! -type f ! -type d >&2
    exit 1
  fi
  asset=$package-$TAG-$platform.tar.gz
  tar -czf "$output/$asset" -C "$stage/$package" .
  (cd "$output"; if command -v sha256sum >/dev/null; then sha256sum "$asset"; else shasum -a 256 "$asset"; fi) > "$output/$asset.sha256"
done
printf 'Packages written to %s\n' "$output"
