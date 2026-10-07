// Validate the predeclared five-case follow-up independently of the full receipt.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { analyze, bootstrap, median, parseRecord, sha256, verifyCompletion } from './abba-summary.mjs';

export const caseIds = ['point_pk_prepared', 'top10', 'join_group_by_city', 'point_pk_sql_text', 'join_point'];
const candidates = new Map([
  ['ea9dd55e8d63c6bbe8190e4318269b7efa05c776', 'scratch-window candidate'],
  ['4f9cb79224bd3d896188c48bdc0f22cdea1a0d39', 'checkpoint-open load-order candidate'],
  ['ac96a7150a5ad4c244605ec09124518d5f016dae', 'bounded owned-decode candidate'],
]);
export const formatRatio = value => Number(value.toFixed(6)) === 1 && value !== 1 ? String(value) : value.toFixed(6);
export function finding(ci) {
  assert(ci.length === 2 && ci.every(x => Number.isFinite(x) && x > 0) && ci[0] <= ci[1]);
  return ci[0] > 1 ? 'reproduced-slowdown' : ci[1] < 1 ? 'speedup' : 'inconclusive';
}

// Wall-clock child lifetime includes image copying, open/recovery, correctness
// checks, queries and output. It is not an isolated open/recovery benchmark.
export function analyzeProcess(plan, records) {
  return plan.cases.map((c, index) => {
    const samples = records.filter(r => r.case === c.id && r.block >= 0);
    assert.equal(samples.length, plan.blocks * 4);
    const durations = samples.map(r => Date.parse(r.finishedAt) - Date.parse(r.startedAt));
    assert(durations.every(x => Number.isFinite(x) && x > 0), 'invalid process duration');
    const ratios = Array.from({ length: plan.blocks }, (_, block) => {
      const d = durations.slice(block * 4, block * 4 + 4);
      return Math.sqrt(d[1] * d[2] / (d[0] * d[3]));
    });
    return { id: c.id, referenceMedianMs: median(durations.filter((_, i) => i % 4 === 0 || i % 4 === 3)),
      candidateMedianMs: median(durations.filter((_, i) => i % 4 === 1 || i % 4 === 2)),
      median: median(ratios), ci95: bootstrap(ratios, (plan.bootstrapSeed + index) >>> 0, plan.bootstrapDraws) };
  });
}

export function analyzeFocused(plan, records, hosts) {
  assert.equal(plan.purpose, 'focused-normal-regression-followup');
  assert.equal(plan.reproductionThreshold, 1);
  assert.deepEqual(plan.cases.map(c => c.id), caseIds, 'missing/reordered focused cases or controls');
  assert(plan.cases.every(c => c.pair === 'normal' && c.image === 'base' && c.writes === false));
  assert.equal(plan.cpus, '2-3');
  assert.equal(plan.nice, 19);
  assert.equal(plan.ioClass, 'idle');
  assert.equal(plan.isolation.exclusive, false, 'exclusive core isolation was unavailable');
  assert.equal(plan.filesystems.normal, 'tmpfs');
  assert.deepEqual(Object.keys(plan.workRoots), ['normal']);
  const summary = analyze(plan, records);
  for (const h of hosts) {
    assert(h.load.length === 3 && h.load.every(x => Number.isFinite(x) && x >= 0), 'invalid host load');
    assert.equal(h.cpus, plan.cpus);
    assert(Array.isArray(h.runnerWorkers) && h.runnerWorkers.every(x => Number.isSafeInteger(x) && x > 0));
    assert.equal(h.cpuCounters.length, 4, 'missing selected-core/SMT counters');
    assert.deepEqual(h.cpuCounters.map(x => x.split(' ')[0]), ['cpu2', 'cpu3', 'cpu66', 'cpu67']);
    assert(h.cpuCounters.every(x => /^cpu\d+(?: +\d+){4,10}$/.test(x)), 'invalid CPU counters');
  }
  for (let i = 1; i < records.length; i++) assert(records[i - 1].finishedAt <= records[i].startedAt, 'overlapping cases');
  assert.equal(hosts.filter(h => h.stage).length, records.length * 2, 'missing/extra per-execution load');
  for (const r of records) {
    assert.equal(r.scheduling.cpus, plan.cpus, 'child affinity drift');
    assert.equal(r.scheduling.nice, plan.nice, 'child priority drift');
    assert.match(r.scheduling.ioPriority, /idle/);
    const samples = hosts.filter(h => h.stage && h.seq === r.seq);
    assert.equal(samples.length, 2);
    for (const [i, h] of samples.entries()) {
      assert.equal(h.stage, ['before', 'after'][i]);
      for (const key of ['case', 'block', 'slot', 'label']) assert.equal(h[key], r[key], 'load attached to wrong execution');
      assert.equal(h.cpus, plan.cpus);
      assert(h.load.length === 3 && h.load.every(x => Number.isFinite(x) && x >= 0));
      assert.equal(h.cpuCounters.length, 4, 'missing selected-core/SMT counters');
      assert.deepEqual(h.cpuCounters.map(x => x.split(' ')[0]), ['cpu2', 'cpu3', 'cpu66', 'cpu67']);
      assert(Array.isArray(h.runnerWorkers));
    }
    assert(samples[0].at <= r.startedAt && samples[1].at >= r.finishedAt, 'load does not bracket execution');
  }
  summary.schema = 'redline-focused-abba-summary-v1';
  summary.cases = summary.cases.map(c => ({ ...c, priorFivePercentVerdict: c.verdict, verdict: finding(c.ci95),
    role: caseIds.indexOf(c.id) < 3 ? 'flagged-case' : 'stable-control',
    pairLoads: Array.from({ length: plan.blocks }, (_, block) => hosts.filter(h => h.case === c.id && h.block === block)
      .map(h => ({ slot: h.slot, stage: h.stage, at: h.at, load: h.load }))) }));
  summary.reproducedSlowdowns = summary.cases.filter(c => c.verdict === 'reproduced-slowdown').length;
  summary.inconclusive = summary.cases.filter(c => c.verdict === 'inconclusive').length;
  delete summary.regressions;
  summary.host = { name: plan.host.name, cpus: plan.cpus, nice: plan.nice, ioClass: plan.ioClass,
    exclusiveIsolation: false, samples: hosts.length,
    load1: { min: Math.min(...hosts.map(h => h.load[0])), median: median(hosts.map(h => h.load[0])), max: Math.max(...hosts.map(h => h.load[0])) } };
  return summary;
}

export function verifyFocusedImages(images, builds) {
  const keys = ['normal/main/base', 'normal/release/base'];
  assert.deepEqual(images.map(i => i.key).sort(), keys);
  assert.deepEqual(builds.map(b => `${b.pair}/${b.label}/${b.kind}`).sort(), keys);
  for (const b of builds) { assert.equal(b.code, 0); assert.equal(b.signal, null); }
  for (const i of images) {
    assert(i.before.length > 0 && i.before.some(f => f.size > 0));
    const names = new Set();
    for (const f of i.before) {
      assert(typeof f.path === 'string' && f.path.length && !path.isAbsolute(f.path) && !f.path.split('/').includes('..') && !names.has(f.path));
      names.add(f.path);
      assert.match(f.sha256, /^[a-f0-9]{64}$/);
      assert(Number.isSafeInteger(f.size) && f.size >= 0);
    }
    assert.deepEqual(i.before, i.after, 'immutable image changed');
  }
}

export function verifyReleaseLineage(plan, build) {
  assert.equal(plan.releaseTagCommit, '9277455d5ad008252053a81d18add39b8cdc8f7b');
  if (plan.comparisonRole === 'adjacent-bisect') {
    assert.equal(plan.versions.release.engineCommit, '7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3');
    assert.equal(plan.measuredMainEngine, '8f880b2162413dfff2e7d5ee0a5dc4a979a851e4');
  } else if (['candidate-fix', 'candidate-release'].includes(plan.comparisonRole)) {
    assert.equal(plan.versions.release.engineCommit, plan.comparisonRole === 'candidate-fix'
      ? '8f880b2162413dfff2e7d5ee0a5dc4a979a851e4' : 'af4fc74cc3280582c4e6bd0583fe503c83118c48');
    assert(candidates.has(plan.measuredMainEngine), 'unknown candidate source');
    assert.equal(plan.productionParent, '94da3b131d6d50009d2d24e026b9010681e1f04d');
  } else {
    assert.equal(plan.versions.release.engineCommit, 'af4fc74cc3280582c4e6bd0583fe503c83118c48');
  }
  assert.equal(build.engine_commit, plan.versions.release.engineCommit, 'unverified reference lineage');
}

export function summarizeFocused(bundle) {
  const read = name => fs.readFileSync(path.join(bundle, name));
  const json = name => JSON.parse(read(name));
  const lines = name => read(name).toString().trim().split('\n').map(JSON.parse);
  const plan = json('plan.json'), receipt = json('receipt.json');
  verifyCompletion(receipt);
  assert.equal(receipt.planSha256, sha256(read('plan.json')));
  const required = ['plan.json', 'records.jsonl', 'host-samples.jsonl', 'image-builds.jsonl',
    'release-build.json', 'main-build.json', 'abba-compare.mjs', 'abba-summary.mjs'].sort();
  assert.deepEqual(receipt.files.map(f => f.path).sort(), required);
  for (const f of receipt.files) assert.equal(sha256(read(f.path)), f.sha256, f.path);
  assert.match(plan.comparisonMain, /^[a-f0-9]{40}$/);
  assert.match(plan.measuredMainEngine, /^[a-f0-9]{40}$/);
  if (plan.comparisonMain !== plan.measuredMainEngine) {
    assert.equal(plan.comparisonMain, '94da3b131d6d50009d2d24e026b9010681e1f04d');
    assert.equal(plan.measuredMainEngine, 'bac7e9186feaa21824369c97767c391100ad1c37');
    assert.equal(plan.mainEngineEquivalenceCommand,
      'git diff bac7e9186feaa21824369c97767c391100ad1c37 94da3b131 -- crates Cargo.toml Cargo.lock .cargo (empty)');
  }
  assert.equal(plan.releaseTagCommit, '9277455d5ad008252053a81d18add39b8cdc8f7b');
  assert.equal(plan.hostSelection.chosen, plan.host.name);
  const loads = ['xbabe1', 'xbabe2', 'xbabe3'].map(h => plan.hostSelection[h][0]);
  assert(loads.every(x => Number.isFinite(x) && x >= 0));
  assert.equal(plan.hostSelection[plan.host.name][0], Math.min(...loads));
  const builds = ['release', 'main'].map(label => {
    const b = json(`${label}-build.json`), v = plan.versions[label];
    assert.equal(b.binary_sha256, v.binarySha256);
    assert.equal(b.engine_commit, v.engineCommit);
    assert.equal(b.harness_tree, 'bb1434a80ca0390401eb628d2785e8387c5f346e');
    assert.equal(b.failpoints, false); assert.equal(b.pgo, false);
    assert.equal(b.rustflags, ''); assert.equal(b.profile, 'release');
    return b;
  });
  verifyReleaseLineage(plan, builds[0]);
  assert.deepEqual(builds[0].engines, builds[1].engines);
  assert.deepEqual(builds[0].rustc_verbose_version, builds[1].rustc_verbose_version);
  assert.equal(builds[0].profile_release, builds[1].profile_release);
  assert.equal(builds[1].engine_commit, plan.measuredMainEngine);
  const baselinePath = path.join(plan.baselineBundle, 'summary.json');
  const baselineBytes = fs.readFileSync(baselinePath);
  assert.equal(sha256(baselineBytes), plan.baselineSummarySha256);
  const baseline = JSON.parse(baselineBytes);
  assert(baseline.publishable && baseline.blockers.length === 0);
  for (const r of baseline.raw) assert.equal(sha256(fs.readFileSync(path.join(plan.baselineBundle, r.path))), r.sha256);
  const original = fs.readFileSync(path.join(plan.baselineBundle, 'v5.1.1/run-1-normal.jsonl'), 'utf8').trim().split('\n').map(parseRecord);
  for (const c of plan.cases) {
    const r = original.find(r => r.workload === c.id && r.label === 'v5.1.1' && r.rep === 0);
    assert(r); assert.equal(r.ops, c.ops); assert.equal(r.digest, c.digest);
    assert.deepEqual(r.settings, c.settings); assert.equal(r.unit, c.unit);
  }
  verifyFocusedImages(receipt.images, lines('image-builds.jsonl'));
  const summary = analyzeFocused(plan, lines('records.jsonl'), lines('host-samples.jsonl'));
  if (['adjacent-bisect', 'candidate-fix'].includes(plan.comparisonRole)) summary.referenceRole = 'production-parent';
  if (['candidate-fix', 'candidate-release'].includes(plan.comparisonRole)) summary.candidate = true;
  if (summary.candidate) summary.candidateLabel = candidates.get(plan.measuredMainEngine);
  if (plan.sourceBisectSeries) summary.sourceBisectSeries = plan.sourceBisectSeries;
  summary.receiptSha256 = sha256(read('receipt.json'));
  summary.versions = { comparisonMain: plan.comparisonMain, measuredMain: builds[1].engine_commit,
    stableRelease: plan.releaseTagCommit, measuredRelease: builds[0].engine_commit,
    mainBinarySha256: builds[1].binary_sha256, releaseBinarySha256: builds[0].binary_sha256,
    harnessTree: builds[1].harness_tree };
  return summary;
}

export function focusedReport(summary) {
  const rows = summary.cases.map(c => `| ${c.id} | ${c.role} | ${formatRatio(c.median)} | ${formatRatio(c.ci95[0])}–${formatRatio(c.ci95[1])} | ${c.verdict} |`);
  const parent = summary.referenceRole === 'production-parent', candidate = summary.candidate === true;
  const comparison = candidate ? summary.candidateLabel ?? 'scratch-window candidate' : 'WAL window commit';
  return (candidate ? `# Focused Normal candidate experiment: ${comparison} versus ${parent ? 'production parent' : 'v5.1.1'}\n\n`
    : parent ? `# Focused Normal source bisect: ${comparison} versus production parent\n\n` : `# Focused Normal follow-up: main versus v5.1.1\n\n`) +
    `Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). ` +
    `Receipt SHA-256: \`${summary.receiptSha256}\`. This diagnostic does not replace publishable quiet-host release throughput.\n\n` +
    `Compared commit \`${summary.versions.comparisonMain}\`; measured engine \`${summary.versions.measuredMain}\` (identical Cargo/crates inputs). ` +
    (parent ? `Reference production parent \`${summary.versions.measuredRelease}\`; this comparison does not measure stable release throughput. ` :
      `Measured release \`${summary.versions.measuredRelease}\` has identical Cargo/crates inputs to stable \`${summary.versions.stableRelease}\`. `) +
    `Both frozen production builds use harness \`${summary.versions.harnessTree}\`, identical flags, original 20,000-row images, full operations and result digests.\n\n` +
    `Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. ` +
    `The median uses sqrt(B1×B2/(A1×A2)), with A=${parent ? 'production parent' : 'release'}/B=${candidate || parent ? comparison : 'main'} elapsed time per equal operation. ` +
    `The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. ` +
    `An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.\n\n` +
    `| Case | Role | ${candidate ? `Candidate ÷ ${parent ? 'parent' : 'release'} time` : parent ? 'WAL commit ÷ parent time' : 'Main ÷ release time'} | Bootstrap95%CI | Finding |\n|---|---|---:|---:|---|\n${rows.join('\n')}\n\n` +
    `Host ${summary.host.name}, taskset${summary.host.cpus}, nice${summary.host.nice}, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; ` +
    `SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. ` +
    `Load1 min/median/max: ${summary.host.load1.min.toFixed(2)}/${summary.host.load1.median.toFixed(2)}/${summary.host.load1.max.toFixed(2)}. ` +
    `Records: ${summary.counts.passed} passed / ${summary.counts.failed} failed / ${summary.counts.skipped} skipped, including${summary.counts.warmup} retained warmups.\n` +
    (summary.sourceBisectSeries ? `\nSource-isolation series: ${summary.sourceBisectSeries.purpose} Host selection was fixed at the beginning of the complete series; every execution still records its current load.\n` : '');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [bundle, mode] = process.argv.slice(2);
  assert(bundle && (!mode || mode === '--check'), 'usage: focused-summary.mjs BUNDLE [--check]');
  const summary = summarizeFocused(bundle);
  const products = { 'summary.json': `${JSON.stringify(summary, null, 2)}\n`, 'README.md': focusedReport(summary) };
  for (const [name, text] of Object.entries(products)) {
    if (mode) assert.equal(fs.readFileSync(path.join(bundle, name), 'utf8'), text, `${name} drift`);
    else fs.writeFileSync(path.join(bundle, name), text);
  }
  console.log(JSON.stringify({ reproducedSlowdowns: summary.reproducedSlowdowns, inconclusive: summary.inconclusive, counts: summary.counts }));
}
