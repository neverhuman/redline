#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
source_root=${ARM64_TEST_SOURCE_ROOT:-$root}
mapfile -t repository_variables < <(git rev-parse --local-env-vars)
unset "${repository_variables[@]}"
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
bash "$source_root/ops/ci/arm64-packages.sh" build
grep -Fq 'buildx build --builder redline-test-builder --platform linux/arm64' "$ARM64_TEST_LOG"
grep -Fq -- '--target archives --build-arg TAG=v5.1.1-test' "$ARM64_TEST_LOG"
grep -Fq -- "--output type=local,dest=$OUTPUT_DIR" "$ARM64_TEST_LOG"
if grep -Fq 'buildx create' "$ARM64_TEST_LOG"; then exit 1; fi
sha=$(git -C "$source_root" rev-parse HEAD)
tree=$(git -C "$source_root" rev-parse 'HEAD^{tree}')
grep -Fq -- "--build-arg SOURCE_SHA=$sha --build-arg SOURCE_TREE=$tree" "$ARM64_TEST_LOG" || {
  printf 'ARM64 build omitted checked-out SHA %s / tree %s\n' "$sha" "$tree" >&2
  exit 1
}
dockerfile=$source_root/ops/ci/arm64-packages.Dockerfile
grep -Fq 'id=redline-arm64-target,target=/workspace/target,sharing=locked' "$dockerfile"
grep -Fq 'ENV OUTPUT_DIR=/packages/${SOURCE_SHA}' "$dockerfile"
grep -Fq 'COPY --from=build /packages/${SOURCE_SHA}/ /' "$dockerfile"
grep -Fq 'package-build-custody.sh checkout "$SOURCE_SHA" "$SOURCE_TREE" && bash ops/ci/packages.sh build && bash ops/ci/package-build-custody.sh archives' "$dockerfile"
bash "$source_root/ops/ci/arm64-packages.sh" check-sources
grep -Fq -- '--target source-verified' "$ARM64_TEST_LOG"
grep -Fq 'FROM source-bound AS build' "$dockerfile"
ignore=$dockerfile.dockerignore
for rule in '**/.env' '**/.env.*' '**/*.log'; do
  grep -Fxq "$rule" "$ignore"
done
# Exceptions must name tracked public files exactly. A wildcard can upload
# ignored private logs while the source checkout guard still reports clean.
{
  printf '!subrepos/redline-central/.env.example\n'
  git -C "$source_root" ls-files 'subrepos/redline-split-ops/release-evidence/*.log' \
    'benchmark-results/perf/regressions/main-bac7e9186-vs-v5.1.1-abba/run.log' \
    'benchmark-results/perf/regressions/main-bac7e9186-vs-v5.1.1-abba/unit-tests.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/two-core/coordinator.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/two-core/session-1/measure.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/two-core/session-2/measure.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/one-core/coordinator.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/one-core/session-1/measure.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/one-core/session-2/measure.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/scheduling-barrier/coordinator.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/scheduling-barrier/measure.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/rejected-affinity/coordinator.log' \
    'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/rejected-affinity/measure.log' | sed 's/^/!/'
} | LC_ALL=C sort > "$work/expected-exceptions"
grep '^!' "$ignore" | LC_ALL=C sort > "$work/actual-exceptions"
diff -u "$work/expected-exceptions" "$work/actual-exceptions"
while IFS= read -r exception; do
  [[ $exception != *'*'* && $exception != *'?'* && $exception != *'['* ]]
  git -C "$source_root" ls-files --error-unmatch -- "${exception#!}" >/dev/null
done < "$work/actual-exceptions"
preflight=$(grep -nF 'timeout 900 bash ops/ci/arm64-packages.sh check-sources' "$source_root/ops/ci/pr-ci.sh" | cut -d: -f1)
families=$(grep -nF 'bash scripts/ci-family.sh all' "$source_root/ops/ci/pr-ci.sh" | cut -d: -f1)
[[ $preflight -lt $families ]]
ARM64_BUILDER_EXISTS=1 bash "$source_root/ops/ci/arm64-packages.sh" runtime
grep -Fq 'buildx create --name redline-test-builder --driver docker-container' "$ARM64_TEST_LOG"
grep -Fq -- '--target verified' "$ARM64_TEST_LOG"
grep -Fq -- "--build-context packages=$source_root/target/packages" "$ARM64_TEST_LOG"
# Identity is checked before costly FFI/installer tests; a stale CLI must
# fail immediately, while every existing runtime gate remains present.
grep -Fq 'package-build-custody.sh archives /workspace/target/packages "$SOURCE_SHA" "$SOURCE_TREE" && bash ops/ci/packages.sh runtime && bash scripts/test-package-licenses.sh' "$dockerfile"
if grep -Eq 'privileged|binfmt|buildx rm|--use' "$ARM64_TEST_LOG"; then exit 1; fi
docker_calls=$(wc -l < "$ARM64_TEST_LOG")
if GITHUB_REPOSITORY_ID=1 bash "$source_root/ops/ci/arm64-packages.sh" build > "$work/authority.log" 2>&1; then
  echo 'noncanonical repository was allowed to package ARM64 archives' >&2
  exit 1
else
  [[ $? == 1 ]]
fi
grep -Fq 'refusing ARM64 packaging for repository id 1' "$work/authority.log"
[[ $(wc -l < "$ARM64_TEST_LOG") == "$docker_calls" ]]
if bash "$source_root/ops/ci/arm64-packages.sh" unknown > "$work/unknown.log" 2>&1; then
  echo 'unknown ARM64 packaging lane was accepted' >&2
  exit 1
else
  [[ $? == 64 ]]
fi
printf 'ARM64 helper: reusable private builder, exact target, archive-only runtime, canonical authority and invalid-lane checks pass.\n'
bash "$root/ops/ci/tests/fixture-git-isolation.sh" ops/ci/tests/package-build-custody.sh
