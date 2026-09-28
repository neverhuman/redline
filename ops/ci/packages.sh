#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
case "${1:?package lane required}" in
  build)
    # A release passes its tag; any other build is v<workspace version>-dev.
    if [ -z "${TAG:-}" ]; then TAG=$(bash ops/ci/release-version.sh dev-tag); fi
    export TAG
    exec bash scripts/package-release.sh
    ;;
  smoke)
    bash scripts/test-package-licenses.sh
    bash scripts/test-binaries.sh
    exec bash scripts/test-package-ffi.sh
    ;;
  installer) exec bash scripts/test-installer.sh ;;
  runtime) exec bash scripts/test-packages.sh ;;
  # The real install.sh against this platform's candidate archive through a
  # file-transport curl, then a C program linked against the installation;
  # and the documented quick start with no development toolchain on PATH.
  native-install) exec bash scripts/test-native-install.sh ;;
  quickstart) exec bash scripts/test-docs-quickstart.sh ;;
  # release-build.yml's post-publication check, run against the candidate.
  published-check) exec bash scripts/test-verify-published.sh ;;
  *) exit 64 ;;
esac
