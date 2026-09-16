// What the Universes view concludes from the snapshot, kept apart from the drawing so it can be tested.
export const short = s => (s ? String(s).slice(0, 8) : '—')

// Every container of every host, with the universe identity its label declares; the runtime
// verifies ownership on each request, the label only names a candidate.
export function universeRows(hosts = []) {
  return hosts.flatMap(h => (h.responses?.inventory?.data?.containers || []).map(c => ({
    key: `${h.id}:${c.Id}`,
    host: { id: h.id, name: h.name, allowActions: !!h.allowActions, canMove: !!h.canMove },
    containerId: c.Id,
    uuid: c.Labels?.['io.podmesh.universe'] || null,
    name: c.Names?.[0] || short(c.Id),
    state: c.State || 'unknown',
    image: c.Image || '',
    raw: c,
  })))
}

export function hostReach(host) {
  const errors = Object.values(host?.errors || {})
  return { ok: errors.length === 0, errors }
}

export function hostSummary(hosts = []) {
  const reach = hosts.map(hostReach)
  return { total: hosts.length, responding: reach.filter(r => r.ok).length }
}

export function matches(row, query) {
  const q = (query || '').trim().toLowerCase()
  if (!q) return true
  return `${row.name} ${row.image} ${row.uuid || ''} ${row.host.name}`.toLowerCase().includes(q)
}

// The lifecycle actions the console offers from the drawer, with the state each applies to and
// whether it is destructive (drawn in terracotta, confirmed as such).
export const ACTIONS = [
  { op: 'start', applies: s => s !== 'running' && s !== 'paused', danger: false },
  { op: 'pause', applies: s => s === 'running', danger: false },
  { op: 'resume', applies: s => s === 'paused', danger: false },
  { op: 'stop', applies: s => s === 'running' || s === 'paused', danger: true },
  { op: 'resources', applies: () => true, danger: false },
  { op: 'move', applies: s => s === 'running', danger: true },
]

// The body POST /api/hosts/:id/actions expects for one action, built from what the operator typed.
// Throws a message when the fields do not make a request the server would accept.
export function actionBody({ op, universe_uuid, operation_id, authorization_ref, memory_mib = '', cpus = '' }) {
  if (!authorization_ref?.trim()) throw Error('authorization')
  const body = { operation: op, universe_uuid, operation_id, authorization_ref: authorization_ref.trim() }
  if (op === 'stop') { body.timeout_seconds = 10; body.on_timeout = 'leave_running' }
  if (op === 'resources') {
    const mib = memory_mib === '' ? undefined : Number(memory_mib)
    const cores = cpus === '' ? undefined : Number(cpus)
    if (mib === undefined && cores === undefined) throw Error('resources.none')
    if (mib !== undefined && (!Number.isInteger(mib) || mib < 32)) throw Error('resources.memory')
    if (cores !== undefined && (!(cores >= 0.1) || cores > 1024)) throw Error('resources.cpus')
    if (mib !== undefined) body.memory_bytes = mib * 1024 * 1024
    if (cores !== undefined) body.cpus = Math.round(cores * 100) / 100
  }
  return body
}

// The body POST /api/moves expects.
export function moveBody({ universe_uuid, source, destination, authorization_ref, keep_source }) {
  if (!authorization_ref?.trim()) throw Error('authorization')
  if (!destination) throw Error('move.destination')
  return { universe_uuid, source, destination, authorization_ref: authorization_ref.trim(), keep_source: !!keep_source }
}
