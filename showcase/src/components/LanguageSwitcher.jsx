import React from 'react'
import { useI18n } from '../i18n/I18nProvider'
import { SUPPORTED_LOCALES } from '../i18n'
import { Flag } from './Flag'

const LANG_LABELS = {
  fr: 'FR',
  en: 'EN',
  es: 'ES',
}

export default function LanguageSwitcher({ className = '' }) {
  const { locale, setLocale, t } = useI18n()

  return (
    <div
      className={`inline-flex items-center gap-1 rounded-full border border-line bg-surface/80 p-1 backdrop-blur-sm ${className}`}
      role="group"
      aria-label={t('language.switch')}
    >
      {SUPPORTED_LOCALES.map((code) => {
        const active = code === locale
        return (
          <button
            key={code}
            type="button"
            onClick={() => setLocale(code)}
            aria-pressed={active}
            className={`flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-semibold transition-all ${
              active
                ? 'bg-accent text-accent-fg shadow-sm'
                : 'text-fg-muted hover:bg-surface-2 hover:text-fg'
            }`}
            title={t(`language.${code}`)}
          >
            <Flag code={code} />
            <span className="text-[11px] font-bold uppercase">{LANG_LABELS[code]}</span>
          </button>
        )
      })}
    </div>
  )
}
