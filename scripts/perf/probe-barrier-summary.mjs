// Completed diagnostic only: pre-exec scheduling barrier with unchanged engine work.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { median, parseRecord, sha256, verifyCompletion } from './abba-summary.mjs';
import { intervals, formatRatio } from './tighter-summary.mjs';
import { verifyFocusedImages } from './focused-summary.mjs';

export function validateProbe(plan,records,hosts) {
  assert.equal(plan.schema,'redline-probe-barrier-plan-v1');assert.equal(plan.blocks,32);
  assert.equal(plan.warmupBlocks,1);assert.equal(plan.rows,20000);assert.equal(plan.childCPU,'23');
  assert.equal(plan.maximumAssignedLogicalCPUs,1);assert.equal(plan.exclusive,false);
  assert.deepEqual(plan.conditions,[{id:'direct',parentCPU:'23'},{id:'gated',parentCPU:'23'}]);
  assert.deepEqual(plan.cases.map(c=>c.id),['top10','join_group_by_city','point_pk_sql_text']);
  assert.equal(records.length,(plan.blocks+1)*plan.cases.length*plan.conditions.length*4);
  const brackets=new Map();
  for(const h of hosts) {
    assert(h.load.length===3&&h.load.every(x=>Number.isFinite(x)&&x>=0));
    assert.deepEqual(h.cpuCounters.map(c=>Number(c.split(' ')[0].slice(3))),[23,87]);
    if(h.stage){const key=h.seq+'/'+h.stage;assert(!brackets.has(key));brackets.set(key,h);}
  }
  const values=new Map(plan.cases.map(c=>[c.id,{direct:[],gated:[]} ]));let seq=0;
  for(let block=-1;block<plan.blocks;block++)for(const c of plan.cases) {
    const conditions=block%2===0?plan.conditions:[...plan.conditions].reverse();
    for(const condition of conditions) {
      const group=[];
      for(const [slot,label] of ['release','main','main','release'].entries()) {
        const r=records[seq++];
        for(const [key,expected] of Object.entries({seq:seq-1,case:c.id,block,slot,label,condition:condition.id,exit:0,signal:null}))assert.equal(r[key],expected,key);
        assert(!r.probeError);assert.equal(r.rawSha256,sha256(r.raw));
        const s=r.scheduling;assert.equal(s.cpus,'23');assert.equal(s.parentCPU,condition.parentCPU);
        assert.equal(s.nice,19);assert.match(s.ioPriority,/idle/);
        assert.equal(s.probeStage,condition.id==='gated'?'before-exec':'running');
        if(condition.id==='gated'){assert.equal(path.basename(s.exeBefore),'bash');assert(s.checkFinishedAt<=s.gateReleasedAt&&s.gateReleasedAt<=r.finishedAt);}
        else assert.equal(s.gateReleasedAt,undefined);
        assert(r.startedAt<=s.checkStartedAt&&s.checkStartedAt<=s.checkFinishedAt&&s.checkFinishedAt<=r.finishedAt);
        for(const sample of [s.schedstatBefore,s.schedstatAfter])assert(/^\d+ \d+ \d+$/.test(sample));
        if(r.seq)assert(records[r.seq-1].finishedAt<=r.startedAt);
        for(const stage of ['before','after']) {
          const h=brackets.get(r.seq+'/'+stage);assert(h);
          for(const key of ['case','block','slot','label','condition'])assert.equal(h[key],r[key]);
          assert.equal(h.parentCPU,condition.parentCPU);
          assert(stage==='before'?h.at<=r.startedAt:h.at>=r.finishedAt);
        }
        const m=parseRecord(r.raw);
        for(const [key,expected] of Object.entries({engine:'redlinedb',workload:c.id,pair:'normal',rows:20000,work_divisor:1,ops:c.ops,digest:c.digest,unit:c.unit}))assert.equal(m[key],expected,key);
        assert.deepEqual(m.settings,c.settings);
        for(const key of ['elapsed_ns','cpu_user_ns','cpu_sys_ns'])assert(Number.isSafeInteger(m[key])&&m[key]>=0);
        assert(m.elapsed_ns>0&&m.cpu_user_ns+m.cpu_sys_ns>0);group.push(m);
      }
      if(block>=0)values.get(c.id)[condition.id].push(group);
    }
  }
  assert.equal(brackets.size,records.length*2);return values;
}
export function summarizeProbe(root,baselineRoot=process.cwd()) {
  const bytes=n=>fs.readFileSync(path.join(root,n)),json=n=>JSON.parse(bytes(n));
  const lines=n=>bytes(n).toString().trim().split('\n').map(JSON.parse);
  const plan=json('plan.json'),receipt=json('receipt.json');verifyCompletion(receipt);
  assert.equal(receipt.planSha256,sha256(bytes('plan.json')));assert.equal(plan.runnerSha256,sha256(bytes('probe-barrier.mjs')));
  assert.deepEqual(receipt.files.map(f=>f.path).sort(),['abba-summary.mjs','host-samples.jsonl','main-build.json','plan.json','probe-barrier.mjs','records.jsonl','release-build.json'].sort());
  for(const f of receipt.files)assert.equal(sha256(bytes(f.path)),f.sha256,f.path);
  const builds=['release','main'].map(label=>json(label+'-build.json'));
  assert.deepEqual(builds.map(b=>b.engine_commit),['9277455d5ad008252053a81d18add39b8cdc8f7b','3e8b2d5b6f13a4440c10ad5c56bff73d3d967538']);
  for(const [i,label] of ['release','main'].entries()) {
    assert.equal(builds[i].binary_sha256,plan.versions[label].binarySha256);
    assert.equal(builds[i].harness_tree,'bb1434a80ca0390401eb628d2785e8387c5f346e');
    assert.equal(builds[i].profile,'release');assert.equal(builds[i].rustflags,'');
    assert.equal(builds[i].source_dirty,false);assert.equal(builds[i].productionArtifactsCleaned,true);assert.equal(builds[i].failpoints,false);assert.equal(builds[i].pgo,false);
  }
  for(const key of ['engines','rustc_verbose_version','cargo_version','profile_release'])assert.deepEqual(builds[0][key],builds[1][key]);
  const original=path.join(baselineRoot,'benchmark-results/perf/regressions/tighter-top-group-3e8-20261008/one-core/session-1/bundle');
  const originalReceiptBytes=fs.readFileSync(path.join(original,'receipt.json'));
  assert.equal(sha256(originalReceiptBytes),plan.originalConfirmationReceiptSha256);
  const originalReceipt=JSON.parse(originalReceiptBytes);verifyCompletion(originalReceipt);
  for(const f of originalReceipt.files)assert.equal(sha256(fs.readFileSync(path.join(original,f.path))),f.sha256,f.path+' seed custody');
  const originalPlan=JSON.parse(fs.readFileSync(path.join(original,'plan.json')));
  assert.deepEqual(plan.cases,originalPlan.cases);
  const imageBuilds=fs.readFileSync(path.join(original,'image-builds.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
  verifyFocusedImages(originalReceipt.images,imageBuilds);verifyFocusedImages(receipt.images,imageBuilds);
  for(const i of receipt.images) {
    assert.deepEqual(i.before,plan.versions[i.key.split('/')[1]].imageInventory);
    assert.deepEqual(i.before,originalReceipt.images.find(x=>x.key===i.key).before);
  }
  const hosts=lines('host-samples.jsonl'),records=lines('records.jsonl'),groups=validateProbe(plan,records,hosts);
  const ratio=(g,key)=>Math.sqrt(g[1][key]*g[2][key]/(g[0][key]*g[3][key]));
  const cases=[...groups].map(([id,conditions],index)=>{
    const summaries=Object.fromEntries(Object.entries(conditions).map(([condition,groups],j)=>{
      const wall=groups.map(g=>ratio(g,'elapsed_ns'));
      const cpu=groups.map(g=>ratio(g.map(m=>({...m,totalCPU:m.cpu_user_ns+m.cpu_sys_ns})),'totalCPU'));
      const gap=label=>median(groups.flatMap(g=>g.filter((_,i)=>['release','main','main','release'][i]===label).map(m=>(m.elapsed_ns-m.cpu_user_ns-m.cpu_sys_ns)/1e6)));
      return [condition,{wall:{median:median(wall),ci95:intervals([wall],plan.bootstrapSeed+index*10+j).ci95,ratios:wall},
        cpu:{median:median(cpu),ci95:intervals([cpu],plan.bootstrapSeed+index*10+j+100).ci95},wallMinusCPUms:{release:gap('release'),main:gap('main')}}];
    }));
    const effect=summaries.gated.wall.ratios.map((r,i)=>r/summaries.direct.wall.ratios[i]);
    return {id,...summaries,schedulingBarrierEffect:{median:median(effect),ci95:intervals([effect],plan.bootstrapSeed+index+200).ci95}};
  });
  return {schema:'redline-probe-barrier-summary-v1',cases,counts:{passed:records.length,failed:0,skipped:0,warmup:plan.cases.length*plan.conditions.length*4,measured:plan.blocks*plan.cases.length*plan.conditions.length*4},receiptSha256:sha256(bytes('receipt.json')),
    host:{name:plan.host,childCPU:plan.childCPU,controllerCPUs:plan.conditions.map(c=>c.parentCPU),exclusive:false,load1:{min:Math.min(...hosts.map(h=>h.load[0])),median:median(hosts.map(h=>h.load[0])),max:Math.max(...hosts.map(h=>h.load[0]))}}};
}
export function probeReport(s) {
  const f=formatRatio;
  const rows=s.cases.flatMap(c=>['direct','gated'].map(condition=>{
    const v=c[condition];return '| '+[c.id,condition,f(v.wall.median),v.wall.ci95.map(f).join('–'),f(v.cpu.median),v.cpu.ci95.map(f).join('–'),v.wallMinusCPUms.release.toFixed(3),v.wallMinusCPUms.main.toFixed(3)].join(' | ')+' |';
  }));
  return `# Scheduling-barrier interference diagnostic

This separate, predeclared crossover tests controller overlap after the retained one-core study and rejected affinity diagnostic. Engine and controller stay on CPU 23 in both conditions. Direct launch checks the running child; gated launch checks a waiting shell, then permits the exact binary to exec with the same PID, affinity, nice and I/O priority. The gate finishes all synchronous scheduling checks before the benchmark starts. The shell wrapper and barrier are explicit treatment differences, outside the engine's query clock. Immutable version-specific seed images, stable 927/current 3e8 binaries, full 20,000-row work, settings and result digests remain identical. The shared CPU is not exclusive. No peer affinity or scheduling changes.

Each case has 32 complete ABBA blocks per condition plus one retained warmup, with condition order alternating by block. Ratios are main/release; lower is better. Seeded 50,000-draw circular moving-block bootstrap over eight adjacent blocks produces exploratory 95% intervals. This does not qualify a release or establish equivalence. Elapsed-minus-process-CPU medians are timer differences, not direct kernel wait measurements; process CPU includes all child threads. Scheduling-check and gate-release timestamps and child schedstat snapshots remain in raw records. No timing or load sample is discarded.

| Case | Launch | Elapsed ratio |95% interval|Process CPU ratio|95% interval|Release elapsed−CPU ms|Main elapsed−CPU ms|
|---|---|---:|---:|---:|---:|---:|---:|
${rows.join('\n')}

${s.counts.passed} successful executions, 0 failures, 0 skips, including ${s.counts.warmup} warmups. Host ${s.host.name}, engine CPU ${s.host.childCPU}, nice 19 and idle I/O, load1 min/median/max ${[s.host.load1.min,s.host.load1.median,s.host.load1.max].map(x=>x.toFixed(2)).join('/')}. Receipt SHA-256 ${s.receiptSha256}. Both prior studies and their contradictory or inconclusive results remain preserved; this diagnostic does not rewrite their observations or attribute a unique engine source cause.
`;
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href) {
  const [root,mode]=process.argv.slice(2);assert(root&&(!mode||mode==='--check'));const s=summarizeProbe(root);
  for(const [name,text] of Object.entries({'summary.json':JSON.stringify(s,null,2)+'\n','README.md':probeReport(s)})) {
    const file=path.join(root,name);if(mode)assert.equal(fs.readFileSync(file,'utf8'),text,name+' drift');else fs.writeFileSync(file,text);
  }
  console.log(JSON.stringify(s));
}
