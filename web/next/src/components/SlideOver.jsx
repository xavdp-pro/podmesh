import { X } from 'lucide-react'
import { motion, AnimatePresence } from 'framer-motion'
import useEscapeKey from '../hooks/useEscapeKey'
import { useI18n } from '../store/useLocaleStore'

export default function SlideOver({ open, onClose, title, subtitle, children }) {
  const { t } = useI18n()
  useEscapeKey(onClose, open)
  return (
    <AnimatePresence>
      {open && (
        <div className="fixed inset-0 z-50 flex justify-end">
          <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="fixed inset-0 bg-ink-900/30 backdrop-blur-[2px]" onClick={onClose} />
          <motion.aside initial={{ x: 40, opacity: 0 }} animate={{ x: 0, opacity: 1 }} exit={{ x: 40, opacity: 0 }} transition={{ duration: 0.22, ease: 'easeOut' }} role="dialog" aria-modal="true" aria-labelledby="slide-title"
            className="relative z-10 flex h-full w-full max-w-xl flex-col border-l border-cream-300 bg-cream-50 shadow-2xl">
            <div className="flex shrink-0 items-center justify-between gap-3 border-b border-cream-300 px-4 py-4 sm:px-6">
              <div className="min-w-0">
                <h2 id="slide-title" className="truncate text-sm font-bold uppercase tracking-wide text-ink-900">{title}</h2>
                {subtitle && <p className="truncate text-xs text-ink-500">{subtitle}</p>}
              </div>
              <button type="button" onClick={onClose} aria-label={t('common.close')} className="rounded-lg p-1.5 text-ink-500 hover:bg-cream-200 hover:text-green-600"><X size={18} /></button>
            </div>
            <div className="flex-1 overflow-y-auto overflow-x-hidden p-4 sm:p-6">{children}</div>
          </motion.aside>
        </div>
      )}
    </AnimatePresence>
  )
}
