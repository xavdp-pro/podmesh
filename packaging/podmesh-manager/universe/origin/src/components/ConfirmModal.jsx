import { AlertTriangle, X } from 'lucide-react'
import { motion, AnimatePresence } from 'framer-motion'
import useEscapeKey from '../hooks/useEscapeKey'
import { useI18n } from '../store/useLocaleStore'

// A modal, never a browser dialog: what is about to happen, in words, with a way out.
export default function ConfirmModal({ open, title, message, confirmLabel, pending = false, onConfirm, onCancel }) {
  const { t } = useI18n()
  useEscapeKey(onCancel, open && !pending)
  return (
    <AnimatePresence>
      {open && (
        <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="fixed inset-0 z-50 flex items-center justify-center bg-ink-900/45 p-4" onClick={pending ? undefined : onCancel}>
          <motion.div initial={{ y: 12, scale: 0.98 }} animate={{ y: 0, scale: 1 }} exit={{ y: 12, scale: 0.98 }} role="dialog" aria-modal="true" aria-labelledby="confirm-title"
            className="w-full max-w-md rounded-xl border border-terra-600/30 bg-cream-50 shadow-xl" onClick={e => e.stopPropagation()}>
            <div className="flex items-start justify-between px-5 pt-5">
              <div className="flex items-start gap-2"><AlertTriangle size={16} className="mt-0.5 text-terra-600" /><h2 id="confirm-title" className="text-base font-semibold text-ink-900">{title}</h2></div>
              <button type="button" onClick={onCancel} disabled={pending} aria-label={t('common.close')} className="text-ink-500 hover:text-ink-900 disabled:opacity-60"><X size={18} /></button>
            </div>
            <p className="px-5 pt-3 text-sm text-ink-700">{message}</p>
            <div className="flex justify-end gap-2 px-5 py-5">
              <button type="button" onClick={onCancel} disabled={pending} className="rounded-md border border-cream-400 px-3 py-2 text-sm text-ink-700 hover:bg-cream-200 disabled:opacity-60">{t('common.cancel')}</button>
              <button type="button" onClick={onConfirm} disabled={pending} className="rounded-md bg-terra-600 px-3 py-2 text-sm font-medium text-cream-50 hover:opacity-90 disabled:opacity-60">{confirmLabel}</button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
