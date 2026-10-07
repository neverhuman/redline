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

export function verifyCompletion(receipt) {
  assert.equal(receipt.exit, 0, 'measurement did not complete');
  assert.equal(receipt.interrupted, undefined, 'interrupted measurement');
  assert.equal(receipt.error, undefined, 'measurement recorded an error');
}

export function verifyImages(images, builds) {
  const expected = ['normal/release/base', 'normal/release/after-updates',
    'normal/main/base', 'normal/main/after-updates', 'strict/release/base', 'strict/main/base'].sort();
  assert(Array.isArray(images) && Array.isArray(builds));
  assert.deepEqual(images.map(x => x.key).sort(), expected, 'image keys missing/duplicate');
  assert.deepEqual(builds.map(x => `${x.pair}/${x.label}/${x.kind}`).sort(), expected, 'image builds missing/duplicate');
  for (const b of builds) {
    assert.equal(b.code, 0, 'failed image build');
    assert.equal(b.signal, null, 'signal-marked image build');
  }
  for (const image of images) {
    assert(Array.isArray(image.before) && image.before.length > 0, 'empty/missing image inventory');
    const names = new Set();
    for (const f of image.before) {
      assert(typeof f.path === 'string' && f.path.length > 0 && !path.isAbsolute(f.path)
        && !f.path.split('/').includes('..') && !names.has(f.path), 'invalid/duplicate image path');
      names.add(f.path);
      assert.match(f.sha256, /^[a-f0-9]{64}$/, 'invalid image hash');
      assert(Number.isSafeInteger(f.size) && f.size >= 0, 'invalid image size');
    }
    assert(image.before.some(f => f.size > 0), 'empty image files');
    assert.deepEqual(image.before, image.after, 'immutable image changed');
  }
}

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
  assert.equal(plan.bootstrapDraws, 10000);
  assert.equal(plan.warmupBlocks, 1);
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
        assert(r.signal === null || r.signal === undefined, 'signal-marked sample');
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
  verifyCompletion(receipt);
  assert.equal(receipt.planSha256, sha256(fs.readFileSync(path.join(bundle, 'plan.json'))));
  const required = ['plan.json', 'records.jsonl', 'host-samples.jsonl', 'image-builds.jsonl',
    'release-build.json', 'main-build.json', 'abba-compare.mjs', 'abba-summary.mjs'];
  assert.deepEqual(receipt.files.map(x => x.path).sort(), [...required].sort(), 'receipt file set changed');
  for (const entry of receipt.files) {
    assert(!path.isAbsolute(entry.path) && !entry.path.split('/').includes('..'));
    assert.equal(sha256(fs.readFileSync(path.join(bundle, entry.path))), entry.sha256, entry.path);
  }
  assert.equal(plan.comparisonMain, 'bac7e9186feaa21824369c97767c391100ad1c37');
  assert.equal(plan.cpus, '2-5');
  assert.equal(plan.nice, 19);
  assert.equal(plan.ioClass, 'idle');
  assert.equal(plan.hostSelection.chosen, plan.host.name);
  const selectedLoad = plan.hostSelection[plan.host.name][0];
  assert(selectedLoad <= plan.hostSelection[plan.host.name === 'xbabe2' ? 'xbabe3' : 'xbabe2'][0]);
  const builds = ['release', 'main'].map(label => {
    const b = JSON.parse(fs.readFileSync(path.join(bundle, `${label}-build.json`)));
    assert.equal(b.binary_sha256, plan.versions[label].binarySha256);
    assert.equal(b.engine_commit, plan.versions[label].engineCommit);
    assert.equal(b.harness_tree, 'bb1434a80ca0390401eb628d2785e8387c5f346e');
    assert.equal(b.failpoints, false);
    assert.equal(b.pgo, false);
    assert.equal(b.rustflags, '');
    assert.equal(b.profile, 'release');
    return b;
  });
  assert.deepEqual(builds[0].engines, builds[1].engines, 'library/harness versions changed');
  assert.equal(builds[1].engine_commit, plan.comparisonMain);
  const baselineBytes = fs.readFileSync(path.join(plan.baselineBundle, 'summary.json'));
  assert.equal(sha256(baselineBytes), plan.baselineSummarySha256);
  const baseline = JSON.parse(baselineBytes);
  assert(baseline.publishable && baseline.blockers.length === 0, 'unaccepted release baseline');
  for (const raw of baseline.raw) {
    assert.equal(sha256(fs.readFileSync(path.join(plan.baselineBundle, raw.path))), raw.sha256, 'baseline raw changed');
  }
  const reference = ['normal', 'strict'].flatMap(pair => fs.readFileSync(
    path.join(plan.baselineBundle, `v5.1.1/run-1-${pair}.jsonl`), 'utf8',
  ).trim().split('\n').map(parseRecord).filter(r => r.label === 'v5.1.1' && r.rep === 0));
  assert.equal(plan.cases.length, 27, 'missing workload');
  assert.deepEqual(plan.cases.map(c => `${c.pair}/${c.id}`).sort(), reference.map(c => `${c.pair}/${c.workload}`).sort());
  for (const c of plan.cases) {
    const r = reference.find(r => r.workload === c.id && r.pair === c.pair);
    assert.equal(c.ops, r.ops); assert.equal(c.digest, r.digest);
    assert.deepEqual(c.settings, r.settings); assert.equal(c.unit, r.unit);
  }
  const imageBuilds = fs.readFileSync(path.join(bundle, 'image-builds.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  verifyImages(receipt.images, imageBuilds);
  const records = fs.readFileSync(path.join(bundle, 'records.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  const summary = analyze(plan, records);
  for (let i = 1; i < records.length; i++) assert(records[i - 1].finishedAt <= records[i].startedAt, 'overlapping cases');
  const loads = fs.readFileSync(path.join(bundle, 'host-samples.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  assert(loads.length >= records.length * 2, 'missing before/after host samples');
  assert(loads[0].at <= records[0].startedAt && loads.at(-1).at >= records.at(-1).finishedAt);
  summary.host = { name: plan.host.name, cpus: plan.cpus, nice: plan.nice,
    ioClass: plan.ioClass, sampleCount: loads.length,
    load1: { min: Math.min(...loads.map(x => x.load[0])), median: median(loads.map(x => x.load[0])),
      max: Math.max(...loads.map(x => x.load[0])) } };
  summary.receiptSha256 = sha256(fs.readFileSync(path.join(bundle, 'receipt.json')));
  summary.versions = { release: builds[0].engine_commit, main: builds[1].engine_commit,
    releaseBinarySha256: builds[0].binary_sha256, mainBinarySha256: builds[1].binary_sha256,
    harnessTree: builds[1].harness_tree, stableTagCommit: plan.releaseTagCommit };
  return summary;
}

export function report(summary) {
  return `# Main versus v5.1.1: ABBA comparison\n\nGenerated by \`scripts/perf/abba-summary.mjs --report\`. ` +
    `This is an owner-approved contended-host diagnostic, not a replacement quiet-host release scoreboard. ` +
    `See [plan.json](plan.json) for the recorded authority, seeds, dataset, scheduling and mounts, ` +
    `[records.jsonl](records.jsonl) for every measured/warmup result and stderr, ` +
    `[host-samples.jsonl](host-samples.jsonl) for load and CI workers, and [receipt.json](receipt.json) for input hashes and immutable-image checks.\n\n` +
    `Main engine: \`${summary.versions.main}\` (binary SHA-256 \`${summary.versions.mainBinarySha256}\`). ` +
    `Release engine: \`${summary.versions.release}\` (binary SHA-256 \`${summary.versions.releaseBinarySha256}\`); ` +
    `its Cargo/crates inputs equal the stable v5.1.1 tag \`${summary.versions.stableTagCommit}\`. ` +
    `Both use scoreboard tree \`${summary.versions.harnessTree}\`, the release profile without failpoints or PGO, ` +
    `identical durability/cache settings, full work and the original release result digests.\n\n` +
    `For each block A=release and B=main run consecutively as ABBA on fresh copies of version-specific seeded images. ` +
    `The block cost ratio is \`sqrt(B1*B2/(A1*A2))\` using elapsed nanoseconds per equal operation. ` +
    `The point estimate is the median of the 12 block ratios. A declared initial ABBA warmup is retained and validated but excluded from estimation. ` +
    `The seeded percentile bootstrap resamples whole blocks, preserving each temporal pair. ` +
    `Crossing intervals are inconclusive; absence of detected regression does not establish equivalence under arbitrary contention. ` +
    `No case is removed, shortened, adaptively rerun or reclassified because of its result. ` +
    `The intervals assume blocks can be resampled as independent observations; they do not remove host-confounding or establish the cause of a slowdown.\n\n` +
    `Records: ${summary.counts.passed} passed, ${summary.counts.failed} failed, ${summary.counts.skipped} skipped; ` +
    `${summary.counts.warmup} retained warmups and ${summary.counts.measured} measured samples.\n\n` + render(summary, '.') + '\n';
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
  assert(bundle, 'usage: node scripts/perf/abba-summary.mjs BUNDLE [--check [README.md]|--report|--render README.md]');
  const summary = summarize(bundle);
  const text = `${JSON.stringify(summary, null, 2)}\n`;
  const file = path.join(bundle, 'summary.json');
  if (mode === '--check') {
    assert.equal(fs.readFileSync(file, 'utf8'), text, 'summary drift');
    assert.equal(fs.readFileSync(path.join(bundle, 'README.md'), 'utf8'), report(summary), 'receipt report drift');
    if (target) {
      const readme = fs.readFileSync(target, 'utf8');
      assert.equal(readme.match(/<!-- main-regression:begin -->[\s\S]*?<!-- main-regression:end -->/)?.[0], render(summary, bundle), 'README drift');
    }
  } else if (mode === '--render') {
    assert.equal(summary.regressions, 0, 'cannot publish a clean main comparison with detected regressions');
    const readme = fs.readFileSync(target, 'utf8');
    assert(readme.includes('<!-- main-regression:begin -->'), 'missing README markers');
    fs.writeFileSync(target, readme.replace(/<!-- main-regression:begin -->[\s\S]*?<!-- main-regression:end -->/, render(summary, bundle)));
  } else if (mode === '--report') {
    fs.writeFileSync(path.join(bundle, 'README.md'), report(summary));
  } else {
    assert.equal(mode, undefined);
    fs.writeFileSync(file, text);
  }
  console.log(JSON.stringify({ cases: summary.cases.length, regressions: summary.regressions, inconclusive: summary.inconclusive, counts: summary.counts }));
}
