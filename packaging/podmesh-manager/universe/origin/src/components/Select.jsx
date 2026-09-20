import { useEffect, useRef, useState } from 'react'
import { ChevronDown, X } from 'lucide-react'

// A styled list, never the browser's: keyboard-driven (arrows, Enter, Escape); with more than four
// options, or when asked, a field that filters and a cross that empties it -- the operator's rule.
export default function Select({ value, onChange, options, placeholder = '—', className = '', searchable = false, searchPlaceholder = '…', noResult = '—', clearSearchLabel = 'Clear the search', label }) {
  const [open, setOpen] = useState(false)
  const [focused, setFocused] = useState(-1)
  const [query, setQuery] = useState('')
  const box = useRef(null)
  const list = useRef(null)
  const canSearch = searchable || options.length > 4
  const selected = options.find(o => o.value === value)
  const selectedIndex = options.findIndex(o => o.value === value)
  const visible = canSearch && query.trim() ? options.filter(o => o.label.toLowerCase().includes(query.trim().toLowerCase())) : options

  useEffect(() => {
    const onDown = e => { if (box.current && !box.current.contains(e.target)) setOpen(false) }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [])
  useEffect(() => { if (open && list.current && focused >= 0) list.current.children[focused]?.scrollIntoView({ block: 'nearest' }) }, [focused, open])

  const toggle = () => setOpen(v => { if (!v) { setFocused(selectedIndex >= 0 ? selectedIndex : 0); if (canSearch) setQuery('') } return !v })
  const pick = o => { onChange(o.value); setOpen(false) }
  const onKeyDown = e => {
    if (!open) { if (['Enter', ' ', 'ArrowDown'].includes(e.key)) { e.preventDefault(); setOpen(true); setFocused(selectedIndex >= 0 ? selectedIndex : 0) } return }
    if (e.key === 'ArrowDown') { e.preventDefault(); setFocused(i => Math.min(i + 1, visible.length - 1)) }
    else if (e.key === 'ArrowUp') { e.preventDefault(); setFocused(i => Math.max(i - 1, 0)) }
    else if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); if (focused >= 0 && visible[focused]) pick(visible[focused]) }
    else if (e.key === 'Escape') { e.preventDefault(); setOpen(false) }
  }
  return (
    <div ref={box} className={`relative ${className}`}>
      <button type="button" onClick={toggle} onKeyDown={onKeyDown} aria-haspopup="listbox" aria-expanded={open} aria-label={label}
        className="flex w-full items-center justify-between gap-2 rounded-md border border-cream-400 bg-cream-50 px-3 py-2 text-left text-sm transition-colors focus:border-green-600 focus:outline-none focus:ring-2 focus:ring-green-600/20">
        <span className={`truncate ${selected ? 'text-ink-900' : 'text-ink-500'}`}>{selected ? selected.label : placeholder}</span>
        <ChevronDown size={15} className={`shrink-0 text-ink-500 transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="absolute z-50 mt-1 w-full overflow-hidden rounded-md border border-cream-300 bg-cream-50 shadow-lg">
          {canSearch && (
            <div className="border-b border-cream-300 bg-cream-100 px-3 py-2">
              <div className="relative">
                <input value={query} onChange={e => { setQuery(e.target.value); setFocused(0) }} onKeyDown={onKeyDown} placeholder={searchPlaceholder} autoFocus
                  className="w-full rounded-md border border-cream-400 bg-cream-50 py-2 pl-3 pr-9 text-sm focus:border-green-600 focus:outline-none focus:ring-2 focus:ring-green-600/20" />
                {query && <button type="button" onClick={() => { setQuery(''); setFocused(0) }} aria-label={clearSearchLabel} className="absolute right-2 top-1/2 -translate-y-1/2 rounded p-1 text-ink-500 hover:bg-cream-200 hover:text-ink-900"><X size={14} /></button>}
              </div>
            </div>
          )}
          <ul ref={list} role="listbox" className="max-h-52 overflow-auto py-1">
            {visible.length === 0 ? <li className="px-3 py-2 text-sm text-ink-500">{noResult}</li> : visible.map((o, i) => (
              <li key={o.value} role="option" aria-selected={o.value === value} onMouseEnter={() => setFocused(i)} onMouseDown={() => pick(o)}
                className={`cursor-pointer select-none px-3 py-2 text-sm transition-colors ${i === focused ? 'bg-green-100 text-green-700' : o.value === value ? 'font-medium text-green-600' : 'text-ink-700 hover:bg-cream-200'}`}>{o.label}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  )
}
