import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { ChevronDown, X } from 'lucide-react'
import { useI18n } from '../../i18n/I18nProvider'

/** Above this many options the list gets a search box automatically (rule 4). */
export const SEARCH_THRESHOLD = 6

/**
 * Styled select, never the native select element. Keyboard: arrows, Enter/Space,
 * Escape. Long lists (> SEARCH_THRESHOLD, or `searchable`) show a search input
 * with a clear cross; `clearable` adds a cross on the field to unset the value.
 *
 * Props: value, onChange(value), options [{ value, label, icon?, disabled? }],
 *        placeholder?, searchable?, clearable?, disabled?, className?
 */
export default function Select({
  value,
  onChange,
  options = [],
  placeholder,
  searchable,
  clearable = false,
  disabled = false,
  className = '',
  id,
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [focused, setFocused] = useState(-1)
  const [query, setQuery] = useState('')
  const containerRef = useRef(null)
  const listRef = useRef(null)
  const searchRef = useRef(null)
  const listId = useId()

  const canSearch = searchable ?? options.length > SEARCH_THRESHOLD
  const selected = options.find((o) => o.value === value) || null
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase()
    return q ? options.filter((o) => String(o.label).toLowerCase().includes(q)) : options
  }, [options, query])

  useEffect(() => {
    function onDocMouseDown(e) {
      if (containerRef.current && !containerRef.current.contains(e.target)) setOpen(false)
    }
    document.addEventListener('mousedown', onDocMouseDown)
    return () => document.removeEventListener('mousedown', onDocMouseDown)
  }, [])

  useEffect(() => {
    if (open && canSearch) searchRef.current?.focus()
  }, [open, canSearch])

  useEffect(() => {
    if (open && listRef.current && focused >= 0) listRef.current.children[focused]?.scrollIntoView({ block: 'nearest' })
  }, [focused, open])

  function openList() {
    if (disabled) return
    setQuery('')
    const idx = visible.findIndex((o) => o.value === value)
    setFocused(idx >= 0 ? idx : 0)
    setOpen(true)
  }

  function pick(option) {
    if (!option || option.disabled) return
    onChange?.(option.value)
    setOpen(false)
  }

  function onKeyDown(e) {
    if (!open) {
      if (['Enter', ' ', 'ArrowDown'].includes(e.key)) {
        e.preventDefault()
        openList()
      }
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setFocused((i) => Math.min(i + 1, visible.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setFocused((i) => Math.max(i - 1, 0))
    } else if (e.key === 'Enter' || (e.key === ' ' && !canSearch)) {
      e.preventDefault()
      pick(visible[focused])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      setOpen(false)
    }
  }

  return (
    <div ref={containerRef} className={`relative ${className}`}>
      <div className="flex items-stretch">
        <button
          type="button"
          id={id}
          disabled={disabled}
          onClick={() => (open ? setOpen(false) : openList())}
          onKeyDown={onKeyDown}
          aria-haspopup="listbox"
          aria-expanded={open}
          aria-controls={listId}
          className={`flex w-full items-center justify-between gap-2 border border-line bg-surface px-3 py-2 text-left text-sm text-fg transition-colors focus:border-accent focus:outline-none focus:ring-2 focus:ring-accent/20 disabled:opacity-60 ${
            clearable && selected ? 'rounded-l-md' : 'rounded-md'
          }`}
        >
          <span className={`flex min-w-0 items-center gap-2 ${selected ? '' : 'text-fg-faint'}`}>
            {selected?.icon && <selected.icon size={15} className="shrink-0 text-accent" />}
            <span className="truncate">{selected ? selected.label : placeholder || t('common.select')}</span>
          </span>
          <ChevronDown size={15} className={`shrink-0 text-fg-faint transition-transform ${open ? 'rotate-180' : ''}`} />
        </button>
        {clearable && selected && !disabled && (
          <button
            type="button"
            onClick={() => onChange?.(null)}
            className="flex shrink-0 items-center rounded-r-md border border-l-0 border-line bg-surface px-2 text-fg-faint hover:bg-surface-2 hover:text-danger"
            aria-label={t('common.clear')}
          >
            <X size={14} />
          </button>
        )}
      </div>

      {open && (
        <div className="absolute z-50 mt-1 w-full overflow-hidden rounded-md border border-line bg-surface shadow-lg">
          {canSearch && (
            <div className="border-b border-line bg-surface-2 px-2 py-2">
              <div className="relative">
                <input
                  ref={searchRef}
                  value={query}
                  onChange={(e) => {
                    setQuery(e.target.value)
                    setFocused(0)
                  }}
                  onKeyDown={onKeyDown}
                  placeholder={t('common.search')}
                  className="w-full rounded-md border border-line bg-surface py-1.5 pl-3 pr-9 text-sm text-fg focus:border-accent focus:outline-none focus:ring-2 focus:ring-accent/20"
                />
                {query && (
                  <button
                    type="button"
                    onClick={() => {
                      setQuery('')
                      setFocused(0)
                      searchRef.current?.focus()
                    }}
                    className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-1 text-fg-faint hover:bg-surface-2 hover:text-fg"
                    aria-label={t('common.clear')}
                  >
                    <X size={14} />
                  </button>
                )}
              </div>
            </div>
          )}
          <ul ref={listRef} id={listId} role="listbox" className="thin-scrollbar max-h-56 overflow-auto py-1">
            {visible.length === 0 ? (
              <li className="px-3 py-2 text-sm text-fg-muted">{t('common.noResults')}</li>
            ) : (
              visible.map((option, i) => (
                <li
                  key={String(option.value)}
                  role="option"
                  aria-selected={option.value === value}
                  aria-disabled={option.disabled || undefined}
                  onMouseEnter={() => setFocused(i)}
                  onMouseDown={(e) => {
                    e.preventDefault()
                    pick(option)
                  }}
                  className={`flex items-center gap-2 px-3 py-2 text-sm select-none ${
                    option.disabled
                      ? 'cursor-not-allowed text-fg-faint'
                      : i === focused
                        ? 'bg-accent-soft text-accent'
                        : option.value === value
                          ? 'font-medium text-accent'
                          : 'text-fg'
                  }`}
                >
                  {option.icon && <option.icon size={14} className="shrink-0" />}
                  <span className="truncate">{option.label}</span>
                </li>
              ))
            )}
          </ul>
        </div>
      )}
    </div>
  )
}
