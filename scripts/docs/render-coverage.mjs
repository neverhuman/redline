#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { renderKnownLimitations, checkCoverage, renderRqlCoverage, checkRqlCoverage } from './coverage.mjs';

const root = resolve('.');
for (const [name, prefix, render, check] of [
  ['docs/known-limitations.md', 'docs-coverage', renderKnownLimitations, checkCoverage],
  ['README.md', 'rql-coverage', renderRqlCoverage, checkRqlCoverage],
  ['docs/rql.md', 'rql-coverage', renderRqlCoverage, checkRqlCoverage],
]) {
  const file = join(root, name);
  const source = readFileSync(file, 'utf8');
  if (process.argv.includes('--check')) check(root, source);
  else {
    const pattern = new RegExp(`<!-- ${prefix}:begin -->[\\s\\S]*?<!-- ${prefix}:end -->`);
    assert.ok(pattern.test(source), `${name} needs coverage markers`);
    writeFileSync(file, name === 'docs/known-limitations.md' ? render(root) : source.replace(pattern, render(root)));
  }
}
