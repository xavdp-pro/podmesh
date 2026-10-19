import { useCallback, useMemo, useRef, useState } from 'react'
import { request } from '../../api/client'
import { hasErrors, pickFieldErrors, validateValues } from './formValidators'

/**
 * Form state for the mold's primitives: values, touched, errors, submitting.
 * Pure ajax: `handleSubmit` calls `preventDefault()` and sends JSON through the
 * api client, never a native POST.
 *
 *   const form = useForm({
 *     initialValues: { email: '', role: 'member' },
 *     rules: { email: [required(), email()], role: oneOf(ROLES) },     // or validate(values) -> errors
 *     submit: (values) => api.createUser(values),                       // or endpoint: { method: 'POST', path: '/users' }
 *     onSuccess: (result, values) => toast(...),
 *   })
 *   <Field label=… error={form.error('email')}><TextInput {...form.field('email')} /></Field>
 *   <form onSubmit={form.handleSubmit}>… or <button onClick={form.handleSubmit}>
 *
 * Errors are i18n keys (or `{ key, vars }`), shown under the field once it was
 * touched or a submit was attempted. Field errors answered by the server as
 * `{ error, fields: { name: key } }` land on the matching fields; the top-level
 * `error` becomes `form.submitError` for a banner.
 */
export function useForm({ initialValues = {}, rules, validate, submit, endpoint, onSuccess, onError } = {}) {
  const initialRef = useRef(initialValues)
  const [values, setValuesState] = useState(initialValues)
  const valuesRef = useRef(initialValues)
  const setValues = useCallback((next) => {
    valuesRef.current = typeof next === 'function' ? next(valuesRef.current) : next
    setValuesState(valuesRef.current)
  }, [])
  const [touched, setTouched] = useState({})
  const [errors, setErrors] = useState({})
  const [submitting, setSubmitting] = useState(false)
  const [submitted, setSubmitted] = useState(false)
  const [submitError, setSubmitError] = useState('')

  const names = useMemo(() => Object.keys({ ...initialRef.current, ...(rules || {}) }), [rules])

  const runValidation = useCallback((next) => {
    const fromRules = rules ? validateValues(next, rules) : {}
    const fromFn = typeof validate === 'function' ? validate(next) || {} : {}
    return { ...fromRules, ...fromFn }
  }, [rules, validate])

  const setValue = useCallback((name, value) => {
    const next = { ...valuesRef.current, [name]: value }
    setValues(next)
    // Re-validate the edited field only, so the message under it follows the typing.
    const all = runValidation(next)
    setErrors((currentErrors) => {
      const copy = { ...currentErrors }
      if (all[name]) copy[name] = all[name]
      else delete copy[name]
      return copy
    })
    setSubmitError('')
  }, [runValidation, setValues])

  const handleChange = useCallback((name) => (eventOrValue) => {
    const target = eventOrValue?.target
    const value = target ? (target.type === 'checkbox' ? target.checked : target.value) : eventOrValue
    setValue(name, value)
  }, [setValue])

  const handleBlur = useCallback((name) => () => {
    setTouched((current) => (current[name] ? current : { ...current, [name]: true }))
  }, [])

  /** Props to spread on TextInput, TextArea, Checkbox, Switch or Select. */
  const field = useCallback((name) => ({
    name,
    value: values[name] ?? '',
    onChange: handleChange(name),
    onBlur: handleBlur(name),
  }), [values, handleChange, handleBlur])

  /** The error to show under a field: only after blur or a submit attempt. */
  const error = useCallback((name) => ((touched[name] || submitted) ? errors[name] : undefined), [touched, submitted, errors])

  const reset = useCallback((next) => {
    const base = next ?? initialRef.current
    if (next) initialRef.current = next
    setValues(base)
    valuesRef.current = base
    setTouched({})
    setErrors({})
    setSubmitted(false)
    setSubmitting(false)
    setSubmitError('')
  }, [])

  const handleSubmit = useCallback(async (event) => {
    event?.preventDefault?.()
    if (submitting) return undefined
    setSubmitted(true)
    setSubmitError('')
    const found = runValidation(values)
    setErrors(found)
    if (hasErrors(found)) return undefined
    setSubmitting(true)
    try {
      const result = submit
        ? await submit(values)
        : await request(endpoint.path, { method: endpoint.method || 'POST', body: values })
      onSuccess?.(result, values)
      return result
    } catch (err) {
      const fieldErrors = pickFieldErrors(err?.fields, names)
      if (hasErrors(fieldErrors)) setErrors((current) => ({ ...current, ...fieldErrors }))
      else setSubmitError(err?.code || err?.message || 'common.serverError')
      onError?.(err)
      return undefined
    } finally {
      setSubmitting(false)
    }
  }, [submitting, runValidation, values, submit, endpoint, onSuccess, onError, names])

  // Keeps setValues (from outside) and the ref in step when a caller passes an updater.
  valuesRef.current = values

  return {
    values,
    errors,
    touched,
    submitting,
    submitted,
    submitError,
    valid: !hasErrors(errors),
    setValue,
    setValues,
    setErrors,
    field,
    error,
    handleChange,
    handleBlur,
    handleSubmit,
    reset,
  }
}

export default useForm
