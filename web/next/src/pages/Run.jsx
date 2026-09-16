import { useMemo, useState } from 'react'
import { Send } from 'lucide-react'
import Select from '../components/Select'
import TextField from '../components/TextField'
import Checkbox from '../components/Checkbox'
import Badge from '../components/Badge'
import Button from '../components/Button'
import Notice from '../components/Notice'
import { useI18n } from '../store/useLocaleStore'
import { useSnapshot } from '../store/useSnapshotStore'
import { postOperation } from '../api/console'
import { buildRequest, initialFields, needsUniverse } from '../model/operation'
import { universeRows } from '../model/universes'

const JSON_TYPES = ['string[]', 'uuid[]', 'object[]', 'object']

// One mechanism for every operation: the form is drawn from the schema the host publishes, the
// request validated against it here and again by the server, the typed answer shown in place.
export default function Run() {
  const { t } = useI18n()
  const { snapshot, refresh } = useSnapshot()
  const hosts = snapshot?.hosts || []
  const [hostId, setHostId] = useState('')
  const [operation, setOperation] = useState('')
  const [fields, setFields] = useState({})
  const [universe, setUniverse] = useState('')
  const [auth, setAuth] = useState('')
  const [error, setError] = useState('')
  const [result, setResult] = useState(null)
  const [busy, setBusy] = useState(false)
  const host = hosts.find(h => h.id === (hostId || hosts[0]?.id))
  const schemas = host?.responses?.capabilities?.data?.schemas || {}
  const ops = useMemo(() => Object.entries(schemas).sort(([a], [b]) => a.localeCompare(b)).map(([name, s]) => ({ value: name, label: name.replaceAll('_', ' '), hint: `${s.kind} · ${s.gate}` })), [schemas])
  const schema = schemas[operation]
  const universes = universeRows(hosts).filter(r => r.host.id === host?.id && r.uuid).map(r => ({ value: r.uuid, label: r.name, hint: r.state }))
  const setField = (name, value) => setFields(f => ({ ...f, [name]: value }))
  const pick = name => { setOperation(name); setFields(initialFields(schemas[name])); setError(''); setResult(null) }

  async function send(e) {
    e.preventDefault()
    if (busy || !schema || !host) return
    setBusy(true); setError(''); setResult(null)
    let request
    try {
      request = buildRequest(schema, operation, fields, universe, auth)
      const body = await postOperation(host.id, request)
      setResult({ ok: !!body?.ok, body, request })
      if (schema.kind !== 'read') refresh()
    } catch (err) {
      if (err instanceof Error) setError(err.message)
      else setResult({ ok: false, body: err, request })
    } finally { setBusy(false) }
  }

  return (
    <section className="space-y-4">
      <div>
        <h1 className="text-xl font-semibold text-ink-900">{t('run.title')}</h1>
        <p className="mt-1 text-sm text-ink-700">{t('run.lead')}</p>
      </div>
      <form onSubmit={send} noValidate className="space-y-4 rounded-xl border border-cream-300 bg-cream-50 p-4">
        <div className="grid gap-3 sm:grid-cols-[1fr_2fr]">
          <div><p className="mb-1.5 text-xs font-medium text-ink-500">{t('run.host')}</p>
            <Select value={host?.id || ''} onChange={id => { setHostId(id); setOperation(''); setResult(null) }} label={t('run.host')}
              options={hosts.map(h => ({ value: h.id, label: h.name, hint: h.allowActions ? t('run.actionsAllowed') : t('run.readOnly') }))} searchPlaceholder={t('common.search')} noResult={t('common.noMatch')} clearSearchLabel={t('common.clearSearch')} /></div>
          <div><p className="mb-1.5 text-xs font-medium text-ink-500">{t('run.operation')}</p>
            <Select value={operation} onChange={pick} options={ops} searchable label={t('run.operation')} disabled={!ops.length}
              placeholder={ops.length ? t('common.choose') : t('run.noSchema')} searchPlaceholder={t('run.searchOperation')} noResult={t('common.noMatch')} clearSearchLabel={t('common.clearSearch')} /></div>
        </div>
        {schema && (
          <>
            <p className="flex flex-wrap items-center gap-2 text-sm text-ink-700">
              <Badge tone={schema.kind === 'read' || schema.kind === 'tool' ? 'muted' : 'green'}>{schema.kind}</Badge>
              {schema.gate !== 'none' && <Badge tone="muted">gate: {schema.gate}</Badge>}
              <span>{schema.description}</span>
            </p>
            {schema.kind === 'tool' && <p className="text-sm text-amber-600">{t('run.tool')}</p>}
            {schema.fields === null && <p className="text-sm text-ink-500">{t('run.noFields')}</p>}
            {schema.kind !== 'tool' && (
              <>
                {needsUniverse(schema) && (universes.length
                  ? <div><p className="mb-1.5 text-xs font-medium text-ink-500">{t('run.universe')}</p><Select value={universe} onChange={setUniverse} options={universes} searchable label={t('run.universe')} placeholder={t('run.chooseUniverse')} searchPlaceholder={t('common.search')} noResult={t('common.noMatch')} clearSearchLabel={t('common.clearSearch')} /></div>
                  : <TextField label={t('run.universe')} value={universe} onChange={setUniverse} placeholder={t('run.universeUuid')} mono />)}
                <div className="grid gap-3 sm:grid-cols-2">
                  {(schema.fields || []).map(f => {
                    const label = `${f.name}${f.required ? ' *' : ''}`
                    const hint = `${f.description || ''}${f.min !== undefined ? ` · min ${f.min}` : ''}${f.max !== undefined ? ` · max ${f.max}` : ''}`
                    if (f.type === 'enum') return <div key={f.name}><p className="mb-1.5 text-xs font-medium text-ink-500">{label}</p><Select value={fields[f.name]} onChange={v => setField(f.name, v)} options={f.values.map(v => ({ value: v, label: v }))} label={f.name} placeholder={t('common.choose')} /><p className="mt-1 text-xs text-ink-500">{hint}</p></div>
                    if (f.type === 'boolean') return <div key={f.name}><p className="mb-1.5 text-xs font-medium text-ink-500">{label}</p><Checkbox label={t('run.yes')} checked={fields[f.name]} onChange={v => setField(f.name, v)} /><p className="mt-1 text-xs text-ink-500">{hint}</p></div>
                    if (JSON_TYPES.includes(f.type)) return (
                      <div key={f.name} className="sm:col-span-2"><label className="mb-1.5 block text-xs font-medium text-ink-500" htmlFor={`field-${f.name}`}>{label}</label>
                        <textarea id={`field-${f.name}`} value={fields[f.name]} onChange={e => setField(f.name, e.target.value)} rows={3} spellCheck={false} placeholder={f.type === 'object' ? '{ }' : '[ ]'}
                          className="w-full rounded-md border border-cream-400 bg-cream-50 px-3 py-2 font-mono text-xs focus:border-green-600 focus:outline-none focus:ring-2 focus:ring-green-600/20" />
                        <p className="mt-1 text-xs text-ink-500">{hint}</p></div>)
                    return <TextField key={f.name} label={label} value={fields[f.name]} onChange={v => setField(f.name, v)} hint={hint} inputMode={['integer', 'number'].includes(f.type) ? 'decimal' : 'text'} />
                  })}
                </div>
                <TextField label={`${t('common.authorization')} *`} value={auth} onChange={setAuth} placeholder={t('common.authorizationPlaceholder')} />
                <Notice bad text={error} />
                <button type="submit" disabled={busy || (schema.kind !== 'read' && !host?.allowActions)}
                  className="inline-flex items-center gap-1.5 rounded-md bg-green-600 px-3 py-2 text-sm font-medium text-cream-50 hover:opacity-90 disabled:opacity-50">
                  <Send size={15} />{busy ? t('common.sending') : schema.kind === 'read' ? t('run.read') : t('run.send', { op: operation.replaceAll('_', ' ') })}
                </button>
                {schema.kind !== 'read' && !host?.allowActions && <p className="text-xs text-ink-500">{t('run.readOnlyHost')}</p>}
              </>
            )}
          </>
        )}
      </form>
      {result && (
        <div className={`space-y-2 rounded-xl border p-4 ${result.ok ? 'border-green-600/30 bg-green-100' : 'border-terra-600/30 bg-terra-100'}`} role="status">
          <p className={`font-medium ${result.ok ? 'text-green-700' : 'text-terra-600'}`}>{result.ok ? t('run.answeredOk') : t('run.refused')}</p>
          {result.body?.error && <p className="text-sm text-terra-600">{result.body.error}</p>}
          <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-all rounded-md bg-cream-50 p-2 text-xs">{JSON.stringify(result.body, null, 2)}</pre>
          {result.request && <details className="text-xs"><summary className="cursor-pointer">{t('run.request')}</summary><pre className="mt-1 whitespace-pre-wrap break-all">{JSON.stringify(result.request, null, 2)}</pre></details>}
        </div>
      )}
    </section>
  )
}
