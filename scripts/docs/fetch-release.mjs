// Download published core packages, never a development or freshly built CLI.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const option = name => { const at = args.indexOf(name); return at < 0 ? undefined : args[at + 1]; };
const contractFile = option('--contract') ?? join(dirname(fileURLToPath(import.meta.url)), 'release-contract.json');
const contract = JSON.parse(readFileSync(contractFile));
const cache = resolve(option('--cache') ?? 'target/docs-reference');
const platform = `${process.platform === 'darwin' ? 'macos' : process.platform === 'linux' ? 'linux' : 'unsupported'}-${process.arch === 'x64' ? 'x86_64' : process.arch}`;
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
function command(program, values) {
  const result = spawnSync(program, values, { stdio: ['ignore', 'ignore', 'inherit'], timeout: 240000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, `${program} ${values.join(' ')} failed`);
}
const receipts = [];
for (const tag of ['v5.1.1', 'v5.1.0']) {
  const expected = contract.releases[tag].archives[platform];
  assert.match(expected ?? '', /^[a-f0-9]{64}$/, `no published ${platform} documentation oracle`);
  const directory = join(cache, `${tag}-${platform}`);
  mkdirSync(directory, { recursive: true });
  const archive = join(directory, `redlinedb-${tag}-${platform}.tar.gz`);
  if (!existsSync(archive)) {
    const pending = `${archive}.download-${process.pid}`;
    command('curl', ['-fSL', '--retry', '3', '--max-time', '180', '-o', pending,
      `https://github.com/neverhuman/redline/releases/download/${tag}/redlinedb-${tag}-${platform}.tar.gz`]);
    assert.equal(hash(pending), expected, `${tag}: downloaded archive digest differs from release contract`);
    renameSync(pending, archive);
  }
  assert.equal(hash(archive), expected, `${tag}: cached archive digest differs from release contract`);
  const checksum = `${archive}.sha256`;
  if (!existsSync(checksum)) command('curl', ['-fSL', '--retry', '3', '--max-time', '60', '-o', checksum,
    `https://github.com/neverhuman/redline/releases/download/${tag}/redlinedb-${tag}-${platform}.tar.gz.sha256`]);
  assert.equal(readFileSync(checksum, 'utf8').trim().split(/\s+/)[0], expected, `${tag}: published checksum differs from pinned contract`);
  const extracted = join(directory, 'package');
  // Extract again after hash verification: a modified cached executable must
  // not become the documentation oracle merely because its archive is valid.
  mkdirSync(extracted, { recursive: true });
  command('tar', ['-xzf', archive, '-C', extracted]);
  const binary = join(extracted, 'bin', 'redlinedb');
  if (tag === 'v5.1.1' && platform === 'linux-x86_64') assert.equal(hash(binary), contract.cliBinarySha256LinuxX64);
  receipts.push({ tag, platform, archive, archiveSha256: expected, binary, binarySha256: hash(binary), server: join(extracted, 'bin', 'redlinedb-server') });
}
const receipt = { schema: 'redline.docs-oracles/v1', contractSha256: hash(contractFile), receipts };
writeFileSync(join(cache, 'oracles.json'), JSON.stringify(receipt, null, 2) + '\n');
process.stdout.write(JSON.stringify(receipt) + '\n');
