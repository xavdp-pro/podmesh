import { cloneElement, isValidElement, useId } from 'react'
import { useI18n } from '../../i18n/I18nProvider'

/** Translates an error that is an i18n key or `{ key, vars }`; plain text passes through. */
export function errorText(t, error) {
  if (!error) return ''
  if (typeof error === 'string') return t(error)
  return t(error.key, error.vars)
}

/**
 * Label, control, hint and error in one block. The single child control gets
 * `id`, `aria-invalid` and `aria-describedby` so the hint and the error are
 * announced. `required` adds the mark; validation itself lives in useForm.
 *
 *   <Field label={t('users.email')} hint={t('users.emailHint')} error={form.error('email')} required>
 *     <TextInput type="email" {...form.field('email')} />
 *   </Field>
 */
export default function Field({ label, hint, error, required = false, id, className = '', children, inline = false }) {
  const { t } = useI18n()
  const autoId = useId()
  const controlId = id || `field-${autoId}`
  const hintId = hint ? `${controlId}-hint` : undefined
  const errorId = error ? `${controlId}-error` : undefined
  const describedBy = [hintId, errorId].filter(Boolean).join(' ') || undefined
  const message = errorText(t, error)

  const control = isValidElement(children)
    ? cloneElement(children, {
      id: children.props.id || controlId,
      'aria-invalid': error ? true : children.props['aria-invalid'],
      'aria-describedby': describedBy,
      invalid: children.props.invalid ?? Boolean(error),
    })
    : children

  if (inline) {
    // Checkbox / switch: the control sits before the label on one line.
    return (
      <div className={`min-w-0 ${className}`}>
        <div className="flex items-start gap-2.5">
          <div className="shrink-0 pt-0.5">{control}</div>
          <label htmlFor={controlId} className="min-w-0 text-sm text-fg">
            {label}
            {required && <span className="ml-0.5 text-danger" aria-hidden="true">*</span>}
            {hint && <span id={hintId} className="mt-0.5 block text-xs text-fg-muted">{hint}</span>}
          </label>
        </div>
        {message && <p id={errorId} role="alert" className="mt-1 text-xs font-medium text-danger">{message}</p>}
      </div>
    )
  }

  return (
    <div className={`min-w-0 ${className}`}>
      {label && (
        <label htmlFor={controlId} className="mb-1.5 block text-xs font-bold uppercase tracking-wide text-fg-faint">
          {label}
          {required && <span className="ml-0.5 text-danger" aria-hidden="true">*</span>}
        </label>
      )}
      {control}
      {hint && !message && <p id={hintId} className="mt-1 text-xs text-fg-muted">{hint}</p>}
      {hint && message && <p id={hintId} className="sr-only">{hint}</p>}
      {message && <p id={errorId} role="alert" className="mt-1 text-xs font-medium text-danger">{message}</p>}
    </div>
  )
}
