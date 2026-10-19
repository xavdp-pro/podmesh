import { forwardRef } from 'react'

/**
 * Boolean controls. Both take `checked` (or `value`, so useForm's field() works)
 * and `onChange(event)`; `invalid` comes from Field. Use with <Field inline>.
 */
export const Checkbox = forwardRef(function Checkbox({ checked, value, invalid = false, className = '', ...props }, ref) {
  return (
    <input
      ref={ref}
      type="checkbox"
      checked={Boolean(checked ?? value)}
      className={`h-4 w-4 rounded border-line accent-accent ${invalid ? 'outline outline-2 outline-danger/60' : ''} ${className}`}
      {...props}
    />
  )
})

/** A checkbox styled as a toggle (role="switch"); same props as Checkbox. */
export const Switch = forwardRef(function Switch({ checked, value, invalid = false, className = '', disabled = false, onChange, id, name, ...props }, ref) {
  const on = Boolean(checked ?? value)
  return (
    <button
      ref={ref}
      type="button"
      role="switch"
      id={id}
      name={name}
      aria-checked={on}
      disabled={disabled}
      // Same event shape as a checkbox so useForm's handleChange reads `target.checked`.
      onClick={() => onChange?.({ target: { type: 'checkbox', name, checked: !on, value: !on } })}
      className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full border transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:opacity-60 ${
        on ? 'border-accent bg-accent' : 'border-line bg-surface-2'
      } ${invalid ? 'ring-2 ring-danger/40' : ''} ${className}`}
      {...props}
    >
      <span className={`inline-block h-5 w-5 rounded-full bg-white shadow transition-transform ${on ? 'translate-x-5' : 'translate-x-0.5'}`} />
    </button>
  )
})

export default Checkbox
