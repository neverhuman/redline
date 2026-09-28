#!/usr/bin/env bash
# Supply-chain scan components, shared by ops/ci/security-family.sh,
# ops/ci/security-receipt.sh and ops/ci/tests/security-policy.sh. Source it.
#
# A component is the directory of a tracked Cargo.lock, except test fixtures
# (anything under ops/ci/tests/ or a fixtures/ directory). Each component needs
# a deny.toml beside its Cargo.lock; the scans pass it to cargo-deny with
# --config, so a component never falls back to a parent directory's policy.
# A component without one fails the scan unless ops/ci/security-exemptions.tsv
# lists it with a reason. An exempt component still gets `cargo audit`.

# Print the component directories of the git checkout at $1, one per line.
security_components() {
    local root=$1 lockfiles lockfile
    lockfiles=$(git -C "$root" ls-files -- '*Cargo.lock')
    while IFS= read -r lockfile; do
        case $lockfile in
            '' | ops/ci/tests/* | fixtures/* | */fixtures/*) continue ;;
        esac
        dirname "$lockfile"
    done <<<"$lockfiles"
}

# Print the exemption reason for component $2 of the checkout at $1 and
# succeed, or fail when it is not exempt.
security_exemption_reason() {
    local root=$1 component=$2 table="$1/ops/ci/security-exemptions.tsv"
    [[ -f $table ]] || return 1
    awk -F'\t' -v component="$component" '
        $1 == component && NF >= 2 && $2 != "" { print $2; found = 1; exit }
        END { exit !found }
    ' "$table"
}

# Print each rule the cargo-deny config $1 does not set. Every component
# denies yanked crates, unknown registries and git sources, and wildcard
# version requirements.
security_policy_gaps() {
    local config=$1 rule
    for rule in 'yanked' 'unknown-registry' 'unknown-git' 'wildcards'; do
        grep -Eq "^[[:space:]]*${rule}[[:space:]]*=[[:space:]]*\"deny\"" "$config" ||
            printf '%s = "deny"\n' "$rule"
    done
}

# Print "component<TAB>deny.toml path" for each component of the checkout at
# $1, or "component<TAB>-" for an exempt one. Fail, naming each, when a
# component has neither a deny.toml nor an exemption, when its deny.toml does
# not set a rule security_policy_gaps checks, or when there is no component.
security_component_configs() {
    local root=$1 components component gaps failed=0
    components=$(security_components "$root")
    if [[ -z $components ]]; then
        printf 'FAIL: no tracked Cargo.lock under %s\n' "$root" >&2
        return 1
    fi
    while IFS= read -r component; do
        if [[ -f $root/$component/deny.toml ]]; then
            gaps=$(security_policy_gaps "$root/$component/deny.toml")
            if [[ -n $gaps ]]; then
                printf 'FAIL: %s/deny.toml does not set: %s\n' "$component" "$(tr '\n' ' ' <<<"$gaps")" >&2
                failed=1
                continue
            fi
            printf '%s\t%s\n' "$component" "$component/deny.toml"
        elif security_exemption_reason "$root" "$component" >/dev/null; then
            printf '%s\t-\n' "$component"
        else
            printf 'FAIL: %s/Cargo.lock has no deny.toml beside it and no entry in ops/ci/security-exemptions.tsv\n' \
                "$component" >&2
            failed=1
        fi
    done <<<"$components"
    return "$failed"
}
