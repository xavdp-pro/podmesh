// What the manager says about this visitor: the view to show and, once signed in, who and what.
// The session itself is an HttpOnly cookie the page never sees; this store holds only its echo.
import { useSyncExternalStore } from 'react'
import { createStore } from 'zustand/vanilla'
import { getState } from '../api/admin'

const store = createStore(set => ({
  state: null,          // {view, login, epoch, scope, administrators, ...identity}
  loading: true,
  failure: '',          // the manager unreachable
  flash: '',            // one line shown on the next view (a session that ended, a change made)
  refresh: async () => {
    set({ loading: true })
    try {
      const state = await getState()
      set({ state, loading: false, failure: '' })
      return state
    } catch (e) {
      set({ loading: false, failure: e?.error || 'unreachable' })
      return null
    }
  },
  setFlash: flash => set({ flash }),
  expired: () => set(s => ({ state: s.state ? { ...s.state, view: 'login' } : s.state, flash: 'sessionEnded' })),
}))

export function useSession(selector = s => s) {
  return useSyncExternalStore(store.subscribe, () => selector(store.getState()), () => selector(store.getInitialState()))
}
useSession.getState = store.getState
