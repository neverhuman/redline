import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { sha256 } from './abba-summary.mjs';
import { verifyStudies, verifyEvidence, verifyReceiptIdentity } from './focused-investigation.mjs';

const studies = [
  'focused-normal-94da-20261007T2100Z', 'focused-bisect7b-20261007T2111Z',
  'focused-bisect-e9-20261007', 'focused-baseline141-20261007',
  'focused-matched-main-20261007', 'focused-matched-7b-retry-20261007',
  'focused-matched-e9-20261007', 'focused-adjacent-8f-vs-7b-20261007',
  'focused-candidate-ea9-vs-parent-20261007', 'series-recovery7b-20261007',
  'series-wal8f-20261007', 'series-main8f-20261007',
  'focused-candidate-checkpoint-parent-20261007',
  'focused-candidate-checkpoint-release-20261007',
  'focused-candidate-checkpoint-owned-parent-20261007',
  'focused-candidate-checkpoint-owned-release-20261007',
];
test('all contradictory studies must remain, including the rejected candidate and controls', () => {
  verifyStudies(studies);
  for (let i = 0; i < studies.length; i++) assert.throws(() => verifyStudies(studies.filter((_, j) => j !== i)));
});
test('duplicate, unknown or escaping paths cannot pad the retained study count', () => {
  for (const extra of [studies[0], 'invented-clean-study', '../elsewhere', '/absolute']) {
    assert.throws(() => verifyStudies([...studies, extra]));
  }
});
const root = new URL('../../benchmark-results/perf/regressions/focused-normal-94da-20261007/', import.meta.url);
test('a valid receipt cannot replace another required source comparison', () => {
  const hash = sha256(fs.readFileSync(new URL(`${studies[0]}/receipt.json`, root))), seen = new Set();
  verifyReceiptIdentity(studies[0], hash, seen);
  assert.throws(() => verifyReceiptIdentity(studies[1], hash, new Set()));
  assert.throws(() => verifyReceiptIdentity(studies[0], hash, seen));
});
test('supplemental evidence cannot omit negative proof or raw traces', () => {
  const { evidence } = JSON.parse(fs.readFileSync(new URL('investigation.json', root)));
  verifyEvidence(evidence);
  for (const required of ['wal-scratch-parent.log.gz', 'source-bisect-series.json',
    'archive-candidate.json', 'rejected-candidate/production.patch.gz', 'trace-diagnostic/17-main.trace']) {
    assert.throws(() => verifyEvidence(evidence.filter(e => e.path !== required)));
  }
  assert.throws(() => verifyEvidence([]));
  assert.throws(() => verifyEvidence([...evidence, evidence[0]]));
  assert.throws(() => verifyEvidence([{ ...evidence[0], path: '../escape' }, ...evidence.slice(1)]));
  assert.throws(() => verifyEvidence([{ ...evidence[0], sha256: 'garbage' }, ...evidence.slice(1)]));
});
