import { spawn } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { join } from 'node:path';
import net from 'node:net';
import { once } from 'node:events';
import assert from 'node:assert/strict';
import { fixtureRoot } from './runner.mjs';


export async function serverChecks(binary, tempRoot) {
  const work = mkdtempSync(join(fixtureRoot(tempRoot), 'redline-docs-server-'));
  const reserve = net.createServer();
  reserve.listen(0, '127.0.0.1');
  await once(reserve, 'listening');
  const port = reserve.address().port;
  await new Promise(resolve => reserve.close(resolve));
  const env = { ...process.env, TMPDIR: work };
  for (const key of Object.keys(env)) if (key.startsWith('REDLINE')) delete env[key];
  const child = spawn(binary, ['--database', join(work, 'new.redline'), '--listen', `127.0.0.1:${port}`], { cwd: work, env });
  let stderr = '';
  child.stderr.on('data', bytes => { stderr += bytes; });
  let socket;
  const deadline = new AbortController();
  const timer = setTimeout(() => { deadline.abort(); socket?.destroy(); child.kill('SIGTERM'); }, 15000);
  try {
    // Startup is bounded by the same protocol deadline as the round trip.
    // A retry count would turn transient refusal into a shorter startup limit.
    for (;;) {
      try {
        socket = net.connect({ host: '127.0.0.1', port });
        await once(socket, 'connect', { signal: deadline.signal });
        break;
      } catch (error) {
        socket.destroy();
        if (deadline.signal.aborted) throw new Error(`server protocol deadline: ${stderr}`);
        if (child.exitCode !== null) throw new Error(`server exited: ${stderr}`);
        if (error.code !== 'ECONNREFUSED') throw error;
        await new Promise(resolve => setTimeout(resolve, 20));
      }
    }
    socket.on('close', () => deadline.abort());
    let buffer = Buffer.alloc(0);
    socket.on('data', bytes => { buffer = Buffer.concat([buffer, bytes]); });
    async function read(count) {
      while (buffer.length < count) await once(socket, 'data', { signal: deadline.signal });
      const result = buffer.subarray(0, count);
      buffer = buffer.subarray(count);
      return result;
    }
    const magic = await read(8);
    assert.equal(magic.toString('hex'), '524c444200010000');
    socket.write(magic);
    const receipts = [];
    async function request(message) {
      const bytes = Buffer.from(JSON.stringify(message));
      const length = Buffer.alloc(4);
      length.writeUInt32BE(bytes.length);
      socket.write(Buffer.concat([length, bytes]));
      const response = JSON.parse((await read((await read(4)).readUInt32BE())).toString());
      receipts.push({ request: message, response });
      return response;
    }
    assert.deepEqual(await request({ cmd: 'hello' }), { status: 'hello', protocol_version: 1, server: 'redlinedb-server' });
    assert.equal((await request({ cmd: 'exec', sql: 'CREATE TABLE note(id INTEGER PRIMARY KEY, body TEXT);' })).status, 'summary');
    assert.equal((await request({ cmd: 'begin', mode: 'deferred' })).status, 'ok');
    assert.equal((await request({ cmd: 'prepare', stmt_id: 1, sql: 'INSERT INTO note VALUES(?, ?);' })).parameter_count, 2);
    assert.equal((await request({ cmd: 'bind', stmt_id: 1, values: [{ type: 'integer', value: 1 }, { type: 'text', value: 'hello' }] })).status, 'bound');
    assert.equal((await request({ cmd: 'step', stmt_id: 1, max_rows: 1 })).done, true);
    assert.equal((await request({ cmd: 'reset', stmt_id: 1 })).status, 'ok');
    assert.equal((await request({ cmd: 'finalize', stmt_id: 1 })).status, 'ok');
    assert.equal((await request({ cmd: 'commit' })).status, 'ok');
    assert.equal((await request({ cmd: 'prepare', stmt_id: 2, sql: 'SELECT body FROM note WHERE id = 1;' })).column_count, 1);
    assert.deepEqual((await request({ cmd: 'step', stmt_id: 2, max_rows: 2 })).rows, [[{ type: 'text', value: 'hello' }]]);
    assert.equal((await request({ cmd: 'finalize', stmt_id: 2 })).status, 'ok');
    assert.equal((await request({ cmd: 'begin' })).status, 'ok');
    assert.equal((await request({ cmd: 'rollback' })).status, 'ok');
    assert.equal((await request({ cmd: 'interrupt' })).status, 'ok');
    assert.equal((await request({ cmd: 'close' })).status, 'ok');
    return { binary, work, magic: magic.toString('hex'), receipts, stderr };
  } finally {
    clearTimeout(timer);
    socket?.destroy();
    child.kill('SIGTERM');
    if (child.exitCode === null && child.signalCode === null) await once(child, 'exit');
  }
}
