import { useI18n } from '../store/useLocaleStore'
import { useSession } from '../store/useSessionStore'

export default function Status() {
  const { t } = useI18n()
  const s = useSession(x => x.state) || {}
  const rows = [['status.logical', s.logical_manager_id], ['status.replica', s.replica_id], ['status.epoch', s.epoch], ['status.scope', s.scope]]
  return (
    <section className="max-w-3xl">
      <h1 className="text-xl font-semibold text-ink-900">{t('status.title')}</h1>
      <p className="mt-1 text-sm text-ink-700">{t('status.lead')}</p>
      <dl className="mt-6 grid grid-cols-[10rem_1fr] gap-x-6 gap-y-2 text-sm">
        {rows.map(([k, v]) => <div key={k} className="contents"><dt className="text-ink-500">{t(k)}</dt><dd><code className="text-ink-900">{String(v ?? '—')}</code></dd></div>)}
      </dl>
    </section>
  )
}
