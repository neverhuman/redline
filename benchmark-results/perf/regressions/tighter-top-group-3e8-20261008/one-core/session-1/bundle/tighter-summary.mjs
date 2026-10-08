import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { median, parseRecord, sha256, verifyCompletion } from './abba-summary.mjs';

export const ids = ['top10', 'join_group_by_city', 'point_pk_sql_text'];
const order = ['release', 'main', 'main', 'release'];
export function intervals(strata, seed, draws = 50000, chunk = 8) {
  assert(strata.length && strata.every(s => s.length >= chunk && s.length % chunk === 0 && s.every(x => Number.isFinite(x) && x > 0)));
  assert(Number.isInteger(seed) && seed > 0 && draws >= 1000 && Number.isInteger(draws));
  let state = seed >>> 0;
  const random = () => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) / 4294967296; };
  const estimates = Array.from({ length: draws }, () => {
    const pooled = [];
    for (const s of strata) for (let k = 0; k < s.length / chunk; k++) {
      const start = Math.floor(random() * s.length);
      for (let offset = 0; offset < chunk; offset++) pooled.push(s[(start + offset) % s.length]);
    }
    return median(pooled);
  }).sort((a,b) => a-b);
  const range = alpha => [estimates[Math.floor(alpha/2*(draws-1))], estimates[Math.ceil((1-alpha/2)*(draws-1))]];
  return { ci95: range(.05), ci975: range(.025), ci9875: range(.0125) };
}
export function classify(adjusted, sessions) {
  assert(adjusted.length === 2 && adjusted[0] <= adjusted[1]);
  assert(sessions.length === 2 && sessions.every(c => c.length === 2 && c[0] <= c[1]));
  return adjusted[0] > 1 && sessions.every(c => c[0] > 1) ? 'reproduced-slowdown'
    : adjusted[1] < 1 && sessions.every(c => c[1] < 1) ? 'reproduced-speedup' : 'inconclusive';
}
export function validateRecords(plan, records, hosts) {
  assert([64,128].includes(plan.blocks)); assert.equal(plan.warmupBlocks,1); assert.equal(plan.rows,20000);
  assert.equal(plan.workDivisor,1); assert.equal(plan.nice,19); assert.equal(plan.ioClass,'idle'); assert.equal(plan.order,'ABBA');
  assert.deepEqual(plan.cases.map(c => c.id).sort(), [...ids].sort());
  assert.equal(records.length, (plan.blocks+1)*plan.cases.length*4);
  const ratios = new Map(ids.map(id => [id,[]]));
  const bracket = new Map();
  for (const h of hosts) {
    assert.equal(h.cpus,plan.cpus);
    assert(h.load.length === 3 && h.load.every(x => Number.isFinite(x) && x>=0));
    assert.deepEqual(h.cpuCounters.map(x => Number(x.split(' ')[0].slice(3))),plan.sampleCPUs);
    assert(Array.isArray(h.runnerWorkers) && h.runnerWorkers.every(Number.isSafeInteger));
    if (h.stage) { const key=h.seq+'/'+h.stage; assert(!bracket.has(key)); bracket.set(key,h); }
  }
  let at=0;
  for (let block=-1; block<plan.blocks; block++) for (const c of plan.cases) {
    const times=[];
    for (let slot=0;slot<4;slot++) {
      const r=records[at++];
      assert.equal(r.seq,at-1);assert.equal(r.case,c.id);assert.equal(r.block,block);
      assert.equal(r.slot,slot);assert.equal(r.label,order[slot]);assert.equal(r.pair,'normal');
      assert.equal(r.exit,0);assert.equal(r.signal,null);assert.equal(r.status,'ok');assert.equal(r.rawSha256,sha256(r.raw));
      assert.equal(r.scheduling.cpus,plan.cpus);assert.equal(r.scheduling.nice,19);assert.match(r.scheduling.ioPriority,/idle/);
      const m=parseRecord(r.raw);
      assert.equal(m.engine,'redlinedb');assert.equal(m.workload,c.id);assert.equal(m.pair,'normal');
      assert.equal(m.rows,20000);assert.equal(m.work_divisor,1);assert.equal(m.ops,c.ops);
      assert.equal(m.digest,c.digest);assert.equal(m.unit,c.unit);assert.deepEqual(m.settings,c.settings);
      assert(Number.isSafeInteger(m.elapsed_ns) && m.elapsed_ns>0);times.push(m.elapsed_ns/c.ops);
      assert(r.startedAt <= r.finishedAt);
      if (r.seq) assert(records[r.seq-1].finishedAt <= r.startedAt,'overlap');
      for (const stage of ['before','after']) {
        const h=bracket.get(r.seq+'/'+stage);assert(h,'missing host bracket');
        for (const key of ['case','block','slot','label']) assert.equal(h[key],r[key]);
        assert(stage === 'before' ? h.at <= r.startedAt : h.at >= r.finishedAt);
      }
    }
    if (block>=0) ratios.get(c.id).push(Math.sqrt(times[1]*times[2]/(times[0]*times[3])));
  }
  assert.equal(bracket.size,records.length*2);
  return ratios;
}
export function session(bundle, baselineRoot) {
  const bytes = name => fs.readFileSync(path.join(bundle,name));
  const json = name => JSON.parse(bytes(name));
  const lines = name => bytes(name).toString().trim().split('\n').map(JSON.parse);
  const plan=json('plan.json'), receipt=json('receipt.json'), method=json('method-predeclared.json');
  verifyCompletion(receipt);assert.equal(receipt.planSha256,sha256(bytes('plan.json')));
  const expected=['plan.json','records.jsonl','host-samples.jsonl','image-builds.jsonl','release-build.json','main-build.json','tighter-compare.mjs','tighter-summary.mjs','abba-summary.mjs','method-predeclared.json'].sort();
  assert.deepEqual(receipt.files.map(f=>f.path).sort(),expected);
  for (const f of receipt.files) assert.equal(sha256(bytes(f.path)),f.sha256);
  assert.equal(plan.methodSha256,sha256(bytes('method-predeclared.json')));
  assert.equal(plan.purpose,'tighter-top-group-v511');assert.equal(plan.bootstrapDraws,50000);
  assert.equal(method.sessions,2);assert.equal(method.blocksPerSession,plan.blocks);assert.equal(method.bootstrapDraws,50000);
  assert.deepEqual(method.primaryCases,ids.slice(0,2));assert.deepEqual(method.controls,ids.slice(2));
  assert.equal(plan.bootstrapSeed,method.bootstrapSeed);assert.equal(method.noTimingDeletion,true);
  assert.equal(method.maximumPinnedCPUs,plan.selectedCPUs.length);
  assert(method.blocksPerSession===64&&method.maximumPinnedCPUs===2&&method.primaryFamilySize===2 ||
    method.blocksPerSession===128&&method.maximumPinnedCPUs===1&&method.primaryFamilySize===4,'undeclared design');
  assert.equal(method.familywiseIntervalsConfidence,1-.05/method.primaryFamilySize);
  assert.equal(plan.releaseTagCommit,'9277455d5ad008252053a81d18add39b8cdc8f7b');
  const builds=['release','main'].map(label=>{
    const b=json(label+'-build.json');const v=plan.versions[label];
    assert.equal(b.engine_commit,method[label]);assert.equal(v.engineCommit,method[label]);
    assert.equal(b.binary_sha256,v.binarySha256);assert.equal(b.harness_tree,method.harnessTree);
    assert.equal(b.profile,'release');assert.equal(b.rustflags,'');assert.equal(b.failpoints,false);assert.equal(b.pgo,false);
    assert.equal(b.source_dirty,false);assert.equal(b.productionArtifactsCleaned,true);assert.equal(b.build_jobs,2);
    return b;
  });
  assert.equal(builds[0].engine_commit,plan.releaseTagCommit);
  for (const key of ['engines','rustc_verbose_version','cargo_version','profile_release']) assert.deepEqual(builds[0][key],builds[1][key],key);
  assert.equal(plan.hostSelection.chosen,plan.host.name);
  const loads=['xbabe1','xbabe2','xbabe3'].map(n=>plan.hostSelection[n][0]);
  assert(loads.every(x=>Number.isFinite(x)&&x>=0));assert.equal(plan.hostSelection[plan.host.name][0],Math.min(...loads));
  assert.equal(new Set(plan.selectedCPUs).size,plan.selectedCPUs.length);assert.equal(plan.isolation.exclusive,false);
  assert.equal(plan.isolation.kernelIsolated,false);assert.equal(plan.comparisonMain,method.main);
  const base=path.join(baselineRoot,'benchmark-results/perf/releases/v5.1.1-vs-v5.1.0-paired-rerun21');
  const summary=fs.readFileSync(path.join(base,'summary.json'));
  assert.equal(sha256(summary),'4a84a36f490f06d4c7e17281533d0cb608fc41f37a0e8ee09a475e509e79ed88');
  const accepted=JSON.parse(summary);assert(accepted.publishable && accepted.blockers.length===0);
  const raw=fs.readFileSync(path.join(base,'v5.1.1/run-1-normal.jsonl'));
  const rawMeta=accepted.raw.find(r=>r.path==='v5.1.1/run-1-normal.jsonl');assert.equal(sha256(raw),rawMeta.sha256);
  const original=raw.toString().trim().split('\n').map(parseRecord);
  for (const c of plan.cases) {
    const r=original.find(r=>r.label==='v5.1.1'&&r.rep===0&&r.workload===c.id);
    assert(r);for(const k of ['ops','digest','unit','settings'])assert.deepEqual(c[k],r[k]);
  }
  const imageBuilds=lines('image-builds.jsonl');
  assert.deepEqual(imageBuilds.map(b=>b.pair+'/'+b.label+'/'+b.kind).sort(),['normal/main/base','normal/release/base']);
  assert(imageBuilds.every(b=>b.code===0&&b.signal===null));
  assert.deepEqual(receipt.images.map(i=>i.key).sort(),['normal/main/base','normal/release/base']);
  for(const i of receipt.images){assert(i.before.length>0&&i.before.some(f=>f.size>0));assert.deepEqual(i.before,i.after);}
  const records=lines('records.jsonl'), hosts=lines('host-samples.jsonl');
  return {plan, builds, method, ratios:validateRecords(plan,records,hosts), hosts, records:records.length,receiptSha256:sha256(bytes('receipt.json'))};
}
export function summarize(root, baselineRoot=process.cwd()) {
  const sessions=['session-1','session-2'].map(n=>session(path.join(root,n,'bundle'),baselineRoot));
  assert.deepEqual(sessions[0].method,sessions[1].method);
  assert.equal(sessions[0].plan.session,1);assert.equal(sessions[1].plan.session,2);
  assert.deepEqual(sessions[0].plan.cases.map(c=>c.id),ids);
  assert.deepEqual(sessions[1].plan.cases.map(c=>c.id),[ids[1],ids[0],ids[2]]);
  assert(sessions[0].hosts.at(-1).at < sessions[1].hosts[0].at,'sessions overlap');
  for(const key of ['cpus','selectedCPUs','sampleCPUs','host','versions','methodSha256','hostSelection'])assert.deepEqual(sessions[0].plan[key],sessions[1].plan[key],key);
  const cases=ids.map((id,index)=>{
    const strata=sessions.map(s=>s.ratios.get(id)), seed=(sessions[0].method.bootstrapSeed+index)>>>0;
    const individual=strata.map((s,i)=>({median:median(s),...intervals([s],seed+i+10)}));
    const pooled=intervals(strata,seed), estimate=median(strata.flat());
    const adjusted=index<2?(sessions[0].method.primaryFamilySize===4?pooled.ci9875:pooled.ci975):pooled.ci95;
    return {id,role:index<2?'primary':'control',median:estimate,...pooled,adjusted,sessionIntervals:individual,verdict:classify(adjusted,individual.map(x=>x.ci95)),excludesOnePercentSlowdown:adjusted[1]<1.01,excludesFivePercentSlowdown:adjusted[1]<1.05,ratios:strata};
  });
  const hosts=sessions.flatMap(s=>s.hosts);
  return {schema:'redline-tighter-summary-v1',method:sessions[0].method,versions:sessions[0].builds.map(b=>({commit:b.engine_commit,binarySha256:b.binary_sha256,harnessTree:b.harness_tree})),sessionReceipts:sessions.map(s=>s.receiptSha256),cases,counts:{passed:sessions.reduce((a,s)=>a+s.records,0),failed:0,skipped:0,warmup:24,measured:sessions.reduce((a,s)=>a+s.plan.blocks*s.plan.cases.length*4,0)},host:{name:sessions[0].plan.host.name,cpus:sessions[0].plan.cpus,exclusive:false,load1:{min:Math.min(...hosts.map(h=>h.load[0])),median:median(hosts.map(h=>h.load[0])),max:Math.max(...hosts.map(h=>h.load[0]))}}};
}
export function formatRatio(x) {
  const rounded=x.toFixed(6);
  return Number(rounded)===1&&x!==1?String(x):rounded;
}
export function report(s) {
  const fmt=formatRatio;
  const rows=s.cases.map(c=>'| '+[c.id,c.role,fmt(c.median),c.ci95.map(fmt).join('–'),c.adjusted.map(fmt).join('–'),c.verdict,c.excludesFivePercentSlowdown?'yes':'no'].join(' | ')+' |');
  const individual=s.cases.flatMap(c=>c.sessionIntervals.map((v,i)=>'| '+[c.id,i+1,fmt(v.median),v.ci95.map(fmt).join('–')].join(' | ')+' |'));
  const confidence=100*s.method.familywiseIntervalsConfidence;
  return `# Tighter Normal Top 10 and grouped-join comparison

Generated from immutable complete-session receipts. Main ${s.method.main} versus exact v5.1.1 ${s.method.release}. Both package-only release builds use harness ${s.method.harnessTree}, identical toolchain/profile/flags and original full-work ${s.method.rows.toLocaleString('en-US')}-row case operations, digests and settings.

Two predeclared sessions each run ${s.method.blocksPerSession} complete ABBA blocks per case plus one retained warmup. Cases are interleaved inside each block; the second session reverses primary case order. Ratios are main/release query elapsed time per equal operation; lower is better. The median is over ${2*s.method.blocksPerSession} ABBA ratios. A ${s.method.bootstrapDraws}-draw seeded circular moving-block bootstrap preserves eight adjacent complete blocks and equal session strata. Per-case 95% intervals and ${confidence}% primary decision intervals are shown; the latter use a Bonferroni family size of ${s.method.primaryFamilySize}. The control uses its 95% interval. A reproduced primary direction requires its decision interval and both session 95% intervals wholly on the same side of 1.0. Crossing remains inconclusive, not equivalence. Bootstrap coverage is an estimate under recorded conditions; it does not eliminate shared-host confounding.${s.method.primaryFamilySize===4?' This single-core follow-up was chosen after the first study stayed inconclusive; its stricter interval accounts for four primary comparisons across the two studies. This remains an exploratory diagnostic, with no further adaptive extension of its declared sample size.':''}

| Case | Role | Main / release | 95% interval | Decision interval | Finding | Upper bound below 1.05? |
|---|---|---:|---:|---:|---|---|
${rows.join('\n')}

| Case | Session | Main / release | Session 95% interval |
|---|---:|---:|---:|
${individual.join('\n')}

Records: ${s.counts.passed} passed / ${s.counts.failed} failed / ${s.counts.skipped} skipped, including ${s.counts.warmup} retained warmups. Host ${s.host.name}, pinned ${s.host.cpus}, nice19 and idle I/O; cores are not exclusive or kernel-isolated. Load1 min/median/max ${[s.host.load1.min,s.host.load1.median,s.host.load1.max].map(x=>x.toFixed(2)).join('/')}. Selected-core/SMT counters, actual child scheduling, CI worker presence and per-execution loads remain in raw telemetry. No samples were dropped for load or timing.

Session receipt SHA-256: ${s.sessionReceipts.join(', ')}. These query-timer diagnostics do not replace published absolute release throughput, attribute a unique cause, or qualify a future release.
`;
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){
  const [root,mode]=process.argv.slice(2);assert(root&&(!mode||mode==='--check'));
  const summary=summarize(root);
  for(const [name,text] of Object.entries({'summary.json':JSON.stringify(summary,null,2)+'\n','README.md':report(summary)})){
    const f=path.join(root,name);if(mode)assert.equal(fs.readFileSync(f,'utf8'),text,name+' drift');else fs.writeFileSync(f,text);
  }
  console.log(JSON.stringify({cases:summary.cases.map(({id,median,ci95,ci975,verdict})=>({id,median,ci95,ci975,verdict})),counts:summary.counts,host:summary.host}));
}
