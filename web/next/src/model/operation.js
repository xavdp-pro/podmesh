// One mechanism for every operation: the form is drawn from the schema the host itself publishes in
// its capabilities -- kind, gate, fields, types, bounds -- validated here against the same schema,
// sent as one JSON request, and answered with the host's typed result.
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
export const needsUniverse = schema => schema?.kind === 'universe' || (schema?.fields || []).some(f => f.name === 'universe_uuid')

export function initialFields(schema) {
  const o = {}
  for (const f of schema?.fields || []) o[f.name] = f.type === 'boolean' ? false : ''
  return o
}

function toValue(f, raw) {
  if (raw === '' || raw === undefined || raw === null) return undefined
  switch (f.type) {
    case 'integer': {
      if (!/^-?\d+$/.test(String(raw).trim())) throw Error(`${f.name} must be an integer`)
      const n = Number(raw)
      if (f.min !== undefined && n < f.min) throw Error(`${f.name} is at least ${f.min}`)
      if (f.max !== undefined && n > f.max) throw Error(`${f.name} is at most ${f.max}`)
      return n
    }
    case 'number': {
      const n = Number(raw)
      if (!Number.isFinite(n)) throw Error(`${f.name} must be a number`)
      if (f.min !== undefined && n < f.min) throw Error(`${f.name} is at least ${f.min}`)
      if (f.max !== undefined && n > f.max) throw Error(`${f.name} is at most ${f.max}`)
      return n
    }
    case 'boolean': return !!raw
    case 'enum': if (!f.values.includes(raw)) throw Error(`${f.name} must be one of ${f.values.join(', ')}`); return raw
    case 'uuid': if (!UUID_RE.test(raw)) throw Error(`${f.name} must be a UUID`); return raw
    case 'string[]': case 'uuid[]': case 'object[]': case 'object': {
      let v
      try { v = JSON.parse(raw) } catch { throw Error(`${f.name} must be valid JSON`) }
      if (f.type === 'object') { if (!v || typeof v !== 'object' || Array.isArray(v)) throw Error(`${f.name} must be a JSON object`); return v }
      if (!Array.isArray(v)) throw Error(`${f.name} must be a JSON array`)
      if (f.type === 'string[]' && !v.every(x => typeof x === 'string')) throw Error(`${f.name} must be an array of strings`)
      if (f.type === 'object[]' && !v.every(x => x && typeof x === 'object')) throw Error(`${f.name} must be an array of objects`)
      return v
    }
    default: return String(raw)
  }
}

export function buildRequest(schema, operation, fields, universe, authorization) {
  const req = { operation, operation_id: crypto.randomUUID(), authorization_ref: authorization }
  if (needsUniverse(schema)) {
    if (!/^[0-9a-f-]{36}$/i.test(universe || '')) throw Error('universe_uuid must be a UUID')
    req.universe_uuid = universe
  }
  for (const f of schema.fields || []) {
    const v = toValue(f, fields[f.name])
    if (v === undefined) { if (f.required) throw Error(`${f.name} is required`); continue }
    req[f.name] = v
  }
  if (!authorization.trim()) throw Error('An authorization reference is required')
  return req
}
