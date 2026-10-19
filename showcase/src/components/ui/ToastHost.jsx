import { useEffect, useState } from 'react'
import { AlertCircle, CheckCircle2, X } from 'lucide-react'

let pushToastFn = null
let toastId = 0

/**
 * Imperative toast: toast('Saved'), toast('Failed', { type: 'error' }).
 * Mount <ToastHost /> once, in Layout. Every mutation gets a toast; never an alert.
 */
export function toast(message, { type = 'success', duration = 3200 } = {}) {
  if (typeof pushToastFn === 'function') pushToastFn({ id: ++toastId, message, type, duration })
}

export function showToast({ title, description, variant = 'success' } = {}) {
  const msg = description ? `${title} — ${description}` : (title || '')
  toast(msg, { type: variant === 'danger' ? 'error' : variant })
}

export function ToastHost() {
  const [items, setItems] = useState([])

  useEffect(() => {
    pushToastFn = (item) => {
      setItems((prev) => [...prev, item])
      window.setTimeout(() => setItems((prev) => prev.filter((t) => t.id !== item.id)), item.duration)
    }
    return () => {
      pushToastFn = null
    }
  }, [])

  if (!items.length) return null

  return (
    <div className="pointer-events-none fixed bottom-6 left-1/2 z-[80] flex w-[min(24rem,calc(100vw-2rem))] -translate-x-1/2 flex-col gap-2">
      {items.map((t) => (
        <div
          key={t.id}
          role="status"
          className={`pointer-events-auto flex items-start gap-2 rounded-xl border bg-surface px-3 py-2.5 text-sm shadow-lg ${
            t.type === 'error' ? 'border-danger/40 text-danger' : 'border-success/40 text-success'
          }`}
        >
          {t.type === 'error' ? <AlertCircle size={16} className="mt-0.5 shrink-0" /> : <CheckCircle2 size={16} className="mt-0.5 shrink-0" />}
          <p className="min-w-0 flex-1 leading-snug text-fg">{t.message}</p>
          <button
            type="button"
            className="rounded p-0.5 text-fg-faint hover:bg-surface-2"
            onClick={() => setItems((prev) => prev.filter((x) => x.id !== t.id))}
            aria-label="close"
          >
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  )
}
