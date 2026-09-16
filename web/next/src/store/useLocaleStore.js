import { useCallback, useSyncExternalStore } from 'react'
import { createStore } from 'zustand/vanilla'
import { detectBrowserLocale, LOCALE_STORAGE_KEY, SUPPORTED_LOCALES, translate } from '../i18n'

const store = createStore((set, get) => ({
  locale: detectBrowserLocale(),
  setLocale: next => {
    if (!SUPPORTED_LOCALES.includes(next)) return
    try { localStorage.setItem(LOCALE_STORAGE_KEY, next) } catch { /* a convenience only */ }
    document.documentElement.lang = next
    set({ locale: next })
  },
  t: (key, vars) => translate(get().locale, key, vars),
}))
if (typeof document !== 'undefined') document.documentElement.lang = store.getState().locale

export function useI18n() {
  const locale = useSyncExternalStore(store.subscribe, () => store.getState().locale, () => store.getInitialState().locale)
  const setLocale = store.getState().setLocale
  const t = useCallback((key, vars) => translate(locale, key, vars), [locale])
  return { locale, setLocale, t }
}
