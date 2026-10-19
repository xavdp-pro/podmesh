import { forwardRef } from 'react'

export const INPUT_CLASS = 'w-full rounded-md border bg-surface px-3 py-2 text-sm text-fg placeholder:text-fg-faint transition-colors focus:outline-none focus:ring-2 disabled:opacity-60'

/** Border and ring follow the validity; used by TextInput and TextArea. */
export function inputStateClass(invalid) {
  return invalid
    ? 'border-danger focus:border-danger focus:ring-danger/20'
    : 'border-line focus:border-accent focus:ring-accent/20'
}

/**
 * Single-line text input. `invalid` (set by Field from its `error`) turns the
 * border red; everything else is a plain <input>. Wrap in <Field> for the label.
 */
const TextInput = forwardRef(function TextInput({ invalid = false, className = '', type = 'text', ...props }, ref) {
  return <input ref={ref} type={type} className={`${INPUT_CLASS} ${inputStateClass(invalid)} ${className}`} {...props} />
})

export default TextInput
