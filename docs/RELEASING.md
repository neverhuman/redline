# Qualifying and publishing a release

`neverhuman/redline` is the release authority. Tags and assets are immutable.
A source correction goes through a reviewed PR and a new candidate tag;
never move a published tag or replace an archive. The release workflow
publishes assets. Do not upload replacement assets by hand.

This guide records the v5.1.1 process and the repository helpers that implement
it. [Release policy](release.md) describes the acceptance contract.

## Prepare source and evidence

Coordinate canonical checkout ownership and paths on the BF board. Start a
branch from freshly fetched `origin/main`, bump the workspace with
`bash ops/ci/release-version.sh bump X.Y.Z`, and write the changelog, upgrade
notes and `docs/releases/vX.Y.Z.md`. Merge only after local `just pr-ci`, an
eligible independent approval of the exact head, and green
`RedlineDB/required` on that head. Use a rebase merge with
`--match-head-commit`; never rebase main or use `--admin`.

Record the source-preparation commit and
`bash ops/ci/source-inputs-sha256.sh`. Performance and parity claims come from
real committed raw evidence. Generate README and release-note blocks through
their report tools, then check them. A scoreboard bundle must pass
`redline-scoreboard summarize --check`, have `publishable: true` and no
blockers. Known failures and skips stay visible. An evidence-only commit must
preserve the recorded source-input hash.

For v5.1.1, paired measurements used a common harness and SQLite library;
Normal ran on tmpfs and Strict used a real disk with a SQLite control paired
with each engine repetition. The committed bundle documents the method.
The strict process-kill receipt is independently checked by
`ops/ci/durability-receipt.sh`. A process-kill result does not certify power
loss. Preserve rejected runs with their archive hashes; never substitute
their figures into a report.

## Create an immutable candidate

First require green CI on the final main commit and passing generated-report
checks. From a clean checkout of that commit, create an annotated tag and
validate it **before** pushing. These are templates for a new release, not
commands to rerun for the already-published v5.1.1:

```bash template
version=X.Y.Z
candidate="v$version-rc.1"
release_commit=FULL_COMMIT_SHA
git tag -a "$candidate" "$release_commit" -m "RedlineDB $candidate"
bash ops/ci/release-version.sh check "$candidate"
git push origin "refs/tags/$candidate"
```

`check` requires matching workspace versions and documents, an annotated tag
at HEAD, canonical repository identity, and no existing GitHub release for
the tag. A stable tag must also be on `origin/main`. Consequently it is not a
post-publication health check: it deliberately refuses an existing release.

The tag starts `.github/workflows/release-build.yml`: validation, reusable
`ci.yml` acceptance, package builds and native runtime checks, acceptance
manifest generation and verification, signed attestations, publication, then
pinned-installer verification on Linux x86_64/ARM64 and macOS Intel/ARM64.
Ordinary PR/main CI now cross-builds and emulates Linux ARM64 and leaves native
macOS packaging to tag runs. That does not replace the release's native
qualification jobs.

## Keep a qualification custody directory

Create a fresh ignored directory outside source for each tag. Retain the run
and release JSON, archives and checksum sidecars, manifest, downloaded
receipt artifacts, attestation verification logs, installer logs, installed
build-info JSON, exit statuses and a hash inventory. Store the inventory
**outside** the receipt directories it inventories; otherwise it changes the
receipt digest. Do not parse an arbitrary custody JSON as a durability
receipt.

The v5.1.1 executor's local wrapper was
`target/review/strict-window/qualify-release.sh TAG COMMIT RUN_ID`. It is an
ignored, host-specific helper, not a checked-in release tool. The stable
custody directory was
`/mnt/fast-scratch/jope-prime-offroot/redlinedb-v511-tests/v5.1.1-qualification`.
Keep bulky archives off the root filesystem. Do not assume that ignored
wrapper exists on the next maintainer's machine; the repository verification
commands below are the portable entry points.

Download every release asset and each receipt artifact listed in
`ops/release/acceptance-receipts` from the successful run. Keep each artifact
in `receipts/NAME/`. Verify from a **clean checkout of the exact tagged
commit**, with the annotated tag fetched:

```bash template
tag=vX.Y.Z-rc.1
run_id=SUCCESSFUL_RELEASE_RUN_ID
release_commit=FULL_COMMIT_SHA
custody=/path/to/fresh/off-root/custody
gh release download "$tag" --repo neverhuman/redline --dir "$custody/assets"
while read -r artifact rest; do
  case "$artifact" in ''|'#'*) continue;; esac
  gh run download "$run_id" --repo neverhuman/redline \
    --name "$artifact" --dir "$custody/receipts/$artifact"
done < ops/release/acceptance-receipts
bash scripts/release/verify-acceptance.sh "$custody/assets/release-acceptance.v1.json" \
  --packages "$custody/assets" --receipts "$custody/receipts" --tag "$tag"
for asset in "$custody/assets"/*.tar.gz "$custody/assets/release-acceptance.v1.json"; do
  gh attestation verify "$asset" --repo neverhuman/redline \
    --signer-workflow neverhuman/redline/.github/workflows/release-build.yml
done
VERIFY_LATEST=0 bash scripts/release/verify-published.sh \
  "$tag" "$release_commit" "$custody/installed prefix"
```

`--receipts` checks the downloaded receipt contents against their digests;
omitting it only checks the manifest's receipt declarations. The acceptance
manifest binds the commit, source tree, source-input hash, compiler, job
results, exact archive set and receipt digests. Also check the workflow run
concluded success and all four `verify-published` jobs passed.

The installer must report the candidate tag and exact commit through
`redlinedb --build-info --json`, including when installed under a prefix with
spaces. Preserve the logs and explicit exit codes before considering the
candidate qualified.

## Publish stable at the qualified commit

Create the annotated stable tag at **the same commit** as the accepted
candidate, run `release-version.sh check` before pushing, then repeat the
whole workflow and qualification. Set `VERIFY_LATEST=1` for the stable
installer check. Confirm `/releases/latest` names the stable tag and that the
GitHub release is neither a draft nor a prerelease. Recheck the README's
pinned installer and generated benchmark blocks; leave committed evidence
and qualification custody intact.

If qualification needs a source fix, merge that fix normally, refresh any
affected evidence and source hashes, and use the next unused candidate tag.
Do not reuse a failed tag.

## v5.1.1 receipts

The immutable tags show the actual progression:

| Tag | Commit | Release-build run | Result |
| --- | --- | --- | --- |
| `v5.1.1-rc.1` | `97b1e36c5746b6921789c52409fcc33f92a27736` | [37058828370](https://github.com/neverhuman/redline/actions/runs/37058828370) | Rejected: custody inventory mistaken for a durability receipt; no release published. |
| `v5.1.1-rc.2` | `810ea3410b9b2fff05077e17a027eb17a797303a` | [37063949030](https://github.com/neverhuman/redline/actions/runs/37063949030) | Rejected at publication: the second durability claim gate had the same defect; no release published. |
| `v5.1.1-rc.3` | `9277455d5ad008252053a81d18add39b8cdc8f7b` | [37069402770](https://github.com/neverhuman/redline/actions/runs/37069402770) | Published and qualified. |
| `v5.1.1` | `9277455d5ad008252053a81d18add39b8cdc8f7b` | [37071604124](https://github.com/neverhuman/redline/actions/runs/37071604124) | Published and qualified, 2026-10-02. |

The stable manifest records source-input SHA-256
`c293f3b7ed8a9dc6ade3dce29147fc041517515fff9cd2501a33d32ce207d065`.
Do not confuse it with the earlier parity/scoreboard source freeze: the two
receipt-gate fixes changed release scripts before rc.3 and stable.
