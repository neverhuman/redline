// Retain contradictory studies; render their figures from verified raw receipts.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { gunzipSync } from 'node:zlib';
import { pathToFileURL } from 'node:url';
import { summarizeFocused, focusedReport, formatRatio, analyzeProcess } from './focused-summary.mjs';
import { sha256 } from './abba-summary.mjs';

const requiredStudies = [
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
const expectedReceipts = {
  "focused-candidate-checkpoint-owned-parent-20261007": "e6ba62e0aacc73d7944a92852d01752609553f70940e2475df28878d3e7ea512",
  "focused-candidate-checkpoint-owned-release-20261007": "2d429ed5c2209a09258316a296ece82d5ce1fb73cc37d98acf2d7476c84971d1",

  "focused-candidate-checkpoint-release-20261007": "3d95cb6f1a45285c893c9eebbec8b963dc8b659acaddfd0a0d1aa7c13a3665ff",
  "focused-candidate-checkpoint-parent-20261007": "4c6d538b921c0679052f015792cd21691731f52c015df73c3c975288029846d6",
  "focused-adjacent-8f-vs-7b-20261007": "5fa3c24afacf2b62d4e53aaee412337820760ded90a8b53d9828d0590b7ea580",
  "focused-baseline141-20261007": "df326ad0420ff22b35e8ac7c3d625f17cdfcd77773057c14e623f26515d305cc",
  "focused-bisect-e9-20261007": "e48ca0020d51510c0d9d164275e0ea42b58ce19a1769858ef0401e645dca4b52",
  "focused-bisect7b-20261007T2111Z": "2e36bb80e3b71313d62e5b506565f15eee5873ba7ff684d9f1a403a0e6995594",
  "focused-candidate-ea9-vs-parent-20261007": "9fce310e1c1b39571a84097eb73b243c60f5ccaab51730ecd1cf91f598470487",
  "focused-matched-7b-retry-20261007": "e991c32d9f982475bb25459a8b4ce8f086b5f418272df37be8eabcb2fb44e0fe",
  "focused-matched-e9-20261007": "f057ef028836de9559901b9eb05e539d746f1c8c6d62113f60de8679ade98bdf",
  "focused-matched-main-20261007": "1499adeca3740be27e5715b90513ee44a0ff4571881125cefa63224c00b67f82",
  "focused-normal-94da-20261007T2100Z": "20df81485c5c860d3ed7ac1f32bdee8c838b7ce5ccf843e3311ad841c7395f95",
  "series-main8f-20261007": "ee845d2f37442786ee9147d9fa31dad7ce89d5d03a36a6461b5ac4c6edebfe22",
  "series-recovery7b-20261007": "dc97f9e73aba7a7ba133df6b5f9d3342577e75df5841f83cffdb65512994381a",
  "series-wal8f-20261007": "53ccfb5c97665ca74362ab5f9d5e3578ca11ce241b5d57cf379b09a919074251"
};
const requiredEvidence = [
  'focused-final-unit.log.gz', 'focused-final-unit.exit', 'focused-final-unit-fixed.log.gz', 'focused-final-unit-fixed.exit',
  'wal-ownership-head.log.gz', 'wal-ownership-head.exit', 'wal-ownership-head-fixed.log.gz', 'wal-ownership-head-fixed.exit',
  'focused-parent-acceptance.log.gz', 'focused-parent-acceptance.exit', 'focused-head-acceptance.log.gz', 'focused-head-acceptance.exit',
  'wal-ownership-parent-final.log.gz', 'wal-ownership-parent-final.exit', 'wal-ownership-parent-final-test.sha256',
  'focused-kernel-head-final.log.gz', 'focused-kernel-head-final.exit', 'candidate-git-custody.json',
  'commit-inventory.json', 'binary-provenance-analysis.json', 'compare-baseline-elf-instructions.py.gz',
  'launch-study.py.gz', 'collect-study.py.gz', 'series-coordinate.py.gz', 'profile-group-minimal.py.gz',
  'source-bisect-series.json', 'source-bisect-series-coordinator-attempt1.log.gz',
  'source-bisect-series-coordinator-attempt1.exit', 'source-bisect-series-coordinator-attempt2.log.gz',
  'source-bisect-series.exit', 'archives-bisect.jsonl', 'archives-matched.jsonl', 'archives-profile.jsonl',
  'archive-candidate.json', 'archives-final.jsonl', 'candidate-production.patch.gz', 'candidate-source.json', 'wal-scratch-parent.log.gz', 'wal-scratch-parent.exit',
  'wal-scratch-head.log.gz', 'wal-scratch-head.exit', 'recovery-guard-head.log.gz',
  'recovery-guard-head.exit', 'recovery-guard-pageimage.log.gz', 'recovery-guard-pageimage.exit',
  'build-candidate-ea9.log.gz', 'candidate-ea9-window512.exit', 'build-candidate-checkpoint-late.log.gz',
  'candidate-checkpoint-late.exit', 'build-candidate-owned-encoded.log.gz',
  'candidate-owned-encoded.exit', 'rejected-candidate/production.patch.gz',
  'rejected-candidate/wal_scan_scratch.rs', 'trace-diagnostic/analysis.json', 'trace-diagnostic/launch.json',
];

export function verifyStudies(studies) {
  assert(Array.isArray(studies) && studies.every(s => typeof s === 'string' && /^[a-zA-Z0-9-]+$/.test(s)));
  assert.equal(new Set(studies).size, studies.length, 'duplicate study');
  for (const required of requiredStudies) assert(studies.includes(required), `missing retained study: ${required}`);
  for (const study of studies) assert(Object.hasOwn(expectedReceipts, study), 'unknown study');
}

export function verifyReceiptIdentity(study, receiptSha256, seen) {
  assert.equal(receiptSha256, expectedReceipts[study], `wrong receipt for ${study}`);
  assert(!seen.has(receiptSha256), 'duplicate receipt'); seen.add(receiptSha256);
}

export function verifyEvidence(evidence) {
  assert(Array.isArray(evidence));
  const paths = new Set();
  for (const input of evidence) {
    assert(typeof input.path === 'string' && input.path.length && !path.isAbsolute(input.path) && !input.path.split('/').some(p => p === '..' || p === '.' || !p));
    assert.match(input.sha256, /^[a-f0-9]{64}$/);
    if (input.path.endsWith('.gz')) assert.match(input.rawSha256, /^[a-f0-9]{64}$/);
    assert(!paths.has(input.path), 'duplicate supplemental evidence'); paths.add(input.path);
  }
  for (const required of requiredEvidence) assert(paths.has(required), `missing supplemental evidence: ${required}`);
  for (let slot = 0; slot < 48; slot++) {
    const label = slot % 4 === 0 || slot % 4 === 3 ? 'release' : 'main';
    for (const suffix of ['trace', 'stdout', 'stderr']) {
      assert(paths.has(`trace-diagnostic/${slot}-${label}.${suffix}`), 'missing raw diagnostic');
    }
  }
}

export function verifyCandidateSource(root, sourceRoot = process.cwd()) {
  const candidate = JSON.parse(fs.readFileSync(path.join(root, 'candidate-source.json')));
  assert.equal(candidate.measuredCommit, 'ac96a7150a5ad4c244605ec09124518d5f016dae');
  assert.equal(candidate.productionParent, '94da3b131d6d50009d2d24e026b9010681e1f04d');
  assert.deepEqual(candidate.files.map(f => f.path), ['crates/kernel/src/engine/recovery.rs', 'crates/kernel/src/wal/manager/storage/scan.rs']);
  for (const f of candidate.files) assert.equal(sha256(fs.readFileSync(path.join(sourceRoot, f.path))), f.sha256, `measured candidate source differs: ${f.path}`);
}

export function investigation(root, check = false) {
  const manifest = JSON.parse(fs.readFileSync(path.join(root, 'investigation.json')));
  assert.equal(manifest.mainCommit, '94da3b131d6d50009d2d24e026b9010681e1f04d');
  verifyStudies(manifest.studies);
  const rows = [], hashes = [], receipts = new Set(), series = [], processRows = [];
  let passed = 0, failed = 0, skipped = 0;
  for (const study of manifest.studies) {
    const bundle = path.join(root, study), summary = summarizeFocused(bundle);
    verifyReceiptIdentity(study, summary.receiptSha256, receipts);
    if (study.startsWith('focused-candidate-checkpoint-owned-')) {
      const plan = JSON.parse(fs.readFileSync(path.join(bundle, 'plan.json')));
      const records = fs.readFileSync(path.join(bundle, 'records.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
      for (const c of analyzeProcess(plan, records)) processRows.push(`| ${study.includes('-parent-') ? 'Parent' : 'Release'} | ${c.id} | ${c.referenceMedianMs} | ${c.candidateMedianMs} | ${formatRatio(c.median)} [${c.ci95.map(formatRatio).join(', ')}] |`);
    }
    if (study.startsWith('series-')) series.push({ study, summary, receipt: JSON.parse(fs.readFileSync(path.join(bundle, 'receipt.json'))) });
    const products = { 'summary.json': `${JSON.stringify(summary, null, 2)}\n`, 'README.md': focusedReport(summary) };
    for (const [name, text] of Object.entries(products)) {
      if (check) assert.equal(fs.readFileSync(path.join(bundle, name), 'utf8'), text, `${study}/${name} drift`);
      else fs.writeFileSync(path.join(bundle, name), text);
    }
    passed += summary.counts.passed; failed += summary.counts.failed; skipped += summary.counts.skipped;
    const ratios = summary.cases.map(c => `${formatRatio(c.median)} [${formatRatio(c.ci95[0])}, ${formatRatio(c.ci95[1])}]`);
    rows.push(`| [${study}](${study}/README.md) | ${summary.versions.measuredRelease.slice(0, 8)} → ${summary.versions.measuredMain.slice(0, 8)} | ${summary.host.name} | ${ratios.join(' | ')} |`);
    hashes.push(`- ${study}: receipt SHA-256 \`${summary.receiptSha256}\`; load min/median/max ${summary.host.load1.min.toFixed(2)}/${summary.host.load1.median.toFixed(2)}/${summary.host.load1.max.toFixed(2)}.`);
  }
  assert(failed === 0 && skipped === 0);
  const ordered = ['series-recovery7b-20261007', 'series-wal8f-20261007', 'series-main8f-20261007'].map(name => series.find(s => s.study === name));
  assert(ordered.every(Boolean));
  for (const s of ordered) {
    assert.equal(s.summary.host.name, ordered[0].summary.host.name, 'series host changed');
    assert.deepEqual(s.summary.sourceBisectSeries, ordered[0].summary.sourceBisectSeries, 'series method changed');
  }
  for (let i = 1; i < ordered.length; i++) assert(ordered[i - 1].receipt.finishedAt <= ordered[i].receipt.startedAt, 'series overlapped or reordered');
  verifyEvidence(manifest.evidence);
  for (const input of manifest.evidence) {
    const bytes = fs.readFileSync(path.join(root, input.path));
    assert.equal(sha256(bytes), input.sha256, input.path);
    if (input.path.endsWith('.gz')) assert.equal(sha256(gunzipSync(bytes)), input.rawSha256, `${input.path} raw bytes`);
  }
  const inventory = JSON.parse(fs.readFileSync(path.join(root, 'commit-inventory.json')));
  assert.equal(inventory.range, 'v5.1.1..94da3b131');
  const result = `# Focused Normal regression investigation\n\n` +
    `Generated from every retained full-work receipt. ${manifest.studies.length} complete studies; ${passed} passed / ${failed} failed / ${skipped} skipped executions, including retained warmups. ` +
    `These diagnostics do not replace publishable release throughput or establish a regression-free future release.\n\n` +
    `The owner requested only three flags and two stable controls, with the reproduction boundary at 1.0. Every study uses the same five cases, 20,000 rows, full operations/digests/settings, 12 ABBA blocks and one warmup block, pinned CPUs2–3, nice19 and idle I/O. ` +
    `The seeded percentile95% bootstrap resamples whole blocks. Intervals are per case without multiplicity correction; crossing1.0 is inconclusive. ` +
    `Hosts were naturally available; cores were pinned but neither kernel-isolated nor exclusive. Selected-core/SMT counters and runner presence are in each host log. ` +
    `The closing three-study series chose the lowest-load host once at its beginning and kept that host; other studies made a fresh lowest-load selection.\n\n` +
    `| Study | Reference → measured engine | Host | Prepared point ratio [CI] | Top10 ratio [CI] | Grouped join ratio [CI] | SQL-text control [CI] | Join-point control [CI] |\n` +
    `|---|---|---|---:|---:|---:|---:|---:|\n${rows.join('\n')}\n\n` +
    `Ratios are measured/reference elapsed time per equal operation. The eight-character hashes above are shorthand; each linked report records full commits, binary hashes and exact reference roles. ` +
    `The scratch-window candidate is a rejected experiment, not a merged fix. Preserve positive, inconclusive and contradictory comparisons together.\n\n` +
    `## Candidate scope and process cost\n\n` +
    `The final candidate retains the 1 MiB WAL read window, adds an owned encoded buffer for ordinary records up to 512 KiB, and defers directory construction when no WAL remains after the checkpoint. ` +
    `The first change adds one allocation, copy and free per ordinary record; oversized records still borrow the window. It does not restore the complete historical allocation sequence or halve resident memory. ` +
    `Before/after data support prepared-lookup improvement; the other flagged intervals cross 1.0 against the parent. The separate matched-release comparison places all three flagged intervals below 1.0, with controls inconclusive. The grouped-join upper bound is very close to 1.0, and parent prepared timings contain reference outliers; the apparent parent gain is not a stable general percentage. Candidate selection was exploratory and adaptive. ` +
    `This is evidence for a mitigation under the recorded conditions, not unique causal attribution, equivalence or a regression-free future release.\n\n` +
    `Child-lifetime diagnostics below use the same retained ABBA blocks and bootstrap seeds. Millisecond wall-clock timestamps include process launch, image copying, open/recovery, correctness checks, queries and output. ` +
    `They do not measure pure open/recovery cost and must not be substituted for the query timer above.\n\n` +
    `| Reference | Case | Reference median ms | Candidate median ms | Candidate/reference child-lifetime ratio [95% CI] |\n|---|---|---:|---:|---:|\n${processRows.join('\n')}\n\n` +
    `## Source and artifact limits\n\n` +
    `[Commit inventory](commit-inventory.json) covers ${inventory.commitCount} commits and ${inventory.changedPathCount} changed-path entries, with hypotheses recorded before bisect. ` +
    `e9 changes range routing/shared rowid detection; 7b moves directory construction before heap redo; 8f changes WAL scan buffering. ` +
    `No direct TopK or grouped-join algorithm change was found. Queries begin after opening, so startup allocation/cache history remains a competing explanation. ` +
    `[Binary provenance](binary-provenance-analysis.json) shows that historical and rebuilt AF4 executables have identical normalized instruction streams but different linked data/addresses. ` +
    `That is not semantic or timing equivalence and does not prove a layout cause. Source comparisons use the matched rebuilt AF4 reference.\n\n` +
    `[Investigation manifest](investigation.json) binds supplemental logs, inventory, source experiments, traced diagnostics and archive custody receipts by SHA-256. ` +
    `Traced executions are separate diagnostics and never timing acceptance; untraced studies above retain their original raw samples.\n\n` +
    `## Receipt hashes and observed load\n\n${hashes.join('\n')}\n`;
  if (check) assert.equal(fs.readFileSync(path.join(root, 'README.md'), 'utf8'), result, 'investigation report drift');
  else fs.writeFileSync(path.join(root, 'README.md'), result);
  return { studies: manifest.studies.length, passed, failed, skipped };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [root, mode, sourceCheck] = process.argv.slice(2);
  assert(root && (!mode || mode === '--check') && (!sourceCheck || sourceCheck === '--candidate-source'), 'usage: focused-investigation.mjs ROOT [--check [--candidate-source]]');
  if (sourceCheck) verifyCandidateSource(root);
  console.log(JSON.stringify(investigation(root, mode === '--check')));
}
