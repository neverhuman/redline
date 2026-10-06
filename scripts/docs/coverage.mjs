import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const load = (root, file) => JSON.parse(readFileSync(join(root, file), 'utf8'));
export function renderCoverage(root) {
  const evidenceFile = 'benchmark-results/sqlite-parity/latest/official-evidence.processed.json';
  const report = load(root, evidenceFile);
  const postgres = load(root, 'metadata/beyond_sqlite/postgres-regression.json');
  const failures = load(root, 'metadata/sqlite_parity/known-failures.json').failures;
  const rows = [];
  for (const [suite, result] of Object.entries(report.suite_summaries)) {
    const rejections = suite === 'beyond_sqlite' ? postgres.declared_rejections.length : 0;
    const unsupported = suite === 'beyond_sqlite' ? postgres.declared_unsupported.length : 0;
    const known = failures.filter(failure => failure.suite === suite);
    assert.equal(result.total, result.passed + result.failed + result.skipped, `${suite}: inconsistent totals`);
    if (suite === 'beyond_sqlite') assert.equal(result.failed, unsupported + postgres.failed_cases.length);
    else if (suite !== 'rql_phase1') assert.equal(result.failed, known.length);
    rows.push(`| ${suite} | ${result.passed - rejections} | ${rejections} | ${unsupported} | ${result.failed - unsupported} | ${result.skipped} |`);
  }
  const digest = createHash('sha256').update(readFileSync(join(root, evidenceFile))).digest('hex');
  return [
    '<!-- docs-coverage:begin -->',
    '<!-- Generated from official-evidence.processed.json and parity policies; do not edit by hand. -->',
    '| Lane | Matching results | Expected rejections | Declared unsupported | Other known failures | Skipped |',
    '| --- | ---: | ---: | ---: | ---: | ---: |', ...rows, '',
    'SQLite matching results include the declared stand-ins; they do not establish module support.', '',
    `Evidence source: \`${report.official_evidence.source_commit}\`; processed evidence SHA-256: \`${digest}\`.`, '',
    'Known SQLite failures (also present in the memory lane):', '',
    ...failures.filter(failure => failure.suite === 'sqlite_parity').map(failure => `- \`${failure.case_id}\` — ${failure.name}: ${failure.reason}`),
    '<!-- docs-coverage:end -->',
  ].join('\n');
}

export function renderKnownLimitations(root, template) {
  template ??= readFileSync(join(root, 'scripts/docs/known-limitations.md.in'), 'utf8');
  return template.replace(/<!-- docs-coverage:begin -->[\s\S]*?<!-- docs-coverage:end -->/, renderCoverage(root));
}

export function checkCoverage(root, source, template) {
  const block = source.match(/<!-- docs-coverage:begin -->[\s\S]*?<!-- docs-coverage:end -->/);
  assert.ok(block, 'known-limitations.md: missing generated coverage block');
  assert.equal(block[0], renderCoverage(root), 'known-limitations.md: coverage differs from committed evidence; regenerate');
  assert.equal(source, renderKnownLimitations(root, template), 'known-limitations.md: generated page differs from its authored template; regenerate');
}

export function renderRqlCoverage(root) {
  const report = load(root, 'benchmark-results/sqlite-parity/latest/official-evidence.processed.json');
  const policy = load(root, 'subrepos/redline-testing/corpus/sqlite_parity/scope-policy.json');
  const result = report.suite_summaries.rql_phase1;
  const exclusions = policy.exceptions.filter(entry => entry.suite === 'rql_phase1');
  assert.deepEqual(exclusions.map(entry => entry.case_id).sort(), [...result.skipped_case_ids].sort());
  const rewrite = exclusions.filter(entry => /^(lower|parse) SQL/.test(entry.reason)).length;
  const divergence = exclusions.filter(entry => entry.reason.startsWith('known RQL/SQLite runtime-output divergence')).length;
  const expectedErrors = exclusions.filter(entry => entry.reason.startsWith('expected-error cases')).length;
  assert.equal(rewrite + divergence + expectedErrors, result.skipped, 'unclassified RQL skip reasons');
  return [
    '<!-- rql-coverage:begin -->',
    '<!-- Generated from committed official evidence and scope policy; do not edit by hand. -->',
    `The \`rql_phase1\` suite passed ${result.passed} of ${result.total} cases, failed ${result.failed},`,
    `and skipped ${result.skipped}. Every skip is declared in advance:`, '',
    `- ${rewrite} cases the phase-1 rewriter cannot parse or lower.`,
    `- ${divergence} known differences between RQL and SQLite output.`,
    `- ${expectedErrors} expected-error cases the suite does not run through RQL.`, '',
    `Evidence source: \`${report.official_evidence.source_commit}\`. A skip is not a pass.`,
    '<!-- rql-coverage:end -->',
  ].join('\n');
}

export function checkRqlCoverage(root, source) {
  const block = source.match(/<!-- rql-coverage:begin -->[\s\S]*?<!-- rql-coverage:end -->/);
  assert.ok(block, 'missing generated RQL coverage block');
  assert.equal(block[0], renderRqlCoverage(root), 'RQL coverage differs from committed evidence; regenerate');
}
