#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
# shellcheck source=ops/release/authority.env
. "$root/ops/release/authority.env"
export GITHUB_REPOSITORY_ID=$REDLINE_REPO_ID
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
cat > "$work/bin/docker" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$ARM64_TEST_LOG"
if [[ $1 == buildx && $2 == inspect ]]; then exit "${ARM64_BUILDER_EXISTS:-0}"; fi
STUB
chmod +x "$work/bin/docker"
export PATH="$work/bin:$PATH" ARM64_TEST_LOG="$work/docker.log"
export TAG=v5.1.1-test OUTPUT_DIR="$work/output with spaces" REDLINE_ARM64_BUILDER=redline-test-builder
export REDLINE_ARM64_CACHE_DIR="$work/cache"
bash "$root/ops/ci/arm64-packages.sh" build
grep -Fq 'buildx build --builder redline-test-builder --platform linux/arm64' "$ARM64_TEST_LOG"
grep -Fq -- '--target archives --build-arg TAG=v5.1.1-test' "$ARM64_TEST_LOG"
grep -Fq -- "--output type=local,dest=$OUTPUT_DIR" "$ARM64_TEST_LOG"
if grep -Fq 'buildx create' "$ARM64_TEST_LOG"; then exit 1; fi
ARM64_BUILDER_EXISTS=1 bash "$root/ops/ci/arm64-packages.sh" runtime
grep -Fq 'buildx create --name redline-test-builder --driver docker-container' "$ARM64_TEST_LOG"
grep -Fq -- '--target verified' "$ARM64_TEST_LOG"
grep -Fq -- "--build-context packages=$root/target/packages" "$ARM64_TEST_LOG"
if grep -Eq 'privileged|binfmt|buildx rm|--use' "$ARM64_TEST_LOG"; then exit 1; fi
docker_calls=$(wc -l < "$ARM64_TEST_LOG")
if GITHUB_REPOSITORY_ID=1 bash "$root/ops/ci/arm64-packages.sh" build > "$work/authority.log" 2>&1; then
  echo 'noncanonical repository was allowed to package ARM64 archives' >&2
  exit 1
else
  [[ $? == 1 ]]
fi
grep -Fq 'refusing ARM64 packaging for repository id 1' "$work/authority.log"
[[ $(wc -l < "$ARM64_TEST_LOG") == "$docker_calls" ]]
if bash "$root/ops/ci/arm64-packages.sh" unknown > "$work/unknown.log" 2>&1; then
  echo 'unknown ARM64 packaging lane was accepted' >&2
  exit 1
else
  [[ $? == 64 ]]
fi
printf 'ARM64 helper: reusable private builder, exact target, archive-only runtime, canonical authority and invalid-lane checks pass.\n'
