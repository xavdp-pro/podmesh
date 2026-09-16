// The operator's web rules (INTENT.md, "Web surfaces"), checked in every web source of the manager:
// no native form post, no JavaScript alert/confirm/prompt, no system select. The fourth rule -- a
// long list is a searchable picker -- is a judgement about length and stays a review item.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const web = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const tree = path.dirname(web);
const walk = dir => fs.readdirSync(dir, {withFileTypes: true}).flatMap(e =>
  e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)]);
const origin = path.join(tree, 'packaging', 'podmesh-manager', 'universe', 'origin');
const sources = [
  ...walk(path.join(web, 'src')).filter(f => /\.(jsx?|mjs|html)$/.test(f)),
  path.join(web, 'index.html'),
  ...(fs.existsSync(path.join(web, 'next', 'src')) ? walk(path.join(web, 'next', 'src')).filter(f => /\.(jsx?|mjs|html)$/.test(f)) : []),
  path.join(web, 'next', 'index.html'),
  ...['src', 'server'].filter(d => fs.existsSync(path.join(origin, d))).flatMap(d => walk(path.join(origin, d))).filter(f => /\.(jsx?|mjs|html)$/.test(f)),
  path.join(origin, 'index.html'),
].filter(f => fs.existsSync(f));
const read = f => fs.readFileSync(f, 'utf8')
  // comments explain the rules and may name what they forbid; code may not
  .replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`\\])\/\/[^\n]*/g, '$1').replace(/^\s*#[^\n]*$/gm, '');

test('the rules are checked against real sources, not an empty list', () => {
  assert.ok(sources.some(f => f.endsWith('main.jsx')), 'the console source is missing from the scan');
  assert.ok(sources.some(f => f.includes('/origin/src/')), 'the manager origin app is missing from the scan');
  assert.ok(sources.some(f => f.includes('/next/src/')), 'the console\'s new front is missing from the scan');
});

test('no native form post: a form carries neither method nor action', () => {
  for (const f of sources) {
    assert.doesNotMatch(read(f), /<form\b[^>]*\b(method|action)\s*=/i, `${path.relative(tree, f)} has a form that can post natively`);
  }
});

test('no JavaScript alert, confirm or prompt: a modal instead', () => {
  for (const f of sources) {
    assert.doesNotMatch(read(f), /(^|[^.\w])(window\.)?(alert|confirm|prompt)\s*\(/, `${path.relative(tree, f)} calls a browser dialog`);
  }
});

test('no system select: a styled list instead', () => {
  for (const f of sources) {
    // lower-case only: <select> is the browser's element, <Select> a styled component of ours
    assert.doesNotMatch(read(f), /<select\b|createElement\(\s*['"]select['"]\s*\)/, `${path.relative(tree, f)} draws a system select`);
  }
});
