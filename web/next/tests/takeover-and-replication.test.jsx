// The takeover modal and the replication panel send exactly what the console server's
// /api/replication route accepts, and show a refusal where the operator looks.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'

vi.mock('../src/api/console', () => ({ postReplication: vi.fn(), getReplication: vi.fn() }))
vi.mock('react-hot-toast', () => ({ default: { success: vi.fn(), error: vi.fn() } }))
const api = await import('../src/api/console')
const { default: TakeoverModal } = await import('../src/features/TakeoverModal')
const { default: ReplicationPanel } = await import('../src/features/ReplicationPanel')
const { takeoverBody, configureBody } = await import('../src/model/replication')

const U = '00000000-0000-4000-8000-000000000001'
const row = { key: 'lab-c:x', uuid: U, name: 'podmesh-demo', host: { id: 'lab-c', name: 'Lab C', allowActions: true } }
const standby = { id: 'lab-b', name: 'Lab B', ssh: 'lab@b' }
const { useSession } = await import('../src/store/useSessionStore')
const copy = { capture: 'live', generation: 7, age_seconds: 42, present_on_host: true }

beforeEach(() => { vi.clearAllMocks(); try { localStorage.setItem('podmesh.console.locale', 'en') } catch { /* jsdom */ } })
afterEach(cleanup)

const typeAuth = value => fireEvent.change(screen.getAllByLabelText('Authorization reference')[0], { target: { value } })

describe('takeover', () => {
  it('sends the planned switchover exactly as the contract says', async () => {
    api.postReplication.mockResolvedValue({ result: 'taken_over', to: 'lab@b', planned: true, capture: 'live', generation: 8, copy_age_seconds: 0, interruption_seconds: 4.16, waited_seconds: 0, promotion_seconds: 1.1 })
    const onDone = vi.fn()
    render(<TakeoverModal open row={row} standby={standby} copy={copy} onClose={() => {}} onDone={onDone} />)
    expect(screen.getByText(/comes back running, with its memory/)).toBeTruthy()
    typeAuth('  mandate-1 ')
    fireEvent.click(screen.getByRole('button', { name: 'Take over' }))
    await waitFor(() => expect(api.postReplication).toHaveBeenCalledTimes(1))
    expect(api.postReplication.mock.calls[0][0]).toEqual({ action: 'takeover', host: 'lab-c', universe_uuid: U, authorization_ref: 'mandate-1', standby: 'lab-b', planned: true })
    await waitFor(() => expect(onDone).toHaveBeenCalled())
    expect(screen.getByText('The universe runs on Lab B.')).toBeTruthy()
    expect(screen.getByText(/nothing lost · interrupted 4.16 s/)).toBeTruthy()
  })

  it('sends a lost-host takeover with planned false, and shows the refusal in place', async () => {
    api.postReplication.mockRejectedValue({ status: 409, result: 'refused', error: 'the active host is reachable and holds a live lease' })
    render(<TakeoverModal open row={row} standby={standby} copy={{ ...copy, capture: 'stopped' }} onClose={() => {}} />)
    expect(screen.getByText(/started afresh, without its memory/)).toBeTruthy()
    fireEvent.click(screen.getByRole('radio', { name: /Active host lost/ }))
    typeAuth('mandate-2')
    fireEvent.click(screen.getByRole('button', { name: 'Take over' }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('holds a live lease'))
    expect(api.postReplication.mock.calls[0][0].planned).toBe(false)
  })

  it('refuses without a mandate and sends nothing', async () => {
    render(<TakeoverModal open row={row} standby={standby} copy={copy} onClose={() => {}} />)
    fireEvent.click(screen.getByRole('button', { name: 'Take over' }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('authorization reference is required'))
    expect(api.postReplication).not.toHaveBeenCalled()
    expect(() => takeoverBody({ host: 'a', universe_uuid: U, authorization_ref: 'x', standby: null, planned: true })).toThrow('takeover.standby')
  })
})

describe('replication panel', () => {
  const status = {
    replication: { active: 'lab@c', standbys: ['lab@b'], mode: 'all', capture: 'live', interval_seconds: 300, chosen_because: 'every host but the active one' },
    schedule: { armed: false }, last_run: { at: 1, ok: true, capture: 'live', stopped_for_seconds: 1.1 },
    standbys: [{ host: 'lab@b', copy }], candidates: [standby],
  }
  it('says what the live mode does and what the last run interrupted, and sends capture on save', async () => {
    api.getReplication.mockResolvedValue(status)
    api.postReplication.mockResolvedValue({ result: 'configured' })
    render(<ReplicationPanel row={row} />)
    await waitFor(() => expect(screen.getByText(/never stopping it/)).toBeTruthy())
    expect(screen.getByText(/interrupted it for 1.1 s/)).toBeTruthy()
    expect(screen.getByRole('button', { name: /Take over here/ }).disabled).toBe(true)
    useSession.setState({ session: { token: 'x', hosts: [{ id: 'lab-b', name: 'Lab B', allowActions: true }] } })
    await waitFor(() => expect(screen.getByRole('button', { name: /Take over here/ }).disabled).toBe(false))
    typeAuth('mandate-3')
    fireEvent.click(screen.getByRole('button', { name: /Save target/ }))
    await waitFor(() => expect(api.postReplication).toHaveBeenCalled())
    expect(api.postReplication.mock.calls[0][0]).toEqual({ action: 'configure', host: 'lab-c', universe_uuid: U, authorization_ref: 'mandate-3', standbys: 'all', interval_seconds: 300, capture: 'live' })
    expect(configureBody({ host: 'h', universe_uuid: U, authorization_ref: 'm', standbys: '2', interval_seconds: '60', capture: 'stopped' })).toMatchObject({ standbys: 2, interval_seconds: 60, capture: 'stopped' })
  })
})

describe('takeover without a copy on the standby', () => {
  it('allows only the planned switchover, which takes the copy first', async () => {
    api.postReplication.mockResolvedValue({ result: 'taken_over', to: 'lab@b', capture: 'live', generation: 1, copy_age_seconds: 1, promotion_seconds: 1 })
    render(<TakeoverModal open row={row} standby={standby} copy={null} onClose={() => {}} />)
    expect(screen.getByText(/holds no copy of the universe yet/)).toBeTruthy()
    expect(screen.getByRole('radio', { name: /Active host lost/ }).disabled).toBe(true)
    typeAuth('mandate-4')
    fireEvent.click(screen.getByRole('button', { name: 'Take over' }))
    await waitFor(() => expect(api.postReplication).toHaveBeenCalled())
    expect(api.postReplication.mock.calls[0][0].planned).toBe(true)
  })
})

describe('continuity', () => {
  it('builds the guard body within the bounds the server checks', async () => {
    const { guardBody, unguardBody } = await import('../src/model/replication')
    expect(guardBody({ host: 'lab-c', universe_uuid: U, authorization_ref: ' m ', lease_seconds: '30', takeover_margin_seconds: '15', tick_seconds: '10', keep_stale: false }))
      .toEqual({ action: 'guard', host: 'lab-c', universe_uuid: U, authorization_ref: 'm', lease_seconds: 30, takeover_margin_seconds: 15, tick_seconds: 10, keep_stale: false })
    expect(() => guardBody({ host: 'h', universe_uuid: U, authorization_ref: 'm', lease_seconds: 20, takeover_margin_seconds: 15, tick_seconds: 10 })).toThrow('continuity.bounds')
    expect(unguardBody({ host: 'h', universe_uuid: U, authorization_ref: 'm' })).toEqual({ action: 'unguard', host: 'h', universe_uuid: U, authorization_ref: 'm' })
  })

  it('warns when a host has no self-fence, and shows the incidents', async () => {
    const { default: ContinuityPanel } = await import('../src/features/ContinuityPanel')
    const status = {
      replication: { interval_seconds: 60 },
      guard: { state: 'guarding', armed: true, lease_seconds: 30, takeover_margin_seconds: 15, tick_seconds: 10, order: ['lab@a', 'lab@b'], last_tick_age_seconds: 4, last_renewal_age_seconds: 4,
        incidents: [{ at: 1789600000, kind: 'lost_host_failover', from: 'lab@c', to: 'lab@a', copy_age_seconds: 24 }] },
      fence: [{ host: 'lab@c', mandate_present: true, timer_active: true }, { host: 'lab@a', mandate_present: false, timer_active: false }],
    }
    render(<ContinuityPanel row={row} status={status} />)
    expect(screen.getByText('guarded')).toBeTruthy()
    expect(screen.getByRole('alert').textContent).toContain('No self-fence on lab@a')
    expect(screen.getByText(/lab@c → lab@a/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Stop guarding' })).toBeTruthy()
  })
})
