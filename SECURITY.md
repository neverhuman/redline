# Security Policy

## Reporting a vulnerability

Report security problems privately through GitHub's private vulnerability
reporting form:

**<https://github.com/neverhuman/redline/security/advisories/new>**

The form opens a draft security advisory that only you and the RedlineDB
maintainers can see; it needs a GitHub account. Do not open a public issue,
pull request or discussion about a suspected vulnerability, and do not post
details anywhere public until the advisory is published.

No personal email address is published for security reports. If you cannot
use the form, open a public issue that says only that you need a private
channel to report a security problem, with no technical details, and a
maintainer will arrange one.

There is no bug bounty.

## Supported versions

| Version | Security fixes |
| --- | --- |
| 5.0.x | Yes. Fixes ship as 5.0 patch releases. |
| 5.x prereleases (alpha, beta, rc) | Best effort. Fixes land in the next prerelease or in 5.0.x. |
| 4.x and earlier | No. Unsupported from the 5.0.0 release; upgrade to 5.0.x. The C ABI changed in 5.0.0, so see `CHANGELOG.md` before upgrading C programs. |

## Scope

In scope, in the released code and in the repository's default branch:

- **C ABI**: the `libredlinedb` shared and static libraries
  (`libredlinedb.so.5`, `libredlinedb.5.dylib`, `libredlinedb.a`) and the
  `redlinedb.h` / `sqlite3.h` headers, for programs that follow the caller
  contract in `docs/compatibility/abi-safety.md`.
- **CLI**: `redlinedb` and `redlinedb-cli`, including dot commands and the
  files they read and write.
- **Server**: `redlinedb-server` and its network protocol, and the
  `redline-web` server and console.
- **The engine behind them**: SQL text, database files or network input that
  lead to memory unsafety, a crash of the host process, data corruption,
  lost committed data, or reading or writing outside the database.
- **Installer**: `install.sh` and the `scripts/install*.sh` scripts.
- **Release archives**: their contents, checksums, provenance and licence
  inventory, and the workflows that build them.
- **redline-testing evidence tooling**: `redline-testing`,
  `redlinedb-client-smoke`, and the evidence and report pipeline, including
  flaws that let a report claim a result that was not measured.

Out of scope:

- Programs that break the C ABI caller contract (for example by passing freed
  handles or invalid pointers); that is undefined behaviour by design.
- Resource use by queries the caller chose to run, unless it bypasses a
  documented limit.
- Vulnerabilities in third-party dependencies with no RedlineDB-specific
  impact. Report those upstream; do tell us if a release ships an affected
  version.
- Scanner output without a demonstrated impact.
- Historical material under `subrepos/redline/`.

## What to include

- The affected component and version: `redlinedb --version`, the release
  archive name, or a commit.
- Operating system and architecture.
- What an attacker can do, and what they need first (local access, a crafted
  database file, network access to a server, and so on).
- A minimal reproduction: SQL, a C program, a database file, or a command
  sequence.
- Whether the issue is known to anyone else, and any disclosure plans.
- How you would like to be credited, or that you prefer not to be.

## What happens next

- **Acknowledgement** within 3 business days.
- **Triage** within 10 business days: whether we can reproduce it, its
  severity, and the versions affected, or a request for more information.
- **Updates** as the fix progresses. You can see and comment on the draft
  advisory throughout.
- **Coordinated disclosure**: by default we publish the advisory, and request
  a CVE where one applies, when the fix is released or 90 days after the
  report, whichever comes first. We can agree a different date with you: later
  if a fix needs more time, earlier if the issue is being exploited or is
  already public.
- **Credit** in the advisory and release notes, unless you ask otherwise.

## Safe harbor

We will not pursue or support legal action against you for security research
on RedlineDB carried out in good faith under this policy. Good faith means
that you:

- test only on installations and data that you own or have permission to
  test;
- avoid privacy violations, destruction or corruption of other people's data,
  and service disruption, and stop once you have shown the problem;
- do not access, keep or share other people's data beyond the minimum needed
  to show the problem;
- report through the private route above and give us the time described here
  before disclosing.

If a third party brings legal action against you for research that followed
this policy, we will say that it did. This policy covers the RedlineDB
project only. It cannot authorize testing of systems that other people or
organizations run, even when they run RedlineDB.
