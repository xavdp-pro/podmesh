import { useId } from 'react'

export default function TextField({ label, value, onChange, autoComplete = 'off', autoFocus = false, placeholder = '', hint = '', inputMode = 'text', readOnly = false, mono = false }) {
  const id = useId()
  return (
    <div>
      <label htmlFor={id} className="mb-1.5 block text-xs font-medium text-ink-500">{label}</label>
      <input id={id} type="text" value={value} onChange={e => onChange?.(e.target.value)} autoComplete={autoComplete} autoFocus={autoFocus} placeholder={placeholder} spellCheck={false} autoCapitalize="off" inputMode={inputMode} readOnly={readOnly}
        className={`w-full rounded-md border border-cream-400 bg-cream-50 px-3 py-2.5 text-sm text-ink-900 transition focus:border-green-600 focus:outline-none focus:ring-2 focus:ring-green-600/20 read-only:text-ink-500 ${mono ? 'font-mono text-xs' : ''}`} />
      {hint && <p className="mt-1 text-xs text-ink-500">{hint}</p>}
    </div>
  )
}
