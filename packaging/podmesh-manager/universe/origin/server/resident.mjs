// The resident behind the origin: its facts through its own read-only inspection, its control
// socket for one typed observation, and the password hashing the store already holds.
import { execFile } from 'node:child_process'
import { readFileSync } from 'node:fs'
import net from 'node:net'
import { randomBytes, scryptSync, timingSafeEqual } from 'node:crypto'

export const SCRYPT = { N: 16384, r: 8, p: 1, len: 32 }

export function readConfig(path) {
  const cfg = JSON.parse(readFileSync(path, 'utf8'))
  const identity = { logical_manager_id: cfg.network.manager.logical_manager_id, replica_id: cfg.network.replica_id }
  const grant = (cfg.network.manager.grants || []).find(g => g.owner_replica_id === identity.replica_id)
  if (!grant) throw new Error('this replica owns no scope in its configuration')
  return { identity, scope: grant.scope, control: cfg.control_socket }
}

// The governor mark: PodMesh's root-only file, present only while this replica holds the role.
export const markReader = path => () => {
  try {
    const mark = JSON.parse(readFileSync(path, 'utf8'))
    return mark && typeof mark === 'object' ? mark : null
  } catch {
    return null
  }
}

// The ordered facts, read by the resident's own read-only inspection (never by opening the store).
export const factsReader = ({ binary, config, state }) => () => new Promise((resolve, reject) => {
  execFile(binary, ['--inspect-store', '--config', config, '--state-dir', state], { timeout: 20000, maxBuffer: 64 << 20 }, (err, stdout) => {
    if (err) return reject(new Error('the store could not be inspected'))
    try {
      resolve(JSON.parse(stdout).ordered_facts || [])
    } catch {
      reject(new Error('the store could not be inspected'))
    }
  })
})

function once(control, request) {
  return new Promise((resolve, reject) => {
    const chunks = []
    const s = net.createConnection(control)
    s.setTimeout(20000, () => { s.destroy(new Error('the resident did not answer in time')) })
    s.on('connect', () => { s.write(request); s.end() })
    s.on('data', c => chunks.push(c))
    s.on('error', reject)
    s.on('close', () => resolve(Buffer.concat(chunks).toString('utf8').trim()))
  })
}

// One typed observation in this replica's scope. The resident answers within a 250 ms control
// deadline (its design): `observed` when the store committed in time, `append_observation_uncertain`
// or `_busy` when it could not say -- and measured on 2026-09-16 the store then holds the fact
// anyway, more often than not. Uncertain is therefore not failed: the same operation ID is retried
// (the resident refuses an ID it already has, so the fact lands once), and when every answer stays
// uncertain the store itself is read back for that exact subject and value. Only a fact absent from
// the store after that is a refusal.
export const observer = ({ control, scope, facts }) => async (subject, value) => {
  const operation_id = randomBytes(16).toString('hex')
  const request = JSON.stringify({ operation: 'append_observation', operation_id, scope, subject, value })
  let last = ''
  for (let attempt = 0; attempt < 4; attempt++) {
    last = await once(control, request)
    if (last.replace(/\s/g, '').includes('"result":"observed"')) return 'observed'
    if (!/append_observation_(uncertain|busy)/.test(last)) break
    await new Promise(r => setTimeout(r, 400))
  }
  if (facts && /append_observation_(uncertain|busy)/.test(last)) {
    for (let attempt = 0; attempt < 5; attempt++) {
      await new Promise(r => setTimeout(r, 300))
      try {
        if ((await facts()).some(f => f.subject === subject && f.value === value)) return 'observed after an uncertain answer, read back from the store'
      } catch { /* the store not readable this instant; try again */ }
    }
  }
  throw new Error(`the resident did not observe it: ${last.slice(0, 200)}`)
}

export function hashPassword(password, salt = randomBytes(16)) {
  const digest = scryptSync(password, salt, SCRYPT.len, { N: SCRYPT.N, r: SCRYPT.r, p: SCRYPT.p, maxmem: 64 << 20 })
  return `scrypt.${SCRYPT.N}.${SCRYPT.r}.${SCRYPT.p}.${salt.toString('hex')}.${digest.toString('hex')}`
}

export function verifyPassword(password, stored) {
  try {
    const [kind, N, r, p, salt, digest] = String(stored).split('.')
    if (kind !== 'scrypt') return false
    const want = scryptSync(password, Buffer.from(salt, 'hex'), digest.length / 2, { N: +N, r: +r, p: +p, maxmem: 64 << 20 })
    return timingSafeEqual(want, Buffer.from(digest, 'hex'))
  } catch {
    return false
  }
}
