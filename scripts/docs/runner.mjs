import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fencedBlocks } from './markdown.mjs';

export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

export function fixtureRoot(explicitRoot) {
  const callerRoot = explicitRoot ?? process.env.TMPDIR;
  const safeCallerRoot = callerRoot && !/^\/tmp(?:\/|$)/.test(resolve(callerRoot));
  const root = safeCallerRoot ? resolve(callerRoot) : resolve(process.env.CARGO_TARGET_DIR ?? 'target', 'docs-check', 'tmp');
  assert.ok(!/^\/tmp(?:\/|$)/.test(root), 'docs build fixtures must live outside /tmp');
  mkdirSync(root, { recursive: true });
  return root;
}

export function fixtureEnv(work, binary) {
  const env = { ...process.env, TMPDIR: work };
  env.RUSTUP_HOME = process.env.RUSTUP_HOME ?? join(process.env.HOME, '.rustup');
  for (const key of Object.keys(env)) if (key.startsWith('REDLINE')) delete env[key];
  env.PATH = `${dirname(binary)}:${process.env.PATH}`;
  env.REDLINEDB_QUIET_DURABILITY = '1';
  return env;
}

export function invoke(binary, args, options, expected) {
  const result = spawnSync(binary, args, { encoding: 'utf8', timeout: 15000, ...options });
  const receipt = { binary, args, exit: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr };
  assert.ifError(result.error);
  assert.equal(result.signal, expected.signal ?? null, JSON.stringify(receipt));
  assert.equal(result.status, expected.exit ?? 0, JSON.stringify(receipt));
  if (expected.stdout !== undefined) assert.equal(result.stdout, expected.stdout, JSON.stringify(receipt));
  if (expected.stderrIncludes) assert.ok(result.stderr.includes(expected.stderrIncludes), JSON.stringify(receipt));
  return receipt;
}

export function executeBlock(binary, block, label, options = {}) {
  const [language, annotation, ...modifiers] = block.info.split(/\s+/);
  assert.equal(annotation, 'doctest');
  assert.ok(['sql', 'bash', 'json', 'rust'].includes(language), `${label}: unsupported doctest language ${language}`);
  assert.ok(modifiers.every(value => ['postgres', 'crlf', 'error'].includes(value)), `${label}: unknown doctest modifier`);
  const work = mkdtempSync(join(fixtureRoot(options.tempRoot), 'redline-docs-'));
  if (options.root) {
    mkdirSync(join(work, 'docs', 'manual'), { recursive: true });
    cpSync(join(resolve(options.root), 'docs', 'manual', 'examples'), join(work, 'docs', 'manual', 'examples'), { recursive: true, dereference: true });
  }
  const env = { ...fixtureEnv(work, binary), ...options.env };
  if (modifiers.includes('postgres')) env.REDLINEDB_RESULT_DIALECT = 'postgres';
  const prefix = language === 'sql' ? '--' : language === 'rust' ? '//' : '#';
  const lines = block.body.split('\n');
  const output = lines.filter(line => line.startsWith(`${prefix} prints: `));
  const errors = lines.filter(line => line.startsWith(`${prefix} error: `));
  const ending = modifiers.includes('crlf') ? '\r\n' : '\n';
  let expected = { stdout: output.length ? output.map(line => line.slice(`${prefix} prints: `.length)).join(ending) + ending : '' };
  if (modifiers.includes('error')) {
    assert.equal(errors.length, 1, `${label}: error doctest requires one error marker`);
    expected = { ...expected, exit: 1, stderrIncludes: errors[0].slice(`${prefix} error: `.length) };
  } else assert.ok(output.length || language === 'rust', `${label}: doctest requires expected output`);
  let receipt;
  if (language === 'sql') receipt = invoke(binary, ['-batch', '-bail', ':memory:'], { cwd: work, env, input: block.body }, expected);
  else if (language === 'bash') receipt = invoke('bash', ['-euo', 'pipefail', '-c', block.body], { cwd: work, env }, expected);
  else if (language === 'json') {
    const split = block.body.indexOf('\n# prints: ');
    assert.ok(split >= 0, `${label}: JSON needs output marker after document`);
    const source = block.body.slice(0, split);
    JSON.parse(source);
    receipt = invoke(binary, ['--rql', ':memory:'], { cwd: work, env, input: source }, expected);
  } else {
    assert.ok(options.rlib, `${label}: Rust doctest needs built facade library`);
    const fixture = options.rustFixture ?? {};
    const source = `${fixture.prefix ?? ''}${block.body}${fixture.suffix ?? ''}`;
    const file = join(work, 'example.rs');
    const executable = join(work, 'example');
    writeFileSync(file, source);
    const compile = invoke(options.rustc ?? 'rustc', ['--edition=2024', '--crate-name', 'docs_example', file, '--extern', `redlinedb=${resolve(options.rlib)}`,
      '-L', `dependency=${join(dirname(resolve(options.rlib)), 'deps')}`, '-o', executable], { cwd: work, env, timeout: 90000 }, { stdout: '' });
    const run = invoke(executable, [], { cwd: work, env }, expected);
    receipt = { compile, ...run };
  }
  return { label, work, info: block.info, sourceSha256: sha256(block.body), ...receipt };
}

export function executeDocument(binary, source, label, options = {}) {
  return fencedBlocks(source).filter(block => block.info.split(/\s+/)[1] === 'doctest')
    .map(block => executeBlock(binary, block, `${label}:${block.line}`, options));
}
