import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { checkReleaseAndGuideClaims } from './assert-release-claims.mjs';
import { executeUpgradeRecipe, parseUpgradeRecipe } from './guides.mjs';
import { fixtureRoot } from './runner.mjs';

const commit = '9277455d5ad008252053a81d18add39b8cdc8f7b';
const sourceInputs = 'c293f3b7ed8a9dc6ade3dce29147fc041517515fff9cd2501a33d32ce207d065';
const fence = [
  'old_redlinedb=/path/to/v5.1.0/redlinedb',
  'database=/path/to/app.redline',
  'backup=/path/to/app-before-v5.1.1.redline',
  '"$old_redlinedb" --build-info --json',
  '"$old_redlinedb" backup "$database" "$backup" --physical',
  '',
].join('\n');
const upgrade = `# Upgrading\n\nv5.1.0 directories open on v5.1.1.\nSource commit \`${commit}\`.\n\n\`\`\`bash upgrade\n${fence}\`\`\`\n`;
const limitations = [
  'Engine::begin(Serializable) is refused with unsupported isolation level.',
  'SQL accepts SET TRANSACTION ISOLATION LEVEL SERIALIZABLE as a session label.',
  'The issue tracker had no open issues when checked on 2026-10-06.',
].join('\n');
const runs = [
  ['v5.1.1-rc.1', '97b1e36c5746b6921789c52409fcc33f92a27736', 37058828370, 'Rejected: custody inventory was not a durability receipt.'],
  ['v5.1.1-rc.2', '810ea3410b9b2fff05077e17a027eb17a797303a', 37063949030, 'Rejected at publication.'],
  ['v5.1.1-rc.3', commit, 37069402770, 'Published and qualified.'],
  ['v5.1.1', commit, 37071604124, 'Published and qualified, 2026-10-02.'],
];
const releasing = [
  '# Releasing',
  '',
  ...runs.map(([tag, sha, id, result]) => `| \`${tag}\` | \`${sha}\` | [${id}](https://github.com/neverhuman/redline/actions/runs/${id}) | ${result} |`),
  '',
  `Source-input SHA-256 \`${sourceInputs}\`.`,
  '',
].join('\n');
const contract = {
  releases: { 'v5.1.1': { commit } },
  qualification: { sourceInputsSha256: sourceInputs, runs: runs.map(([tag, headSha, id, result]) => ({
    tag, headSha, id, conclusion: result.startsWith('Published') ? 'success' : 'cancelled',
  })) },
};
const issues = {
  schema: 'redline.docs-issues/v1',
  repository: 'neverhuman/redline',
  capturedAt: '2026-10-06T01:11:37.707Z',
  issues: [],
};
const pages = {
  'docs/upgrade-v5.1.1.md': upgrade,
  'docs/known-limitations.md': limitations,
  'docs/RELEASING.md': releasing,
};
const read = (overlay = {}) => file => overlay[file] ?? pages[file] ?? 'corrected guidance';

function stubBinary(work, backupStatus) {
  const path = join(work, 'old-redlinedb');
  writeFileSync(path, `#!/bin/bash
if [[ "$1" == "--build-info" ]]; then printf '%s' '{"tag":"v5.1.0"}'; exit 0; fi
if [[ "$1" == backup && "$4" == --physical ]]; then exit ${backupStatus}; fi
exit 1
`);
  chmodSync(path, 0o755);
  return path;
}

test('assert-release-claims accepts the three guides when they match the contract and issue snapshot', () => {
  checkReleaseAndGuideClaims(read(), contract, issues);
});

test('a wrong release run id fails inside assert-release-claims', () => {
  const wrong = releasing.replace('37058828370', '99999999999');
  assert.throws(() => checkReleaseAndGuideClaims(read({ 'docs/RELEASING.md': wrong }), contract, issues), /wrong v5\.1\.1-rc\.1 run/);
});

test('a wrong source-input hash fails inside assert-release-claims', () => {
  const wrong = releasing.replace(sourceInputs, '0'.repeat(64));
  assert.throws(() => checkReleaseAndGuideClaims(read({ 'docs/RELEASING.md': wrong }), contract, issues), /wrong stable source-input hash/);
});

test('an empty issue snapshot rejects a non-empty issue sentence', () => {
  const wrong = limitations.replace('had no open issues when checked on 2026-10-06', 'had 4 open issues when checked on 2026-10-06');
  assert.throws(() => checkReleaseAndGuideClaims(read({ 'docs/known-limitations.md': wrong }), contract, issues), /issue count\/date differs/);
});

test('the upgrade backup fence is executed, so a failing backup is not a bash -n pass', () => {
  const syntax = spawnSync('bash', ['-n'], { input: fence, encoding: 'utf8' });
  assert.equal(syntax.status, 0);
  const work = mkdtempSync(join(fixtureRoot(), 'upgrade-fence-'));
  const binary = stubBinary(work, 1);
  assert.throws(() => executeUpgradeRecipe(upgrade, work, { env: { ...process.env } }, binary,
    join(work, 'app.redline'), join(work, 'before.redline'), '{"tag":"v5.1.0"}'), /"exit":1(?:,|})/);
});

test('the same fence passes when the backup command exits 0', () => {
  const work = mkdtempSync(join(fixtureRoot(), 'upgrade-fence-ok-'));
  const binary = stubBinary(work, 0);
  const receipt = executeUpgradeRecipe(upgrade, work, { env: { ...process.env } }, binary,
    join(work, 'app.redline'), join(work, 'before.redline'), '{"tag":"v5.1.0"}');
  assert.equal(receipt.exit, 0);
  assert.equal(receipt.fixtureAssignments.old_redlinedb, binary);
});

test('fixture substitution cannot hide an invalid upgrade assignment', () => {
  const broken = upgrade.replace('database=/path/to/app.redline', 'database=/path/to/"app.redline');
  assert.throws(() => parseUpgradeRecipe(broken), /invalid shell syntax/);
});

test('fixture substitution cannot hide extra upgrade operations or assignments', () => {
  for (const suffix of ['; printf undocumented', '\ndatabase=/path/to/another.redline']) {
    const broken = upgrade.replace('database=/path/to/app.redline', 'database=/path/to/app.redline' + suffix);
    assert.throws(() => parseUpgradeRecipe(broken), /placeholder/);
  }
});
