import test from 'node:test';
import assert from 'node:assert/strict';
import { analyzeFocused, analyzeProcess, caseIds, finding, formatRatio, focusedReport, verifyFocusedImages, verifyReleaseLineage } from './focused-summary.mjs';
import { sha256 } from './abba-summary.mjs';

function fixture(ratios = Array(12).fill(1)) {
  const settings = { durability: 'Normal', cache_bytes: 67108864, buffer_pool_pages: 4096, query_pool_threads: 0 };
  const plan = { schema: 'redline-abba-plan-v1', purpose: 'focused-normal-regression-followup',
    reproductionThreshold: 1, threshold: 1.05, blocks: 12, rows: 20000, order: 'ABBA', warmupBlocks: 1,
    bootstrapSeed: 202610072, bootstrapDraws: 10000, cpus: '2-3', nice: 19, ioClass: 'idle',
    isolation: { exclusive: false }, filesystems: { normal: 'tmpfs' }, workRoots: { normal: 'fixture' }, host: { name: 'xbabe3' },
    cases: caseIds.map(id => ({ id, pair: 'normal', image: 'base', writes: false, unit: 'stmt', ops: 200000,
      digest: '-9223372036854775000', settings })) };
  const records = [], hosts = [];
  for (const c of plan.cases) for (let block = -1; block < 12; block++) {
    for (const [slot, label] of ['release', 'main', 'main', 'release'].entries()) {
      const raw = JSON.stringify({ workload: c.id, pair: 'normal', rows: 20000, work_divisor: 1, ops: c.ops,
        unit: c.unit, engine: 'redlinedb', elapsed_ns: label === 'release' ? 1000000 : Math.round(1000000 * ratios[Math.max(0, block)]),
        settings }).replace(/}$/, `,"digest":${c.digest}}`);
      const r = { seq: records.length, case: c.id, pair: 'normal', block, slot, label, exit: 0, signal: null,
        status: 'ok', raw, rawSha256: sha256(raw), startedAt: '2026-10-07T00:00:00Z', finishedAt: '2026-10-07T00:00:00Z',
        scheduling: { cpus: '2-3', nice: 19, ioPriority: 'idle' } };
      records.push(r);
      for (const stage of ['before', 'after']) hosts.push({ case: c.id, seq: r.seq, block, slot, label, stage,
        at: r.startedAt, cpus: '2-3', load: [20, 30, 40], runnerWorkers: [123],
        cpuCounters: ['cpu2 1 2 3 4', 'cpu3 1 2 3 4', 'cpu66 1 2 3 4', 'cpu67 1 2 3 4'] });
    }
  }
  return { plan, records, hosts };
}

test('owner reproduction boundary is1.0, independent of the previous5% flag', () => {
  const { plan, records, hosts } = fixture(Array(12).fill(1.03));
  const s = analyzeFocused(plan, records, hosts);
  assert.equal(s.reproducedSlowdowns, 5);
  assert(s.cases.every(c => c.priorFivePercentVerdict === 'within-threshold' && c.verdict === 'reproduced-slowdown'));
  assert.deepEqual(s.counts, { passed: 260, failed: 0, skipped: 0, warmup: 20, measured: 240 });
});
test('speedups and crossing intervals do not reproduce slowdowns', () => {
  assert.equal(finding([0.8, 0.9]), 'speedup');
  for (const ci of [[0.9, 1.1], [1, 1.1], [0.9, 1], [1, 1]]) assert.equal(finding(ci), 'inconclusive');
  assert.equal(finding([1.01, 1.1]), 'reproduced-slowdown');
  assert.throws(() => finding([1.1, 1]));
});
test('process cost uses complete ABBA child lifetimes and rejects invalid timestamps', () => {
  const { plan, records } = fixture();
  for (const r of records) {
    r.startedAt = '2026-10-07T00:00:00.000Z';
    r.finishedAt = r.label === 'release' ? '2026-10-07T00:00:00.100Z' : '2026-10-07T00:00:00.200Z';
  }
  assert(analyzeProcess(plan, records).every(c => c.median === 2 && c.ci95.every(x => x === 2)));
  records.find(r => r.block === 0).finishedAt = 'invalid';
  assert.throws(() => analyzeProcess(plan, records), /invalid process duration/);
});
test('display rounding preserves which side of1.0 an endpoint lies on', () => {
  for (const x of [0.9997465670523391, 0.99999999999, 1.00000000001]) {
    assert.equal(Math.sign(Number(formatRatio(x)) - 1), Math.sign(x - 1));
  }
  assert.equal(formatRatio(1), '1.000000');
});
test('each execution must have both bracketing load samples with matching pair identity', () => {
  for (const corrupt of [h => h.pop(), h => h.push(h[0]), h => { h[1].slot = 1; },
    h => { h[0].at = '2026-10-08T00:00:00Z'; }, h => { h[0].load[0] = -1; },
    h => { h[0].cpuCounters.pop(); }]) {
    const { plan, records, hosts } = fixture(); corrupt(hosts);
    assert.throws(() => analyzeFocused(plan, records, hosts));
  }
});
test('periodic telemetry cannot inject invalid loads or CPU counters into the report', () => {
  for (const corrupt of [h => { h.load[0] = null; }, h => { h.load = [0]; },
    h => { h.cpuCounters[0] = 'cpu2 garbage'; }]) {
    const { plan, records, hosts } = fixture();
    const periodic = structuredClone(hosts[0]); delete periodic.stage; corrupt(periodic); hosts.push(periodic);
    assert.throws(() => analyzeFocused(plan, records, hosts));
  }
});
test('release provenance rejects a matching but unverified plan/build commit', () => {
  const plan = { releaseTagCommit: '9277455d5ad008252053a81d18add39b8cdc8f7b',
    versions: { release: { engineCommit: 'af4fc74cc3280582c4e6bd0583fe503c83118c48' } } };
  verifyReleaseLineage(plan, { engine_commit: plan.versions.release.engineCommit });
  plan.versions.release.engineCommit = '1'.repeat(40);
  assert.throws(() => verifyReleaseLineage(plan, { engine_commit: plan.versions.release.engineCommit }));
});
test('child affinity and priority must match for both builds', () => {
  for (const corrupt of [r => { r[0].scheduling.cpus = '2-5'; },
    r => { r[1].scheduling.nice = 0; }, r => { r[2].scheduling.ioPriority = 'none'; }]) {
    const { plan, records, hosts } = fixture(); corrupt(records);
    assert.throws(() => analyzeFocused(plan, records, hosts));
  }
});
test('case selection, controls, full work and correctness fail closed', () => {
  for (const corrupt of [p => p.cases.pop(), p => p.cases.reverse(), p => { p.cases[0].pair = 'strict'; },
    p => { p.cases[0].ops = 100000; }, p => { p.cases[0].digest = '123'; }, p => { p.isolation.exclusive = true; }]) {
    const { plan, records, hosts } = fixture(); corrupt(plan);
    assert.throws(() => analyzeFocused(plan, records, hosts));
  }
});
test('focused receipt requires exactly two successful immutable Normal base images', () => {
  const inventory = [{ path: 'data.bin', size: 512, sha256: 'a'.repeat(64) }];
  const images = ['main', 'release'].map(label => ({ key: `normal/${label}/base`, before: inventory, after: inventory }));
  const builds = ['main', 'release'].map(label => ({ pair: 'normal', label, kind: 'base', code: 0, signal: null }));
  verifyFocusedImages(images, builds);
  for (const corrupt of [i => i.pop(), i => { i[0].key = i[1].key; },
    i => { i[0].before = []; }, i => { i[0].after = [{ ...inventory[0], size: 513 }]; }]) {
    const i = structuredClone(images); corrupt(i); assert.throws(() => verifyFocusedImages(i, builds));
  }
  const b = structuredClone(builds); b[0].signal = 'SIGTERM';
  assert.throws(() => verifyFocusedImages(images, b));
});

// An adjacent production-history comparison must bind both known endpoints.
test('source bisect reference cannot masquerade as release or choose arbitrary endpoints', () => {
  const plan = { releaseTagCommit: '9277455d5ad008252053a81d18add39b8cdc8f7b', comparisonRole: 'adjacent-bisect',
    measuredMainEngine: '8f880b2162413dfff2e7d5ee0a5dc4a979a851e4',
    versions: { release: { engineCommit: '7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3' } } };
  const build = { engine_commit: plan.versions.release.engineCommit };
  assert.doesNotThrow(() => verifyReleaseLineage(plan, build));
  assert.throws(() => verifyReleaseLineage({ ...plan, comparisonRole: 'bisect' }, build));
  assert.throws(() => verifyReleaseLineage({ ...plan, measuredMainEngine: 'a'.repeat(40) }, build));
});

test('candidate release check binds its actual release reference and report labels', () => {
  const plan = { releaseTagCommit: '9277455d5ad008252053a81d18add39b8cdc8f7b', comparisonRole: 'candidate-release',
    productionParent: '94da3b131d6d50009d2d24e026b9010681e1f04d',
    measuredMainEngine: 'ea9dd55e8d63c6bbe8190e4318269b7efa05c776',
    versions: { release: { engineCommit: 'af4fc74cc3280582c4e6bd0583fe503c83118c48' } } };
  const build = { engine_commit: plan.versions.release.engineCommit };
  verifyReleaseLineage(plan, build);
  assert.throws(() => verifyReleaseLineage({ ...plan, comparisonRole: 'candidate-fix' }, build));
  const f = fixture(), s = analyzeFocused(f.plan, f.records, f.hosts);
  Object.assign(s, { candidate: true, receiptSha256: 'b'.repeat(64),
    versions: { comparisonMain: plan.measuredMainEngine, measuredMain: plan.measuredMainEngine,
      measuredRelease: build.engine_commit, stableRelease: plan.releaseTagCommit, harnessTree: 'b'.repeat(40) } });
  const report = focusedReport(s);
  assert.match(report, /candidate experiment: scratch-window candidate versus v5.1.1/);
  assert.match(report, /A=release\/B=scratch-window candidate/);
  assert.match(report, /Candidate ÷ release time/);
  assert.doesNotMatch(report, /main versus v5.1.1/);
});

test('adjacent bisect report names its actual reference instead of claiming stable equivalence', () => {
  const { plan, records, hosts } = fixture();
  const summary = analyzeFocused(plan, records, hosts);
  Object.assign(summary, { referenceRole: 'production-parent', receiptSha256: 'b'.repeat(64),
    versions: { comparisonMain: '8f880b2162413dfff2e7d5ee0a5dc4a979a851e4', measuredMain: '8f880b2162413dfff2e7d5ee0a5dc4a979a851e4',
      measuredRelease: '7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3', stableRelease: '9277455d5ad008252053a81d18add39b8cdc8f7b', harnessTree: 'b'.repeat(40) } });
  const text = focusedReport(summary);
  assert.match(text, /WAL window commit versus production parent/);
  assert.match(text, /WAL commit ÷ parent time/);
  assert.match(text, /A=production parent\/B=WAL window commit/);
  assert.doesNotMatch(text, /has identical Cargo\/crates inputs to stable/);
  assert.doesNotMatch(text, /main versus v5.1.1/);
});

test('candidate comparison binds frozen production parent and measured source', () => {
  const plan = { releaseTagCommit: '9277455d5ad008252053a81d18add39b8cdc8f7b', comparisonRole: 'candidate-fix',
    productionParent: '94da3b131d6d50009d2d24e026b9010681e1f04d',
    measuredMainEngine: 'ea9dd55e8d63c6bbe8190e4318269b7efa05c776',
    versions: { release: { engineCommit: '8f880b2162413dfff2e7d5ee0a5dc4a979a851e4' } } };
  const build = { engine_commit: plan.versions.release.engineCommit };
  assert.doesNotThrow(() => verifyReleaseLineage(plan, build));
  assert.throws(() => verifyReleaseLineage({ ...plan, productionParent: 'a'.repeat(40) }, build));
  assert.throws(() => verifyReleaseLineage({ ...plan, measuredMainEngine: 'a'.repeat(40) }, build));
});
