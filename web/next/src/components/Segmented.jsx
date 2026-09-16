// A choice among a few named options, drawn as buttons: a radio group in the page's own clothes.
export default function Segmented({ value, onChange, options, label, disabled = false }) {
  return (
    <div role="radiogroup" aria-label={label} className="grid gap-2 sm:grid-cols-2">
      {options.map(o => (
        <button key={o.value} type="button" role="radio" aria-checked={value === o.value} disabled={disabled || o.disabled} onClick={() => onChange(o.value)}
          className={`rounded-md border px-3 py-2 text-left text-sm transition disabled:opacity-60 ${value === o.value ? 'border-green-600 bg-green-100 text-green-700' : 'border-cream-400 bg-cream-50 text-ink-700 hover:bg-cream-200'}`}>
          <span className="block font-medium">{o.label}</span>
          {o.hint && <span className="mt-0.5 block text-xs text-ink-500">{o.hint}</span>}
        </button>
      ))}
    </div>
  )
}
