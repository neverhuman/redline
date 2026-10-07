import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// Read retained records to stdout only; never extract or replay historical SQL.
const root = dirname(fileURLToPath(import.meta.url));
const manifest = JSON.parse(readFileSync(join(root, 'manifest.json'), 'utf8'));
const archive = join(root, manifest.archive.path);
const digest = (data) => createHash('sha256').update(data).digest('hex');
const data = readFileSync(archive);
if (data.length !== manifest.archive.bytes || digest(data) !== manifest.archive.sha256) {
  throw new Error('Historical audit archive hash or size differs');
}
const list = (flag) => execFileSync('tar', [flag, archive], { encoding: 'utf8' }).trimEnd().split('\n');
const names = list('-tzf');
const expectedNames = Object.keys(manifest.files);
if (new Set(names).size !== names.length || names.length !== expectedNames.length ||
    names.some((name) => !Object.hasOwn(manifest.files, name))) {
  throw new Error('Historical audit archive has missing, extra or duplicate records');
}
const descriptions = list('-tvzf');
if (descriptions.length !== names.length || descriptions.some((line) => !line.startsWith('-'))) {
  throw new Error('Historical audit record is not a regular file');
}
const records = new Map();
for (const name of names) {
  const expected = manifest.files[name];
  const content = execFileSync('tar', ['-xOzf', archive, '--', name], {
    maxBuffer: expected.bytes + 65536,
  });
  if (content.length !== expected.bytes || digest(content) !== expected.sha256) {
    throw new Error(`Historical audit record differs: ${name}`);
  }
  records.set(name, content);
}
for (const [name, expected] of Object.entries(manifest.derived.divergences)) {
  const rows = records.get(name).toString('utf8').split('\n').filter((line) => line.trim()).map(JSON.parse);
  const kinds = {};
  for (const row of rows) kinds[row.kind] = (kinds[row.kind] ?? 0) + 1;
  if (rows.length !== expected.records ||
      Object.keys(kinds).length !== Object.keys(expected.kinds).length ||
      Object.entries(kinds).some(([kind, count]) => expected.kinds[kind] !== count)) {
    throw new Error(`Historical divergence counts differ: ${name}`);
  }
}
const inventory = JSON.parse(records.get('unsafe-inventory.json').toString('utf8'));
const counts = Object.fromEntries(Object.entries(inventory).filter(([, value]) => Number.isInteger(value)));
const expected = manifest.derived.unsafe_inventory;
if (Object.keys(counts).length !== Object.keys(expected).length ||
    Object.entries(counts).some(([key, value]) => expected[key] !== value)) {
  throw new Error('Historical unsafe inventory counts differ');
}
console.log(`Historical audit records: ${names.length} verified; no SQL replay or current-head scan.`);
