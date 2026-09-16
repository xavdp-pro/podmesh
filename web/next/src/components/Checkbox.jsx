import { useId } from 'react'

export default function Checkbox({ label, checked, onChange, disabled = false }) {
  const id = useId()
  return (
    <label htmlFor={id} className="flex items-center gap-2 text-sm text-ink-700">
      <input id={id} type="checkbox" checked={!!checked} disabled={disabled} onChange={e => onChange(e.target.checked)} className="h-4 w-4 rounded border-cream-400 accent-green-600" />
      {label}
    </label>
  )
}
