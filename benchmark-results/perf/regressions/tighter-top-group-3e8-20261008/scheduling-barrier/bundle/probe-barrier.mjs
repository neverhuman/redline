// Separate diagnostic: scheduling checks before exec, never changed engine work.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn, execFileSync } from 'node:child_process';
import { sha256, parseRecord } from './abba-summary.mjs';

const [planPath]=process.argv.slice(2);
const plan=JSON.parse(fs.readFileSync(planPath));
assert.equal(plan.schema,'redline-probe-barrier-plan-v1');
assert.equal(plan.host,'xbabe3');assert.equal(os.hostname(),plan.host);
assert.equal(plan.blocks,32);assert.equal(plan.rows,20000);assert.equal(plan.childCPU,'23');
assert.deepEqual(plan.conditions,[{id:'direct',parentCPU:'23'},{id:'gated',parentCPU:'23'}]);
assert.equal(Number(execFileSync('ps',['-o','ni=','-p',String(process.pid)],{encoding:'utf8'}).trim()),19);
assert.match(execFileSync('ionice',['-p',String(process.pid)],{encoding:'utf8'}),/idle/);
assert(!fs.existsSync(plan.output));fs.mkdirSync(plan.output,{recursive:true});
fs.copyFileSync(planPath,path.join(plan.output,'plan.json'));
fs.copyFileSync(new URL(import.meta.url),path.join(plan.output,'probe-barrier.mjs'));
fs.copyFileSync(new URL('./abba-summary.mjs',import.meta.url),path.join(plan.output,'abba-summary.mjs'));
const inventory=dir=>{
  const entries=[];
  const walk=(base,relative='')=>{
    for(const name of fs.readdirSync(base).sort()) {
      const file=path.join(base,name),rel=path.posix.join(relative,name),st=fs.lstatSync(file);
      assert(!st.isSymbolicLink());
      if(st.isDirectory())walk(file,rel);else {assert(st.isFile());entries.push({path:rel,size:st.size,sha256:sha256(fs.readFileSync(file))});}
    }
  };walk(dir);return entries;
};
const images={};
for(const label of ['release','main']) {
  const v=plan.versions[label];assert.equal(sha256(fs.readFileSync(v.binary)),v.binarySha256);
  fs.copyFileSync(v.build,path.join(plan.output,label+'-build.json'));
  const source=inventory(v.image);assert.deepEqual(source,v.imageInventory,'seed image custody differs');
  const image=path.join(plan.workRoot,'images',label);assert(!fs.existsSync(image));
  fs.mkdirSync(path.dirname(image),{recursive:true});fs.cpSync(v.image,image,{recursive:true,errorOnExist:true,force:false});
  assert.deepEqual(inventory(image),source);images[label]={image,before:source};
}
let condition,seq=0,active;
const receipt={schema:'redline-probe-interference-receipt-v1',exit:1,startedAt:new Date().toISOString(),files:[],planSha256:sha256(fs.readFileSync(planPath))};
const cpuMask=pid=>fs.readFileSync(`/proc/${pid}/status`,'utf8').match(/^Cpus_allowed_list:\s*(.+)$/m)[1];
const hostSample=(context={})=>{
  let runnerWorkers=[];
  try {runnerWorkers=execFileSync('pgrep',['-x','Runner.Worker'],{encoding:'utf8'}).trim().split('\n').filter(Boolean).map(Number);}
  catch(e){assert.equal(e.status,1);}
  fs.appendFileSync(path.join(plan.output,'host-samples.jsonl'),JSON.stringify({...context,condition:condition?.id,parentCPU:cpuMask(process.pid),at:new Date().toISOString(),load:os.loadavg(),runnerWorkers,
    cpuCounters:fs.readFileSync('/proc/stat','utf8').split('\n').filter(l=>[23,87].includes(Number(l.split(' ')[0].slice(3))))})+'\n');
};
const sampler=setInterval(hostSample,10000);
const invoke=(v,args)=>new Promise(async(resolve,reject)=>{
  // A waiting shell preserves PID/affinity/nice/I/O across exec.
  const gated=condition.id==='gated';
  const child=gated
    ?spawn('bash',['-c','set -e; read -r start; test \"$start\" = go; exec \"$@\"','checked-worker',v.binary,...args],{stdio:['pipe','pipe','pipe']})
    :spawn(v.binary,args,{stdio:['ignore','pipe','pipe']});active=child;
  let out='',err='',scheduling,probeError;
  child.stdout.on('data',x=>out+=x);child.stderr.on('data',x=>err+=x);
  const timer=setTimeout(()=>child.kill('SIGKILL'),600000);
  const done=new Promise((yes,no)=>{child.on('error',no);child.on('close',(code,signal)=>yes({code,signal}));});
  try {
    const checkStartedAt=new Date().toISOString(),schedstatBefore=fs.readFileSync(`/proc/${child.pid}/schedstat`,'utf8').trim();
    scheduling={probeStage:gated?'before-exec':'running',pid:child.pid,cpus:cpuMask(child.pid),parentCPU:cpuMask(process.pid),checkStartedAt,
      nice:Number(execFileSync('ps',['-o','ni=','-p',String(child.pid)],{encoding:'utf8'}).trim()),
      ioPriority:execFileSync('ionice',['-p',String(child.pid)],{encoding:'utf8'}).trim(),
      checkFinishedAt:new Date().toISOString(),schedstatBefore,schedstatAfter:fs.readFileSync(`/proc/${child.pid}/schedstat`,'utf8').trim()};
    assert.equal(scheduling.cpus,'23');assert.equal(scheduling.parentCPU,condition.parentCPU);assert.equal(scheduling.nice,19);assert.match(scheduling.ioPriority,/idle/);
    scheduling.exeBefore=fs.readlinkSync(`/proc/${child.pid}/exe`);
    if(gated)assert.equal(path.basename(scheduling.exeBefore),'bash');
    if(gated){scheduling.gateReleasedAt=new Date().toISOString();child.stdin.end('go\n');}
  } catch(e){probeError=e;child.kill('SIGKILL');}
  try {const outcome=await done;resolve({...outcome,out,err,scheduling,probeError:probeError?.stack});}
  catch(e){reject(e);}finally{clearTimeout(timer);active=undefined;}
});
for(const signal of ['SIGINT','SIGTERM'])process.on(signal,()=>{receipt.interrupted=signal;active?.kill('SIGTERM');});
try {
  for(let block=-1;block<plan.blocks;block++) for(const c of plan.cases) {
    const conditions=block%2===0?plan.conditions:[...plan.conditions].reverse();
    for(condition of conditions) {
      // Both conditions keep our controller and engine on the same CPU.
      execFileSync('taskset',['-apc',condition.parentCPU,String(process.pid)]);
      assert.equal(cpuMask(process.pid),condition.parentCPU);
      for(const [slot,label] of ['release','main','main','release'].entries()) {
        assert(!receipt.interrupted);const work=path.join(plan.workRoot,'cases',String(seq));assert(!fs.existsSync(work));
        hostSample({stage:'before',seq,case:c.id,block,slot,label});
        const startedAt=new Date().toISOString();
        const outcome=await invoke(plan.versions[label],['case','--engine','redline','--workload',c.id,'--rows','20000','--pair','normal','--work-divisor','1','--image',images[label].image,'--work',work]);
        const record={seq:seq++,case:c.id,condition:condition.id,block,slot,label,startedAt,finishedAt:new Date().toISOString(),exit:outcome.code,signal:outcome.signal,raw:outcome.out,rawSha256:sha256(outcome.out),stderr:outcome.err,scheduling:outcome.scheduling,probeError:outcome.probeError};
        fs.appendFileSync(path.join(plan.output,'records.jsonl'),JSON.stringify(record)+'\n');
        hostSample({stage:'after',seq:seq-1,case:c.id,block,slot,label});
        assert.equal(record.exit,0);assert.equal(record.signal,null);assert(!record.probeError);
        const measured=parseRecord(record.raw);
        assert.equal(measured.digest,c.digest);assert.equal(measured.ops,c.ops);assert.equal(measured.rows,20000);assert.equal(measured.work_divisor,1);assert.deepEqual(measured.settings,c.settings);
        fs.rmSync(work,{recursive:true});
      }
    }
    if(c===plan.cases.at(-1)&&block>=0&&(block+1)%8===0)console.log(new Date().toISOString(),block+1,'/32 blocks',seq,'records');
  }
  receipt.images=[];
  for(const [label,{image,before}] of Object.entries(images)) {
    const after=inventory(image);assert.deepEqual(after,before);receipt.images.push({key:'normal/'+label+'/base',before,after});
    assert.equal(sha256(fs.readFileSync(plan.versions[label].binary)),plan.versions[label].binarySha256);
  }
  assert(!receipt.interrupted);receipt.exit=0;
}catch(e){receipt.error=e.stack;console.error(e.stack);process.exitCode=1;}
finally {
  clearInterval(sampler);hostSample();receipt.finishedAt=new Date().toISOString();
  for(const name of fs.readdirSync(plan.output).sort())receipt.files.push({path:name,sha256:sha256(fs.readFileSync(path.join(plan.output,name)))});
  fs.writeFileSync(path.join(plan.output,'receipt.json'),JSON.stringify(receipt,null,2)+'\n');
}
