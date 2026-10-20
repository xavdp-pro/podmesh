import React, { useEffect, useState } from 'react'
import { Moon, Sun } from 'lucide-react'
import { useI18n } from '../../i18n/I18nProvider'

const STORAGE_KEY = 'theme'

function currentTheme() {
  return document.documentElement.classList.contains('theme-dark') ? 'dark' : 'light'
}

export function applyTheme(theme) {
  document.documentElement.classList.toggle('theme-dark', theme === 'dark')
  document.documentElement.classList.toggle('theme-light', theme !== 'dark')
  document.documentElement.setAttribute('data-theme', theme)
  document.documentElement.style.colorScheme = theme
}

/**
 * Light/dark switch matching xavdp.pro.
 */
export default function ThemeToggle({ className = '' }) {
  const { t } = useI18n()
  const [theme, setTheme] = useState(currentTheme)

  useEffect(() => {
    applyTheme(theme)
    try {
      localStorage.setItem(STORAGE_KEY, theme)
    } catch {}
  }, [theme])

  const dark = theme === 'dark'
  return (
    <button
      type="button"
      onClick={() => setTheme(dark ? 'light' : 'dark')}
      aria-pressed={dark}
      className={`flex h-9 w-9 items-center justify-center rounded-full p-2 text-fg-muted transition-colors hover:bg-surface-2 hover:text-fg focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent ${className}`}
      title={dark ? t('theme.light') : t('theme.dark')}
      aria-label={dark ? t('theme.enableLight') : t('theme.enableDark')}
    >
      {dark ? <Sun size={17} aria-hidden="true" /> : <Moon size={17} aria-hidden="true" />}
    </button>
  )
}
