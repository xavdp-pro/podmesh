import React from 'react'
import { translate, detectBrowserLocale } from '../i18n'

// React unmounts the whole tree on an uncaught drawing error; a blank page is never the answer.
export default class Boundary extends React.Component {
  constructor(props) { super(props); this.state = { error: null } }
  static getDerivedStateFromError(error) { return { error } }
  render() {
    if (!this.state.error) return this.props.children
    const t = key => translate(detectBrowserLocale(), key)
    return (
      <main className="booting" role="alert">
        <p>{t('brand')}</p>
        <h1 className="text-xl font-semibold text-ink-900">{t('error.title')}</h1>
        <p><code>{String(this.state.error?.message || this.state.error)}</code></p>
        <p>{t('error.why')}</p>
        <button type="button" onClick={() => window.location.reload()} className="rounded-md bg-green-600 px-3 py-2 text-sm text-cream-50">{t('error.reload')}</button>
      </main>
    )
  }
}
