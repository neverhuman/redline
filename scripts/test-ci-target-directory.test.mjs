import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const source = path.resolve(process.env.TARGET_TEST_SOURCE || process.cwd());
function checkLane(lane, external, relative = false, scratchInput = process.env.TMPDIR || path.join(source, 'target/review/ci-target-test')) {
  const scratch = path.resolve(scratchInput);
  fs.mkdirSync(scratch, { recursive: true });
  const tmp = fs.mkdtempSync(path.join(scratch, 'target-contract-'));
  try {
    const root = path.join(tmp, 'source');
    fs.mkdirSync(root);
    for (const name of ['scripts/ci-family.sh', 'ops/ci/lib.sh', 'ops/ci/check-report.sh']) {
      const dest = path.join(root, name);
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.copyFileSync(path.join(source, name), dest);
    }
    const commands = path.join(root, 'commands'), trace = path.join(root, 'trace');
    const target = relative ? path.join(root, 'relative cache')
      : external ? path.join(tmp, 'external cache') : path.join(root, 'target');
    const script = (name, body) => {
      const dest = path.join(root, name);
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.writeFileSync(dest, '#!/usr/bin/env bash\nset -euo pipefail\n' + body + '\n', { mode: 0o755 });
    };
    script('commands/cargo', 'printf "%s\\n" "$CARGO_TARGET_DIR" >> "$TRACE"');
    script('commands/git', 'printf "%s\\n" "3e8b2d5b6f13a4440c10ad5c56bff73d3d967538"');
    script('commands/npx', 'if [[ "$*" == *"playwright test"* ]]; then test "$REDLINE_WEB_TARGET_BIN" = "$EXPECTED/release/redlinedb"; fi');
    script('scripts/build-from-source.sh', 'test "$CARGO_TARGET_DIR" = "$EXPECTED"');
    script('scripts/test-binaries.sh', 'test "$1" = "$EXPECTED/release"');
    fs.mkdirSync(path.join(root, 'subrepos/redline-web/apps/web'), { recursive: true });
    fs.writeFileSync(path.join(root, 'README.md'), 'fixture\n');
    // An explicit cache must not select a stale checkout-local reporter.
    script('target/release/redline-testing', external ? 'exit 91' : 'printf "%s\\n" "$1" >> "$TRACE"');
    if (external) {
      const runner = path.join(target, 'release/redline-testing');
      fs.mkdirSync(path.dirname(runner), { recursive: true });
      fs.writeFileSync(runner, '#!/usr/bin/env bash\nprintf "%s\\n" "$1" >> "$TRACE"\n', { mode: 0o755 });
    }
    const env = { ...process.env, PATH: commands + ':' + process.env.PATH, EXPECTED: target, TRACE: trace };
    delete env.CARGO_TARGET_DIR;
    if (external) env.CARGO_TARGET_DIR = relative ? path.basename(target) : target;
    const args = lane === 'runner'
      ? ['-c', '. ops/ci/lib.sh; ci_install_redline_testing_local() { test "$CI_REDLINE_TESTING_LOCAL_BIN" = "$EXPECTED/release/redline-testing"; }; ci_install_redline_testing']
      : lane === 'report' ? ['ops/ci/check-report.sh'] : ['scripts/ci-family.sh', lane];
    const result = spawnSync('bash', args, { cwd: root, env, encoding: 'utf8' });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, result.stderr);
    if (lane === 'report' || lane === 'testing') {
      const expected = lane === 'report' ? ['report', 'check-postgres'] : [target, target];
      assert.deepEqual(fs.readFileSync(trace, 'utf8').trim().split('\n'), expected);
    }
  } finally {
    fs.rmSync(tmp, { recursive: true });
  }
}
for (const lane of ['testing', 'runner', 'report', 'integration']) {
  for (const external of [false, true]) {
    test(lane + (external ? ' external cache' : ' default cache'), () => checkLane(lane, external));
  }
}
for (const lane of ['runner', 'integration']) {
  test(lane + ' relative cache with spaces', () => checkLane(lane, true, true));
}
test('relative caller scratch directory resolves before changing child cwd', () =>
  checkLane('integration', true, true, 'target/review/ci-target-test-relative'));
