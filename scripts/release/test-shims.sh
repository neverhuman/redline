#!/usr/bin/env bash
# Shared stand-ins for the package tests (sourced, not run):
#
#   write_no_toolchain <dir>  cargo, rustc, cc, gcc, clang, node, npm, npx,
#                             just and rtk that fail when called, so a test
#                             proves the binary path needs none of them.
#   write_release_transport <dir>
#                             a curl that serves the canonical installer URL
#                             (raw.githubusercontent.com/neverhuman/redline/
#                             $RELEASE_TAG/install.sh -> $RELEASE_INSTALLER)
#                             and release downloads of $RELEASE_TAG
#                             ($RELEASE_PACKAGES/<asset>) from local files, and
#                             answers releases/latest with $RELEASE_TAG's page.
#                             Every other URL fails, as does an asset the
#                             directory lacks (404).

write_no_toolchain() {
  local dir=$1 tool
  mkdir -p "$dir"
  for tool in cargo rustc cc gcc clang node npm npx just rtk; do
    printf '#!/bin/sh\necho "%s: development tool invoked on the binary path" >&2\nexit 1\n' "$tool" > "$dir/$tool"
    chmod +x "$dir/$tool"
  done
}

write_release_transport() {
  mkdir -p "$1"
  cat > "$1/curl" <<'BIN'
#!/usr/bin/env bash
set -eu
url='' out='' format=''
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) out=$2; shift 2 ;;
    -w) format=$2; shift 2 ;;
    https://*) url=$1; shift ;;
    *) shift ;;
  esac
done
send() { if [[ -n $out && $out != - ]]; then cp "$1" "$out"; else cat "$1"; fi; }
repo=https://github.com/neverhuman/redline
case "$url" in
  "$repo/releases/latest")
    [[ $format == '%{url_effective}' ]] || exit 22
    printf '%s/releases/tag/%s' "$repo" "$RELEASE_TAG" ;;
  "https://raw.githubusercontent.com/neverhuman/redline/$RELEASE_TAG/install.sh") send "$RELEASE_INSTALLER" ;;
  "$repo/releases/download/$RELEASE_TAG/"*)
    [[ -f $RELEASE_PACKAGES/${url##*/} ]] || { printf 'curl: (22) The requested URL returned error: 404\n' >&2; exit 22; }
    send "$RELEASE_PACKAGES/${url##*/}" ;;
  *) printf 'release transport: no route to %s\n' "$url" >&2; exit 22 ;;
esac
BIN
  chmod +x "$1/curl"
}
