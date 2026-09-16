// The operator's web rules (INTENT.md, "Web surfaces") on this front's sources, and the two catalogs
// carrying the same keys: no native form post, no browser dialog, no system select, every form
// submission intercepted.
import { describe, it, expect } from 'vitest'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import en from '../src/i18n/locales/en.json'
import fr from '../src/i18n/locales/fr.json'

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)))
const walk = dir => fs.readdirSync(dir, { withFileTypes: true }).flatMap(e => (e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)]))
const sources = [...walk(path.join(root, 'src')).filter(f => /\.(jsx?|mjs)$/.test(f)), path.join(root, 'index.html')]
const code = f => fs.readFileSync(f, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`\\])\/\/[^\n]*/g, '$1')
const keys = (o, p = '') => Object.entries(o).flatMap(([k, v]) => (v && typeof v === 'object' ? keys(v, `${p}${k}.`) : [`${p}${k}`])).sort()

describe('web rules', () => {
  it('scans real sources', () => { expect(sources.some(f => f.endsWith('TakeoverModal.jsx'))).toBe(true) })
  it('has no form that posts natively, and every form intercepts its submission', () => {
    for (const f of sources) {
      const text = code(f)
      expect(text, f).not.toMatch(/<form\b[^>]*\b(method|action)\s*=/i)
      if (/<form\b/.test(text)) expect(text, f).toMatch(/preventDefault\(\)/)
    }
  })
  it('never calls a browser dialog', () => {
    for (const f of sources) expect(code(f), f).not.toMatch(/(^|[^.\w])(window\.)?(alert|confirm|prompt)\s*\(/)
  })
  it('never draws a system select', () => {
    for (const f of sources) expect(code(f), f).not.toMatch(/<select\b/)
  })
})

describe('catalogs', () => {
  it('carry the same keys in English and French', () => { expect(keys(fr)).toEqual(keys(en)) })
  it('name every literal key the sources use', () => {
    const used = new Set()
    for (const f of sources) for (const m of code(f).matchAll(/\bt\(\s*'([a-zA-Z0-9_.]+)'/g)) used.add(m[1])
    const known = new Set(keys(en))
    expect([...used].filter(k => !known.has(k))).toEqual([])
  })
})
