#!/usr/bin/env bash
set -euo pipefail

REMOTE="${REMOTE:-xbabe1}"
# The checkout on the bench host; a relative path is under the remote login directory.
REMOTE_DIR="${REMOTE_DIR:-RedlineDB}"

rsync -a --delete \
  --exclude '.git/' \
  --exclude 'target/' \
  ./ "${REMOTE}:${REMOTE_DIR}/"
