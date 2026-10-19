/**
 * Pure helpers behind DataTable: sorting, searching, filtering and paging of
 * in-memory rows (client mode) and the query the component sends in server
 * mode. No React here, so tests run in plain Node.
 */

export const DEFAULT_PAGE_SIZES = [10, 20, 50]
export const DEFAULT_PAGE_SIZE = 20

/** Reads a cell: `accessor` (function or dotted path), else `row[key]`. */
export function getCellValue(row, column) {
  if (!row || !column) return undefined
  const { accessor, key } = column
  if (typeof accessor === 'function') return accessor(row)
  const path = typeof accessor === 'string' ? accessor : key
  return String(path).split('.').reduce((current, part) => current?.[part], row)
}

/**
 * Numbers and dates compare numerically, everything else as text with the
 * locale's natural order ("item 2" before "item 10"). Nulls sink to the end.
 */
export function compareValues(left, right, locale = 'fr') {
  const l = left instanceof Date ? left.getTime() : left
  const r = right instanceof Date ? right.getTime() : right
  const lEmpty = l === null || l === undefined || l === ''
  const rEmpty = r === null || r === undefined || r === ''
  if (lEmpty && rEmpty) return 0
  if (lEmpty) return 1
  if (rEmpty) return -1
  if (typeof l === 'number' && typeof r === 'number') return l - r
  if (typeof l === 'boolean' && typeof r === 'boolean') return Number(l) - Number(r)
  return String(l).localeCompare(String(r), locale, { sensitivity: 'base', numeric: true })
}

/** Header click cycle: none -> asc -> desc -> none. Another column starts at asc. */
export function nextSort(current, key) {
  if (!current || current.key !== key) return { key, dir: 'asc' }
  if (current.dir === 'asc') return { key, dir: 'desc' }
  return null
}

/** Stable sort of a copy; `sort` is `{ key, dir }` or null (original order). */
export function sortRows(rows, columns, sort, locale) {
  if (!sort?.key) return rows
  const column = columns.find((c) => c.key === sort.key)
  if (!column) return rows
  const sign = sort.dir === 'desc' ? -1 : 1
  const compare = column.compare || compareValues
  return rows
    .map((row, index) => ({ row, index, value: column.sortValue ? column.sortValue(row) : getCellValue(row, column) }))
    .sort((a, b) => sign * compare(a.value, b.value, locale) || a.index - b.index)
    .map((entry) => entry.row)
}

/** Case-insensitive substring search over the columns not marked `searchable: false`. */
export function searchRows(rows, columns, query) {
  const q = String(query || '').trim().toLowerCase()
  if (!q) return rows
  const searchable = columns.filter((c) => c.searchable !== false)
  return rows.filter((row) => searchable.some((column) => {
    const value = column.searchValue ? column.searchValue(row) : getCellValue(row, column)
    return value !== null && value !== undefined && String(value).toLowerCase().includes(q)
  }))
}

/**
 * Applies exact-match filters. `filters` is `{ key: value }` (empty values are
 * ignored); a filter definition may carry `match(row, value)` for anything
 * other than equality on `row[key]`.
 */
export function filterRows(rows, filters, definitions = []) {
  const active = Object.entries(filters || {}).filter(([, value]) => value !== null && value !== undefined && value !== '')
  if (!active.length) return rows
  return rows.filter((row) => active.every(([key, value]) => {
    const def = definitions.find((d) => d.key === key)
    if (def?.match) return def.match(row, value)
    const cell = key.split('.').reduce((current, part) => current?.[part], row)
    return String(cell) === String(value)
  }))
}

export function pageCount(total, pageSize) {
  return Math.max(1, Math.ceil((Number(total) || 0) / Math.max(1, pageSize)))
}

/** Keeps `page` inside 1..pageCount. */
export function clampPage(page, total, pageSize) {
  return Math.min(Math.max(1, Number(page) || 1), pageCount(total, pageSize))
}

export function paginateRows(rows, page, pageSize) {
  const p = clampPage(page, rows.length, pageSize)
  const start = (p - 1) * pageSize
  return rows.slice(start, start + pageSize)
}

/** Sort, search, filter and page in-memory rows in one call (client mode). */
export function applyClientQuery(rows, columns, { sort, q, filters, page, pageSize }, { filterDefinitions = [], locale } = {}) {
  const filtered = filterRows(searchRows(rows, columns, q), filters, filterDefinitions)
  const sorted = sortRows(filtered, columns, sort, locale)
  return { rows: paginateRows(sorted, page, pageSize), total: sorted.length }
}

/**
 * The query DataTable sends in server mode, matching server/lib/listQuery.js:
 * `{ page, pageSize, sort, dir, q, f_<key> }`. Empty values are left out.
 */
export function buildListParams({ page = 1, pageSize = DEFAULT_PAGE_SIZE, sort = null, q = '', filters = {} } = {}) {
  const params = { page, pageSize }
  if (sort?.key) {
    params.sort = sort.key
    params.dir = sort.dir === 'desc' ? 'desc' : 'asc'
  }
  const text = String(q || '').trim()
  if (text) params.q = text
  for (const [key, value] of Object.entries(filters || {})) {
    if (value !== null && value !== undefined && value !== '') params[`f_${key}`] = value
  }
  return params
}

/** `{ a: 1, b: 'x y' }` -> `a=1&b=x+y`; null, undefined and '' are skipped. */
export function toQueryString(params = {}) {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value === null || value === undefined || value === '') continue
    search.set(key, String(value))
  }
  return search.toString()
}

/** Range shown in the footer: `{ from, to }` as 1-based row numbers, 0-0 when empty. */
export function pageRange(page, pageSize, total) {
  if (!total) return { from: 0, to: 0 }
  const from = (page - 1) * pageSize + 1
  return { from, to: Math.min(total, from + pageSize - 1) }
}
