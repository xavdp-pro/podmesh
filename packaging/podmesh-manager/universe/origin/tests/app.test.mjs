// Every gate of the origin's administration API, against stubs: a fact list the test sets, an
// append that records what it was asked, and a mark the test places or removes.
import { describe, it, expect, beforeEach } from 'vitest'
import request from 'supertest'
import { createApp, administrators } from '../server/app.mjs'
import { factsReader, hashPassword, markReader, observer, storeState } from '../server/resident.mjs'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'
import fs from 'node:fs'

const identity = { logical_manager_id: 'logical', replica_id: 'replica-one' }
const SCOPE = 'm-u2/lab-a/observations'
const OTHER = 'm-u2/lab-b/observations'
let mark, facts, appended, app, storeClosed
const stored = hashPassword('podmesh', Buffer.from('00112233445566778899aabbccddeeff', 'hex'))
const fact = (subject, value, revision = 1, scope = SCOPE) => ({ scope, subject, value, subject_revision: revision })
const cookieOf = res => (res.headers['set-cookie'] || [])[0] || ''

beforeEach(() => {
  mark = { epoch: 7, marked_at: 1 }
  facts = []
  appended = []
  storeClosed = null
  app = createApp({
    identity, scope: SCOPE, dist: '/nonexistent',
    activeManager: () => mark,
    facts: async () => facts,
    append: async (subject, value) => { appended.push({ subject, value }); return 'observed' },
    storeState: async () => (storeClosed ? { closed: true, reason: storeClosed } : { closed: false }),
    secret: Buffer.alloc(32, 7),
  })
})

async function signIn(login = 'admin', password = 'podmesh') {
  const res = await request(app).post('/admin/api/login').send({ login, password })
  return cookieOf(res).split(';')[0]
}

describe('fail-closed', () => {
  it('answers 503 on every path without the active manager\'s mark, the API included', async () => {
    mark = null
    for (const p of ['/', '/ready', '/admin', '/admin/api/state']) {
      const res = await request(app).get(p)
      expect(res.status).toBe(503)
      expect(res.body.reason).toBe('not the active manager')
    }
    expect((await request(app).post('/admin/api/login').send({ login: 'x', password: 'y' })).status).toBe(503)
  })
  it('answers the contract JSON at /ready with the mark', async () => {
    const res = await request(app).get('/ready')
    expect(res.status).toBe(200)
    expect(res.body).toEqual({ ready: true, ...identity, epoch: 7, marked_at: 1 })
  })
  it('closes again when the mark goes mid-session', async () => {
    facts = [fact('admin.user.admin', stored)]
    const cookie = await signIn()
    mark = null
    expect((await request(app).get('/admin/api/state').set('Cookie', cookie)).status).toBe(503)
  })
})

describe('fail-closed with the store', () => {
  it('refuses administration with a named reason while the resident reports its store closed', async () => {
    storeClosed = 'corrupt: stored audit is invalid'
    for (const p of ['/admin', '/admin/', '/admin/api/state']) {
      const res = await request(app).get(p)
      expect(res.status, p).toBe(503)
      expect(res.body.reason).toBe('the manager store is closed')
      expect(res.body.detail).toBe('corrupt: stored audit is invalid')
      expect(res.body.replica_id).toBe(identity.replica_id)
    }
    expect((await request(app).post('/admin/api/login').send({ login: 'admin', password: 'podmesh' })).status).toBe(503)
  })
  it('keeps serving what the mark published: the page and the contract JSON', async () => {
    storeClosed = 'corrupt: stored fact hash mismatch'
    expect((await request(app).get('/')).status).toBe(200)
    expect((await request(app).get('/ready')).body).toEqual({ ready: true, ...identity, epoch: 7, marked_at: 1 })
  })
  it('closes mid-session and writes nothing while the store is closed', async () => {
    facts = [fact('admin.user.admin', stored), fact('admin.flag.admin', 'changed')]
    const cookie = await signIn()
    expect((await request(app).get('/admin/api/state').set('Cookie', cookie)).body.view).toBe('admin')
    storeClosed = 'corrupt: stored audit is invalid'
    const res = await request(app).post('/admin/api/users').set('Cookie', cookie).send({ login: 'second', password: 'a-good-new-password' })
    expect(res.status).toBe(503)
    expect(res.body.reason).toBe('the manager store is closed')
    expect(appended).toEqual([])
  })
  it('refuses administration when the resident does not answer, and when the origin was built without a store state', async () => {
    const silent = createApp({
      identity, scope: SCOPE, dist: '/nonexistent',
      activeManager: () => mark, facts: async () => facts, append: async () => 'observed',
      storeState: async () => { throw new Error('connect ENOENT') },
      secret: Buffer.alloc(32, 7),
    })
    let res = await request(silent).get('/admin/api/state')
    expect(res.status).toBe(503)
    expect(res.body.reason).toBe('the manager store is closed')
    expect(res.body.detail).toBe('the resident did not answer')
    const unbuilt = createApp({
      identity, scope: SCOPE, dist: '/nonexistent',
      activeManager: () => mark, facts: async () => facts, append: async () => 'observed',
      secret: Buffer.alloc(32, 7),
    })
    res = await request(unbuilt).get('/admin/api/state')
    expect(res.status).toBe(503)
    expect(res.body.detail).toBe('the origin was started without a store state')
  })
})

describe("the resident's store state", () => {
  async function control(answer) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'podmesh-state-'))
    const sock = path.join(dir, 'control.sock')
    const server = net.createServer(s => { s.on('data', () => {}); s.on('end', () => { if (answer === null) return; s.end(answer) }) })
    await new Promise(r => server.listen(sock, r))
    return { sock, close: () => server.close() }
  }
  it('is read from the resident status: open, closed with its reason, unreported, unanswered', async () => {
    const open = await control(JSON.stringify({ kind: 'resident_observation', store_closed: false, store_closed_reason: null }))
    expect(await storeState({ control: open.sock })()).toEqual({ closed: false })
    open.close()
    const shut = await control(JSON.stringify({ store_closed: true, store_closed_reason: 'corrupt: stored fact hash mismatch' }))
    expect(await storeState({ control: shut.sock })()).toEqual({ closed: true, reason: 'corrupt: stored fact hash mismatch' })
    shut.close()
    const unreported = await control('{"error":"status_unavailable"}')
    expect(await storeState({ control: unreported.sock })()).toEqual({ closed: true, reason: 'the resident did not report the state of its store' })
    unreported.close()
    const silent = await control(null)
    expect(await storeState({ control: silent.sock, timeout: 300 })()).toEqual({ closed: true, reason: 'the resident did not answer' })
    silent.close()
    expect(await storeState({ control: path.join(os.tmpdir(), 'podmesh-no-such-socket'), timeout: 300 })()).toEqual({ closed: true, reason: 'the resident did not answer' })
  }, 20000)
})

describe('no form posts', () => {
  it('sets a policy that forbids submitting a form and admits no inline script', async () => {
    const csp = (await request(app).get('/ready')).headers['content-security-policy']
    expect(csp).toContain("form-action 'none'")
    expect(csp).toMatch(/script-src 'self'(;|$)/)
    expect(csp).not.toMatch(/script-src[^;]*unsafe-inline/)
  })
  it('refuses a urlencoded body on the API (415) and any post outside it (405)', async () => {
    facts = [fact('admin.user.admin', stored)]
    expect((await request(app).post('/admin/api/login').type('form').send('login=admin&password=podmesh')).status).toBe(415)
    expect((await request(app).post('/admin/login').type('form').send('login=admin&password=podmesh')).status).toBe(405)
    expect(appended).toEqual([])
  })
  it('refuses a body that is not JSON or is too large', async () => {
    expect((await request(app).post('/admin/api/login').set('Content-Type', 'application/json').send('{not json')).status).toBe(400)
    expect((await request(app).post('/admin/api/login').send({ login: 'x'.repeat(5000) })).status).toBe(400)
  })
})

describe('where administrators come from', () => {
  it('with none on record: the state says so and a creation without a session appends nothing', async () => {
    expect((await request(app).get('/admin/api/state')).body).toEqual({ view: 'none', ...identity })
    expect((await request(app).post('/admin/api/users').send({ login: 'intruder', password: 'correcthorsebattery' })).status).toBe(401)
    expect(appended).toEqual([])
  })
  it('with one on record: the state asks to sign in and names nobody', async () => {
    facts = [fact('admin.user.admin', stored)]
    const res = await request(app).get('/admin/api/state')
    expect(res.body).toEqual({ view: 'login', ...identity })
    expect(JSON.stringify(res.body)).not.toContain('admin.user')
  })
  it('a revoked login is no administrator; the highest revision of a scope decides', () => {
    const admins = administrators([fact('admin.user.x', stored, 1), fact('admin.user.x', 'revoked', 2), fact('admin.user.y', 'revoked', 1), fact('admin.user.y', stored, 2)])
    expect([...admins.keys()]).toEqual(['y'])
  })
})

describe('signing in', () => {
  beforeEach(() => { facts = [fact('admin.user.admin', stored)] })
  it('refuses a wrong password with its reason and opens no session', async () => {
    const res = await request(app).post('/admin/api/login').send({ login: 'admin', password: 'wrong-password' })
    expect(res.status).toBe(401)
    expect(res.body.error).toBe('Wrong administrator or password.')
    expect(cookieOf(res)).toBe('')
  })
  it('opens a session on the right one, the login trimmed and lower-cased, the cookie HttpOnly, Secure, SameSite=Strict, on /admin', async () => {
    const res = await request(app).post('/admin/api/login').send({ login: ' Admin ', password: 'podmesh' })
    expect(res.status).toBe(200)
    const cookie = cookieOf(res)
    expect(cookie).toMatch(/^podmesh_admin=\S+; Path=\/admin; Max-Age=3600; HttpOnly; SameSite=Strict; Secure$/)
    const state = await request(app).get('/admin/api/state').set('Cookie', cookie.split(';')[0])
    expect(state.body.view).toBe('admin')
    expect(state.body.administrators).toEqual([{ login: 'admin', scopes: [SCOPE], conflict: false }])
    expect(state.body.epoch).toBe(7)
  })
  it('closes a login for a minute after five failures', async () => {
    for (let i = 0; i < 5; i++) expect((await request(app).post('/admin/api/login').send({ login: 'admin', password: 'no' })).status).toBe(401)
    expect((await request(app).post('/admin/api/login').send({ login: 'admin', password: 'podmesh' })).status).toBe(429)
    expect((await request(app).post('/admin/api/login').send({ login: 'other', password: 'podmesh' })).status).toBe(401)
  })
  it('a session survives no forged cookie and no revoked account', async () => {
    expect((await request(app).get('/admin/api/state').set('Cookie', 'podmesh_admin=forged')).body.view).toBe('login')
    const cookie = await signIn()
    facts = [fact('admin.user.admin', stored, 1), fact('admin.user.admin', 'revoked', 2), fact('admin.user.other', stored)]
    expect((await request(app).get('/admin/api/state').set('Cookie', cookie)).body.view).toBe('login')
  })
  it('signing out clears the cookie', async () => {
    const cookie = await signIn()
    const res = await request(app).post('/admin/api/logout').set('Cookie', cookie).send({})
    expect(cookieOf(res)).toMatch(/^podmesh_admin=; Path=\/admin; Max-Age=0;/)
  })
})

describe('creating an administrator', () => {
  let cookie
  beforeEach(async () => { facts = [fact('admin.user.admin', stored)]; cookie = await signIn() })
  it('refuses a bad login, a short password, a password equal to its login and a login already taken, appending nothing', async () => {
    for (const [body, fragment] of [
      [{ login: 'UPPER CASE', password: 'correcthorsebattery' }, 'A login is 1 to 64'],
      [{ login: 'x', password: 'short' }, 'at least 12'],
      [{ login: 'repeatrepeats', password: 'repeatrepeats' }, 'not a password'],
      [{ login: 'admin', password: 'correcthorsebattery' }, 'already exists'],
    ]) {
      const res = await request(app).post('/admin/api/users').set('Cookie', cookie).send(body)
      expect(res.status, JSON.stringify(body)).toBe(400)
      expect(res.body.error).toContain(fragment)
    }
    expect(appended).toEqual([])
  })
  it("writes one observation in this replica's scope, the password hashed and absent from it", async () => {
    const res = await request(app).post('/admin/api/users').set('Cookie', cookie).send({ login: 'second', password: 'another-good-password' })
    expect(res.status).toBe(200)
    expect(appended).toHaveLength(1)
    expect(appended[0].subject).toBe('admin.user.second')
    expect(appended[0].value).toMatch(/^scrypt\.16384\.8\.1\.[0-9a-f]{32}\.[0-9a-f]{64}$/)
    expect(appended[0].value.length).toBeLessThanOrEqual(128)
    expect(JSON.stringify(appended)).not.toContain('another-good-password')
  })
  it('shows a login written in two scopes as a conflict', async () => {
    facts = [fact('admin.user.admin', stored), fact('admin.user.admin', stored, 1, OTHER)]
    const state = await request(app).get('/admin/api/state').set('Cookie', cookie)
    expect(state.body.administrators[0].conflict).toBe(true)
  })
  it('revokes another administrator, never the signed-in one nor the last one', async () => {
    facts = [fact('admin.user.admin', stored), fact('admin.user.second', stored)]
    expect((await request(app).post('/admin/api/revoke').set('Cookie', cookie).send({ login: 'admin' })).status).toBe(400)
    expect((await request(app).post('/admin/api/revoke').set('Cookie', cookie).send({ login: 'nobody' })).status).toBe(404)
    expect((await request(app).post('/admin/api/revoke').set('Cookie', cookie).send({ login: 'second' })).status).toBe(200)
    expect(appended).toEqual([{ subject: 'admin.user.second', value: 'revoked' }])
    facts = [fact('admin.user.admin', stored)]
    expect((await request(app).post('/admin/api/revoke').set('Cookie', cookie).send({ login: 'admin' })).status).toBe(400)
  })
})

describe('the deployment password', () => {
  let cookie
  beforeEach(async () => { facts = [fact('admin.user.admin', stored), fact('admin.flag.admin', 'must_change')]; cookie = await signIn() })
  it('opens only the change view and lets the account name or revoke nobody', async () => {
    expect((await request(app).get('/admin/api/state').set('Cookie', cookie)).body).toEqual({ view: 'change', login: 'admin', ...identity })
    const res = await request(app).post('/admin/api/users').set('Cookie', cookie).send({ login: 'third', password: 'another-good-password' })
    expect(res.status).toBe(403)
    expect((await request(app).post('/admin/api/revoke').set('Cookie', cookie).send({ login: 'x' })).status).toBe(403)
    expect(appended).toEqual([])
  })
  it('refuses a wrong current password, differing entries, a short one and the old one again', async () => {
    for (const [body, fragment] of [
      [{ current: 'wrong', next: 'a-good-new-password', again: 'a-good-new-password' }, 'current password is wrong'],
      [{ current: 'podmesh', next: 'a-good-new-password', again: 'different-one' }, 'two new entries differ'],
      [{ current: 'podmesh', next: 'short', again: 'short' }, 'at least 12'],
      [{ current: 'podmesh', next: 'podmesh', again: 'podmesh' }, 'at least 12'],
    ]) {
      const res = await request(app).post('/admin/api/password').set('Cookie', cookie).send(body)
      expect(res.status, JSON.stringify(body)).toBe(400)
      if (fragment) expect(res.body.error).toContain(fragment)
    }
    expect(appended).toEqual([])
  })
  it('the change writes the new hash and clears the flag, the new password absent from what is written', async () => {
    const res = await request(app).post('/admin/api/password').set('Cookie', cookie).send({ current: 'podmesh', next: 'a-good-new-password', again: 'a-good-new-password' })
    expect(res.status).toBe(200)
    expect(appended.map(a => a.subject)).toEqual(['admin.user.admin', 'admin.flag.admin'])
    expect(appended[1].value).toBe('changed')
    expect(JSON.stringify(appended)).not.toContain('a-good-new-password')
  })
})


describe('an uncertain answer from the resident', () => {
  async function control(answer, seen) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'podmesh-origin-'))
    const sock = path.join(dir, 'control.sock')
    const server = net.createServer(s => { let buf = ''; s.on('data', d => { buf += d }); s.on('end', () => { seen.push(JSON.parse(buf)); s.end(answer) }) })
    await new Promise(r => server.listen(sock, r))
    return { sock, close: () => server.close() }
  }
  it('is read back from the store, and a fact that landed is a success written once', async () => {
    const seen = []
    const { sock, close } = await control('{"error":"append_observation_uncertain"}', seen)
    const store = []
    const append = observer({ control: sock, scope: SCOPE, facts: async () => { if (seen.length >= 2 && !store.length) store.push({ subject: 'admin.user.x', value: 'v' }); return store } })
    const answer = await append('admin.user.x', 'v')
    expect(answer).toContain('read back')
    expect(new Set(seen.map(s => s.operation_id)).size).toBe(1)
    close()
  }, 20000)
  it('is a refusal when the fact never lands', async () => {
    const seen = []
    const { sock, close } = await control('{"error":"append_observation_uncertain"}', seen)
    const append = observer({ control: sock, scope: SCOPE, facts: async () => [] })
    await expect(append('admin.user.y', 'v')).rejects.toThrow(/did not observe/)
    close()
  }, 20000)
})

describe("the active manager's mark on disk", () => {
  const dir = () => fs.mkdtempSync(path.join(os.tmpdir(), 'podmesh-mark-'))
  it('is read from the first path that holds one, the previous path only when the new one is absent', () => {
    const d = dir()
    const renamed = path.join(d, 'active-manager.json'), previous = path.join(d, 'governor.json')
    const read = markReader([renamed, previous])
    expect(read()).toBeNull()
    fs.writeFileSync(previous, JSON.stringify({ epoch: 153, marked_at: 1 }))
    expect(read()).toEqual({ epoch: 153, marked_at: 1 })
    fs.writeFileSync(renamed, JSON.stringify({ epoch: 154, marked_at: 2 }))
    expect(read()).toEqual({ epoch: 154, marked_at: 2 })
    fs.rmSync(renamed); fs.rmSync(previous)
    expect(read()).toBeNull()
  })
  it('stays closed on a mark that is not a JSON object', () => {
    const d = dir()
    const file = path.join(d, 'active-manager.json')
    fs.writeFileSync(file, 'not json')
    expect(markReader([file])()).toBeNull()
    fs.writeFileSync(file, '7')
    expect(markReader([file])()).toBeNull()
    fs.writeFileSync(file, '[]')
    expect(markReader([file])()).toBeNull()
  })
})

describe("the store's facts", () => {
  it('are read through the facts-only inspection, and a failed or unreadable inspection is an error', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'podmesh-facts-'))
    const args = path.join(dir, 'arguments')
    const binary = path.join(dir, 'podmesh-managerd')
    const answer = JSON.stringify({ history_count: 1, ordered_facts: [fact('admin.user.admin', stored)], logical_history_sha256: '0'.repeat(64) })
    fs.writeFileSync(binary, `#!/bin/sh\nprintf '%s\\n' "$@" > '${args}'\nprintf '%s' '${answer}'\n`, { mode: 0o755 })
    const read = factsReader({ binary, config: '/etc/podmesh-manager/config.json', state: '/var/lib/podmesh-manager' })
    expect(await read()).toEqual([fact('admin.user.admin', stored)])
    expect(fs.readFileSync(args, 'utf8').trim().split('\n')).toEqual(['--inspect-store', '--facts-only', '--config', '/etc/podmesh-manager/config.json', '--state-dir', '/var/lib/podmesh-manager'])
    fs.writeFileSync(binary, '#!/bin/sh\necho "corrupt: stored fact hash mismatch" >&2\nexit 1\n')
    await expect(read()).rejects.toThrow('the store could not be inspected')
    fs.writeFileSync(binary, '#!/bin/sh\nprintf "not json"\n')
    await expect(read()).rejects.toThrow('the store could not be inspected')
  })
})
