// Replication and takeover, as the console server's /api/replication route expects them.
export const INTERVALS = [60, 300, 900, 3600, 21600, 86400]
export const CAPTURES = ['live', 'stopped']

export function ago(seconds, t) {
  if (seconds == null) return '—'
  const s = Math.round(seconds)
  if (s < 60) return t('time.secondsAgo', { n: s })
  if (s < 3600) return t('time.minutesAgo', { n: Math.round(s / 60) })
  return t('time.hoursAgo', { n: (s / 3600).toFixed(1) })
}

export function duration(seconds, t) {
  if (seconds == null) return '—'
  const s = Math.round(seconds)
  if (s < 60) return t('time.seconds', { n: s })
  if (s < 3600) return t('time.minutes', { n: Math.round(s / 60) })
  return t('time.hours', { n: (s / 3600).toFixed(1) })
}

// configure: where it replicates, how often, and whether the capture stops the universe or not.
export function configureBody({ host, universe_uuid, authorization_ref, standbys, interval_seconds, capture }) {
  if (!authorization_ref?.trim()) throw Error('authorization')
  return { action: 'configure', host, universe_uuid, authorization_ref: authorization_ref.trim(), standbys: standbys === 'all' ? 'all' : Number(standbys), interval_seconds: Number(interval_seconds), capture }
}

// run / start / stop: the universe and the mandate, nothing else.
export function replicationActionBody({ action, host, universe_uuid, authorization_ref }) {
  if (!authorization_ref?.trim()) throw Error('authorization')
  return { action, host, universe_uuid, authorization_ref: authorization_ref.trim() }
}

// takeover: the universe on its active host, the standby to promote it on, and whether the operator
// chose it (planned: the active copy is stopped and released first) or lost the active host.
export function takeoverBody({ host, universe_uuid, authorization_ref, standby, planned }) {
  if (!authorization_ref?.trim()) throw Error('authorization')
  if (!standby) throw Error('takeover.standby')
  return { action: 'takeover', host, universe_uuid, authorization_ref: authorization_ref.trim(), standby, planned: !!planned }
}

// guard: continuity of service under the operator's mandate; bounds as the server checks them.
export function guardBody({ host, universe_uuid, authorization_ref, lease_seconds, takeover_margin_seconds, tick_seconds, keep_stale }) {
  if (!authorization_ref?.trim()) throw Error('authorization')
  const [lease, margin, tick] = [lease_seconds, takeover_margin_seconds, tick_seconds].map(Number)
  if (![lease, margin, tick].every(Number.isInteger) || lease < 5 || lease > 3600 || margin < 5 || margin > 3600 || tick < 2 || lease < 3 * tick) throw Error('continuity.bounds')
  return { action: 'guard', host, universe_uuid, authorization_ref: authorization_ref.trim(), lease_seconds: lease, takeover_margin_seconds: margin, tick_seconds: tick, keep_stale: !!keep_stale }
}

export function unguardBody({ host, universe_uuid, authorization_ref }) {
  if (!authorization_ref?.trim()) throw Error('authorization')
  return { action: 'unguard', host, universe_uuid, authorization_ref: authorization_ref.trim() }
}

// The standby row's host is an SSH target; the console names it by the candidate with that target.
export function standbyOf(row, candidates = []) {
  const c = candidates.find(x => x.ssh === row.host)
  return c ? { id: c.id, name: c.name, ssh: c.ssh } : { id: null, name: row.host, ssh: row.host }
}
