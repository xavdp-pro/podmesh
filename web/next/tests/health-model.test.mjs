// The health view's link rule: an idle link is confirmed once per refresh, so it stays healthy up to the
// refresh plus the longest backoff plus a margin; a failed attempt after the last success degrades it at once.
import { describe, it, expect } from 'vitest'
import { LINK_MARGIN_MS, linkBoundMs, linkState, replicationSummary } from '../src/model/health'

const figures = { refresh_ms: 600_000, max_backoff_ms: 30_000 }
const bound = 600_000 + 30_000 + LINK_MARGIN_MS

describe('link health', () => {
  it('keeps an idle acknowledged link healthy within the liveness bound, and failing past it', () => {
    const idle = { ...figures, outcome: 'authenticated_import_receipt', acknowledged_unchanged: true }
    expect(linkBoundMs(idle)).toBe(bound)
    expect(linkState({ ...idle, last_success_age_ms: 540_000, last_attempt_age_ms: 540_000 })).toBe('healthy')
    expect(linkState({ ...idle, last_success_age_ms: bound, last_attempt_age_ms: bound })).toBe('healthy')
    expect(linkState({ ...idle, last_success_age_ms: bound + 1, last_attempt_age_ms: bound + 1 })).toBe('failing')
  })
  it('degrades a dead peer from its first failed attempt and fails it past the bound', () => {
    const dead = { ...figures, outcome: 'local_exchange_failure', acknowledged_unchanged: false }
    expect(linkState({ ...dead, last_success_age_ms: 601_000, last_attempt_age_ms: 500 })).toBe('degraded')
    expect(linkState({ ...dead, last_success_age_ms: bound + 1, last_attempt_age_ms: 20_000 })).toBe('failing')
    expect(linkState({ ...dead, last_success_age_ms: null, last_attempt_age_ms: 500 })).toBe('failing')
    expect(linkState({ ...figures, outcome: 'authenticated_remote_refusal', last_success_age_ms: 1_000, last_attempt_age_ms: 10 })).toBe('degraded')
  })
  it('judges a resident that reports neither figure on a one-minute refresh and a five-minute backoff', () => {
    expect(linkState({ outcome: 'authenticated_import_receipt', last_success_age_ms: 61_000 })).toBe('healthy')
    expect(linkState({ outcome: 'authenticated_import_receipt', last_success_age_ms: 376_000 })).toBe('failing')
    expect(linkState({ outcome: 'local_exchange_failure', last_success_age_ms: 5_000 })).toBe('degraded')
  })
  it('summarises the replication as its worst link', () => {
    const idle = { ...figures, outcome: 'authenticated_import_receipt', last_success_age_ms: 590_000, last_attempt_age_ms: 590_000 }
    const dead = { ...figures, outcome: 'local_exchange_failure', last_success_age_ms: 610_000, last_attempt_age_ms: 1_000 }
    expect(replicationSummary([{ name: 'a', managers: [{ links: [idle, idle] }] }]).state).toBe('healthy')
    expect(replicationSummary([{ name: 'a', managers: [{ links: [idle, dead] }] }]).state).toBe('degraded')
  })
})
