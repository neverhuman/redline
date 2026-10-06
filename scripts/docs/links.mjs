import assert from 'node:assert/strict';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { dirname, resolve, relative } from 'node:path';

function anchors(source) {
  const found = new Set();
  const repetitions = new Map();
  for (const match of source.matchAll(/<a\s+id=["']([^"']+)["']/g)) found.add(match[1]);
  for (const match of source.matchAll(/^#{1,6}\s+(.+?)\s*#*\s*$/gm)) {
    const slug = match[1].replace(/<[^>]*>/g, '').toLowerCase()
      .replace(/[^\p{L}\p{N}\s_-]/gu, '').replace(/\s/g, '-');
    const duplicate = repetitions.get(slug) ?? 0;
    found.add(duplicate ? `${slug}-${duplicate}` : slug);
    repetitions.set(slug, duplicate + 1);
  }
  return found;
}

// Check repository links without making network requests or executing recipes.
export function checkLinks(root, file, source, read = path => readFileSync(resolve(root, path), 'utf8'), planned = new Set()) {
  const receipts = [];
  const prose = source.replace(/^ {0,3}(`{3,}|~{3,})[^\n]*\n[\s\S]*?^ {0,3}\1\s*$/gm, '');
  for (const match of prose.matchAll(/\[[^\]\n]*\]\(([^\s)]+)(?:\s+"[^"]*")?\)/g)) {
    const target = match[1].replace(/^<|>$/g, '');
    if (/^[a-z]+:/i.test(target) || target.startsWith('//')) continue;
    const [name, fragment] = target.split('#');
    const path = name ? resolve(root, dirname(file), decodeURIComponent(name)) : resolve(root, file);
    const key = relative(root, path);
    assert.ok(!key.startsWith('../'), `${file}: link escapes repository: ${target}`);
    assert.ok(existsSync(path) || planned.has(key), `${file}: missing link target ${target}`);
    if (fragment && key.endsWith('.md')) assert.ok(anchors(read(key)).has(decodeURIComponent(fragment)), `${file}: missing anchor ${target}`);
    receipts.push({ file, target });
  }
  return receipts;
}
