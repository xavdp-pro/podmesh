import { useEffect } from 'react'
import { useAuth } from '../auth/AuthProvider'

let socket = null

/** The shared Socket.IO client, or null when realtime is off or the user is signed out. */
export function getSocket() {
  return socket
}

/**
 * Connects when the API reports REALTIME_ENABLED and a user is signed in.
 * socket.io-client is loaded on demand so an app that never enables realtime
 * ships nothing for it. Subscribe with getSocket()?.on('event', handler).
 */
export function useRealtime(enabled) {
  const { user } = useAuth()

  useEffect(() => {
    if (!enabled || !user) return undefined
    let cancelled = false
    import('socket.io-client').then(({ io }) => {
      if (cancelled) return
      socket = io({ path: '/socket.io', withCredentials: true })
    })
    return () => {
      cancelled = true
      socket?.disconnect()
      socket = null
    }
  }, [enabled, user])
}
