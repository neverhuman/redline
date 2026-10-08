import assert from 'node:assert/strict';
import test from 'node:test';
import { verifyManifest } from './tighter-investigation.mjs';
const fixture=()=>({schema:'redline-tighter-investigation-v1',studies:[
  {name:'two-core',archiveSha256:'a'.repeat(64),sessionReceipts:['1'.repeat(64),'2'.repeat(64)]},
  {name:'one-core',archiveSha256:'b'.repeat(64),sessionReceipts:['3'.repeat(64),'4'.repeat(64)]},
],diagnostic:{name:'scheduling-barrier',archiveSha256:'c'.repeat(64),receiptSha256:'5'.repeat(64)},
rejected:{name:'rejected-affinity',archiveSha256:'d'.repeat(64),receiptSha256:'6'.repeat(64),records:175,eligible:false}});
test('both independently identified studies are required',()=>verifyManifest(fixture()));
for(const [name,change] of [
  ['missing initial study',m=>m.studies.shift()],
  ['repeated independent session',m=>m.studies[1].sessionReceipts[1]=m.studies[0].sessionReceipts[0]],
  ['missing session',m=>m.studies[1].sessionReceipts.pop()],
  ['unverified archive hash',m=>m.studies[1].archiveSha256='unknown'],
  ['missing scheduling diagnostic',m=>delete m.diagnostic],
  ['rejected sample labeled eligible',m=>m.rejected.eligible=true],
  ['rejected sample count differs',m=>m.rejected.records++],
  ['unverified diagnostic archive',m=>m.diagnostic.archiveSha256='unknown'],
])test('rejects '+name,()=>{const m=fixture();change(m);assert.throws(()=>verifyManifest(m));});
