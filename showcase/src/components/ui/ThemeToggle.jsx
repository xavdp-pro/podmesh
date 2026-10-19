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
      className={`flex h-9 w-9 items-center justify-center rounded-full border border-line bg-surface/80 text-fg-muted transition-colors hover:bg-surface-2 hover:text-fg shadow-xs ${className}`}
      title={dark ? t('theme.light') : t('theme.dark')}
      aria-label={dark ? t('theme.enableLight') : t('theme.enableDark')}
    >
      {dark ? <Sun size={17} /> : <Moon size={17} />}
    </button>
  )
}
