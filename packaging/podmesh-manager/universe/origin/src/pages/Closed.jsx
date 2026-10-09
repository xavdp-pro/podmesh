import { useI18n } from '../store/useLocaleStore'

export default function Closed({ kind }) {
  const { t } = useI18n()
  return (
    <main className="booting">
      <p className="text-xs font-semibold tracking-widest text-ink-500">{t('brand')}</p>
      <h1 className="mt-1 text-xl font-semibold text-ink-900">{t(`${kind}.title`)}</h1>
      <p className={`mt-3 rounded-md border-l-2 px-3 py-2 text-sm ${kind === 'none' ? 'border-green-600 bg-cream-200 text-ink-700' : 'border-terra-600 bg-terra-100 text-terra-600'}`}>{t(`${kind}.why`)}</p>
      {kind === 'none' && <><p className="mt-3"><code>{t('none.how')}</code></p><p className="mt-3 text-sm text-ink-700">{t('none.doctrine')}</p></>}
    </main>
  )
}
