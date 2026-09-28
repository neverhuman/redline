# shellcheck shell=bash
# Where each release package keeps its own records (DX-08). Sourced, not run.
#
# Every archive carries LICENSE, NOTICE, VERSION, licenses/, DEPENDENCIES.tsv,
# sbom.cdx.json and build-provenance.json (whose "package" field names it).
# The core package keeps them in share/redlinedb/, the path install.sh and the
# release checks read. redline-web and redline-testing keep theirs in
# share/redlinedb/components/<package>/, so extracting the packages together,
# in any order, never replaces another package's records.
# redline-testing's runtime data (corpus/, metadata/, schemas/, templates/)
# stays in share/redlinedb/, where the runner looks for it.

# shellcheck disable=SC2034 # read by the scripts that source this file
release_packages=(redlinedb redline-web redline-testing)

# package_share <package>: the archive-relative directory of its records.
package_share() {
  case $1 in
    redlinedb) printf 'share/redlinedb\n' ;;
    redline-web | redline-testing) printf 'share/redlinedb/components/%s\n' "$1" ;;
    *) printf 'unknown release package: %s\n' "$1" >&2; return 1 ;;
  esac
}

# archive_package <path>: the package of <package>-v<version>-<platform>.tar.gz.
archive_package() {
  local name=${1##*/} package
  for package in redline-testing redline-web redlinedb; do
    if [[ $name == "$package"-v[0-9]* ]]; then
      printf '%s\n' "$package"
      return 0
    fi
  done
  printf 'not a release archive name: %s\n' "$name" >&2
  return 1
}

# archive_provenance <archive>: its build-provenance.json, from its own records.
archive_provenance() {
  local package
  package=$(archive_package "$1") || return 1
  tar -xzOf "$1" "./$(package_share "$package")/build-provenance.json"
}
