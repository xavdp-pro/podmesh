/**
 * UI strings. Keys are English, dictionaries are fr.json (default) and en.json.
 * translate() is pure so tests and non-React code can use it; useI18n() is the
 * React binding (see I18nProvider in ./I18nProvider.jsx).
 */
import fr from './fr.json'
import en from './en.json'
import es from './es.json'

export const SUPPORTED_LOCALES = ['fr', 'en', 'es']
export const DEFAULT_LOCALE = 'fr'
export const LOCALE_STORAGE_KEY = 'locale'

const catalogs = { fr, en, es }

function getNested(obj, path) {
  return path.split('.').reduce((current, key) => current?.[key], obj)
}

export function detectLocale() {
  if (typeof window === 'undefined') return DEFAULT_LOCALE
  try {
    const saved = localStorage.getItem(LOCALE_STORAGE_KEY)
    if (saved && SUPPORTED_LOCALES.includes(saved)) return saved
  } catch {
    // storage unavailable
  }
  const lang = (navigator.language || DEFAULT_LOCALE).slice(0, 2).toLowerCase()
  return SUPPORTED_LOCALES.includes(lang) ? lang : DEFAULT_LOCALE
}

/** Looks a key up in the locale, falls back to French, then to the key itself. */
export function translate(locale, key, vars = {}) {
  const catalog = catalogs[locale] || catalogs[DEFAULT_LOCALE]
  let text = getNested(catalog, key) ?? getNested(catalogs[DEFAULT_LOCALE], key) ?? key
  if (typeof text !== 'string') return key
  for (const [name, value] of Object.entries(vars)) {
    text = text.replaceAll(`{{${name}}}`, String(value ?? ''))
  }
  return text
}

/** True when a string coming from the API is a translation key (e.g. "auth.linkUsed"). */
export function isKey(locale, value) {
  return typeof value === 'string' && /^[a-z]+(\.[a-zA-Z]+)+$/.test(value) && translate(locale, value) !== value
}
