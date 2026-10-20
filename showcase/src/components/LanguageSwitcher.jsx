import React, { useEffect, useId, useRef, useState } from 'react'
import { Check } from 'lucide-react'
import { useI18n } from '../i18n/I18nProvider'
import { SUPPORTED_LOCALES } from '../i18n'
import { Flag } from './Flag'

export default function LanguageSwitcher({ className = '' }) {
  const { locale, setLocale, t } = useI18n()
  const [open, setOpen] = useState(false)
  const root = useRef(null)
  const trigger = useRef(null)
  const menuId = useId()

  useEffect(() => {
    if (!open) return
    const outside = (event) => {
      if (!root.current?.contains(event.target)) setOpen(false)
    }
    const escape = (event) => {
      if (event.key === 'Escape') {
        setOpen(false)
        trigger.current?.focus()
      }
    }
    document.addEventListener('pointerdown', outside)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('pointerdown', outside)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  const focusOption = (index) => {
    requestAnimationFrame(() => root.current?.querySelectorAll('[role="menuitemradio"]')[index]?.focus())
  }

  return (
    <div ref={root} className={`relative ${className}`} onBlur={(event) => {
      if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false)
    }}>
      <button
        ref={trigger}
        type="button"
        aria-expanded={open}
        aria-haspopup="menu"
        aria-controls={open ? menuId : undefined}
        aria-label={`${t(`language.${locale}`)}, ${t('language.switch')}`}
        title={t('language.switch')}
        onClick={() => setOpen(!open)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault()
            setOpen(true)
            focusOption(event.key === 'ArrowUp' ? SUPPORTED_LOCALES.length - 1 : 0)
          }
        }}
        className="flex h-9 w-9 items-center justify-center rounded-full p-1.5 transition-colors hover:bg-surface-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
      >
        <Flag code={locale} />
      </button>
      {open && (
        <div id={menuId} role="menu" aria-label={t('language.switch')}
          className="absolute right-0 top-full z-50 mt-1 w-44 overflow-hidden rounded-xl border border-line bg-surface p-1 shadow-lg"
          onKeyDown={(event) => {
            const options = [...event.currentTarget.querySelectorAll('[role="menuitemradio"]')]
            const current = options.indexOf(document.activeElement)
            let next
            if (event.key === 'ArrowDown') next = (current + 1) % options.length
            if (event.key === 'ArrowUp') next = (current - 1 + options.length) % options.length
            if (event.key === 'Home') next = 0
            if (event.key === 'End') next = options.length - 1
            if (next !== undefined) { event.preventDefault(); options[next]?.focus() }
          }}>
          {SUPPORTED_LOCALES.map((code) => (
            <button key={code} type="button" role="menuitemradio" aria-checked={code === locale}
              onClick={() => { setLocale(code); setOpen(false); trigger.current?.focus() }}
              className="flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-sm text-fg-muted transition-colors hover:bg-surface-2 focus:bg-surface-2 focus:outline-none">
              <Flag code={code} />
              <span className={code === locale ? 'font-medium text-fg' : ''}>{t(`language.${code}`)}</span>
              {code === locale && <Check size={14} className="ml-auto text-accent" aria-hidden="true" />}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
