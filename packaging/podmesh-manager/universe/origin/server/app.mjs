// The manager universe's origin as an Express application, fail-closed on the active manager's mark.
//
// Surfaces: GET / (the human page), GET /ready (the publisher contract's JSON), the administration
// app under /admin (built files) and its API under /admin/api. Without the mark every path answers
// 503: a connector that reaches a replica which is not the active manager gets nothing.
//
// Administrators are replicated facts (`admin.user.<login>` with a scrypt hash, `admin.flag.<login>`
// while the deployment password is still in place), written in this replica's own scope through
// the resident's control socket. The first one never comes from here: with none on record the state
// says so and names the host door. Pure AJAX (INTENT.md, "Web surfaces"): the API takes JSON only,
// a native form cannot reach it, and the page's policy forbids submitting one.
import express from 'express'
import helmet from 'helmet'
import rateLimit from 'express-rate-limit'
import jwt from 'jsonwebtoken'
import { randomBytes } from 'node:crypto'
import path from 'node:path'
import { existsSync } from 'node:fs'
import { hashPassword, verifyPassword } from './resident.mjs'

export const SUBJECT = 'admin.user.'
export const FLAG = 'admin.flag.'
export const MUST_CHANGE = 'must_change'
const LOGIN = /^[a-z0-9._-]{1,64}$/
const MIN_PASSWORD = 12
const SESSION_SECONDS = 3600
const COOKIE = 'podmesh_admin'

// login -> {value, scopes, revision}: within a scope the highest revision of a subject is its
// state; a login written in two scopes is a conflict, reported, never resolved behind the operator.
export function administrators(facts) {
  const found = new Map()
  for (const f of facts) {
    const subject = f.subject || ''
    if (!subject.startsWith(SUBJECT)) continue
    const login = subject.slice(SUBJECT.length)
    const revision = f.subject_revision || 0
    const e = found.get(login) || { value: null, scopes: [], revision: -1 }
    if (revision >= e.revision) { e.value = f.value; e.revision = revision }
    if (f.scope && !e.scopes.includes(f.scope)) e.scopes.push(f.scope)
    found.set(login, e)
  }
  return new Map([...found].filter(([, e]) => e.value && e.value !== 'revoked'))
}

export function flags(facts) {
  const found = new Map()
  for (const f of facts) {
    const subject = f.subject || ''
    if (!subject.startsWith(FLAG)) continue
    const login = subject.slice(FLAG.length)
    const revision = f.subject_revision || 0
    if (revision >= (found.get(login)?.revision ?? -1)) found.set(login, { revision, value: f.value })
  }
  return new Map([...found].map(([l, e]) => [l, e.value]))
}

const cookies = req => Object.fromEntries((req.headers.cookie || '').split(';').map(p => p.trim().split('=')).filter(([k]) => k).map(([k, ...v]) => [k, v.join('=')]))

export function createApp({ identity, scope, activeManager, facts, append, storeState = async () => ({ closed: true, reason: 'the origin was started without a store state' }), dist = path.resolve(import.meta.dirname, '../dist'), secret = randomBytes(32), secure = true, now = () => Date.now() }) {
  const app = express()
  app.disable('x-powered-by')
  app.set('trust proxy', 1)
  app.use(helmet({
    contentSecurityPolicy: {
      useDefaults: false,
      directives: {
        'default-src': ["'none'"],
        'script-src': ["'self'"],
        'style-src': ["'self'", "'unsafe-inline'"],
        'img-src': ["'self'", 'data:'],
        'font-src': ["'self'"],
        'connect-src': ["'self'"],
        'form-action': ["'none'"],
        'frame-ancestors': ["'none'"],
        'base-uri': ["'none'"],
      },
    },
    crossOriginEmbedderPolicy: false,
    referrerPolicy: { policy: 'no-referrer' },
  }))
  app.use((req, res, next) => { res.set('Cache-Control', 'no-store'); next() })

  const closed = (req, res) => {
    const p = req.path
    const reason = p === '/' || p === '/index.html' || p === '/ready' || p.startsWith('/admin') ? 'not the active manager' : 'no such path'
    res.status(503).json({ ready: false, reason, ...identity })
  }
  // Fail-closed first: nothing is served without the mark, the administration app included.
  app.use((req, res, next) => { req.mark = activeManager(); if (!req.mark) return closed(req, res); next() })

  // Fail-closed with the store: the administration reads and writes replicated facts, so it is
  // refused while the resident reports its store closed for its process, and refused too when the
  // resident does not answer. The mark and /ready are unchanged: they say what PodMesh published,
  // which a closed store does not withdraw.
  app.use('/admin', async (req, res, next) => {
    let state
    try {
      state = await storeState()
    } catch {
      state = { closed: true, reason: 'the resident did not answer' }
    }
    if (!state || state.closed) {
      return res.status(503).json({ ready: false, reason: 'the manager store is closed', detail: state?.reason || 'the resident did not answer', ...identity })
    }
    next()
  })

  app.get(['/', '/index.html'], (req, res) => {
    res.type('html').send(`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>PodMesh manager</title><style>body{margin:0;background:#f4efe4;color:#1c1916;font-family:ui-sans-serif,system-ui,sans-serif}main{max-width:40rem;margin:12vh auto;padding:0 1.5rem}h1{font-size:1.6rem;font-weight:600}p,dd{line-height:1.45;color:#4a433b}dl{display:grid;grid-template-columns:8rem 1fr;gap:.35rem 1rem}dt{color:#7a7268}a{color:#215547}</style><main><p>PODMESH / MANAGER ORIGIN</p><h1>This replica is the active manager.</h1><p>The public hostname reaches the replica that currently holds the exclusive role. Machine JSON stays at <a href="/ready"><code>/ready</code></a>, administration at <a href="/admin/">/admin</a>.</p><dl><dt>epoch</dt><dd><code>${Number(req.mark.epoch) || ''}</code></dd><dt>replica</dt><dd><code>${identity.replica_id}</code></dd><dt>logical</dt><dd><code>${identity.logical_manager_id}</code></dd></dl></main></html>`)
  })
  app.get('/ready', (req, res) => res.json({ ready: true, ...identity, epoch: req.mark.epoch, marked_at: req.mark.marked_at }))

  // --- the API: JSON in, JSON out, nothing else
  const api = express.Router()
  api.use((req, res, next) => {
    if (req.method !== 'POST') return next()
    const kind = (req.headers['content-type'] || '').split(';')[0].trim().toLowerCase()
    if (kind !== 'application/json') return res.status(415).json({ error: 'The API takes a JSON body (Content-Type: application/json), at most 4096 bytes.' })
    next()
  })
  api.use(express.json({ limit: '4kb', strict: true }))
  api.use((err, req, res, next) => err ? res.status(400).json({ error: 'The body is not valid JSON, or is larger than 4096 bytes.' }) : next())

  const setSession = (res, login) => {
    const token = jwt.sign({ login }, secret, { algorithm: 'HS256', expiresIn: SESSION_SECONDS })
    res.set('Set-Cookie', `${COOKIE}=${token}; Path=/admin; Max-Age=${SESSION_SECONDS}; HttpOnly; SameSite=Strict${secure ? '; Secure' : ''}`)
  }
  const clearSession = res => res.set('Set-Cookie', `${COOKIE}=; Path=/admin; Max-Age=0; HttpOnly; SameSite=Strict${secure ? '; Secure' : ''}`)
  const sessionOf = (req, admins) => {
    const token = cookies(req)[COOKIE]
    if (!token) return null
    try {
      const { login } = jwt.verify(token, secret, { algorithms: ['HS256'] })
      return admins.has(login) ? login : null
    } catch {
      return null
    }
  }
  const store = async res => {
    try {
      return await facts()
    } catch {
      res.status(503).json({ error: 'The store could not be read.' })
      return null
    }
  }

  api.get('/state', async (req, res) => {
    const all = await store(res)
    if (!all) return
    const admins = administrators(all)
    if (!admins.size) return res.json({ view: 'none', ...identity })
    const login = sessionOf(req, admins)
    if (!login) return res.json({ view: 'login', ...identity })
    if (flags(all).get(login) === MUST_CHANGE) return res.json({ view: 'change', login, ...identity })
    res.json({
      view: 'admin', login, epoch: req.mark.epoch, scope, ...identity,
      administrators: [...admins].sort(([a], [b]) => a.localeCompare(b)).map(([l, e]) => ({ login: l, scopes: e.scopes, conflict: e.scopes.length > 1 })),
    })
  })

  // Five failed sign-ins for one login close it for a minute (the peer address is the connector's,
  // so the key is the login, not the address).
  const attempts = rateLimit({
    windowMs: 60_000, limit: 5, standardHeaders: false, legacyHeaders: false, skipSuccessfulRequests: true,
    keyGenerator: req => `login:${String(req.body?.login || '').trim().toLowerCase()}`,
    handler: (req, res) => res.status(429).json({ error: 'Too many attempts for that administrator. Wait a minute.' }),
    validate: false,
  })
  api.post('/login', attempts, async (req, res) => {
    const login = String(req.body?.login || '').trim().toLowerCase()
    const password = String(req.body?.password || '')
    const all = await store(res)
    if (!all) return
    const e = administrators(all).get(login)
    if (!e || !verifyPassword(password, e.value)) return res.status(401).json({ error: 'Wrong administrator or password.' })
    setSession(res, login)
    res.json({ ok: true })
  })
  api.post('/logout', (req, res) => { clearSession(res); res.json({ ok: true }) })

  const authed = async (req, res) => {
    const all = await store(res)
    if (!all) return null
    const admins = administrators(all)
    const login = sessionOf(req, admins)
    if (!login) { res.status(401).json({ error: 'Sign in first. An administrator is named by an administrator.' }); return null }
    return { all, admins, login }
  }
  api.post('/password', async (req, res) => {
    const s = await authed(req, res)
    if (!s) return
    const current = String(req.body?.current || ''), next = String(req.body?.next || ''), again = String(req.body?.again || '')
    const record = s.admins.get(s.login)
    let problem = null
    if (!record || !verifyPassword(current, record.value)) problem = 'The current password is wrong.'
    else if (next !== again) problem = 'The two new entries differ.'
    else if (next.length < MIN_PASSWORD) problem = `A password is at least ${MIN_PASSWORD} characters.`
    else if (next === current) problem = 'The new password is the old one.'
    else if (next.toLowerCase() === s.login) problem = 'A password that is the login is not a password.'
    if (problem) return res.status(400).json({ error: problem })
    try {
      await append(SUBJECT + s.login, hashPassword(next))
      await append(FLAG + s.login, 'changed')
    } catch (e) {
      return res.status(503).json({ error: `The resident refused: ${e.message}` })
    }
    res.json({ ok: true, message: 'The password was changed.' })
  })
  api.post('/users', async (req, res) => {
    const s = await authed(req, res)
    if (!s) return
    if (flags(s.all).get(s.login) === MUST_CHANGE) return res.status(403).json({ error: 'Replace the deployment password before naming anyone.' })
    const login = String(req.body?.login || '').trim().toLowerCase()
    const password = String(req.body?.password || '')
    let problem = null
    if (!LOGIN.test(login)) problem = 'A login is 1 to 64 characters from a-z, 0-9, dot, dash and underscore.'
    else if (s.admins.has(login)) problem = 'That administrator already exists.'
    else if (password.length < MIN_PASSWORD) problem = `A password is at least ${MIN_PASSWORD} characters.`
    else if (password.toLowerCase() === login) problem = 'A password that is the login is not a password.'
    if (problem) return res.status(400).json({ error: problem })
    try {
      await append(SUBJECT + login, hashPassword(password))
    } catch (e) {
      return res.status(503).json({ error: `The resident refused: ${e.message}` })
    }
    res.json({ ok: true, message: `Administrator ${login} created in ${scope}; it replicates to the other replicas.` })
  })
  api.post('/revoke', async (req, res) => {
    const s = await authed(req, res)
    if (!s) return
    if (flags(s.all).get(s.login) === MUST_CHANGE) return res.status(403).json({ error: 'Replace the deployment password before revoking anyone.' })
    const login = String(req.body?.login || '').trim().toLowerCase()
    if (!s.admins.has(login)) return res.status(404).json({ error: 'No such administrator.' })
    if (login === s.login) return res.status(400).json({ error: 'An administrator does not revoke the account that is signed in.' })
    if (s.admins.size < 2) return res.status(400).json({ error: 'The last administrator is not revoked from here.' })
    try {
      await append(SUBJECT + login, 'revoked')
    } catch (e) {
      return res.status(503).json({ error: `The resident refused: ${e.message}` })
    }
    res.json({ ok: true, message: `Administrator ${login} revoked; the revocation replicates.` })
  })
  api.all('/{*rest}', (req, res) => res.status(404).json({ error: 'No such call.' }))
  app.use('/admin/api', api)

  // --- the administration app: built files, and the page for every other GET under /admin
  if (existsSync(dist)) {
    app.use('/admin', express.static(dist, { index: false, redirect: false }))
    app.get(['/admin', '/admin/{*rest}'], (req, res) => res.sendFile(path.join(dist, 'index.html')))
  }
  // No form posts: nothing outside the API accepts a POST.
  app.post('/{*rest}', (req, res) => res.status(405).json({ error: 'No form posts here; the page calls /admin/api with JSON.' }))
  app.use((req, res) => closed(req, res))
  return app
}
