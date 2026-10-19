/**
 * Pure helpers behind the column picker of DataTable: which columns may be
 * hidden, which are hidden, and the localStorage round trip (wrapped, so a
 * table works without storage: private mode, blocked site data, tests).
 *
 * The persisted value is the list of hidden keys, under `datatable:<id>:hidden`.
 */

export const STORAGE_PREFIX = 'datatable:'

export function storageKey(tableId) {
  return `${STORAGE_PREFIX}${tableId}:hidden`
}

/** A column stays unless it says `hideable: false`. */
export function isHideable(column) {
  return column?.hideable !== false
}

/** Sanitises a list of hidden keys against the columns: unknown or non-hideable keys are dropped. */
export function normaliseHidden(hidden, columns) {
  if (!Array.isArray(hidden)) return []
  const allowed = new Set(columns.filter(isHideable).map((c) => c.key))
  return [...new Set(hidden.filter((key) => allowed.has(key)))]
}

export function visibleColumns(columns, hidden = []) {
  const set = new Set(hidden)
  return columns.filter((column) => !set.has(column.key) || !isHideable(column))
}

/** Adds or removes a key; a non-hideable column never enters the list. */
export function toggleHidden(hidden, key, columns) {
  const column = columns.find((c) => c.key === key)
  if (!column || !isHideable(column)) return hidden
  return hidden.includes(key) ? hidden.filter((k) => k !== key) : [...hidden, key]
}

/** Reads the saved list for a table id (empty when absent, unreadable or without storage). */
export function loadHidden(tableId, columns, storage) {
  if (!tableId) return []
  try {
    const store = storage || globalThis.localStorage
    const raw = store?.getItem(storageKey(tableId))
    return normaliseHidden(raw ? JSON.parse(raw) : [], columns)
  } catch {
    return []
  }
}

/** Saves the list; an empty list removes the entry. Never throws. */
export function saveHidden(tableId, hidden, storage) {
  if (!tableId) return
  try {
    const store = storage || globalThis.localStorage
    if (!store) return
    if (hidden.length) store.setItem(storageKey(tableId), JSON.stringify(hidden))
    else store.removeItem(storageKey(tableId))
  } catch {
    // storage unavailable
  }
}
