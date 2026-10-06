import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkGuideClaims } from './guides.mjs';

// These statements contradicted actual release probes. Guard their correction
// as well as executing the examples: prose cannot quietly restore the claim.
export function checkReleaseClaims(read) {
  const checks = [
    ['docs/manual/02-start-here.md', /\/v5\.0\.0\/install\.sh|tag = "v5\.0\.0"|git checkout v5\.0\.0/, 'active manual install/build/dependency still names v5.0.0'],
    ['docs/api-stability.md', /tag = "v5\.1\.0"/, 'active embedding dependency still names v5.1.0'],
    ['docs/manual/08-embed.md', /--database` is an existing file/, 'server creates a missing directory, not a database file'],
    ['docs/manual/06-sql-you-will-write.md', /In the Postgres dialect they render as `t` and `f`/, 'literal booleans render as integers in the published binary'],
    ['docs/beyond-postgres-skips.md', /current score is 265\s*\/\s*265/, 'declared-unsupported cases are not passing queries'],
    ['docs/testing.md', /The bench harness honors the `REDLINEDB_BENCH_KILL`/, 'the advertised kill switch is not implemented in v5.1.1'],
  ];
  const failures = checks.filter(([file, pattern]) => pattern.test(read(file))).map(([file, , reason]) => `${file}: ${reason}`);
  assert.equal(failures.length, 0, failures.join('\n'));
}

// Keep the standalone prose gate and full checker on the same guide checks.
export function checkReleaseAndGuideClaims(read, contract, issues) {
  checkReleaseClaims(read);
  return checkGuideClaims(read, contract, issues);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const at = process.argv.indexOf('--root');
  const root = at < 0 ? process.cwd() : process.argv[at + 1];
  const read = file => readFileSync(join(root, file), 'utf8');
  const contract = JSON.parse(read('scripts/docs/release-contract.json'));
  const issues = JSON.parse(read('scripts/docs/issues.json'));
  checkReleaseAndGuideClaims(read, contract, issues);
  console.log('v5.1.1 documentation claim checks passed');
}
