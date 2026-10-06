import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';

export function markdownFiles(root) {
  const files = ['README.md', 'CHANGELOG.md', 'CONTRIBUTING.md'];
  function visit(dir) {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.name.endsWith('.md')) files.push(relative(root, path));
    }
  }
  visit(join(root, 'docs'));
  return files.sort();
}

// Match Markdown fences rather than treating an embedded shorter fence as
// the closing fence. This also preserves the exact source of each example.
export function fencedBlocks(source) {
  const blocks = [];
  let open;
  for (const [index, line] of source.split('\n').entries()) {
    if (!open) {
      const match = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
      if (match) open = { line: index + 1, info: match[2].trim(), fence: match[1], lines: [] };
    } else if (new RegExp(`^ {0,3}${open.fence[0]}{${open.fence.length},}\\s*$`).test(line)) {
      blocks.push({ line: open.line, info: open.info, body: open.lines.join('\n') + '\n' });
      open = undefined;
    } else open.lines.push(line);
  }
  if (open) throw new Error(`unclosed code fence at line ${open.line}`);
  return blocks;
}

export function readBlocks(root, file) {
  return fencedBlocks(readFileSync(join(root, file), 'utf8'));
}
