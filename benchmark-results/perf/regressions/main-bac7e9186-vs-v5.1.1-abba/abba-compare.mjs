// Run existing scoreboard case/image children, without changing timed work.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn, execFileSync } from 'node:child_process';
import { sha256, parseRecord } from './abba-summary.mjs';

const [planPath] = process.argv.slice(2);
assert(planPath, 'usage: node scripts/perf/abba-compare.mjs PLAN.json');
const plan = JSON.parse(fs.readFileSync(planPath));
assert.equal(plan.schema, 'redline-abba-plan-v1');
assert.equal(plan.host.name, os.hostname());
assert.equal(plan.cpus, fs.readFileSync('/proc/self/status', 'utf8').match(/^Cpus_allowed_list:\s*(.+)$/m)[1]);
assert.equal(Number(execFileSync('ps', ['-o', 'ni=', '-p', String(process.pid)], { encoding: 'utf8' }).trim()), plan.nice);
assert.equal(plan.cpus, '2-5');
assert.equal(plan.nice, 19);
assert.equal(plan.ioClass, 'idle');
assert.match(execFileSync('ionice', ['-p', String(process.pid)], { encoding: 'utf8' }), /idle/);
assert.equal(plan.blocks, 12);
assert.equal(plan.rows, 20000);
assert.equal(plan.order, 'ABBA');
const bundle = plan.output;
assert(!fs.existsSync(bundle), 'immutable bundle already exists');
fs.mkdirSync(bundle, { recursive: true });
fs.copyFileSync(planPath, path.join(bundle, 'plan.json'));
for (const label of ['release', 'main']) {
  const v = plan.versions[label];
  assert.equal(sha256(fs.readFileSync(v.binary)), v.binarySha256);
  fs.copyFileSync(v.build, path.join(bundle, `${label}-build.json`));
}
fs.copyFileSync(new URL(import.meta.url), path.join(bundle, 'abba-compare.mjs'));
fs.copyFileSync(new URL('./abba-summary.mjs', import.meta.url), path.join(bundle, 'abba-summary.mjs'));
const hostFile = path.join(bundle, 'host-samples.jsonl');
const recordFile = path.join(bundle, 'records.jsonl');
const hostSample = () => {
  const status = { at: new Date().toISOString(), load: os.loadavg(),
    freeMemory: os.freemem(), cpus: plan.cpus, runnerWorkers: [] };
  for (const pid of fs.readdirSync('/proc').filter(x => /^\d+$/.test(x))) {
    try {
      const cmd = fs.readFileSync(`/proc/${pid}/cmdline`, 'utf8').replaceAll('\0', ' ');
      if (cmd.includes('/Runner.Worker ')) status.runnerWorkers.push(Number(pid));
    } catch {} // A process may exit between listing and reading procfs.
  }
  fs.appendFileSync(hostFile, `${JSON.stringify(status)}\n`);
};
hostSample();
const sampler = setInterval(hostSample, 10000);
const images = new Map();
let seq = 0;
let active;
const startedAt = new Date().toISOString();
const receipt = { schema: 'redline-abba-receipt-v1', startedAt, exit: 1,
  planSha256: sha256(fs.readFileSync(planPath)), files: [] };
const invoke = (binary, args, budget) => new Promise((resolve, reject) => {
  const child = spawn(binary, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  active = child;
  let out = '', err = '';
  child.stdout.on('data', x => { out += x; });
  child.stderr.on('data', x => { err += x; });
  const timer = setTimeout(() => child.kill('SIGKILL'), budget * 1000);
  child.on('error', reject);
  child.on('close', (code, signal) => {
    clearTimeout(timer); active = undefined;
    resolve({ code, signal, out, err });
  });
});
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => {
  active?.kill('SIGTERM');
  receipt.interrupted = signal;
});
const imageHashes = dir => {
  const entries = [];
  const walk = (base, relative = '') => {
    for (const name of fs.readdirSync(base).sort()) {
      const file = path.join(base, name); const rel = path.posix.join(relative, name);
      if (fs.statSync(file).isDirectory()) walk(file, rel);
      else entries.push({ path: rel, sha256: sha256(fs.readFileSync(file)), size: fs.statSync(file).size });
    }
  };
  walk(dir); return entries;
};
try {
  for (const [pair, root] of Object.entries(plan.workRoots)) {
    assert.equal(execFileSync('findmnt', ['-no', 'FSTYPE', '--target', root], { encoding: 'utf8' }).trim(), plan.filesystems[pair]);
    assert(pair === 'normal' ? plan.filesystems[pair] === 'tmpfs' : !['tmpfs', 'ramfs', 'overlay'].includes(plan.filesystems[pair]));
    const kinds = [...new Set(plan.cases.filter(c => c.pair === pair).map(c => c.image))];
    for (const label of ['release', 'main']) for (const kind of kinds) {
      assert(!receipt.interrupted, 'interrupted');
      const image = path.join(root, 'images', label, kind);
      assert(!fs.existsSync(image), 'stale image');
      const args = ['image', '--engine', 'redline', '--kind', kind, '--rows', String(plan.rows), '--out', image];
      if (kind === 'after-updates') args.push('--base', path.join(root, 'images', label, 'base'));
      const outcome = await invoke(plan.versions[label].binary, args, 3600);
      fs.appendFileSync(path.join(bundle, 'image-builds.jsonl'), `${JSON.stringify({ pair, label, kind, ...outcome })}\n`);
      assert.equal(outcome.code, 0, `image failed ${outcome.err}`);
      const before = imageHashes(image);
      images.set(`${pair}/${label}/${kind}`, { image, before });
    }
  }
  for (const c of plan.cases) {
    console.log(`${new Date().toISOString()} begin ${c.pair}/${c.id}`);
    for (let block = -1; block < plan.blocks; block++) for (const [slot, label] of ['release', 'main', 'main', 'release'].entries()) {
      assert(!receipt.interrupted, 'interrupted');
      const image = images.get(`${c.pair}/${label}/${c.image}`).image;
      const work = path.join(plan.workRoots[c.pair], 'cases', String(seq));
      assert(!fs.existsSync(work), 'stale case');
      hostSample();
      const start = new Date().toISOString();
      const args = ['case', '--engine', 'redline', '--workload', c.id, '--rows', String(plan.rows),
        '--pair', c.pair, '--work-divisor', '1', '--image', image, '--work', work];
      const outcome = await invoke(plan.versions[label].binary, args, 600);
      const record = { seq: seq++, case: c.id, pair: c.pair, block, slot, label, startedAt: start,
        finishedAt: new Date().toISOString(), exit: outcome.code, signal: outcome.signal,
        status: outcome.code === 0 ? 'ok' : 'error', raw: outcome.out,
        rawSha256: sha256(outcome.out), stderr: outcome.err };
      fs.appendFileSync(recordFile, `${JSON.stringify(record)}\n`);
      hostSample();
      assert.equal(outcome.code, 0, `case failed ${outcome.err}`);
      const measured = parseRecord(outcome.out);
      assert.equal(measured.digest, c.digest, 'result mismatch');
      assert.equal(measured.ops, c.ops, 'unequal work');
      // Only this run's freshly created case copy; immutable input images remain.
      fs.rmSync(work, { recursive: true });
    }
    console.log(`${new Date().toISOString()} done ${c.pair}/${c.id} 12 ABBA blocks`);
  }
  for (const label of ['release', 'main']) {
    assert.equal(sha256(fs.readFileSync(plan.versions[label].binary)), plan.versions[label].binarySha256);
  }
  receipt.images = [];
  for (const [key, { image, before }] of images) {
    const after = imageHashes(image);
    assert.deepEqual(after, before, 'image changed');
    receipt.images.push({ key, before, after });
  }
  receipt.exit = 0;
} catch (e) {
  receipt.error = e.stack;
  console.error(e.stack);
  process.exitCode = 1;
} finally {
  clearInterval(sampler); hostSample();
  receipt.finishedAt = new Date().toISOString();
  for (const name of fs.readdirSync(bundle).sort()) {
    receipt.files.push({ path: name, sha256: sha256(fs.readFileSync(path.join(bundle, name))) });
  }
  fs.writeFileSync(path.join(bundle, 'receipt.json'), `${JSON.stringify(receipt, null, 2)}\n`);
}
