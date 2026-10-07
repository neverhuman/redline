// Independently recalculate the historical CLI table from its raw samples.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { median, sha256 } from './abba-summary.mjs';

const bundle = 'benchmark-results/sqlite-parity/releases/v5.0.0';
const summaryBytes = fs.readFileSync(path.join(bundle, 'summary.json'));
const summary = JSON.parse(summaryBytes);
assert(summary.publishable && summary.publication_blockers.length === 0);
const versions = summary.labels.map(label => {
  const runs = label.runs.map(meta => {
    const bytes = fs.readFileSync(path.join(bundle, meta.raw));
    assert.equal(sha256(bytes), meta.raw_sha256);
    const cases = new Map();
    for (const line of bytes.toString().trim().split('\n')) {
      const r = JSON.parse(line);
      if (!cases.has(r.case_id)) cases.set(r.case_id, []);
      cases.get(r.case_id).push({ status: r.status, role: r.sample_role,
        target: r.target_elapsed_ns, reference: r.reference_elapsed_ns });
    }
    const passed = [...cases].filter(([, rs]) => rs.length === 4 && rs.every(r => r.status === 'passed')).map(([id]) => id);
    const failed = [...cases.values()].filter(rs => rs.some(r => r.status === 'failed')).length;
    const skipped = [...cases.values()].filter(rs => rs.some(r => r.status === 'skipped')).length;
    assert.equal(passed.length, meta.passed); assert.equal(failed, meta.failed); assert.equal(skipped, meta.skipped);
    return { cases, passed: new Set(passed), failed, skipped, run: meta.run };
  });
  return { label, runs };
});
const common = [...versions[0].runs[0].passed].filter(id => versions.every(v => v.runs.every(r => r.passed.has(id))));
assert.equal(common.length, summary.common_pass_set.cases);
const results = versions.map(({ label, runs }, index) => {
  const figures = runs.map(run => {
    const ratios = common.map(id => {
      const samples = run.cases.get(id);
      assert.deepEqual(samples.map(r => r.role).sort(), ['measured:1', 'measured:2', 'measured:3', 'warmup']);
      const measured = samples.filter(r => r.role.startsWith('measured:'));
      assert(measured.every(r => r.target > 0 && r.reference > 0));
      return median(measured.map(r => r.target)) / median(measured.map(r => r.reference));
    }).sort((a, b) => a - b);
    return { run: run.run, median: median(ratios), p95: ratios[Math.ceil(0.95 * ratios.length) - 1],
      passed: run.passed.size, failed: run.failed, skipped: run.skipped };
  });
  for (const field of ['median', 'p95']) {
    const values = figures.map(r => r[field]);
    const actual = { value: median(values), min: Math.min(...values), max: Math.max(...values) };
    for (const k of ['value', 'min', 'max']) assert(Math.abs(actual[k] - label.common[field][k]) <= actual[k] * 1e-12);
  }
  const passed = [...runs[0].passed].filter(id => runs.every(r => r.passed.has(id))).length;
  assert.equal(passed, label.passed_cases);
  const fmt = f => `${f.value.toFixed(3)}× (${f.min.toFixed(3)}–${f.max.toFixed(3)})`;
  let delta = '—';
  if (index) {
    const prev = summary.labels[index - 1].common.median, now = label.common.median;
    delta = now.min <= prev.max && prev.min <= now.max ? 'within noise'
      : `${now.value >= prev.value ? '+' : ''}${((now.value / prev.value - 1) * 100).toFixed(1)}%`;
  }
  const row = `| ${label.label} | \`${label.source_commit.slice(0, 9)}\` | ${passed} | ${fmt(label.common.median)} | ${fmt(label.common.p95)} | ${delta} |`;
  assert(fs.readFileSync('README.md', 'utf8').includes(row), `README figure drift: ${label.label}`);
  return { label: label.label, figures, passedEveryRun: passed, row };
});
console.log(JSON.stringify({ bundle, summarySha256: sha256(summaryBytes), commonCases: common.length, results,
  scope: 'Recalculation of immutable historical receipts, not new v5.1.1 CLI timings', allPublishedFiguresMatch: true }, null, 2));
