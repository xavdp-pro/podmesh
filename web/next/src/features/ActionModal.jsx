import { useState } from 'react'
import toast from 'react-hot-toast'
import ConfirmModal from '../components/ConfirmModal'
import TextField from '../components/TextField'
import Select from '../components/Select'
import Checkbox from '../components/Checkbox'
import Notice from '../components/Notice'
import { useI18n } from '../store/useLocaleStore'
import { postAction, postMove } from '../api/console'
import { actionBody, moveBody, ACTIONS } from '../model/universes'

// One lifecycle action on one universe, behind a modal: the target and the mandate, the fields the
// action needs, and the answer where the operator looks. The operation identity is drawn once when
// the modal opens, so a retry runs the same request once.
export default function ActionModal({ row, op, hosts = [], onClose, onDone }) {
  const { t } = useI18n()
  const [auth, setAuth] = useState('')
  const [memoryMib, setMemoryMib] = useState('')
  const [cpus, setCpus] = useState('')
  const [destination, setDestination] = useState('')
  const [keepSource, setKeepSource] = useState(false)
  const [operationId] = useState(() => crypto.randomUUID())
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [result, setResult] = useState(null)
  const open = !!row && !!op
  const action = ACTIONS.find(a => a.op === op)
  const destinations = hosts.filter(h => h.canMove && h.id !== row?.host.id).map(h => ({ value: h.id, label: h.name }))

  async function proceed() {
    if (!row) return
    setPending(true); setError('')
    try {
      if (op === 'move') {
        const body = moveBody({ universe_uuid: row.uuid, source: row.host.id, destination, authorization_ref: auth, keep_source: keepSource })
        const report = await postMove(body)
        setResult(report)
        toast.success(t('actions.moved', { name: row.name, host: hosts.find(h => h.id === destination)?.name || destination }))
      } else {
        const body = actionBody({ op, universe_uuid: row.uuid, operation_id: operationId, authorization_ref: auth, memory_mib: memoryMib, cpus })
        const answer = await postAction(row.host.id, body)
        if (answer?.ok === false) throw { error: answer.error || t('common.refused') }
        setResult(answer)
        toast.success(t('actions.done', { op: t(`actions.${op}.label`), name: row.name }))
      }
      onDone?.()
    } catch (e) {
      const code = e instanceof Error ? e.message : null
      setError(code ? t(`validation.${code}`) : e?.error || e?.message || t('common.refused'))
    } finally { setPending(false) }
  }

  return (
    <ConfirmModal open={open} danger={!!action?.danger} pending={pending} onCancel={onClose} onConfirm={proceed} hideConfirm={!!result}
      title={op ? t('actions.title', { op: t(`actions.${op}.label`), name: row?.name || '' }) : ''}
      message={op ? t(`actions.${op}.message`) : ''}
      confirmLabel={op ? t('actions.confirm', { op: t(`actions.${op}.label`) }) : ''}>
      {row && <p className="text-xs text-ink-500">{t('universe.host')}: <span className="text-ink-900">{row.host.name}</span> · <code className="text-ink-900">{row.uuid}</code></p>}
      {op === 'resources' && (
        <div className="grid gap-3 sm:grid-cols-2">
          <TextField label={t('actions.resources.memory')} value={memoryMib} onChange={setMemoryMib} placeholder="512" inputMode="numeric" />
          <TextField label={t('actions.resources.cpus')} value={cpus} onChange={setCpus} placeholder="1.5" inputMode="decimal" />
        </div>
      )}
      {op === 'move' && (
        <>
          <div>
            <p className="mb-1.5 text-xs font-medium text-ink-500">{t('actions.move.destination')}</p>
            <Select value={destination} onChange={setDestination} options={destinations} label={t('actions.move.destination')} placeholder={destinations.length ? t('common.choose') : t('actions.move.noDestination')} disabled={!destinations.length} searchPlaceholder={t('common.search')} noResult={t('common.noMatch')} clearSearchLabel={t('common.clearSearch')} />
          </div>
          <Checkbox label={t('actions.move.keepSource')} checked={keepSource} onChange={setKeepSource} />
        </>
      )}
      {!result && <TextField label={t('common.authorization')} value={auth} onChange={setAuth} placeholder={t('common.authorizationPlaceholder')} autoFocus={op !== 'move' && op !== 'resources'} />}
      <p className="text-xs text-ink-500">{t('actions.operationId', { id: operationId })}</p>
      <Notice bad text={error} />
      {result && (
        <Notice>
          <p className="font-medium">{op === 'move' ? t('actions.move.result', { host: hosts.find(h => h.id === destination)?.name || destination }) : t('actions.ok')}</p>
          {Array.isArray(result.steps) && result.steps.length > 0 && (
            <ol className="mt-1 list-decimal pl-5 text-xs">{result.steps.map((s, i) => <li key={i}>{s.step} · {s.host} · {s.ok ? 'ok' : t('common.refused')}{s.seconds ? ` · ${s.seconds}s` : ''}{s.error ? ` — ${s.error}` : ''}</li>)}</ol>
          )}
          <details className="mt-1 text-xs"><summary className="cursor-pointer">{t('common.rawAnswer')}</summary><pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-all">{JSON.stringify(result, null, 2)}</pre></details>
        </Notice>
      )}
    </ConfirmModal>
  )
}
