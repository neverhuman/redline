// A contended-host diagnostic, separate from release scoreboard publication.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';

export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
export const median = xs => {
  const a = [...xs].sort((x, y) => x - y);
  assert(a.length > 0, 'empty sample');
  return (a[Math.floor((a.length - 1) / 2)] + a[Math.floor(a.length / 2)]) / 2;
};
// serde writes signed 64-bit digests; preserve them without JS rounding.
export const parseRecord = raw => JSON.parse(raw.replace(/"digest":(-?\d+)/g, '"digest":"$1"'));

export function bootstrap(ratios, seed, draws = 10000) {
  assert(ratios.length >= 12 && ratios.every(x => Number.isFinite(x) && x > 0));
  assert(Number.isInteger(seed) && seed > 0 && draws >= 10000);
  let state = seed >>> 0;
  const random = () => {
    state ^= state << 13; state ^= state >>> 17; state ^= state << 5;
    return (state >>> 0) / 4294967296;
  };
  const samples = Array.from({ length: draws }, () => median(
    Array.from({ length: ratios.length }, () => ratios[Math.floor(random() * ratios.length)]),
  )).sort((a, b) => a - b);
  return [samples[Math.floor(0.025 * (draws - 1))], samples[Math.ceil(0.975 * (draws - 1))]];
}

export function analyze(plan, records) {
  assert.equal(plan.schema, 'redline-abba-plan-v1');
  assert.equal(plan.blocks, 12);
  assert.equal(plan.rows, 20000);
  assert.equal(plan.order, 'ABBA');
  assert.equal(plan.threshold, 1.05);
  assert.equal(records.length, plan.cases.length * (plan.blocks + 1) * 4, 'incomplete/extra samples');
  let offset = 0;
  const results = plan.cases.map((c, caseIndex) => {
    const ratios = [];
    const settings = new Map();
    for (let block = -1; block < plan.blocks; block++) {
      const samples = records.slice(offset, offset += 4);
      samples.forEach((r, slot) => {
        assert.equal(r.seq, offset - 4 + slot, 'global sequence mismatch');
        assert.equal(r.case, c.id);
        assert.equal(r.pair, c.pair);
        assert.equal(r.block, block);
        assert.equal(r.slot, slot);
        assert.equal(r.label, ['release', 'main', 'main', 'release'][slot], 'not ABBA');
        assert.equal(r.exit, 0, 'failed sample');
        assert.equal(r.status, 'ok');
        assert.equal(r.rawSha256, sha256(r.raw), 'raw hash mismatch');
        const m = parseRecord(r.raw);
        assert.equal(m.workload, c.id);
        assert.equal(m.pair, c.pair);
        assert.equal(m.rows, plan.rows);
        assert.equal(m.work_divisor, 1, 'reduced work');
        assert.equal(m.ops, c.ops, 'work count changed');
        assert.equal(m.digest, c.digest, 'result differs from published receipt');
        assert.equal(m.unit, c.unit);
        assert.equal(m.engine, 'redlinedb');
        assert(Number.isSafeInteger(m.elapsed_ns) && m.elapsed_ns > 0);
        const encoded = JSON.stringify(m.settings);
        assert.equal(encoded, JSON.stringify(c.settings), 'durability/cache settings changed');
        settings.set(r.label, encoded);
        assert(r.startedAt <= r.finishedAt);
        if (slot) assert(samples[slot - 1].finishedAt <= r.startedAt, 'overlapping samples');
      });
      assert.equal(settings.get('release'), settings.get('main'));
      const costs = samples.map(r => parseRecord(r.raw).elapsed_ns / c.ops);
      // Geometric means cancel a linear trend in log time across ABBA.
      const ratio = Math.sqrt(costs[1] * costs[2] / (costs[0] * costs[3]));
      if (block >= 0) ratios.push(ratio); // First ABBA block is declared warmup.
    }
    const seed = (plan.bootstrapSeed + caseIndex) >>> 0;
    const ci = bootstrap(ratios, seed, plan.bootstrapDraws);
    return { pair: c.pair, id: c.id, summary: c.summary, blocks: ratios.length,
      samplesPerVersion: 2 * ratios.length, ratios, median: median(ratios), ci95: ci,
      bootstrapSeed: seed, verdict: ci[0] > plan.threshold ? 'regression'
        : ci[1] <= plan.threshold ? 'within-threshold' : 'inconclusive' };
  });
  return { schema: 'redline-abba-summary-v1', method: 'paired-ABBA-block-bootstrap',
    ratio: 'main/release elapsed time per equal operation; lower is better',
    cases: results, regressions: results.filter(r => r.verdict === 'regression').length,
    inconclusive: results.filter(r => r.verdict === 'inconclusive').length,
    counts: { passed: records.length, failed: 0, skipped: 0,
      warmup: plan.cases.length * 4, measured: plan.cases.length * plan.blocks * 4 } };
}

export function summarize(bundle) {
  const plan = JSON.parse(fs.readFileSync(path.join(bundle, 'plan.json')));
  const receipt = JSON.parse(fs.readFileSync(path.join(bundle, 'receipt.json')));
  assert.equal(receipt.exit, 0, 'measurement did not complete');
  assert.equal(receipt.planSha256, sha256(fs.readFileSync(path.join(bundle, 'plan.json'))));
  for (const entry of receipt.files) {
    assert(!path.isAbsolute(entry.path) && !entry.path.split('/').includes('..'));
    assert.equal(sha256(fs.readFileSync(path.join(bundle, entry.path))), entry.sha256, entry.path);
  }
  const records = fs.readFileSync(path.join(bundle, 'records.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  const summary = analyze(plan, records);
  const loads = fs.readFileSync(path.join(bundle, 'host-samples.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  assert(loads.length >= records.length * 2, 'missing before/after host samples');
  assert(loads[0].at <= records[0].startedAt && loads.at(-1).at >= records.at(-1).finishedAt);
  summary.host = { name: plan.host.name, cpus: plan.cpus, nice: plan.nice,
    ioClass: plan.ioClass, sampleCount: loads.length,
    load1: { min: Math.min(...loads.map(x => x.load[0])), median: median(loads.map(x => x.load[0])),
      max: Math.max(...loads.map(x => x.load[0])) } };
  summary.receiptSha256 = sha256(fs.readFileSync(path.join(bundle, 'receipt.json')));
  return summary;
}

export function render(summary, bundle) {
  const rows = summary.cases.map(c => `| ${c.pair} | ${c.id} | ${c.median.toFixed(3)} | ${c.ci95[0].toFixed(3)}–${c.ci95[1].toFixed(3)} | ${c.verdict} |`);
  return `<!-- main-regression:begin -->\n<!-- Generated by scripts/perf/abba-summary.mjs; do not edit by hand. -->\n` +
    `Post-release main \`bac7e9186\` versus v5.1.1: ${summary.cases.length} fixed-work cases, ` +
    `${summary.regressions} detected regressions, ${summary.inconclusive} inconclusive cases. ` +
    `Elapsed-time ratios below 1 favor main. A regression requires the entire bootstrap 95% interval above 1.05.\n\n` +
    `| Mode | Case | Main ÷ release time | Bootstrap 95% CI | Finding |\n|---|---|---:|---:|---|\n${rows.join('\n')}\n\n` +
    `Each case ran 12 ABBA blocks (24 measured repetitions per version), after one warmup block, ` +
    `on the same fresh 20,000-row images and unchanged release scoreboard harness. ` +
    `This is a contended-host comparison on ${summary.host.name}, pinned to CPUs ${summary.host.cpus}, ` +
    `nice ${summary.host.nice} with idle I/O priority. Load averages during sampling: ` +
    `${summary.host.load1.min.toFixed(2)}–${summary.host.load1.max.toFixed(2)} (median ${summary.host.load1.median.toFixed(2)}). ` +
    `The percentile bootstrap resamples complete ABBA blocks 10,000 times using recorded seeds; intervals are per case, without a multiple-comparison correction. ` +
    `Normal uses tmpfs; Strict uses real disk. These ratios do not replace the quiet-host release throughput figures above. ` +
    `[Method, raw records and hashes](${bundle}/), [receipt](${bundle}/receipt.json) ` +
    `(SHA-256 \`${summary.receiptSha256}\`).\n<!-- main-regression:end -->`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [bundle, mode, target] = process.argv.slice(2);
  assert(bundle, 'usage: node scripts/perf/abba-summary.mjs BUNDLE [--check|--render TARGET]');
  const summary = summarize(bundle);
  const text = `${JSON.stringify(summary, null, 2)}\n`;
  const file = path.join(bundle, 'summary.json');
  if (mode === '--check') {
    assert.equal(fs.readFileSync(file, 'utf8'), text, 'summary drift');
    if (target) {
      const readme = fs.readFileSync(target, 'utf8');
      assert.equal(readme.match(/<!-- main-regression:begin -->[\s\S]*?<!-- main-regression:end -->/)?.[0], render(summary, bundle), 'README drift');
    }
  } else if (mode === '--render') {
    const readme = fs.readFileSync(target, 'utf8');
    assert(readme.includes('<!-- main-regression:begin -->'), 'missing README markers');
    fs.writeFileSync(target, readme.replace(/<!-- main-regression:begin -->[\s\S]*?<!-- main-regression:end -->/, render(summary, bundle)));
  } else {
    assert.equal(mode, undefined);
    fs.writeFileSync(file, text);
  }
  console.log(JSON.stringify({ cases: summary.cases.length, regressions: summary.regressions, inconclusive: summary.inconclusive, counts: summary.counts }));
}
