#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
case "${1:?package lane required}" in
  build) exec bash scripts/package-release.sh ;;
  smoke)
    bash scripts/test-package-licenses.sh
    bash scripts/test-binaries.sh
    exec bash scripts/test-package-ffi.sh
    ;;
  installer) exec bash scripts/test-installer.sh ;;
  runtime) exec bash scripts/test-packages.sh ;;
  # The real install.sh against this platform's candidate archive through a
  # file-transport curl, then a C program linked against the installation.
  native-install) exec bash scripts/test-native-install.sh ;;
  *) exit 64 ;;
esac
