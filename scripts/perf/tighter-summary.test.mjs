import test from 'node:test';
import assert from 'node:assert/strict';
import { intervals, classify, validateRecords, ids, formatRatio, verifyAffinity } from './tighter-summary.mjs';
import { sha256 } from './abba-summary.mjs';
import { verifyFocusedImages } from './focused-summary.mjs';
const fixture=(blocks=64)=>{
  const settings={durability:'Normal',cache_bytes:67108864,buffer_pool_pages:4096,query_pool_threads:0};
  const plan={blocks,warmupBlocks:1,rows:20000,workDivisor:1,nice:19,ioClass:'idle',order:'ABBA',cpus:'40-41',selectedCPUs:[40,41],sampleCPUs:[40,41,104,105],cases:ids.map(id=>({id,ops:20000,digest:'1516718079838037000',unit:'row',settings}))};
  const records=[],hosts=[];let seq=0;
  for(let block=-1;block<blocks;block++)for(const c of plan.cases)for(const [slot,label] of ['release','main','main','release'].entries()){
    const elapsed=Math.round(1000000*(block<0&&label==='main'?10:label==='main'?1.02:1)*Math.exp(.003*slot));
    const raw=JSON.stringify({engine:'redlinedb',workload:c.id,pair:'normal',rows:20000,work_divisor:1,ops:c.ops,digest:c.digest,unit:c.unit,settings,elapsed_ns:elapsed});
    const r={seq,case:c.id,block,slot,label,pair:'normal',exit:0,signal:null,status:'ok',raw,rawSha256:sha256(raw),scheduling:{cpus:plan.cpus,nice:19,ioPriority:'idle'},startedAt:new Date(seq*10+2).toISOString(),finishedAt:new Date(seq*10+8).toISOString()};
    records.push(r);
    for(const [stage,delta] of [['before',1],['after',9]])hosts.push({seq,stage,case:r.case,block,slot,label,at:new Date(seq*10+delta).toISOString(),load:[2,2,2],cpus:plan.cpus,cpuCounters:plan.sampleCPUs.map(c=>'cpu'+c+' 1 2 3 4'),runnerWorkers:[]});
    seq++;
  }
  return {plan,records,hosts};
};
test('full-work ABBA cancels linear log drift and excludes only declared warmup',()=>{
  const f=fixture(),r=validateRecords(f.plan,f.records,f.hosts);
  for(const id of ids){assert.equal(r.get(id).length,64);assert(Math.abs(r.get(id)[0]-1.02)<.000002);}
});
for(const [name,mutate] of [
 ['missing execution',f=>f.records.pop()],
 ['incorrect ABBA label',f=>f.records[1].label='release'],
 ['signal-marked execution',f=>f.records[6].signal='SIGTERM'],
 ['wrong child affinity',f=>f.records[5].scheduling.cpus='1-2'],
 ['missing before/after host record',f=>f.hosts.pop()],
 ['duplicate host bracket',f=>f.hosts.push(f.hosts[0])],
 ['raw hash tampering',f=>f.records[4].raw+=' '],
 ['reduced work',f=>{const r=f.records[4],m=JSON.parse(r.raw);m.work_divisor=2;r.raw=JSON.stringify(m);r.rawSha256=sha256(r.raw);}],
 ['incorrect digest',f=>{const r=f.records[4],m=JSON.parse(r.raw);m.digest='1516718079838036999';r.raw=JSON.stringify(m);r.rawSha256=sha256(r.raw);}]
])test('rejects '+name,()=>{const f=fixture();mutate(f);assert.throws(()=>validateRecords(f.plan,f.records,f.hosts));});
test('cluster bootstrap is deterministic and retains wider adjacent-block uncertainty',()=>{
  const s=Array.from({length:64},(_,i)=>.8+i*.006);
  const clustered=intervals([s],1234,2000,8), independent=intervals([s],1234,2000,1);
  assert.deepEqual(clustered,intervals([s],1234,2000,8));
  assert(clustered.ci95[1]-clustered.ci95[0]>independent.ci95[1]-independent.ci95[0]);
  assert(clustered.ci975[0]<=clustered.ci95[0]&&clustered.ci975[1]>=clustered.ci95[1]);
  assert(clustered.ci9875[0]<=clustered.ci975[0]&&clustered.ci9875[1]>=clustered.ci975[1]);
});
test('follow-up requires all128 blocks and still rejects missing work',()=>{
  const f=fixture(128),r=validateRecords(f.plan,f.records,f.hosts);
  assert.equal(r.get(ids[0]).length,128);f.records.pop();assert.throws(()=>validateRecords(f.plan,f.records,f.hosts));
});
test('undeclared repeat counts are rejected',()=>{
  const f=fixture();f.plan.blocks=63;assert.throws(()=>validateRecords(f.plan,f.records,f.hosts));
});
test('formatting never moves a marginal bound across one',()=>{
  for(const x of [.99999999,1.00000001])assert.equal(Number(formatRatio(x)),x);
  assert.equal(formatRatio(1),'1.000000');
});
test('constant samples retain exact null and never become a detected direction',()=>{
  const x=intervals([Array(64).fill(1),Array(64).fill(1)],123,1000);
  assert.deepEqual(x,{ci95:[1,1],ci975:[1,1],ci9875:[1,1]});assert.equal(classify(x.ci975,[[1,1],[1,1]]),'inconclusive');
});
test('pooled result cannot override contradictory sessions or crossing adjusted bound',()=>{
  assert.equal(classify([1.01,1.03],[[1.01,1.04],[.99,1.03]]),'inconclusive');
  assert.equal(classify([.99,1.02],[[1.01,1.04],[1.01,1.05]]),'inconclusive');
  assert.equal(classify([1.01,1.03],[[1.01,1.04],[1.01,1.03]]),'reproduced-slowdown');
  assert.equal(classify([.97,.99],[[.96,.99],[.97,.99]]),'reproduced-speedup');
});
test('CPU ranges must match the declared one or two selected CPUs exactly',()=>{
  for(const p of [{cpus:'23',selectedCPUs:[23]},{cpus:'32,46',selectedCPUs:[32,46]},
    {cpus:'40-41',selectedCPUs:[41,40]}])verifyAffinity(p);
  for(const p of [{cpus:'23-25',selectedCPUs:[23]},{cpus:'23,23',selectedCPUs:[23]},
    {cpus:'23',selectedCPUs:[23,23]},{cpus:'25-23',selectedCPUs:[23]},
    {cpus:'23,',selectedCPUs:[23]},{cpus:'23-25',selectedCPUs:[23,24,25]}])assert.throws(()=>verifyAffinity(p));
});
test('two-image receipts reject malformed, missing, duplicate and changed input inventories',()=>{
  const fixture=()=>{
    const keys=['normal/main/base','normal/release/base'];
    return {images:keys.map(key=>({key,before:[{path:'heap/data',sha256:'a'.repeat(64),size:4096}],
      after:[{path:'heap/data',sha256:'a'.repeat(64),size:4096}]})),
      builds:keys.map(key=>{const [pair,label,kind]=key.split('/');return {pair,label,kind,code:0,signal:null};})};
  };
  const valid=fixture();verifyFocusedImages(valid.images,valid.builds);
  for(const mutate of [f=>f.images.pop(),f=>f.builds.pop(),f=>f.images[0].before=[],
    f=>{delete f.images[0].before[0].sha256;},f=>f.images[0].before[0].sha256='bad',
    f=>f.images[0].before[0].size=-1,f=>f.images[0].before[0].size=1.5,
    f=>f.images[0].before[0].path='../data',f=>f.images[0].before[0].path='/data',
    f=>f.images[0].before.push(f.images[0].before[0]),f=>f.images[0].after[0].sha256='b'.repeat(64)]) {
    const f=fixture();mutate(f);assert.throws(()=>verifyFocusedImages(f.images,f.builds));
  }
});
function gatedFixture() {
  const f=fixture();f.plan.schedulingBarrier=true;
  for(const r of f.records)Object.assign(r.scheduling,{probeStage:'before-exec',exeBefore:'/usr/bin/bash',
    checkStartedAt:new Date(r.seq*10+3).toISOString(),checkFinishedAt:new Date(r.seq*10+4).toISOString(),
    gateReleasedAt:new Date(r.seq*10+5).toISOString()});
  return f;
}
test('gated receipts retain full ABBA work and prove checks precede release',()=>{
  const f=gatedFixture();assert.equal(validateRecords(f.plan,f.records,f.hosts).get(ids[0]).length,64);
});
test('a gate released before checks finish is rejected',()=>{
  const f=gatedFixture();f.records[0].scheduling.gateReleasedAt=f.records[0].startedAt;
  assert.throws(()=>validateRecords(f.plan,f.records,f.hosts));
});
test('gated checks must observe a waiting shell, not an already-running benchmark',()=>{
  const f=gatedFixture();f.records[0].scheduling.exeBefore='/bin/redline-scoreboard';
  assert.throws(()=>validateRecords(f.plan,f.records,f.hosts));
});
test('undeclared scheduling-barrier values are rejected',()=>{
  for(const value of [false,'yes']){const f=fixture();f.plan.schedulingBarrier=value;assert.throws(()=>validateRecords(f.plan,f.records,f.hosts));}
});
