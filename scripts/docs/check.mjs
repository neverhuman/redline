import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { fencedBlocks, markdownFiles } from './markdown.mjs';
import { executeBlock, fixtureEnv, fixtureRoot, invoke, sha256 } from './runner.mjs';
import { configurationChecks, releaseIdentity, upgradeChecks } from './behavior.mjs';
import { checkCoverage, checkRqlCoverage } from './coverage.mjs';
import { checkReleaseAndGuideClaims } from './assert-release-claims.mjs';
import { checkLinks } from './links.mjs';
import { serverChecks } from './server.mjs';

const args = process.argv.slice(2);
const option = name => { const at = args.indexOf(name); return at < 0 ? undefined : args[at + 1]; };
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(option('--root') ?? '.');
const overlay = option('--overlay') ? JSON.parse(readFileSync(option('--overlay'))) : {};
const read = file => overlay[file] ?? readFileSync(join(root, file), 'utf8');
const files = [...new Set([...markdownFiles(root), ...Object.keys(overlay)])].sort();
const contractBytes = readFileSync(option('--contract') ?? join(here, 'release-contract.json'));
const contract = JSON.parse(contractBytes);
const issues = JSON.parse(readFileSync(option('--issues') ?? join(here, 'issues.json')));
const scopes = JSON.parse(readFileSync(option('--scopes') ?? join(here, 'scope.json')));
const oracles = option('--oracles') ? JSON.parse(readFileSync(option('--oracles'))).receipts : [];
const currentOracle = oracles.find(entry => entry.tag === 'v5.1.1');
const oldOracle = oracles.find(entry => entry.tag === 'v5.1.0');
const binary = resolve(option('--binary') ?? currentOracle?.binary);
const oldBinary = resolve(option('--old-binary') ?? oldOracle?.binary);
const server = resolve(option('--server') ?? currentOracle?.server);
const work = mkdtempSync(join(fixtureRoot(), 'redline-docs-check-'));
const output = option('--output') ?? join(root, 'target/docs-check/receipt.json');
const rlib = option('--rlib') ?? join(root, 'target/debug/libredlinedb.rlib');
const rustc = spawnSync('rustup', ['which', 'rustc'], { cwd: root, encoding: 'utf8' });
assert.equal(rustc.status, 0, rustc.stderr);
const receipt = { schema: 'redline.docs-check/v1', contractSha256: sha256(contractBytes), binary, oldBinary, server,
  documents: [], links: [], executions: [], templates: [], deferred: [], claims: [], excluded: [], failures: [] };

function rustFixture(file) {
  if (/\/0[67]-/.test(file)) return {
    prefix: 'use redlinedb::{Database,params};\nfn main()->redlinedb::Result<()>{\nlet db=Database::create(std::env::temp_dir().join("fragment.redline"))?;\nlet mut conn=db.connect()?;\nconn.execute("CREATE TABLE note(id INTEGER PRIMARY KEY,body TEXT)",())?;\nlet body="hello";\n',
    suffix: 'let value:String=conn.query_row("SELECT body FROM note",())?;println!("{value}");Ok(())}\n',
  };
  if (file.endsWith('08-embed.md')) return {
    suffix: 'fn main()->redlinedb::Result<()>{let db=open()?;let mut conn=db.connect()?;let value:i64=conn.query_row("SELECT 1",())?;println!("{value}");Ok(())}\n',
  };
  return {};
}

function syntax(block, file) {
  // Operator placeholders are deliberately not executed. Replace only
  // angle-bracket placeholders before parsing the shell grammar.
  const source = block.body.replace(/<[A-Za-z][A-Za-z0-9 _./-]*>/g, 'DOCS_TEMPLATE');
  const result = spawnSync('bash', ['-n'], { input: source, encoding: 'utf8' });
  assert.equal(result.status, 0, `${file}:${block.line}: invalid shell template: ${result.stderr}`);
  receipt.templates.push({ file, line: block.line, sourceSha256: sha256(block.body), check: 'bash -n (not executed)' });
}

try {
  receipt.claims.push(checkReleaseAndGuideClaims(read, contract, issues));
  receipt.executions.push(releaseIdentity(binary, 'v5.1.1', contract, work));
  for (const help of contract.help) receipt.executions.push({ check: 'published help', ...invoke(binary, help.args,
    { cwd: work, env: fixtureEnv(work, binary) }, { stdout: help.stdout, exit: help.exit }) });
  for (const flag of contract.cliOptions) receipt.executions.push({ check: 'published CLI option', ...invoke(binary,
    [...flag.args, ...(flag.args.includes('-batch') ? [] : ['-batch']), ':memory:', 'SELECT 1;'],
    { cwd: work, env: fixtureEnv(work, binary) }, { stdout: flag.stdout }) });
  receipt.executions.push({ check: 'published server help', ...invoke(server, ['--help'],
    { cwd: work, env: fixtureEnv(work, binary) }, contract.serverHelp) });
  checkCoverage(root, read('docs/known-limitations.md'), option('--known-template') ? readFileSync(option('--known-template'), 'utf8') : undefined);
  for (const file of files) {
    const scope = scopes.documents[file] ?? (file.startsWith('docs/audits/') ? { kind: 'peer-owned', reason: 'Grok owns post-release audit evidence.' } : undefined);
    assert.ok(scope?.kind && scope?.reason, `${file}: classify documentation scope before claiming it was checked`);
    const source = read(file);
    if (scope.kind === 'current') receipt.links.push(...checkLinks(root, file, source, read,
      new Set([...Object.keys(overlay), ...(option('--overlay') ? ['scripts/docs/issues.json','scripts/docs/known-limitations.md.in'] : [])])));
    const blocks = fencedBlocks(source);
    receipt.documents.push({ file, scope, sourceSha256: sha256(source), blocks: blocks.length });
    for (const block of blocks) {
      const [language, annotation] = block.info.split(/\s+/);
      const label = `${file}:${block.line}`;
      if (annotation === 'doctest') {
        receipt.executions.push(executeBlock(binary, block, label, { root, rlib, rustc: rustc.stdout.trim(), rustFixture: rustFixture(file) }));
      } else if (language === 'rust' && annotation === 'readme') {
        assert.equal(block.body, readFileSync(join(root, 'crates/redlinedb/examples/readme.rs'), 'utf8'), 'README Rust example differs from compiled example');
        receipt.deferred.push({ label, check: 'byte-match only here; compilation and two runs belong to fast preflight and are not executed by this checker', sourceSha256: sha256(block.body) });
      } else if (scope.kind !== 'current') {
        receipt.excluded.push({ label, info: block.info, reason: scope.reason, sourceSha256: sha256(block.body) });
      } else if (annotation === 'upgrade') {
        receipt.claims.push({ label, check: 'fence registered for execution by the published two-version upgrade fixture below', sourceSha256: sha256(block.body) });
      } else if (annotation === 'quickstart') {
        syntax(block, file);
        receipt.templates.at(-1).check = 'installer execution by test-docs-quickstart.sh; shell syntax here';
      } else if (['bash', 'sh', 'shell'].includes(language) || (!language && /^(rtk |jankurai )/m.test(block.body))) {
        assert.ok(annotation === 'template' || scopes.nonExecutable[`${file}#sha256=${sha256(block.body)}`], `${label}: annotate executable examples or register the operator-template reason`);
        syntax(block, file);
      } else if (file === 'docs/rql.md' && language === 'json') {
        JSON.parse(block.body);
        const result = invoke(binary, ['--rql', ':memory:'], { cwd: work, env: fixtureEnv(work, binary), input: block.body }, { stdout: 'Ada\n' });
        assert.ok(source.includes('prints `Ada`'), 'RQL output paragraph must match the executed example');
        receipt.executions.push({ label, sourceSha256: sha256(block.body), ...result });
      } else {
        const key = `${file}#sha256=${sha256(block.body)}`;
        const reason = scopes.nonExecutable[key];
        assert.ok(reason, `${label}: unclassified example; mark doctest or explain why it cannot execute`);
        receipt.excluded.push({ label, info: block.info, reason, sourceSha256: sha256(block.body) });
      }
    }
    if (scope.kind === 'current') {
      for (const match of source.matchAll(/tag\s*=\s*"(v\d+\.\d+\.\d+)"/g)) assert.equal(match[1], 'v5.1.1', `${file}: stale dependency tag`);
      for (const match of source.matchAll(/raw\.githubusercontent\.com\/neverhuman\/redline\/(v\d+\.\d+\.\d+)\/install\.sh/g)) assert.equal(match[1], 'v5.1.1', `${file}: stale installer`);
      for (const block of blocks.filter(value => value.body.includes('jankurai audit '))) {
        assert.ok(block.body.includes('--no-score-history'), `${file}: audit may write score history into source`);
        assert.ok(!/--(?:json|md)\s+\.jankurai\//.test(block.body), `${file}: audit writes into source`);
      }
    }
  }
  const evidence = JSON.parse(read('benchmark-results/sqlite-parity/latest/official-evidence.processed.json'));
  const policy = JSON.parse(read('subrepos/redline-testing/corpus/sqlite_parity/scope-policy.json'));
  const rql = evidence.suite_summaries.rql_phase1;
  const exclusions = policy.exceptions.filter(entry => entry.suite === 'rql_phase1');
  assert.deepEqual(exclusions.map(entry => entry.case_id).sort(), [...rql.skipped_case_ids].sort(), 'RQL exclusions differ from measured skips');
  for (const file of ['README.md', 'docs/rql.md']) {
    checkRqlCoverage(root, read(file));
    const text = read(file);
    assert.match(text, new RegExp(`passed\\s+${rql.passed} (?:of|/) ${rql.total} cases`));
    assert.match(text, new RegExp(`(?:skipped|Skipped) ${rql.skipped}`));
    assert.ok(text.includes(evidence.official_evidence.source_commit.slice(0, 9)), `${file}: stale RQL evidence commit`);
  }
  receipt.executions.push(...configurationChecks(binary), ...upgradeChecks(binary, oldBinary, contract, read('docs/upgrade-v5.1.1.md')));
  receipt.executions.push({ label: 'published TCP server round trip', ...await serverChecks(server) });
  receipt.executions.push({ label: 'first.sql', ...invoke(binary, ['-batch', '-bail', ':memory:'],
    { cwd: work, env: fixtureEnv(work, binary), input: read('docs/manual/examples/first.sql') }, { stdout: '1|hello\n' }) });
  receipt.complete = true;
} catch (error) {
  receipt.complete = false;
  receipt.failures.push({ message: error.message, stack: error.stack });
  process.exitCode = 1;
} finally {
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, JSON.stringify(receipt, null, 2) + '\n');
  console.log(`Documentation checks: ${receipt.executions.length} executions, ${receipt.templates.length} shell templates parsed, ${receipt.deferred.length} checks delegated with their execution boundary, ${receipt.excluded.length} explicitly unexecuted blocks, ${receipt.failures.length} failures. Receipt: ${output}`);
  if (receipt.failures.length) console.error(receipt.failures[0].message);
}
