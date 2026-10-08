import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { spawnWaiting, releaseWaiting } from './checked-worker.mjs';

const workerTest = (name, fn) => test(name, { timeout: 10000 }, fn);
function fixture(t, args) {
  const child = spawnWaiting(process.execPath, ['--input-type=module', '-e', ...args]);
  let out = '', err = '';
  child.stdout.on('data', x => { out += x; });
  child.stderr.on('data', x => { err += x; });
  const done = new Promise((resolve, reject) => {
    child.on('error', reject);
    child.on('close', (code, signal) => resolve({ code, signal, out, err }));
  });
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
  t.after(async () => {
    clearTimeout(timer);
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await done.catch(() => {});
  });
  return { child, done };
}
workerTest('benchmark stays unstarted until checks finish, then exec preserves PID and affinity', async t => {
  const { child, done } = fixture(t, [`import fs from 'node:fs'; process.stdout.write(JSON.stringify({pid:process.pid,cpus:fs.readFileSync('/proc/self/status','utf8').match(/^Cpus_allowed_list:\\s*(.+)$/m)[1]}));`]);
  const before = fs.readFileSync(`/proc/${child.pid}/status`, 'utf8').match(/^Cpus_allowed_list:\s*(.+)$/m)[1];
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(fs.readlinkSync(`/proc/${child.pid}/exe`).split('/').at(-1), 'bash');
  releaseWaiting(child);
  const result = await done;
  assert.equal(result.code, 0); assert.equal(result.signal, null);
  assert.deepEqual(JSON.parse(result.out), { pid: child.pid, cpus: before });
});
workerTest('failed scheduling checks can terminate the waiter without starting the benchmark', async t => {
  const { child, done } = fixture(t, [`process.stdout.write('started');`]);
  await new Promise(resolve => setTimeout(resolve, 20));
  child.kill('SIGTERM');
  const result = await done;
  assert.equal(result.out, ''); assert.equal(result.signal, 'SIGTERM');
});
workerTest('EOF cannot accidentally release the worker', async t => {
  const { child, done } = fixture(t, [`process.stdout.write('started');`]);
  child.stdin.end();
  const result = await done;
  assert.equal(result.code, 1); assert.equal(result.out, '');
});
workerTest('an invalid release token cannot start the benchmark', async t => {
  const { child, done } = fixture(t, [`process.stdout.write('started');`]);
  child.stdin.end('invalid\n');
  const result = await done;
  assert.equal(result.code, 1); assert.equal(result.out, '');
});
workerTest('benchmark arguments with shell syntax are passed literally', async t => {
  const values = ['space and newline\n', '$(false); exit 91', '$HOME', '`false`'];
  const { child, done } = fixture(t, [`process.stdout.write(JSON.stringify(process.argv.slice(1)));`, '--', ...values]);
  releaseWaiting(child);
  const result = await done;
  assert.equal(result.code, 0); assert.deepEqual(JSON.parse(result.out), values);
});
workerTest('the actual benchmark exit code and stderr are retained', async t => {
  const { child, done } = fixture(t, [`process.stderr.write('benchmark rejected'); process.exit(23);`]);
  releaseWaiting(child);
  const result = await done;
  assert.equal(result.code, 23); assert.equal(result.err, 'benchmark rejected');
});
