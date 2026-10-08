import test from 'node:test';
import assert from 'node:assert/strict';
import { validateProbe } from './probe-barrier-summary.mjs';
import { sha256 } from './abba-summary.mjs';
function fixture() {
  const settings={durability:'Normal',cache_bytes:67108864,buffer_pool_pages:4096,query_pool_threads:0};
  const plan={schema:'redline-probe-barrier-plan-v1',blocks:32,warmupBlocks:1,rows:20000,childCPU:'23',
    maximumAssignedLogicalCPUs:1,exclusive:false,conditions:[{id:'direct',parentCPU:'23'},{id:'gated',parentCPU:'23'}],
    cases:['top10','join_group_by_city','point_pk_sql_text'].map(id=>({id,ops:20000,unit:'row',digest:'1516718079838037000',settings}))};
  const records=[],hosts=[];let seq=0;
  for(let block=-1;block<32;block++)for(const c of plan.cases)for(const condition of block%2===0?plan.conditions:[...plan.conditions].reverse()) {
    for(const [slot,label] of ['release','main','main','release'].entries()) {
      const raw=JSON.stringify({engine:'redlinedb',workload:c.id,pair:'normal',rows:20000,work_divisor:1,ops:20000,unit:'row',digest:c.digest,settings,elapsed_ns:1000000,cpu_user_ns:900000,cpu_sys_ns:10000});
      const s={cpus:'23',parentCPU:'23',nice:19,ioPriority:'idle',probeStage:condition.id==='gated'?'before-exec':'running',
        checkStartedAt:new Date(seq*10+3).toISOString(),checkFinishedAt:new Date(seq*10+4).toISOString(),schedstatBefore:'100 0 1',schedstatAfter:'200 0 2'};
      if(condition.id==='gated')Object.assign(s,{exeBefore:'/usr/bin/bash',gateReleasedAt:new Date(seq*10+5).toISOString()});
      const r={seq,case:c.id,condition:condition.id,block,slot,label,exit:0,signal:null,raw,rawSha256:sha256(raw),scheduling:s,
        startedAt:new Date(seq*10+2).toISOString(),finishedAt:new Date(seq*10+8).toISOString()};records.push(r);
      for(const [stage,offset] of [['before',1],['after',9]])hosts.push({seq,case:c.id,condition:condition.id,block,slot,label,stage,parentCPU:'23',
        at:new Date(seq*10+offset).toISOString(),load:[1,1,1],cpuCounters:['cpu23 1 2 3 4','cpu87 1 2 3 4']});
      seq++;
    }
  }
  return {plan,records,hosts};
}
test('complete direct/gated crossover retains both conditions and warmups',()=>{
  const f=fixture(),v=validateProbe(f.plan,f.records,f.hosts);
  for(const groups of v.values()){assert.equal(groups.direct.length,32);assert.equal(groups.gated.length,32);}
});
for(const [name,mutate] of [
  ['missing execution',f=>f.records.pop()],
  ['incorrect condition order',f=>f.records[0].condition='direct'],
  ['changed child CPU',f=>f.records[0].scheduling.cpus='24'],
  ['changed parent CPU',f=>f.records[0].scheduling.parentCPU='24'],
  ['signal-marked child',f=>f.records[0].signal='SIGTERM'],
  ['raw result tampering',f=>f.records[0].raw+=' '],
  ['missing host bracket',f=>f.hosts.pop()],
  ['unreleased gate',f=>{delete f.records[0].scheduling.gateReleasedAt;}],
  ['gate before checks',f=>f.records[0].scheduling.gateReleasedAt=f.records[0].startedAt],
  ['metadata race',f=>f.records[0].probeError='ENOENT /proc child status'],
])test('rejects '+name,()=>{const f=fixture();mutate(f);assert.throws(()=>validateProbe(f.plan,f.records,f.hosts));});
