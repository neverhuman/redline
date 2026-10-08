// Preserve both predeclared studies and derive their index from complete receipts.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { gunzipSync } from 'node:zlib';
import { pathToFileURL } from 'node:url';
import { summarize, report, formatRatio } from './tighter-summary.mjs';
import { sha256 } from './abba-summary.mjs';
import { summarizeProbe, probeReport } from './probe-barrier-summary.mjs';

export function verifyManifest(manifest) {
  assert.equal(manifest.schema,'redline-tighter-investigation-v1');
  assert.deepEqual(manifest.studies.map(s=>s.name),['two-core','one-core']);
  assert.equal(manifest.diagnostic.name,'scheduling-barrier');
  assert.equal(manifest.rejected.name,'rejected-affinity');
  assert.equal(manifest.rejected.eligible,false);assert.equal(manifest.rejected.records,175);
  for(const item of [manifest.diagnostic,manifest.rejected]) {
    assert.match(item.archiveSha256,/^[a-f0-9]{64}$/);assert.match(item.receiptSha256,/^[a-f0-9]{64}$/);
  }
  const receipts=new Set();
  for(const study of manifest.studies) {
    assert.equal(study.sessionReceipts.length,2);
    assert.match(study.archiveSha256,/^[a-f0-9]{64}$/);
    for(const hash of study.sessionReceipts) {
      assert.match(hash,/^[a-f0-9]{64}$/);assert(!receipts.has(hash),'repeated session receipt');receipts.add(hash);
    }
  }
}
function verifyDiagnosticCustody(root,item,custodyName,exit) {
  const custody=JSON.parse(fs.readFileSync(path.join(root,custodyName)));
  assert.equal(custody.archiveSha256,item.archiveSha256);
  for(const f of custody.files.filter(f=>f.path.startsWith('bundle/'))) {
    const bytes=fs.readFileSync(path.join(root,item.name,f.path));
    assert.equal(bytes.length,f.size,f.path+' size');assert.equal(sha256(bytes),f.sha256,f.path+' custody');
  }
  const receiptBytes=fs.readFileSync(path.join(root,item.name,'bundle/receipt.json'));
  assert.equal(sha256(receiptBytes),item.receiptSha256);
  const receipt=JSON.parse(receiptBytes);assert.equal(receipt.exit,exit);
  for(const f of receipt.files)assert.equal(sha256(fs.readFileSync(path.join(root,item.name,'bundle',f.path))),f.sha256);
  for(const name of ['coordinator','measure'])assert.equal(fs.readFileSync(path.join(root,item.name,name+'.exit'),'utf8'),exit+'\n');
  return receipt;
}
export function investigation(root, check=false) {
  const manifest=JSON.parse(fs.readFileSync(path.join(root,'investigation.json')));
  verifyManifest(manifest);
  const products={},results=[];
  for(const [index,study] of manifest.studies.entries()) {
    const bundle=path.join(root,study.name),s=summarize(bundle);
    assert.deepEqual(s.sessionReceipts,study.sessionReceipts);
    assert.equal(s.method.main,'3e8b2d5b6f13a4440c10ad5c56bff73d3d967538');
    assert.equal(s.method.release,'9277455d5ad008252053a81d18add39b8cdc8f7b');
    assert.equal(s.method.blocksPerSession,index?128:64);
    assert.equal(s.method.maximumPinnedCPUs,index?1:2);
    assert.equal(s.method.primaryFamilySize,index?4:2);
    assert.match(study.archiveSha256,/^[a-f0-9]{64}$/);
    const custody=JSON.parse(fs.readFileSync(path.join(root,index?'one-core-custody.json':'complete-custody.json')));
    assert.equal(custody.archiveSha256,study.archiveSha256,'archive custody differs');
    for(const f of custody.files.filter(f=>/^session-[12]\/bundle\//.test(f.path))) {
      const bytes=fs.readFileSync(path.join(bundle,f.path));
      assert.equal(bytes.length,f.size,f.path+' size');assert.equal(sha256(bytes),f.sha256,f.path+' custody');
    }
    products[study.name+'/summary.json']=JSON.stringify(s,null,2)+'\n';
    products[study.name+'/README.md']=report(s);results.push(s);
  }
  verifyDiagnosticCustody(root,manifest.diagnostic,'probe-barrier-custody.json',0);
  const rejected=verifyDiagnosticCustody(root,manifest.rejected,'probe-first-rejected-custody.json',1);
  assert.match(rejected.error,/assert\(!record.probeError\)/);
  const rejectedRecords=fs.readFileSync(path.join(root,manifest.rejected.name,'bundle/records.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(rejectedRecords.length,manifest.rejected.records);
  assert(rejectedRecords.some(r=>r.probeError),'rejected scheduling metadata error missing');
  const diagnostic=summarizeProbe(path.join(root,manifest.diagnostic.name,'bundle'));
  products[manifest.diagnostic.name+'/bundle/summary.json']=JSON.stringify(diagnostic,null,2)+'\n';
  products[manifest.diagnostic.name+'/bundle/README.md']=probeReport(diagnostic);
  const seen=new Set();
  for(const f of manifest.evidence) {
    assert(typeof f.path==='string'&&!path.isAbsolute(f.path)&&!f.path.split('/').some(p=>!p||p==='.'||p==='..'));
    assert(!seen.has(f.path));seen.add(f.path);
    const bytes=fs.readFileSync(path.join(root,f.path));assert.equal(sha256(bytes),f.sha256,f.path);
    if(f.path.endsWith('.gz'))assert.equal(sha256(gunzipSync(bytes)),f.rawSha256,f.path+' raw bytes');
  }
  for(const required of ['complete-custody.json','one-core-custody.json','link-removal.json','jope-cache-owner-note.json',
    'build-release.log.gz','build-main.log.gz','method-unit.log.gz','method-one-core-exact.log.gz',
    'external-target-parent.log.gz','external-target-head-final.log.gz','build-pair.py.gz',
    'external-target-node-parent.log.gz','external-target-node-head.log.gz','method-review-final.log.gz',
    'collect-complete.py.gz','collect-one-core.py.gz'])assert(seen.has(required),'missing evidence '+required);
  for(const name of ['build-pair','build-pair-wrapper','build-release','build-main','collect-complete',
    'collect-one-core','external-target-node-head','method-review-final','source-preservation-final',
    'probe-barrier-observer','collect-probe-barrier','collect-probe-barrier-wrapper','summary-probe-barrier',
    'method-barrier-controls-final','build-bisect','build-bisect-wrapper']) {
    assert(seen.has(name+'.exit'));assert.equal(fs.readFileSync(path.join(root,name+'.exit'),'utf8'),'0\n',name+' failed');
  }
  assert(seen.has('external-target-node-parent.exit'));
  assert.equal(fs.readFileSync(path.join(root,'external-target-node-parent.exit'),'utf8'),'1\n','parent failure missing');
  const rows=results.flatMap((s,i)=>s.cases.map(c=>'| '+[manifest.studies[i].name,c.id,formatRatio(c.median),
    c.adjusted.map(formatRatio).join('–'),c.role==='primary'?100*s.method.familywiseIntervalsConfidence+'%':'95%',c.verdict].join(' | ')+' |'));
  const originalPassed=results.reduce((n,s)=>n+s.counts.passed,0),passed=originalPassed+diagnostic.counts.passed;
  const corrected=diagnostic.cases.map(c=>'| '+[c.id,formatRatio(c.gated.wall.median),c.gated.wall.ci95.map(formatRatio).join('–')].join(' | ')+' |');
  products['README.md']=`# Tighter Top 10 and grouped-join investigation

Two complete studies and a separate scheduling-control crossover compare exact main ${results[0].method.main} with v5.1.1 ${results[0].method.release}. The completed datasets contain ${passed} successful raw executions (${originalPassed} in the original studies and ${diagnostic.counts.passed} in the crossover), zero failed engine executions and zero skips. A separate rejected diagnostic retains ${manifest.rejected.records} partial records and an exit-1 metadata failure; it is not counted as passing evidence. All warmups and load/timing observations are preserved. Lower main/release query-time ratios are better.

| Study | Case | Main / release median | Decision interval | Confidence | Finding |
|---|---|---:|---:|---:|---|
${rows.join('\n')}

The [first study](two-core/README.md) used two pinned CPUs and 64 ABBA blocks per case in each of two sessions. Both primary decisions stayed inconclusive. The [final confirmation](one-core/README.md) was declared afterward, uses one pinned logical CPU to prevent migration between pinned cores, and doubles the blocks. Its stricter primary interval uses a four-comparison Bonferroni correction as a conservative diagnostic; this does not establish fixed-sample familywise coverage for the adaptive investigation. Both sessions must independently agree before a direction is reproduced. No further sample extension was made. These are exploratory shared-host diagnostics; crossing 1.0 does not establish equivalence, and upper bounds apply only under the recorded conditions.

The original single-core launcher synchronously checked the running child on the same CPU. The [completed scheduling-barrier crossover](scheduling-barrier/bundle/README.md) holds engine/controller CPU, binaries, seed images and work constant, but completes scheduling checks before permitting the gated binary to exec. Direct launch reproduces the large elapsed slowdown; gated launch removes most of it and the main elapsed-minus-process-CPU gap. This supports controller overlap as the source of the large diagnostic slowdown. The waiting-shell treatment and shared-host conditions are explicit, so it does not establish a unique engine source cause.

| Case | Gated main / release median | Exploratory 95% interval |
|---|---:|---:|
${corrected.join('\n')}

Neither primary gated interval lies wholly above the owner's 1.05 regression threshold. A small Top 10 difference remains; this single exploratory crossover neither establishes equivalence nor qualifies a future release. The [rejected affinity precursor](rejected-affinity/bundle/receipt.json) failed when a scheduling probe inspected an already-exited child; its partial records have no computed timing interval. Three [historical candidate builds](historical-builds/) are preserved for attribution, but no completed historical timing bisect is claimed.

The original release throughput tables remain bound to their publishable bundles. No absolute README figure is replaced by these query-timer diagnostics. The future launcher uses the verified pre-exec barrier; CI regression tests exercise waiting, same-PID execution, failed checks, EOF, literal arguments and child failure without running timings in CI.

[Input manifest](investigation.json) pins every session/diagnostic receipt, off-root archive hash and supplemental source/build/verification log. Raw plans, records, scheduling and core/SMT/load telemetry are committed within each dataset. Every archive payload inventory was independently hash-verified before import. The cache target symlink was removed after the first complete study; builds use a per-command external Cargo target directory. Default and explicit-target CI checks retain every original lane.
`;
  for(const [name,text] of Object.entries(products)) {
    const file=path.join(root,name);
    if(check)assert.equal(fs.readFileSync(file,'utf8'),text,name+' drift');else fs.writeFileSync(file,text);
  }
  return {studies:results.length,diagnostics:1,passed,failed:0,skipped:0,rejectedRecords:manifest.rejected.records,
    cases:results.map(s=>s.cases.map(({id,verdict})=>({id,verdict}))),
    gated:diagnostic.cases.map(c=>({id:c.id,median:c.gated.wall.median,ci95:c.gated.wall.ci95}))};
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href) {
  const [root,mode]=process.argv.slice(2);assert(root&&(!mode||mode==='--check'));
  console.log(JSON.stringify(investigation(root,mode==='--check')));
}
