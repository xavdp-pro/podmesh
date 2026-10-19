import { useEffect, useState } from 'react'
import { AlertTriangle, X } from 'lucide-react'
import useEscapeKey from '../../hooks/useEscapeKey'
import { useI18n } from '../../i18n/I18nProvider'

/**
 * The replacement for window.confirm. Optional `confirmationText` asks the user
 * to type a word (e.g. "DELETE") before the destructive button enables.
 */
export default function ConfirmModal({
  open = true,
  title,
  message,
  confirmLabel,
  danger = true,
  confirmationText,
  pending = false,
  onConfirm,
  onCancel,
}) {
  const { t } = useI18n()
  const [typed, setTyped] = useState('')
  const valid = !confirmationText || typed.trim().toUpperCase() === String(confirmationText).toUpperCase()

  useEscapeKey(onCancel, open && !pending)

  useEffect(() => {
    if (!open) setTyped('')
  }, [open])

  if (!open) return null

  function cancel() {
    if (!pending) onCancel?.()
  }

  const confirmClass = danger
    ? 'bg-danger text-white hover:opacity-90'
    : 'bg-accent text-accent-fg hover:bg-accent-hover'

  return (
    <div className="fixed inset-0 z-[90] flex items-center justify-center bg-black/45 p-4" onClick={cancel} role="presentation">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-modal-title"
        className="w-full max-w-md rounded-xl border border-line bg-surface shadow-xl"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-start justify-between gap-3 px-5 pt-5">
          <div className="flex min-w-0 items-start gap-2">
            {danger && <AlertTriangle size={18} className="mt-0.5 shrink-0 text-danger" />}
            <div className="min-w-0">
              <h2 id="confirm-modal-title" className="text-base font-semibold text-fg">{title}</h2>
              {confirmationText && <p className="mt-0.5 text-xs text-fg-muted">{t('common.irreversible')}</p>}
            </div>
          </div>
          <button type="button" onClick={cancel} disabled={pending} className="rounded p-1 text-fg-faint hover:bg-surface-2 hover:text-fg disabled:opacity-60" aria-label={t('common.close')}>
            <X size={18} />
          </button>
        </div>
        <div className="px-5 pt-3">
          {message && <p className="text-sm text-fg-muted">{message}</p>}
          {confirmationText && (
            <>
              <p className="mt-3 text-sm text-fg-muted">{t('common.typeToConfirm', { text: confirmationText })}</p>
              <input
                value={typed}
                onChange={(event) => setTyped(event.target.value)}
                placeholder={confirmationText}
                className="mt-3 w-full rounded-md border border-danger/50 bg-surface px-3 py-2 text-sm uppercase tracking-wide text-fg focus:border-danger focus:outline-none focus:ring-2 focus:ring-danger/20"
                autoFocus
                disabled={pending}
              />
            </>
          )}
        </div>
        <div className="flex justify-end gap-3 px-5 py-5">
          <button type="button" onClick={cancel} disabled={pending} className="rounded-md border border-line px-4 py-2 text-sm font-medium text-fg hover:bg-surface-2 disabled:opacity-60">
            {t('common.cancel')}
          </button>
          <button type="button" onClick={onConfirm} disabled={pending || !valid} className={`rounded-md px-4 py-2 text-sm font-medium disabled:opacity-60 ${confirmClass}`}>
            {pending ? t('common.working') : confirmLabel || t('common.confirm')}
          </button>
        </div>
      </div>
    </div>
  )
}
