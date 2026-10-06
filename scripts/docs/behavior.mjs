import assert from 'node:assert/strict';
import { mkdtempSync, symlinkSync } from 'node:fs';
import { join } from 'node:path';
import { fixtureEnv, fixtureRoot, invoke } from './runner.mjs';
import { executeUpgradeRecipe } from './guides.mjs';

export function releaseIdentity(binary, release, contract, work) {
  const result = invoke(binary, ['--build-info', '--json'], { cwd: work, env: fixtureEnv(work, binary) }, {});
  const info = JSON.parse(result.stdout);
  assert.equal(info.schema, 'redline.build-info/v1');
  assert.equal(info.tag, release);
  assert.equal(info.version, release.slice(1));
  assert.equal(info.source_sha, contract.releases[release].commit);
  assert.equal(info.repository_url, 'https://github.com/neverhuman/redline');
  assert.equal(info.repository_id, 1390165945);
  assert.equal(info.sqlite_oracle, '3.53.1');
  return { release, ...result };
}

export function upgradeChecks(binary, oldBinary, contract, upgradeSource, tempRoot) {
  const work = mkdtempSync(join(fixtureRoot(tempRoot), 'redline-docs-upgrade-'));
  const options = { cwd: work, env: fixtureEnv(work, binary) };
  const database = join(work, 'app.redline');
  const backup = join(work, 'before.redline');
  const oldInfo = releaseIdentity(oldBinary, 'v5.1.0', contract, work);
  return [
    oldInfo,
    // identity checked above; the recipe below executes the actual fenced commands.
    invoke(oldBinary, ['-batch', '-bail', database,
      "CREATE TABLE a(id INTEGER PRIMARY KEY,v TEXT); CREATE TABLE b(id INTEGER PRIMARY KEY,v TEXT); CREATE INDEX b_v ON b(v); INSERT INTO a VALUES(7,'a'); INSERT INTO b VALUES(20,'saved');"], options, { stdout: '' }),
    executeUpgradeRecipe(upgradeSource, work, options, oldBinary, database, backup, oldInfo.stdout),
    invoke(binary, ['-batch', '-bail', database,
      "SELECT id,v FROM a; SELECT id,v FROM b; INSERT INTO a(v) VALUES('new'); SELECT id,v FROM a ORDER BY id; PRAGMA integrity_check;"],
    options, { stdout: '7|a\n20|saved\n7|a\n8|new\nok\n' }),
    invoke(oldBinary, ['-batch', '-bail', backup, "SELECT id,v FROM b WHERE v='saved';"], options, { stdout: '20|saved\n' }),
    { label: 'upgrade-backup-v5.1.1', ...invoke(binary, ['-batch', '-bail', backup,
      "SELECT id,v FROM a; SELECT id,v FROM b WHERE v='saved'; PRAGMA integrity_check;"], options, { stdout: '7|a\n20|saved\nok\n' }) },
  ];
}

export function configurationChecks(binary, tempRoot) {
  const work = mkdtempSync(join(fixtureRoot(tempRoot), 'redline-docs-config-'));
  const base = { cwd: work, env: fixtureEnv(work, binary) };
  const receipts = [];
  for (const [input, expected] of [[undefined, 'strict'], ['strict', 'strict'], ['full', 'strict'], [' STRICT ', 'strict'],
    ['normal', 'normal'], ['unsafe_dev', 'unsafe_dev'], ['unsafe-dev', 'unsafe_dev'], ['off', 'unsafe_dev']]) {
    const env = { ...base.env };
    if (input !== undefined) env.REDLINEDB_DEFAULT_DURABILITY = input;
    receipts.push({ label: `durability:${input ?? 'unset'}`, ...invoke(binary, ['-batch', '-bail', join(work, `mode-${receipts.length}`),
      'CREATE TABLE t(x); PRAGMA redline_durability;'], { ...base, env }, { stdout: `${expected}\n` }) });
  }
  for (const quiet of [undefined, '0', '1']) {
    const env = { ...base.env, REDLINEDB_DEFAULT_DURABILITY: 'normal' };
    delete env.REDLINEDB_QUIET_DURABILITY;
    if (quiet !== undefined) env.REDLINEDB_QUIET_DURABILITY = quiet;
    const receipt = invoke(binary, ['-batch', '-bail', join(work, `quiet-${quiet}`), 'CREATE TABLE t(x);'], { ...base, env }, { stdout: '' });
    assert.equal(receipt.stderr.includes('durability defaults to Normal'), quiet === undefined);
    receipts.push({ label: `quiet:${quiet ?? 'unset'}`, ...receipt });
  }
  for (const [input, expected] of [['postgres', 't'], ['POSTGRES', '1'], ['', '1']]) {
    receipts.push({ label: `dialect:${input}`, ...invoke(binary, ['-batch', '-bail', ':memory:', 'SELECT 1 = 1;'],
      { ...base, env: { ...base.env, REDLINEDB_RESULT_DIALECT: input } }, { stdout: `${expected}\n` }) });
  }
  const disk = join(work, 'disk');
  receipts.push(invoke(binary, ['-batch', '-bail', disk, 'CREATE TABLE t(x); INSERT INTO t VALUES(7);'], base, { stdout: '' }));
  const link = join(work, 'linked');
  symlinkSync(disk, link, 'dir');
  receipts.push({ label: 'nofollow-follows-existing-link', ...invoke(binary, ['-nofollow', '-batch', '-bail', link, 'SELECT x FROM t;'], base, { stdout: '7\n' }) });
  receipts.push({ label: 'readonly-refuses-write', ...invoke(binary, ['-readonly', '-batch', '-bail', disk, 'INSERT INTO t VALUES(8);'], base,
    { stdout: '', exit: 1, stderrIncludes: 'readonly' }) });
  for (const flag of [['-heap', '1000000'], ['-lookaside', '16', '32'], ['-mmap', '999999'], ['-vfs', 'not-a-vfs'],
    ['-maxsize', '1'], ['-append'], ['-utf8'], ['-no-utf8'], ['-no-rowid-in-view']]) {
    receipts.push({ label: `compatibility-option:${flag.join(' ')}`, ...invoke(binary, [...flag, '-batch', ':memory:', 'SELECT 1;'], base, { stdout: '1\n' }) });
  }
  return receipts;
}
