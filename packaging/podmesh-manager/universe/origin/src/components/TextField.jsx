import { useId } from 'react'

export default function TextField({ label, value, onChange, autoComplete = 'off', autoFocus = false, placeholder = '' }) {
  const id = useId()
  return (
    <div>
      <label htmlFor={id} className="mb-1.5 block text-xs font-medium text-ink-500">{label}</label>
      <input id={id} type="text" value={value} onChange={e => onChange(e.target.value)} autoComplete={autoComplete} autoFocus={autoFocus} placeholder={placeholder} spellCheck={false} autoCapitalize="off"
        className="w-full rounded-md border border-cream-400 bg-cream-50 px-3 py-2.5 text-sm text-ink-900 transition focus:border-green-600 focus:outline-none focus:ring-2 focus:ring-green-600/20" />
    </div>
  )
}
