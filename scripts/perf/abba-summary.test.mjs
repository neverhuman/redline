import test from 'node:test';
import assert from 'node:assert/strict';
import { analyze, bootstrap, parseRecord, sha256, verifyCompletion, verifyImages } from './abba-summary.mjs';

function fixture(ratios = Array(12).fill(1)) {
  const c = { pair: 'normal', id: 'point_pk_prepared', ops: 200000,
    digest: '-9223372036854775000', unit: 'stmt', settings: { durability: 'Normal' } };
  const plan = { schema: 'redline-abba-plan-v1', blocks: 12, rows: 20000,
    order: 'ABBA', warmupBlocks: 1, threshold: 1.05, bootstrapSeed: 20261007, bootstrapDraws: 10000, cases: [c] };
  const records = [];
  for (let block = -1; block < 12; block++) for (const [slot, label] of ['release', 'main', 'main', 'release'].entries()) {
    const elapsed = label === 'release' ? 1000000 : Math.round(1000000 * (ratios[Math.max(0, block)]));
    const raw = JSON.stringify({ workload: c.id, pair: 'normal', rows: 20000,
      work_divisor: 1, ops: c.ops, unit: c.unit, engine: 'redlinedb', elapsed_ns: elapsed,
      settings: c.settings }).replace(/}$/, `,"digest":${c.digest}}`);
    records.push({ seq: records.length, case: c.id, pair: 'normal', block, slot, label,
      exit: 0, status: 'ok', raw, rawSha256: sha256(raw), startedAt: '2026-10-07T00:00:00Z',
      finishedAt: '2026-10-07T00:00:00Z' });
  }
  return { plan, records };
}

test('cost orientation detects slower main and accepts faster main', () => {
  for (const [ratio, verdict] of [[1.1, 'regression'], [0.8, 'within-threshold'], [1.05, 'within-threshold']]) {
    const { plan, records } = fixture(Array(12).fill(ratio));
    const s = analyze(plan, records);
    assert.equal(s.cases[0].median, ratio);
    assert.equal(s.cases[0].verdict, verdict);
    assert.deepEqual(s.cases[0].ci95, [ratio, ratio]);
    assert.equal(s.counts.skipped, 0);
    assert.equal(s.counts.warmup, 4);
  }
});
test('an interval crossing 1.05 is inconclusive, never a regression or clean proof', () => {
  const { plan, records } = fixture([0.8, 0.9, 1, 1, 1, 1, 1.1, 1.1, 1.2, 1.2, 1.3, 1.4]);
  const c = analyze(plan, records).cases[0];
  assert(c.ci95[0] < 1.05 && c.ci95[1] > 1.05);
  assert.equal(c.verdict, 'inconclusive');
});
test('bootstrap resamples paired blocks deterministically using recorded seeds', () => {
  const xs = Array.from({ length: 12 }, (_, i) => 0.5 + i / 10);
  assert.deepEqual(bootstrap(xs, 7), bootstrap(xs, 7));
  assert.throws(() => bootstrap(xs.slice(1), 7));
});
test('64-bit result digests are preserved exactly', () => {
  assert.equal(parseRecord('{"digest":-9223372036854775000}').digest, '-9223372036854775000');
});
test('missing/extra, reordered, failed and overlapping samples fail closed', () => {
  for (const corrupt of [
    r => r.pop(), r => r.push(r[0]), r => { r[1].label = 'release'; },
    r => { r[2].seq = 0; }, r => { r[1].exit = 1; },
    r => { r[1].startedAt = '2026-10-06T00:00:00Z'; },
  ]) {
    const { plan, records } = fixture(); corrupt(records);
    assert.throws(() => analyze(plan, records));
  }
});
test('unequal work, settings, correctness or tampered raw bytes fail closed', () => {
  for (const mutate of [
    raw => raw.replace('"work_divisor":1', '"work_divisor":2'),
    raw => raw.replace('"ops":200000', '"ops":100000'),
    raw => raw.replace('9223372036854775000', '9223372036854775001'),
    raw => raw.replace('"Normal"', '"Strict"'),
  ]) {
    const { plan, records } = fixture(); records[0].raw = mutate(records[0].raw);
    records[0].rawSha256 = sha256(records[0].raw);
    assert.throws(() => analyze(plan, records));
  }
  const { plan, records } = fixture(); records[0].raw += ' ';
  assert.throws(() => analyze(plan, records));
});
test('warmup is retained and checked, but does not affect the median', () => {
  const { plan, records } = fixture();
  for (const r of records.slice(0, 4)) if (r.label === 'main') {
    r.raw = r.raw.replace('"elapsed_ns":1000000', '"elapsed_ns":100000000');
    r.rawSha256 = sha256(r.raw);
  }
  assert.equal(analyze(plan, records).cases[0].median, 1);
});
test('late interruption/error cannot qualify an exit-zero receipt', () => {
  verifyCompletion({ exit: 0 });
  for (const receipt of [{ exit: 143 }, { exit: 0, interrupted: 'SIGTERM' },
    { exit: 0, interrupted: 'SIGINT' }, { exit: 0, error: 'failure' }]) {
    assert.throws(() => verifyCompletion(receipt));
  }
  const { plan, records } = fixture(); records[records.length - 1].signal = 'SIGTERM';
  assert.throws(() => analyze(plan, records));
});
test('image custody requires six distinct successful builds and nonempty immutable inventories', () => {
  const keys = ['normal/release/base', 'normal/release/after-updates', 'normal/main/base',
    'normal/main/after-updates', 'strict/release/base', 'strict/main/base'];
  const inventory = [{ path: 'db.redline/data.bin', sha256: 'a'.repeat(64), size: 512 }];
  const validImages = keys.map(key => ({ key, before: inventory, after: inventory }));
  const validBuilds = keys.map(key => {
    const [pair, label, kind] = key.split('/'); return { pair, label, kind, code: 0, signal: null };
  });
  verifyImages(validImages, validBuilds);
  for (const corrupt of [
    i => { i[0] = {}; }, i => { i[0] = i[1]; }, i => i.pop(),
    i => { i[0].before = []; i[0].after = []; },
    i => { i[0].before[0].sha256 = 'invalid'; },
    i => { i[0].before.push(i[0].before[0]); },
    i => { i[0].after = [{ ...i[0].before[0], size: 513 }]; },
  ]) {
    const images = structuredClone(validImages); corrupt(images);
    assert.throws(() => verifyImages(images, validBuilds));
  }
  for (const corrupt of [
    b => b.pop(), b => { b[0] = b[1]; }, b => { b[0].code = 1; },
    b => { b[0].signal = 'SIGTERM'; },
  ]) {
    const builds = structuredClone(validBuilds); corrupt(builds);
    assert.throws(() => verifyImages(validImages, builds));
  }
});
