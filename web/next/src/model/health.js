// What the health view concludes from the figures, kept apart from the drawing so it can be tested.
//
// A resident pushes its snapshot to a peer that lacks it at once, and otherwise once per refresh, so an idle link's
// last success is legitimately as old as the refresh (ten minutes by default). Its liveness bound is the refresh, plus
// the longest backoff between two attempts, plus a margin for one exchange (three 2 s frame deadlines), a visit of
// the resident's other peers and one interval. A link is healthy while its last authenticated success is within that
// bound and no attempt failed after it. It is degraded as soon as an attempt fails after that success, while the
// success is within the bound: the resident has just said the peer did not answer, and waiting for the bound would
// hide a dead peer for up to one more backoff. It is failing past the bound, or when it never succeeded.
export const LINK_MARGIN_MS = 15_000
// Residents older than the exchange lot of 2026-09-17 report neither figure: they pushed at least once a minute, and
// a resident's backoff is at most five minutes.
export const LEGACY_REFRESH_MS = 60_000
export const LEGACY_MAX_BACKOFF_MS = 300_000
// The resident's exchange audit table is never compacted and every audit write re-verifies it: past
// a few megabytes each exchange nears the 2 s network deadline (measured 2026-09-16).
export const STORE_WARN_BYTES = 4 * 1024 * 1024

export function linkBoundMs(link) {
  return (link.refresh_ms ?? LEGACY_REFRESH_MS) + (link.max_backoff_ms ?? LEGACY_MAX_BACKOFF_MS) + LINK_MARGIN_MS
}

export function linkState(link) {
  const age = link.last_success_age_ms
  if (age == null || age > linkBoundMs(link)) return 'failing'
  // The last attempt is the success itself, unless one failed after it: an authenticated refusal is a failure too.
  const failedSince = link.outcome !== 'authenticated_import_receipt' || (link.last_attempt_age_ms != null && link.last_attempt_age_ms < age)
  return failedSince ? 'degraded' : 'healthy'
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
