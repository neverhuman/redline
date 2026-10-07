import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chmodSync, mkdtempSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import net from 'node:net';
import { fixtureRoot } from './runner.mjs';
import { serverChecks } from './server.mjs';

const binary = process.env.REDLINE_DOCS_TEST_SERVER;
assert.ok(binary, 'set REDLINE_DOCS_TEST_SERVER to the verified published server');

function fixture(body) {
  const directory = mkdtempSync(join(fixtureRoot(), 'docs-server-readiness-'));
  const script = join(directory, 'server');
  writeFileSync(script, `#!${process.execPath}\n${body}\n`);
  chmodSync(script, 0o755);
  return script;
}

test('startup can outlast one hundred refused connections within the protocol deadline', { timeout: 25000 }, async t => {
  const connect = net.connect.bind(net);
  let attempts = 0;
  t.mock.method(net, 'connect', options => {
    attempts++;
    if (attempts <= 105) {
      const socket = new net.Socket();
      queueMicrotask(() => socket.emit('error', Object.assign(new Error('fixture connection refused'), { code: 'ECONNREFUSED' })));
      return socket;
    }
    return connect(options);
  });
  const receipt = await serverChecks(binary);
  assert.ok(attempts > 105);
  assert.equal(receipt.magic, '524c444200010000');
  assert.equal(receipt.receipts.at(-1).request.cmd, 'close');
});

test('a server that never listens still fails at the unchanged protocol deadline', { timeout: 25000 }, async () => {
  const server = fixture("process.stderr.write('fixture never ready\\n'); setInterval(() => {}, 1000);");
  await assert.rejects(serverChecks(server), /server protocol deadline: fixture never ready/);
});

test('a server that exits during startup cannot pass the documentation check', { timeout: 25000 }, async () => {
  const server = fixture("process.stderr.write('fixture startup failure\\n'); process.exit(1);");
  await assert.rejects(serverChecks(server), /server exited: fixture startup failure/);
});

test('unexpected connection errors fail instead of being retried', { timeout: 25000 }, async t => {
  const server = fixture('setInterval(() => {}, 1000);');
  let attempts = 0;
  t.mock.method(net, 'connect', () => {
    attempts++;
    const socket = new net.Socket();
    queueMicrotask(() => socket.emit('error', Object.assign(new Error('fixture connection denied'), { code: 'EACCES' })));
    return socket;
  });
  await assert.rejects(serverChecks(server), /fixture connection denied/);
  assert.equal(attempts, 1);
});
