import { AnimatePresence, motion } from 'framer-motion'
import { X } from 'lucide-react'
import useEscapeKey from '../../hooks/useEscapeKey'
import { useI18n } from '../../i18n/I18nProvider'

/**
 * Right-hand panel for forms and detail sheets; full width on phones.
 * The body scrolls, the header stays. Close with the cross, the backdrop or Escape.
 */
export default function SlideOver({ isOpen, onClose, title, children, footer = null, widthClass = 'max-w-lg' }) {
  const { t } = useI18n()
  useEscapeKey(onClose, isOpen)

  return (
    <AnimatePresence>
      {isOpen && (
        <div className="fixed inset-0 z-[85] flex justify-end">
          <motion.div
            className="fixed inset-0 bg-black/30 backdrop-blur-[2px]"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.18 }}
            onClick={onClose}
          />
          <motion.section
            role="dialog"
            aria-modal="true"
            aria-label={title}
            className={`relative z-10 flex h-full w-full ${widthClass} flex-col border-l border-line bg-surface shadow-2xl max-sm:max-w-none`}
            initial={{ x: '100%' }}
            animate={{ x: 0 }}
            exit={{ x: '100%' }}
            transition={{ type: 'tween', duration: 0.22, ease: 'easeOut' }}
          >
            <header className="flex shrink-0 items-center justify-between gap-3 border-b border-line px-4 py-4 sm:px-6">
              <h2 className="min-w-0 truncate text-sm font-bold uppercase tracking-wide text-fg sm:text-base">{title}</h2>
              <button
                type="button"
                onClick={onClose}
                className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg text-fg-faint transition-colors hover:bg-surface-2 hover:text-accent"
                aria-label={t('common.close')}
              >
                <X size={18} />
              </button>
            </header>
            <div className="min-h-0 flex-1 overflow-y-auto p-4 sm:p-6">{children}</div>
            {footer && <footer className="shrink-0 border-t border-line px-4 py-3 sm:px-6">{footer}</footer>}
          </motion.section>
        </div>
      )}
    </AnimatePresence>
  )
}
