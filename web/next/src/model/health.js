// What the health view concludes from the figures, kept apart from the drawing so it can be tested.
export const LINK_HEALTHY_MS = 60_000
export const LINK_DEGRADED_MS = 300_000
// The resident's exchange audit table is never compacted and every audit write re-verifies it: past
// a few megabytes each exchange nears the 2 s network deadline (measured 2026-09-16).
export const STORE_WARN_BYTES = 4 * 1024 * 1024

export function linkState(link) {
  const age = link.last_success_age_ms
  if (link.outcome && link.outcome.startsWith('authenticated') && age != null && age < LINK_HEALTHY_MS) return 'healthy'
  if (age != null && age < LINK_DEGRADED_MS) return 'degraded'
  return 'failing'
}

export function replicationSummary(hosts) {
  const managers = hosts.flatMap(h => (h.managers || []).map(m => ({ ...m, host: h.name })))
  const links = managers.flatMap(m => (m.links || []).map(l => ({ ...l, host: m.host, state: linkState(l) })))
  const count = s => links.filter(l => l.state === s).length
  const largest = Math.max(0, ...managers.map(m => m.store_bytes || 0))
  const state = !managers.length ? 'absent' : managers.some(m => m.error) || count('failing') ? 'failing' : count('degraded') ? 'degraded' : 'healthy'
  return { state, replicas: managers.length, links: links.length, healthy: count('healthy'), degraded: count('degraded'), failing: count('failing'), largestStoreBytes: largest, storeWarning: largest >= STORE_WARN_BYTES }
}

export function ratio(used, total) {
  if (!total || used == null) return null
  return Math.max(0, Math.min(1, used / total))
}

export function bytes(n) {
  if (n == null) return '—'
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let i = 0; let v = n
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++ }
  return `${v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`
}
