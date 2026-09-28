#!/usr/bin/env bash
# Supply-chain and secret-scan receipt for a release candidate (S8-08).
#
# Runs, for the commit checked out at the repository root:
#   - ops/ci/tests/security-policy.sh (the policy still refuses what it must);
#   - `cargo audit` and `cargo deny check --config <component>/deny.toml` for
#     every component from ops/ci/security-lib.sh;
#   - `gitleaks git --log-opts=--all --redact` over the full history.
# and writes target/security/receipt.json with the candidate SHA, the sha256 of
# every lockfile and deny config, the tool versions, the HEAD and age of each
# advisory database the scans used, every exit code, the gitleaks result, and
# the reviewed exceptions (advisory ignores, deny exemptions, the gitleaks
# allowlist). Raw logs and the redacted gitleaks report sit beside it.
#
# It exits 1, after writing the receipt, when any scan fails, when an advisory
# database was last fetched more than 24 hours ago, when the checkout is
# shallow (the history scan would be partial) or when the tree has changes
# (the scans would not describe the SHA). SECURITY_RECEIPT_ALLOW_DIRTY=1 and
# SECURITY_RECEIPT_ALLOW_SHALLOW=1 relax the last two for a local trial; the
# receipt records that they were set, and says "fail" for a release.
#
#   ops/ci/security-receipt.sh
#
# Needs git, jq, sha256sum, cargo-audit, cargo-deny and gitleaks (GITLEAKS
# names the binary; default gitleaks on PATH, else target/ci/tools/gitleaks).
# No secret is printed: gitleaks runs with --redact.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
# shellcheck source=ops/ci/security-lib.sh
source ops/ci/security-lib.sh

max_age_hours=24
out=$root/target/security
logs=$out/logs
rm -rf "$out"
mkdir -p "$logs"
failures=()
fail() { failures+=("$*"); printf 'FAIL: %s\n' "$*" >&2; }

gitleaks_bin=${GITLEAKS:-}
if [[ -z $gitleaks_bin ]]; then
    if command -v gitleaks >/dev/null; then gitleaks_bin=gitleaks; else gitleaks_bin=$root/target/ci/tools/gitleaks; fi
fi
tool_version() { "$@" 2>/dev/null | head -n1 || printf 'unavailable'; }
cargo_audit_version=$(tool_version cargo audit --version)
cargo_deny_version=$(tool_version cargo deny --version)
gitleaks_version=$(tool_version "$gitleaks_bin" version)

sha=$(git rev-parse HEAD)
shallow=$(git rev-parse --is-shallow-repository)
dirty=false
[[ -z $(git status --porcelain --untracked-files=normal) ]] || dirty=true
allow_dirty=${SECURITY_RECEIPT_ALLOW_DIRTY:-0}
allow_shallow=${SECURITY_RECEIPT_ALLOW_SHALLOW:-0}
if [[ $dirty == true && $allow_dirty != 1 ]]; then fail "the working tree has changes; commit them or scan a clean checkout"; fi
if [[ $shallow == true && $allow_shallow != 1 ]]; then fail "shallow clone: the history scan would be partial (fetch with --unshallow)"; fi

# ---- policy tests ----------------------------------------------------------
policy_exit=0
GITLEAKS=$gitleaks_bin bash ops/ci/tests/security-policy.sh "$root" >"$logs/policy-tests.log" 2>&1 || policy_exit=$?
[[ $policy_exit -eq 0 ]] || fail "ops/ci/tests/security-policy.sh exited $policy_exit"

# ---- dependency scans ------------------------------------------------------
components_exit=0
components=$(security_component_configs "$root" 2>"$logs/components.log") || components_exit=$?
[[ $components_exit -eq 0 ]] || fail "component check: $(tr '\n' ' ' <"$logs/components.log")"
lockfiles_json='[]'
deny_urls=''
while IFS=$'\t' read -r component config; do
    [[ -n $component ]] || continue
    name=$(printf '%s' "$component" | tr '/.' '__')
    lockfile=Cargo.lock
    [[ $component == . ]] || lockfile=$component/Cargo.lock
    audit_exit=0
    (cd "$component" && cargo audit --file Cargo.lock --json) >"$logs/$name.audit.json" 2>"$logs/$name.audit.log" || audit_exit=$?
    [[ $audit_exit -eq 0 ]] || fail "cargo audit exited $audit_exit for $component"
    vulnerabilities=$(jq '.vulnerabilities.count // null' "$logs/$name.audit.json" 2>/dev/null || printf 'null')
    warnings=$(jq '[.warnings // {} | to_entries[] | .value | length] | add // 0' "$logs/$name.audit.json" 2>/dev/null || printf 'null')
    deny_exit=null config_sha=null exemption=null
    if [[ $config == - ]]; then
        exemption=$(security_exemption_reason "$root" "$component")
    else
        deny_exit=0
        # --locked: cargo metadata must not rewrite a stale Cargo.lock, or the
        # receipt would record a lockfile that is not the candidate's.
        (cd "$component" && cargo deny --locked -L info check --config "$root/$config") >"$logs/$name.deny.log" 2>&1 || deny_exit=$?
        [[ $deny_exit -eq 0 ]] || fail "cargo deny exited $deny_exit for $component"
        config_sha=$(sha256sum "$config" | cut -d' ' -f1)
        deny_urls+=$(sed -n 's/.*advisory database \([^ ]*\) fetched.*/\1/p' "$logs/$name.deny.log")$'\n'
    fi
    lockfiles_json=$(jq -c \
        --arg path "$lockfile" \
        --arg sha "$(sha256sum "$component/Cargo.lock" | cut -d' ' -f1)" \
        --arg config "$config" --arg config_sha "$config_sha" --arg exemption "$exemption" \
        --argjson audit_exit "$audit_exit" --argjson deny_exit "$deny_exit" \
        --argjson vulnerabilities "$vulnerabilities" --argjson warnings "$warnings" \
        '. + [{path: $path, sha256: $sha,
               deny_config: (if $config == "-" then null else $config end),
               deny_config_sha256: (if $config_sha == "null" then null else $config_sha end),
               deny_exemption: (if $exemption == "null" then null else $exemption end),
               audit_exit: $audit_exit, audit_vulnerabilities: $vulnerabilities,
               audit_warnings: $warnings, deny_exit: $deny_exit}]' <<<"$lockfiles_json")
done <<<"$components"
# The scans must not have changed the checkout the receipt describes.
if [[ $dirty == false && -n $(git status --porcelain --untracked-files=normal) ]]; then
    fail "the scans changed the working tree: $(git status --porcelain --untracked-files=normal | head -n 3 | tr '\n' ' ')"
fi

# ---- advisory databases ----------------------------------------------------
now=$(date -u +%s)
cargo_home=${CARGO_HOME:-$HOME/.cargo}
databases=''
db_record() { # path, used-by; appends to $databases (no subshell, so fail() counts)
    local path=$1 head committed fetched age
    if ! head=$(git -C "$path" rev-parse HEAD 2>/dev/null); then
        fail "advisory database $path is missing or not a git checkout"
        databases+=$(jq -nc --arg path "$path" --arg by "$2" '{used_by: $by, path: $path, head: null}')$'\n'
        return
    fi
    committed=$(git -C "$path" log -1 --format=%ct HEAD)
    fetched=$(stat -c %Y "$path/.git/FETCH_HEAD" 2>/dev/null || printf '%s' "$committed")
    age=$(((now - fetched) / 3600))
    # Compare seconds: whole hours would let a 24h59m-old database pass.
    ((now - fetched <= max_age_hours * 3600)) ||
        fail "advisory database $path was last fetched $(((now - fetched) / 60)) minutes ago (limit ${max_age_hours}h)"
    databases+=$(jq -nc --arg by "$2" --arg path "$path" --arg head "$head" \
        --arg url "$(git -C "$path" config --get remote.origin.url 2>/dev/null || true)" \
        --argjson committed "$committed" --argjson fetched "$fetched" --argjson age "$age" \
        '{used_by: $by, path: $path, url: $url, head: $head,
          head_committed_at: ($committed | todate), fetched_at: ($fetched | todate), age_hours: $age}')$'\n'
}
db_record "$cargo_home/advisory-db" cargo-audit
deny_ran=$(printf '%s' "$components" | awk -F'\t' '$2 != "-" && $2 != ""' | wc -l)
if [[ $deny_ran -gt 0 && -z ${deny_urls//$'\n'/} ]]; then
    fail "cargo deny did not report which advisory database it fetched; its freshness cannot be checked"
fi
while IFS= read -r url; do
    [[ -n $url ]] || continue
    found=''
    for dir in "$cargo_home"/advisory-dbs/*/; do
        dir=${dir%/}
        remote=$(git -C "$dir" config --get remote.origin.url 2>/dev/null || true)
        if [[ ${remote,,} == "${url,,}" || ${remote,,} == "${url,,}.git" ]]; then found=$dir; break; fi
    done
    if [[ -z $found ]]; then
        fail "cannot find the cargo-deny advisory database for $url under $cargo_home/advisory-dbs"
        continue
    fi
    db_record "$found" "cargo-deny ($url)"
done < <(printf '%s' "$deny_urls" | sort -fu)
databases_json=$(jq -sc 'unique_by(.path)' <<<"$databases")

# ---- secret scan over the full history -------------------------------------
gitleaks_exit=0
"$gitleaks_bin" git --log-opts=--all --redact --no-banner --config "$root/.gitleaks.toml" \
    --report-format json --report-path "$out/gitleaks-history.json" "$root" \
    >"$logs/gitleaks-history.log" 2>&1 || gitleaks_exit=$?
findings=$(jq 'length' "$out/gitleaks-history.json" 2>/dev/null || printf 'null')
commits=$(sed -n 's/.* \([0-9][0-9]*\) commits scanned.*/\1/p' "$logs/gitleaks-history.log" | tail -n1)
[[ $gitleaks_exit -eq 0 ]] || fail "gitleaks exited $gitleaks_exit with ${findings} finding(s); see target/security/gitleaks-history.json (redacted)"

# ---- reviewed exceptions ---------------------------------------------------
advisory_ignores='[]'
while IFS=$'\t' read -r component config; do
    [[ -n $component && $config != - ]] || continue
    ids=$(awk '/^\[/{section=$0} section=="[advisories]" && !/^[[:space:]]*#/' "$config" |
        grep -oE 'RUSTSEC-[0-9]{4}-[0-9]{4}|GHSA(-[a-z0-9]{4}){3}' | sort -u || true)
    while IFS= read -r id; do
        [[ -n $id ]] || continue
        advisory_ignores=$(jq -c --arg config "$config" --arg id "$id" '. + [{config: $config, id: $id}]' <<<"$advisory_ignores")
    done <<<"$ids"
done <<<"$components"
exemptions=$(awk -F'\t' '!/^#/ && NF >= 2 { print }' ops/ci/security-exemptions.tsv 2>/dev/null |
    jq -Rsc 'split("\n") | map(select(length > 0) | split("\t") | {component: .[0], reason: .[1]})')
allowlist=$(sed -n "s/^[[:space:]]*'''\(.*\)''',\{0,1\}[[:space:]]*$/\1/p" .gitleaks.toml | jq -Rsc 'split("\n") | map(select(length > 0))')
allowlist_target=$(sed -n 's/^regexTarget = "\(.*\)"$/\1/p' .gitleaks.toml)

# ---- receipt ---------------------------------------------------------------
result=pass
[[ ${#failures[@]} -eq 0 ]] || result=fail
failures_json=$(printf '%s\n' "${failures[@]+"${failures[@]}"}" | jq -Rsc 'split("\n") | map(select(length > 0))')
jq -n \
    --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg sha "$sha" --arg ref "$(git symbolic-ref -q --short HEAD || printf 'detached')" \
    --argjson dirty "$dirty" --argjson shallow "$shallow" \
    --argjson allow_dirty "$([[ $allow_dirty == 1 ]] && echo true || echo false)" \
    --argjson allow_shallow "$([[ $allow_shallow == 1 ]] && echo true || echo false)" \
    --arg cargo_audit "$cargo_audit_version" --arg cargo_deny "$cargo_deny_version" --arg gitleaks "$gitleaks_version" \
    --argjson max_age "$max_age_hours" --argjson databases "$databases_json" \
    --argjson lockfiles "$lockfiles_json" --argjson policy_exit "$policy_exit" \
    --argjson components_exit "$components_exit" \
    --argjson gitleaks_exit "$gitleaks_exit" --argjson findings "$findings" \
    --arg commits "${commits:-}" \
    --arg report_sha "$(sha256sum "$out/gitleaks-history.json" 2>/dev/null | cut -d' ' -f1)" \
    --argjson advisory_ignores "$advisory_ignores" --argjson exemptions "$exemptions" \
    --argjson allowlist "$allowlist" --arg allowlist_target "$allowlist_target" \
    --arg result "$result" --argjson failures "$failures_json" \
    '{schema: "redline.security-receipt.v1", generated_at: $generated_at,
      candidate: {sha: $sha, ref: $ref, dirty: $dirty, shallow: $shallow,
                  allow_dirty: $allow_dirty, allow_shallow: $allow_shallow},
      tools: {cargo_audit: $cargo_audit, cargo_deny: $cargo_deny, gitleaks: $gitleaks},
      advisory_databases: {max_age_hours: $max_age, used: $databases},
      policy_tests: {command: "ops/ci/tests/security-policy.sh", exit: $policy_exit},
      components_exit: $components_exit,
      lockfiles: $lockfiles,
      secret_scan: {command: "gitleaks git --log-opts=--all --redact", exit: $gitleaks_exit,
                    findings: $findings, commits_scanned: (if $commits == "" then null else ($commits | tonumber) end),
                    report: "target/security/gitleaks-history.json", report_sha256: $report_sha},
      reviewed_exceptions: {advisory_ignores: $advisory_ignores, deny_exemptions: $exemptions,
                            gitleaks_allowlist: {regex_target: $allowlist_target, regexes: $allowlist}},
      result: $result, failures: $failures}' >"$out/receipt.json"
printf 'wrote %s: %s\n' "target/security/receipt.json" "$result"
[[ $result == pass ]]
