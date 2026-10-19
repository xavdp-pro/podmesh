import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react'
import { api, UNAUTHORIZED_EVENT } from '../api/client'

const AuthContext = createContext(null)

/**
 * Holds the session: `user` is undefined while loading, null when signed out,
 * an object when signed in. `reason` explains a forced sign-out to the login page.
 */
export function AuthProvider({ children }) {
  const [user, setUser] = useState(undefined)
  const [reason, setReason] = useState('')

  const refresh = useCallback(async () => {
    try {
      const data = await api.me()
      setUser(data.user)
      setReason('')
      return data.user
    } catch {
      setUser(null)
      return null
    }
  }, [])

  useEffect(() => {
    refresh()
  }, [refresh])

  useEffect(() => {
    function onUnauthorized(event) {
      setReason(event.detail?.reason || 'auth.required')
      setUser(null)
    }
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized)
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized)
  }, [])

  const logout = useCallback(async () => {
    try {
      await api.logout()
    } catch {
      // the cookie is gone either way
    }
    setUser(null)
    setReason('')
  }, [])

  const value = useMemo(() => ({ user, reason, refresh, logout, clearReason: () => setReason('') }), [user, reason, refresh, logout])
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>
}

export function useAuth() {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth must be used inside <AuthProvider>')
  return ctx
}
