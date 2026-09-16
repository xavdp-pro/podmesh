// The console's local session: its token and the hosts it is configured with. The token is read from
// /api/session at start and again whenever a call answers 401.
import { useSyncExternalStore } from 'react'
import { createStore } from 'zustand/vanilla'
import { getSession } from '../api/console'
import { setToken, setRenewHandler } from '../api'

const store = createStore(set => ({
  session: null,        // {token, mode, hosts:[{id,name,allowActions,canMove}]}
  failure: '',          // the console server unreachable
  refresh: async () => {
    try {
      const session = await getSession()
      setToken(session.token)
      set({ session, failure: '' })
      return session.token
    } catch (e) {
      set({ failure: e?.error || 'unreachable' })
      return null
    }
  },
}))
setRenewHandler(() => store.getState().refresh())

export function useSession(selector = s => s) {
  return useSyncExternalStore(store.subscribe, () => selector(store.getState()), () => selector(store.getInitialState()))
}
useSession.getState = store.getState
useSession.setState = store.setState
