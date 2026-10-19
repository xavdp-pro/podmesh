import { createContext, useCallback, useContext, useMemo, useState } from 'react'
import { detectLocale, LOCALE_STORAGE_KEY, SUPPORTED_LOCALES, translate, isKey } from './index.js'

const I18nContext = createContext(null)

export function I18nProvider({ children }) {
  const [locale, setLocaleState] = useState(detectLocale)

  const setLocale = useCallback((next) => {
    if (!SUPPORTED_LOCALES.includes(next)) return
    try {
      localStorage.setItem(LOCALE_STORAGE_KEY, next)
    } catch {
      // storage unavailable
    }
    document.documentElement.lang = next
    setLocaleState(next)
  }, [])

  const value = useMemo(() => ({
    locale,
    setLocale,
    t: (key, vars) => translate(locale, key, vars),
    /** Translates an API error when it is a key, otherwise returns it as-is. */
    tError: (value, fallbackKey = 'common.serverError') => {
      if (!value) return translate(locale, fallbackKey)
      return isKey(locale, value) ? translate(locale, value) : String(value)
    },
  }), [locale, setLocale])

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useI18n() {
  const ctx = useContext(I18nContext)
  if (!ctx) throw new Error('useI18n must be used inside <I18nProvider>')
  return ctx
}
