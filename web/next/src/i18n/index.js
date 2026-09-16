import fr from './locales/fr.json'
import en from './locales/en.json'

export const SUPPORTED_LOCALES = ['en', 'fr']
export const LOCALE_STORAGE_KEY = 'podmesh.console.locale'
export const catalogs = { fr, en }
const nested = (obj, path) => path.split('.').reduce((c, k) => c?.[k], obj)

export function detectBrowserLocale() {
  if (typeof window === 'undefined') return 'en'
  try {
    const saved = localStorage.getItem(LOCALE_STORAGE_KEY)
    if (saved && SUPPORTED_LOCALES.includes(saved)) return saved
  } catch { /* a convenience only */ }
  const lang = (navigator.language || 'en').slice(0, 2).toLowerCase()
  return SUPPORTED_LOCALES.includes(lang) ? lang : 'en'
}

export function translate(locale, key, vars = {}) {
  let text = nested(catalogs[locale] || catalogs.en, key) ?? nested(catalogs.en, key) ?? key
  if (typeof text !== 'string') return key
  for (const [name, value] of Object.entries(vars)) text = text.replaceAll(`{{${name}}}`, String(value ?? ''))
  return text
}
