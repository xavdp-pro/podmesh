import { useEffect } from 'react'

export default function useEscapeKey(handler, enabled = true) {
  useEffect(() => {
    if (!enabled) return undefined
    const onKey = e => { if (e.key === 'Escape') handler(e) }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [handler, enabled])
}
