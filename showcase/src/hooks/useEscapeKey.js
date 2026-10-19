import { useEffect } from 'react'

/** Calls `handler` on Escape while `enabled`. */
export default function useEscapeKey(handler, enabled = true) {
  useEffect(() => {
    if (!enabled || !handler) return undefined
    function onKeyDown(event) {
      if (event.key === 'Escape') handler(event)
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [handler, enabled])
}
