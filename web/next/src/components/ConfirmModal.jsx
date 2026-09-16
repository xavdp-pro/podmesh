import { createPortal } from 'react-dom'
import { AlertTriangle, X } from 'lucide-react'
import { motion, AnimatePresence } from 'framer-motion'
import useEscapeKey from '../hooks/useEscapeKey'
import { useI18n } from '../store/useLocaleStore'

// A modal, never a browser dialog: what is about to happen, in words, with a way out. It may carry
// fields (the authorization reference, a destination) between its message and its buttons; a
// destructive action is drawn in terracotta, an ordinary one in green. It is drawn on the page's body:
// opened from inside the drawer, it would otherwise be trapped under the drawer's own layer.
export default function ConfirmModal({ open, title, message, confirmLabel, pending = false, danger = true, onConfirm, onCancel, children, footer = null, hideConfirm = false }) {
  const { t } = useI18n()
  useEscapeKey(onCancel, open && !pending)
  if (typeof document === 'undefined') return null
  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="fixed inset-0 z-[60] flex items-center justify-center bg-ink-900/45 p-4" onClick={pending ? undefined : onCancel}>
          <motion.div initial={{ y: 12, scale: 0.98 }} animate={{ y: 0, scale: 1 }} exit={{ y: 12, scale: 0.98 }} role="dialog" aria-modal="true" aria-labelledby="confirm-title"
            className={`max-h-[92dvh] w-full max-w-md overflow-y-auto rounded-xl border bg-cream-50 shadow-xl ${danger ? 'border-terra-600/30' : 'border-green-600/30'}`} onClick={e => e.stopPropagation()}>
            <div className="flex items-start justify-between px-5 pt-5">
              <div className="flex items-start gap-2">{danger && <AlertTriangle size={16} className="mt-0.5 shrink-0 text-terra-600" />}<h2 id="confirm-title" className="text-base font-semibold text-ink-900">{title}</h2></div>
              <button type="button" onClick={onCancel} disabled={pending} aria-label={t('common.close')} className="text-ink-500 hover:text-ink-900 disabled:opacity-60"><X size={18} /></button>
            </div>
            {message && <p className="px-5 pt-3 text-sm text-ink-700">{message}</p>}
            {children && <div className="space-y-3 px-5 pt-4">{children}</div>}
            <div className="flex flex-wrap items-center justify-end gap-2 px-5 py-5">
              {footer}
              <button type="button" onClick={onCancel} disabled={pending} className="rounded-md border border-cream-400 px-3 py-2 text-sm text-ink-700 hover:bg-cream-200 disabled:opacity-60">{hideConfirm ? t('common.close') : t('common.cancel')}</button>
              {!hideConfirm && <button type="button" onClick={onConfirm} disabled={pending} className={`rounded-md px-3 py-2 text-sm font-medium text-cream-50 hover:opacity-90 disabled:opacity-60 ${danger ? 'bg-terra-600' : 'bg-green-600'}`}>{confirmLabel}</button>}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  )
}
