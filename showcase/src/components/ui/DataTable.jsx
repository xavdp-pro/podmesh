import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import { AlertCircle, ArrowDown, ArrowUp, ArrowUpDown, ChevronLeft, ChevronRight, Columns3, Inbox, Lock, RefreshCw, RotateCcw, Search, X } from 'lucide-react'
import { useI18n } from '../../i18n/I18nProvider'
import useEscapeKey from '../../hooks/useEscapeKey'
import Select from './Select'
import ConfirmModal from './ConfirmModal'
import { isHideable, loadHidden, saveHidden, toggleHidden, visibleColumns } from './columnVisibility'
import {
  applyClientQuery,
  buildListParams,
  clampPage,
  DEFAULT_PAGE_SIZE,
  DEFAULT_PAGE_SIZES,
  getCellValue,
  nextSort,
  pageCount,
  pageRange,
} from './dataTableUtils'

/**
 * Declarative data table with two modes behind one API.
 *
 * Client mode: pass `rows`; sorting, search, filters and paging happen in memory.
 * Server mode: pass `fetcher(params)` resolving `{ rows, total }`; the component
 * sends `{ page, pageSize, sort, dir, q, f_<key> }` (see server/lib/listQuery.js),
 * debounces the search and keeps the previous rows while the next page loads.
 *
 * Columns: [{
 *   key, header (i18n key or plain text), accessor (fn | dotted path), render(row),
 *   sortable, align ('left' | 'right' | 'center'), width (CSS value),
 *   hideBelow ('sm' | 'md' | 'lg' | 'xl'), wrap (allow multi-line instead of truncating),
 *   mobile ('primary' | 'secondary' | 'meta' | 'hidden'), searchable (false to skip),
 *   sortValue(row), searchValue(row), compare(a, b)
 * }]
 * Below `md` the rows become cards: `primary` is the title line, `secondary`
 * the line under it, `meta` a label/value grid, `hidden` left out.
 *
 * Filters: [{ key, label, options: [{ value, label }], placeholder, match(row, value) }]
 * rendered with the mold's Select (never a native select).
 *
 * Actions: `bulkActions` (with `selectable`) and `rowActions` are
 * [{ key, label, icon, danger, confirm: { title, message, confirmLabel, confirmationText }, onAction }];
 * a destructive action declares `confirm` and goes through ConfirmModal.
 *
 * Column picker: `columnPicker` adds a menu to show or hide columns (a column
 * with `hideable: false` stays); the choice is saved per `id` in localStorage
 * (see columnVisibility.js) and "reset columns" clears it.
 */
export default function DataTable({
  id,
  columns: allColumns = [],
  columnPicker = false,
  rows: inputRows,
  rowKey = 'id',
  fetcher,
  filters: filterDefinitions = [],
  searchable = true,
  searchPlaceholder,
  pageSizes = DEFAULT_PAGE_SIZES,
  defaultPageSize = DEFAULT_PAGE_SIZE,
  defaultSort = null,
  selectable = false,
  bulkActions = [],
  rowActions,
  onRowClick,
  onSelectionChange,
  loading: loadingProp = false,
  error: errorProp = null,
  onRetry,
  emptyTitle,
  emptyHint,
  stickyHeader = true,
  toolbar,
  className = '',
  debounceMs = 300,
  reloadKey,
}) {
  const { t, locale } = useI18n()
  const serverMode = typeof fetcher === 'function'
  const keyOf = useCallback((row) => (typeof rowKey === 'function' ? rowKey(row) : row?.[rowKey]), [rowKey])

  const [sort, setSort] = useState(defaultSort)
  const [q, setQ] = useState('')
  const [debouncedQ, setDebouncedQ] = useState('')
  const [filters, setFilters] = useState({})
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(defaultPageSize)
  const [selected, setSelected] = useState(() => new Set())
  const [pendingAction, setPendingAction] = useState(null)
  const [actionBusy, setActionBusy] = useState(false)
  const [hidden, setHidden] = useState(() => (columnPicker ? loadHidden(id, allColumns) : []))
  const columns = useMemo(() => (columnPicker ? visibleColumns(allColumns, hidden) : allColumns), [allColumns, hidden, columnPicker])

  function setHiddenColumns(next) {
    setHidden(next)
    saveHidden(id, next)
  }

  // Server mode state: previous rows stay on screen while the next page loads.
  const [serverRows, setServerRows] = useState([])
  const [serverTotal, setServerTotal] = useState(0)
  const [serverLoading, setServerLoading] = useState(serverMode)
  const [serverError, setServerError] = useState(null)
  const [reloadTick, setReloadTick] = useState(0)
  const requestId = useRef(0)

  useEffect(() => {
    const timer = setTimeout(() => setDebouncedQ(q), debounceMs)
    return () => clearTimeout(timer)
  }, [q, debounceMs])

  const params = useMemo(() => buildListParams({ page, pageSize, sort, q: debouncedQ, filters }), [page, pageSize, sort, debouncedQ, filters])

  useEffect(() => {
    if (!serverMode) return undefined
    const id = ++requestId.current
    setServerLoading(true)
    fetcher(params)
      .then((result) => {
        if (id !== requestId.current) return
        setServerRows(Array.isArray(result?.rows) ? result.rows : [])
        setServerTotal(Number(result?.total) || 0)
        setServerError(null)
      })
      .catch((err) => {
        if (id !== requestId.current) return
        setServerError(err)
      })
      .finally(() => {
        if (id === requestId.current) setServerLoading(false)
      })
    return undefined
  }, [serverMode, fetcher, params, reloadTick, reloadKey])

  const clientResult = useMemo(() => {
    if (serverMode) return null
    return applyClientQuery(inputRows || [], columns, { sort, q: debouncedQ, filters, page, pageSize }, { filterDefinitions, locale })
  }, [serverMode, inputRows, columns, sort, debouncedQ, filters, page, pageSize, filterDefinitions, locale])

  const rows = serverMode ? serverRows : clientResult.rows
  const total = serverMode ? serverTotal : clientResult.total
  const loading = serverMode ? serverLoading : loadingProp
  const error = serverMode ? serverError : errorProp
  const pages = pageCount(total, pageSize)
  const activeFilterCount = Object.values(filters).filter((v) => v !== null && v !== undefined && v !== '').length
  const narrowed = Boolean(debouncedQ) || activeFilterCount > 0

  // A page beyond the last one (after a filter or a delete) snaps back.
  useEffect(() => {
    const clamped = clampPage(page, total, pageSize)
    if (clamped !== page && !loading) setPage(clamped)
  }, [page, total, pageSize, loading])

  // Selection only ever holds keys of rows that still exist on screen.
  useEffect(() => {
    if (!selectable || !selected.size) return
    const visible = new Set(rows.map(keyOf))
    const kept = new Set([...selected].filter((k) => visible.has(k)))
    if (kept.size !== selected.size) setSelected(kept)
  }, [rows, selectable, selected, keyOf])

  useEffect(() => {
    onSelectionChange?.(rows.filter((row) => selected.has(keyOf(row))))
  }, [selected, rows, keyOf, onSelectionChange])

  function resetPage() {
    setPage(1)
  }

  function onSortClick(column) {
    if (!column.sortable) return
    setSort((current) => nextSort(current, column.key))
    resetPage()
  }

  function setFilter(key, value) {
    setFilters((current) => ({ ...current, [key]: value }))
    resetPage()
  }

  function clearNarrowing() {
    setQ('')
    setDebouncedQ('')
    setFilters({})
    resetPage()
  }

  function retry() {
    if (serverMode) setReloadTick((n) => n + 1)
    else onRetry?.()
  }

  function toggleRow(key) {
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  const allSelected = rows.length > 0 && rows.every((row) => selected.has(keyOf(row)))
  const someSelected = rows.some((row) => selected.has(keyOf(row)))

  function toggleAll() {
    setSelected(allSelected ? new Set() : new Set(rows.map(keyOf)))
  }

  const selectedRows = rows.filter((row) => selected.has(keyOf(row)))

  /** Runs an action now, or parks it behind ConfirmModal when it declares `confirm`. */
  function runAction(action, target) {
    if (action.confirm) {
      setPendingAction({ action, target })
      return
    }
    Promise.resolve(action.onAction?.(target)).then(() => {
      if (Array.isArray(target)) setSelected(new Set())
    })
  }

  async function confirmPending() {
    if (!pendingAction) return
    setActionBusy(true)
    try {
      await pendingAction.action.onAction?.(pendingAction.target)
      if (Array.isArray(pendingAction.target)) setSelected(new Set())
      setPendingAction(null)
    } finally {
      setActionBusy(false)
    }
  }

  const actionsFor = useCallback((row) => {
    if (!rowActions) return []
    return typeof rowActions === 'function' ? rowActions(row) || [] : rowActions
  }, [rowActions])
  const hasRowActions = Boolean(rowActions)

  const range = pageRange(page, pageSize, total)
  const showSkeleton = loading && rows.length === 0 && !error
  const showEmpty = !loading && !error && rows.length === 0
  const showError = Boolean(error) && rows.length === 0

  const header = (column) => (column.header ? t(column.header) : '')

  return (
    <div className={`min-w-0 ${className}`}>
      {(searchable || filterDefinitions.length > 0 || toolbar || columnPicker) && (
        <div className="mb-3 flex flex-wrap items-center gap-2">
          {searchable && (
            <div className="relative min-w-0 flex-1 basis-56">
              <Search size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-fg-faint" />
              <input
                value={q}
                onChange={(e) => {
                  setQ(e.target.value)
                  resetPage()
                }}
                placeholder={searchPlaceholder || t('common.search')}
                aria-label={t('table.search')}
                className="w-full rounded-md border border-line bg-surface py-2 pl-9 pr-9 text-sm text-fg focus:border-accent focus:outline-none focus:ring-2 focus:ring-accent/20"
              />
              {q && (
                <button
                  type="button"
                  onClick={() => {
                    setQ('')
                    resetPage()
                  }}
                  className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-1 text-fg-faint hover:bg-surface-2 hover:text-fg"
                  aria-label={t('common.clear')}
                >
                  <X size={14} />
                </button>
              )}
            </div>
          )}
          {filterDefinitions.map((filter) => (
            <div key={filter.key} className="min-w-0 basis-44 grow sm:grow-0">
              <Select
                value={filters[filter.key] ?? null}
                onChange={(value) => setFilter(filter.key, value)}
                options={filter.options || []}
                placeholder={filter.placeholder || (filter.label ? t(filter.label) : t('common.select'))}
                clearable
              />
            </div>
          ))}
          {narrowed && (
            <button type="button" onClick={clearNarrowing} className="inline-flex items-center gap-1 rounded-md px-2 py-2 text-xs font-medium text-fg-muted hover:bg-surface-2 hover:text-fg">
              <X size={13} /> {t('table.resetFilters')}
            </button>
          )}
          {columnPicker && (
            <ColumnPicker
              columns={allColumns}
              hidden={hidden}
              onToggle={(key) => setHiddenColumns(toggleHidden(hidden, key, allColumns))}
              onReset={() => setHiddenColumns([])}
              t={t}
            />
          )}
          {toolbar}
        </div>
      )}

      {selectable && selected.size > 0 && (
        <div className="mb-3 flex flex-wrap items-center gap-2 rounded-lg border border-accent/30 bg-accent-soft px-3 py-2 text-sm text-fg" role="region" aria-label={t('table.bulkActions')}>
          <span className="font-medium">{t('table.selected', { n: selected.size })}</span>
          <div className="flex flex-wrap items-center gap-2">
            {bulkActions.map((action) => (
              <button
                key={action.key}
                type="button"
                onClick={() => runAction(action, selectedRows)}
                className={`inline-flex items-center gap-1 rounded-md border px-2.5 py-1 text-xs font-semibold ${
                  action.danger ? 'border-danger/50 text-danger hover:bg-danger-soft' : 'border-line bg-surface text-fg hover:bg-surface-2'
                }`}
              >
                {action.icon && <action.icon size={13} />}
                {t(action.label)}
              </button>
            ))}
          </div>
          <button type="button" onClick={() => setSelected(new Set())} className="ml-auto rounded p-1 text-fg-faint hover:bg-surface hover:text-fg" aria-label={t('table.clearSelection')}>
            <X size={14} />
          </button>
        </div>
      )}

      {error && rows.length > 0 && (
        <div className="mb-2 flex items-center gap-2 rounded-md border border-danger/40 bg-danger-soft px-3 py-2 text-xs text-danger" role="alert">
          <AlertCircle size={14} className="shrink-0" />
          <span className="min-w-0 flex-1 truncate">{t('table.error')}</span>
          <button type="button" onClick={retry} className="font-semibold underline-offset-2 hover:underline">{t('table.retry')}</button>
        </div>
      )}

      <div className={`relative rounded-xl border border-line bg-surface ${loading && rows.length > 0 ? 'opacity-70' : ''} transition-opacity`} aria-busy={loading || undefined}>
        {loading && rows.length > 0 && <div className="absolute inset-x-0 top-0 h-0.5 animate-pulse rounded-t-xl bg-accent" />}

        {showSkeleton && <Skeleton columns={columns} count={Math.min(pageSize, 6)} />}

        {showError && (
          <div className="flex flex-col items-center gap-2 px-4 py-12 text-center">
            <AlertCircle size={32} className="text-danger" />
            <p className="text-sm font-medium text-fg">{t('table.error')}</p>
            <button type="button" onClick={retry} className="mt-1 inline-flex items-center gap-1.5 rounded-md border border-line px-3 py-1.5 text-sm font-medium text-fg hover:bg-surface-2">
              <RefreshCw size={14} /> {t('table.retry')}
            </button>
          </div>
        )}

        {showEmpty && (
          <div className="flex flex-col items-center gap-2 px-4 py-12 text-center">
            <Inbox size={32} className="text-fg-faint" />
            <p className="text-sm font-medium text-fg">{emptyTitle || t('table.empty')}</p>
            <p className="text-xs text-fg-muted">{narrowed ? t('table.emptyFiltered') : emptyHint}</p>
            {narrowed && (
              <button type="button" onClick={clearNarrowing} className="mt-1 rounded-md border border-line px-3 py-1.5 text-xs font-medium text-fg hover:bg-surface-2">
                {t('table.resetFilters')}
              </button>
            )}
          </div>
        )}

        {rows.length > 0 && (
          <>
            {/* Desktop: a fixed-layout table that never overflows; cells truncate. */}
            <table className="hidden w-full table-fixed border-collapse text-sm md:table">
              <thead>
                <tr>
                  {selectable && (
                    <th scope="col" className={`w-10 border-b border-line bg-surface px-3 py-2 ${stickyHeader ? 'sticky top-0 z-10' : ''}`}>
                      <input
                        type="checkbox"
                        checked={allSelected}
                        ref={(el) => {
                          if (el) el.indeterminate = !allSelected && someSelected
                        }}
                        onChange={toggleAll}
                        aria-label={t('table.selectAll')}
                        className="accent-accent"
                      />
                    </th>
                  )}
                  {columns.map((column) => {
                    const active = sort?.key === column.key
                    const ariaSort = active ? (sort.dir === 'asc' ? 'ascending' : 'descending') : column.sortable ? 'none' : undefined
                    return (
                      <th
                        key={column.key}
                        scope="col"
                        aria-sort={ariaSort}
                        style={column.width ? { width: column.width } : undefined}
                        className={`border-b border-line bg-surface px-3 py-2 text-xs font-bold uppercase tracking-wide text-fg-faint ${alignClass(column.align)} ${hideClass(column.hideBelow)} ${stickyHeader ? 'sticky top-0 z-10' : ''}`}
                      >
                        {column.sortable ? (
                          <button
                            type="button"
                            onClick={() => onSortClick(column)}
                            className={`inline-flex max-w-full items-center gap-1 rounded hover:text-fg ${active ? 'text-accent' : ''}`}
                            title={active ? (sort.dir === 'asc' ? t('table.sortDesc') : t('table.sortNone')) : t('table.sortAsc')}
                          >
                            <span className="truncate">{header(column)}</span>
                            {active ? (sort.dir === 'asc' ? <ArrowUp size={12} className="shrink-0" /> : <ArrowDown size={12} className="shrink-0" />) : <ArrowUpDown size={12} className="shrink-0 opacity-40" />}
                          </button>
                        ) : (
                          <span className="block truncate">{header(column)}</span>
                        )}
                      </th>
                    )
                  })}
                  {hasRowActions && (
                    <th scope="col" className={`w-24 border-b border-line bg-surface px-3 py-2 text-right text-xs font-bold uppercase tracking-wide text-fg-faint ${stickyHeader ? 'sticky top-0 z-10' : ''}`}>
                      <span className="sr-only">{t('table.actions')}</span>
                    </th>
                  )}
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => {
                  const key = keyOf(row)
                  const isSelected = selected.has(key)
                  const clickable = typeof onRowClick === 'function'
                  return (
                    <tr
                      key={key}
                      onClick={clickable ? () => onRowClick(row) : undefined}
                      onKeyDown={clickable ? (e) => {
                        if (e.key === 'Enter' && e.target === e.currentTarget) onRowClick(row)
                      } : undefined}
                      tabIndex={clickable ? 0 : undefined}
                      aria-selected={selectable ? isSelected : undefined}
                      className={`border-b border-line last:border-b-0 ${isSelected ? 'bg-accent-soft/60' : ''} ${clickable ? 'cursor-pointer hover:bg-surface-2 focus:outline-none focus-visible:bg-surface-2' : ''}`}
                    >
                      {selectable && (
                        <td className="px-3 py-2" onClick={(e) => e.stopPropagation()}>
                          <input type="checkbox" checked={isSelected} onChange={() => toggleRow(key)} aria-label={t('table.selectRow')} className="accent-accent" />
                        </td>
                      )}
                      {columns.map((column) => (
                        <Cell key={column.key} row={row} column={column} />
                      ))}
                      {hasRowActions && (
                        <td className="px-3 py-2 text-right" onClick={(e) => e.stopPropagation()}>
                          <div className="inline-flex items-center gap-1">
                            {actionsFor(row).map((action) => (
                              <button
                                key={action.key}
                                type="button"
                                onClick={() => runAction(action, row)}
                                title={t(action.label)}
                                aria-label={t(action.label)}
                                className={`rounded-md border p-1.5 ${action.danger ? 'border-danger/30 text-danger hover:bg-danger-soft' : 'border-line text-fg-muted hover:bg-surface-2 hover:text-fg'}`}
                              >
                                {action.icon ? <action.icon size={14} /> : <span className="px-1 text-xs">{t(action.label)}</span>}
                              </button>
                            ))}
                          </div>
                        </td>
                      )}
                    </tr>
                  )
                })}
              </tbody>
            </table>

            {/* Phones: one card per row. */}
            <ul className="divide-y divide-line md:hidden">
              {rows.map((row) => (
                <Card
                  key={keyOf(row)}
                  row={row}
                  columns={columns}
                  selectable={selectable}
                  selected={selected.has(keyOf(row))}
                  onToggle={() => toggleRow(keyOf(row))}
                  onClick={onRowClick}
                  actions={actionsFor(row)}
                  onAction={(action) => runAction(action, row)}
                  t={t}
                />
              ))}
            </ul>
          </>
        )}
      </div>

      {(total > 0 || page > 1) && (
        <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-fg-muted">
          <span className="whitespace-nowrap">{t('table.range', { from: range.from, to: range.to, total })}</span>
          <div className="flex items-center gap-2">
            <span className="whitespace-nowrap">{t('table.rowsPerPage')}</span>
            <Select
              value={pageSize}
              onChange={(value) => {
                setPageSize(Number(value))
                resetPage()
              }}
              options={pageSizes.map((n) => ({ value: n, label: String(n) }))}
              className="w-20"
            />
          </div>
          <div className="ml-auto flex items-center gap-1">
            <button type="button" onClick={() => setPage((p) => Math.max(1, p - 1))} disabled={page <= 1 || loading} className="rounded-md border border-line p-1.5 text-fg hover:bg-surface-2 disabled:opacity-40" aria-label={t('table.previous')}>
              <ChevronLeft size={14} />
            </button>
            <span className="whitespace-nowrap px-1">{t('table.page', { page, count: pages })}</span>
            <button type="button" onClick={() => setPage((p) => Math.min(pages, p + 1))} disabled={page >= pages || loading} className="rounded-md border border-line p-1.5 text-fg hover:bg-surface-2 disabled:opacity-40" aria-label={t('table.next')}>
              <ChevronRight size={14} />
            </button>
          </div>
        </div>
      )}

      <ConfirmModal
        open={Boolean(pendingAction)}
        title={t(confirmOf(pendingAction).title || 'table.confirmTitle')}
        message={t(confirmOf(pendingAction).message || 'table.confirmMessage', { n: Array.isArray(pendingAction?.target) ? pendingAction.target.length : 1 })}
        confirmLabel={t(confirmOf(pendingAction).confirmLabel || pendingAction?.action.label || 'common.confirm')}
        confirmationText={confirmOf(pendingAction).confirmationText}
        danger={pendingAction ? pendingAction.action.danger !== false : true}
        pending={actionBusy}
        onCancel={() => setPendingAction(null)}
        onConfirm={confirmPending}
      />
    </div>
  )
}

/** Show/hide menu of the column picker: checkboxes in a popover, never a native select. */
function ColumnPicker({ columns, hidden, onToggle, onReset, t }) {
  const [open, setOpen] = useState(false)
  const ref = useRef(null)
  const menuId = useId()

  useEscapeKey(() => setOpen(false), open)

  useEffect(() => {
    if (!open) return undefined
    function onDocMouseDown(e) {
      if (ref.current && !ref.current.contains(e.target)) setOpen(false)
    }
    document.addEventListener('mousedown', onDocMouseDown)
    return () => document.removeEventListener('mousedown', onDocMouseDown)
  }, [open])

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="true"
        aria-expanded={open}
        aria-controls={menuId}
        className={`inline-flex items-center gap-1.5 rounded-md border px-2.5 py-2 text-xs font-medium ${hidden.length ? 'border-accent/40 text-accent' : 'border-line text-fg-muted hover:bg-surface-2 hover:text-fg'}`}
        title={t('table.columnsMenu')}
      >
        <Columns3 size={14} />
        <span>{t('table.columns')}</span>
        {hidden.length > 0 && <span className="rounded-full bg-accent-soft px-1.5 text-[10px] font-bold">{hidden.length}</span>}
      </button>
      {open && (
        <div id={menuId} role="group" aria-label={t('table.columnsMenu')} className="absolute right-0 z-50 mt-1 w-56 rounded-md border border-line bg-surface p-1 shadow-lg">
          <ul className="thin-scrollbar max-h-64 overflow-auto py-1">
            {columns.map((column) => {
              const locked = !isHideable(column)
              const checked = locked || !hidden.includes(column.key)
              return (
                <li key={column.key}>
                  <label className={`flex items-center gap-2 rounded px-2 py-1.5 text-sm ${locked ? 'text-fg-faint' : 'text-fg hover:bg-surface-2'}`}>
                    <input type="checkbox" checked={checked} disabled={locked} onChange={() => onToggle(column.key)} className="accent-accent" />
                    <span className="min-w-0 flex-1 truncate">{column.header ? t(column.header) : column.key}</span>
                    {locked && <Lock size={12} className="shrink-0" aria-label={t('table.columnLocked')} />}
                  </label>
                </li>
              )
            })}
          </ul>
          <div className="border-t border-line p-1">
            <button
              type="button"
              onClick={() => {
                onReset()
                setOpen(false)
              }}
              disabled={hidden.length === 0}
              className="flex w-full items-center gap-1.5 rounded px-2 py-1.5 text-left text-xs font-medium text-fg-muted hover:bg-surface-2 hover:text-fg disabled:opacity-50"
            >
              <RotateCcw size={12} /> {t('table.resetColumns')}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

/** `confirm` may be `true` (default wording) or `{ title, message, confirmLabel, confirmationText }` (i18n keys). */
function confirmOf(pending) {
  const value = pending?.action?.confirm
  return value && typeof value === 'object' ? value : {}
}

const HIDE = { sm: 'hidden sm:table-cell', md: 'hidden md:table-cell', lg: 'hidden lg:table-cell', xl: 'hidden xl:table-cell' }
const ALIGN = { right: 'text-right', center: 'text-center' }

function hideClass(bp) {
  return HIDE[bp] || ''
}

function alignClass(align) {
  return ALIGN[align] || 'text-left'
}

function renderCell(row, column) {
  if (typeof column.render === 'function') return column.render(row)
  const value = getCellValue(row, column)
  if (value === null || value === undefined) return ''
  if (value instanceof Date) return value.toLocaleString()
  return String(value)
}

function Cell({ row, column }) {
  const content = renderCell(row, column)
  const title = typeof content === 'string' ? content : undefined
  return (
    <td
      title={column.wrap ? undefined : title}
      className={`px-3 py-2 align-top text-fg ${alignClass(column.align)} ${hideClass(column.hideBelow)} ${column.wrap ? 'break-words' : 'truncate'} ${column.className || ''}`}
    >
      {content}
    </td>
  )
}

function Card({ row, columns, selectable, selected, onToggle, onClick, actions, onAction, t }) {
  const primary = columns.find((c) => c.mobile === 'primary') || columns[0]
  const secondary = columns.find((c) => c.mobile === 'secondary') || (columns[1]?.mobile === undefined ? columns[1] : null)
  const meta = columns.filter((c) => c !== primary && c !== secondary && c.mobile !== 'hidden')
  const clickable = typeof onClick === 'function'
  return (
    <li className={`flex gap-3 px-4 py-3 ${selected ? 'bg-accent-soft/60' : ''}`}>
      {selectable && (
        <input type="checkbox" checked={selected} onChange={onToggle} aria-label={t('table.selectRow')} className="mt-1 shrink-0 self-start accent-accent" />
      )}
      <div
        className={`min-w-0 flex-1 ${clickable ? 'cursor-pointer' : ''}`}
        onClick={clickable ? () => onClick(row) : undefined}
        onKeyDown={clickable ? (e) => {
          if (e.key === 'Enter' && e.target === e.currentTarget) onClick(row)
        } : undefined}
        role={clickable ? 'button' : undefined}
        tabIndex={clickable ? 0 : undefined}
      >
        {primary && <div className="truncate text-sm font-semibold text-fg">{renderCell(row, primary)}</div>}
        {secondary && <div className="mt-0.5 truncate text-xs text-fg-muted">{renderCell(row, secondary)}</div>}
        {meta.length > 0 && (
          <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
            {meta.map((column) => (
              <div key={column.key} className="contents">
                <dt className="truncate font-medium text-fg-faint">{column.header ? t(column.header) : ''}</dt>
                <dd className="min-w-0 break-words text-fg">{renderCell(row, column)}</dd>
              </div>
            ))}
          </dl>
        )}
        {actions.length > 0 && (
          <div className="mt-3 flex flex-wrap gap-2 border-t border-line pt-2" onClick={(e) => e.stopPropagation()}>
            {actions.map((action) => (
              <button
                key={action.key}
                type="button"
                onClick={() => onAction(action)}
                className={`inline-flex min-h-9 items-center gap-1 rounded-md border px-2.5 text-xs font-medium ${action.danger ? 'border-danger/30 text-danger hover:bg-danger-soft' : 'border-line text-fg hover:bg-surface-2'}`}
              >
                {action.icon && <action.icon size={13} />}
                {t(action.label)}
              </button>
            ))}
          </div>
        )}
      </div>
    </li>
  )
}

function Skeleton({ columns, count }) {
  const widths = ['w-3/4', 'w-1/2', 'w-2/3', 'w-1/3', 'w-5/6']
  return (
    <div className="animate-pulse divide-y divide-line" aria-hidden="true">
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className="flex gap-4 px-4 py-3">
          {columns.slice(0, 4).map((column, j) => (
            <div key={column.key} className={`h-3 rounded bg-surface-2 ${widths[(i + j) % widths.length]}`} />
          ))}
        </div>
      ))}
    </div>
  )
}
