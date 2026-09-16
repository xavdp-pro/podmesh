// What the gateway last collected from every host: identities, capabilities, inventories. Read every
// thirty seconds; older than a minute, it is stale and the page says so before any action.
import { useSyncExternalStore } from 'react'
import { createStore } from 'zustand/vanilla'
import { getSnapshot } from '../api/console'

export const STALE_AFTER_MS = 60000
export const POLL_MS = 30000

const store = createStore(set => ({
  snapshot: null,       // {receivedAt, hosts:[...]}
  error: '',
  busy: false,
  now: Date.now(),
  refresh: async () => {
    set({ busy: true })
    try {
      const snapshot = await getSnapshot()
      set({ snapshot, error: '', busy: false, now: Date.now() })
      return snapshot
    } catch (e) {
      set({ error: e?.error || 'unreachable', busy: false, now: Date.now() })
      return null
    }
  },
  tick: () => set({ now: Date.now() }),
}))

export function useSnapshot(selector = s => s) {
  return useSyncExternalStore(store.subscribe, () => selector(store.getState()), () => selector(store.getInitialState()))
}
useSnapshot.getState = store.getState
