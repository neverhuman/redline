import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fencedBlocks } from './markdown.mjs';
import { invoke, sha256 } from './runner.mjs';

export function checkGuideClaims(read, contract, issues) {
  const upgrade = read('docs/upgrade-v5.1.1.md');
  const releasing = read('docs/RELEASING.md');
  const limitations = read('docs/known-limitations.md');
  assert.ok(upgrade.includes(contract.releases['v5.1.1'].commit), 'upgrade guide: wrong stable source commit');
  assert.ok(upgrade.includes('v5.1.0') && upgrade.includes('v5.1.1'), 'upgrade guide: missing release identities');
  parseUpgradeRecipe(upgrade);
  assert.ok(!/Serializable isolation is refused/.test(limitations), 'limitations: SQL accepts serializable as a session label');
  assert.ok(limitations.includes('Engine::begin(Serializable)'), 'limitations: distinguish the Rust isolation API');
  for (const run of contract.qualification.runs) {
    const line = releasing.split('\n').find(line => line.startsWith(`| \`${run.tag}\` |`));
    assert.ok(line, `release guide: missing ${run.tag}`);
    assert.ok(line.includes(`\`${run.headSha}\``), `release guide: wrong ${run.tag} commit`);
    assert.ok(line.includes(`[${run.id}](https://github.com/neverhuman/redline/actions/runs/${run.id})`), `release guide: wrong ${run.tag} run`);
    assert.ok(line.includes(run.conclusion === 'success' ? 'Published and qualified' : 'Rejected'), `release guide: wrong ${run.tag} outcome`);
  }
  assert.ok(releasing.includes(`\`${contract.qualification.sourceInputsSha256}\``), 'release guide: wrong stable source-input hash');
  assert.equal(issues.schema, 'redline.docs-issues/v1');
  assert.equal(issues.repository, 'neverhuman/redline');
  assert.ok(Array.isArray(issues.issues), 'issue snapshot: missing issues');
  const count = issues.issues.length;
  const date = new Date(issues.capturedAt).toISOString().slice(0, 10);
  const normalized = limitations.replace(/\s+/g, ' ');
  const claim = count === 0 ? `had no open issues when checked on ${date}` : `had ${count} open issues when checked on ${date}`;
  assert.ok(normalized.includes(claim), 'limitations: issue count/date differs from the committed issue snapshot');
  return { check: 'new guide claims bound to release and dated issue receipts', runs: contract.qualification.runs, issueCount: count, issueDate: date };
}

export function parseUpgradeRecipe(source) {
  const blocks = fencedBlocks(source).filter(block => block.info === 'bash upgrade');
  assert.equal(blocks.length, 1, 'upgrade guide: mark exactly one bash upgrade recipe');
  const block = blocks[0];
  const syntax = spawnSync('bash', ['-n'], { input: block.body, encoding: 'utf8' });
  assert.ifError(syntax.error);
  assert.equal(syntax.status, 0, `upgrade guide: invalid shell syntax: ${syntax.stderr}`);
  for (const name of ['old_redlinedb', 'database', 'backup']) {
    const assignments = block.body.split('\n').filter(line => new RegExp(`^${name}=`).test(line));
    assert.equal(assignments.length, 1, `upgrade guide: expected one ${name} placeholder`);
    assert.match(assignments[0], new RegExp(`^${name}=/path/to/[A-Za-z0-9_./-]+$`), `upgrade guide: expected a plain ${name} placeholder`);
  }
  const operations = block.body.split('\n').filter(line => line.trim() && !line.startsWith('#') && !/^(old_redlinedb|database|backup)=/.test(line));
  assert.deepEqual(operations, ['"$old_redlinedb" --build-info --json', '"$old_redlinedb" backup "$database" "$backup" --physical'], 'upgrade guide: unsupported command or backup flag');
  return block;
}

export function executeUpgradeRecipe(source, work, options, oldBinary, database, backup, infoOutput) {
  const block = parseUpgradeRecipe(source);
  const recipe = block.body
    .replace(/^old_redlinedb=.*$/m, 'old_redlinedb="$REDLINE_DOCS_OLD_BINARY"')
    .replace(/^database=.*$/m, 'database="$REDLINE_DOCS_UPGRADE_DATABASE"')
    .replace(/^backup=.*$/m, 'backup="$REDLINE_DOCS_UPGRADE_BACKUP"');
  const env = { ...options.env, REDLINE_DOCS_OLD_BINARY: oldBinary, REDLINE_DOCS_UPGRADE_DATABASE: database, REDLINE_DOCS_UPGRADE_BACKUP: backup };
  return { label: 'upgrade guide physical-backup fence', sourceSha256: sha256(block.body), fixtureAssignments: { old_redlinedb: oldBinary, database, backup }, ...invoke('bash', ['-euo', 'pipefail', '-c', recipe], { cwd: work, env }, { stdout: infoOutput }) };
}
