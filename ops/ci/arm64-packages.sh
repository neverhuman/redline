#!/usr/bin/env bash
# Real ARM64 cross-build and emulated runtime checks on a Linux X64 runner.
# BuildKit supplies QEMU privately: no host binfmt registration or service changes.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
# Keep the caller's release authority at the container boundary: Docker does
# not inherit GITHUB_REPOSITORY_ID for package-release.sh to check itself.
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
repository_id=${GITHUB_REPOSITORY_ID:-$REDLINE_REPO_ID}
if [[ $repository_id != "$REDLINE_REPO_ID" ]]; then
  printf 'refusing ARM64 packaging for repository id %s: canonical repository is %s (id %s)\n' \
    "$repository_id" "$REDLINE_REPO_SLUG" "$REDLINE_REPO_ID" >&2
  exit 1
fi
case ${1:-} in
  build) stage=archives; output=${OUTPUT_DIR:-$root/dist/release-packages} ;;
  runtime) stage=verified; output=$root/target/ci/arm64-runtime ;;
  *) printf 'usage: arm64-packages.sh build|runtime\n' >&2; exit 64 ;;
esac
builder=${REDLINE_ARM64_BUILDER:-redline-packages-arm64}
if ! docker buildx inspect "$builder" >/dev/null 2>&1; then
  # Precreate buildx's state volume on fast scratch when available. This
  # builder owns its cache; no global builder selection or pruning occurs.
  cache=${REDLINE_ARM64_CACHE_DIR:-${RUNNER_TOOL_CACHE:-$root/target/ci}/arm64-buildkit}
  if [[ -d /mnt/fast-scratch && -z ${REDLINE_ARM64_CACHE_DIR:-} ]]; then
    cache=/mnt/fast-scratch/redline-ci-$(id -u)/arm64-buildkit
    if [[ ! -d $cache ]]; then
      sudo install -d -m 0755 -o "$(id -u)" -g "$(id -g)" "$cache"
    fi
  fi
  mkdir -p "$cache"
  cache=$(cd "$cache" && pwd)
  docker volume create --driver local --opt type=none --opt "device=$cache" --opt o=bind \
    "buildx_buildkit_${builder}0_state" >/dev/null
  docker buildx create --name "$builder" --driver docker-container \
    --driver-opt image=moby/buildkit@sha256:0168606be2315b7c807a03b3d8aa79beefdb31c98740cebdffdfeebf31190c9f >/dev/null
fi
packages=${ARM64_PACKAGES_DIR:-$root/target/packages}
mkdir -p "$output" "$packages"
output=$(cd "$output" && pwd)
packages=$(cd "$packages" && pwd)
tag=${TAG:-$(bash ops/ci/release-version.sh dev-tag)}
docker buildx build --builder "$builder" --platform linux/arm64 --progress plain \
  --file ops/ci/arm64-packages.Dockerfile --target "$stage" --build-arg "TAG=$tag" \
  --build-context "packages=$packages" --output "type=local,dest=$output" .
