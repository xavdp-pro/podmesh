/**
 * Pure validation helpers behind useForm. No React here, so tests run in plain
 * Node and the server could reuse the same rules.
 *
 * A rule is `(value, values) => error | undefined` where `error` is an i18n key
 * (string) or `{ key, vars }` for messages with placeholders. `rules` given to
 * `validateValues` is `{ fieldName: rule | rule[] }`; the first failing rule of
 * a field wins.
 */

export const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/

function isBlank(value) {
  if (value === null || value === undefined) return true
  if (typeof value === 'string') return value.trim() === ''
  if (Array.isArray(value)) return value.length === 0
  return false
}

/** Fails on null, undefined, blank strings, empty arrays and `false` when `checked` is set. */
export function required(key = 'form.required', { checked = false } = {}) {
  return (value) => (isBlank(value) || (checked && value !== true) ? key : undefined)
}

/** Loose e-mail shape (`a@b.c`); the server stays the judge. Blank passes: combine with `required`. */
export function email(key = 'form.email') {
  return (value) => (isBlank(value) || EMAIL_RE.test(String(value).trim()) ? undefined : key)
}

export function minLength(n, key = 'form.minLength') {
  return (value) => (isBlank(value) || String(value).trim().length >= n ? undefined : { key, vars: { n } })
}

export function maxLength(n, key = 'form.maxLength') {
  return (value) => (isBlank(value) || String(value).length <= n ? undefined : { key, vars: { n } })
}

/** The value must be one of `allowed` (compared with `===`). Blank passes. */
export function oneOf(allowed, key = 'form.invalidChoice') {
  return (value) => (isBlank(value) || allowed.includes(value) ? undefined : key)
}

/** `pattern(/^\d+$/, 'form.digits')`. Blank passes. */
export function pattern(re, key = 'form.pattern') {
  return (value) => (isBlank(value) || re.test(String(value)) ? undefined : key)
}

/** Runs rules in order and returns the first error. */
export function compose(...rules) {
  return (value, values) => {
    for (const rule of rules.flat()) {
      const error = rule?.(value, values)
      if (error) return error
    }
    return undefined
  }
}

/**
 * Validates every field of `rules` against `values`.
 * @returns {Record<string, string | { key: string, vars?: object }>} errors, empty when valid
 */
export function validateValues(values = {}, rules = {}) {
  const errors = {}
  for (const [name, rule] of Object.entries(rules)) {
    const error = compose(rule)(values[name], values)
    if (error) errors[name] = error
  }
  return errors
}

export function hasErrors(errors) {
  return Boolean(errors) && Object.keys(errors).length > 0
}

/**
 * Keeps the field errors sent by the API (`{ error, fields: { email: 'users.emailTaken' } }`)
 * that belong to known fields; unknown names are dropped so a rogue answer cannot
 * stick a message on nothing.
 */
export function pickFieldErrors(fields, knownNames) {
  const out = {}
  if (!fields || typeof fields !== 'object') return out
  for (const [name, value] of Object.entries(fields)) {
    if (!knownNames.includes(name)) continue
    if (typeof value === 'string' || (value && typeof value === 'object' && typeof value.key === 'string')) out[name] = value
  }
  return out
}
