import { useEffect, useId, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { GLOSSARY_PATTERN, lookup } from '../data/glossary'

// Shared reading aid from xavdp.pro: hover, keyboard and touch definitions.

function portalRoot() {
  return document.body
}

function placePanel(box) {
  const width = Math.min(288, window.innerWidth - 24)
  const gap = 10
  const estimate = 150
  const centerX = box.left + box.width / 2

  let left = centerX - width / 2
  left = Math.max(12, Math.min(left, window.innerWidth - width - 12))

  const arrowLeft = Math.max(18, Math.min(width - 18, centerX - left))
  const spaceAbove = box.top
  const spaceBelow = window.innerHeight - box.bottom
  const below = spaceAbove < estimate + 28 && spaceBelow > spaceAbove

  if (below) {
    return {
      top: box.bottom + gap,
      bottom: 'auto',
      left,
      width,
      arrowLeft,
      placement: 'below',
    }
  }

  return {
    top: 'auto',
    bottom: window.innerHeight - box.top + gap,
    left,
    width,
    arrowLeft,
    placement: 'above',
  }
}

function Term({ text, lang }) {
  const entry = lookup(text)
  const [open, setOpen] = useState(false)
  const [place, setPlace] = useState(null)
  const triggerRef = useRef(null)
  const panelRef = useRef(null)
  const closeTimer = useRef(0)
  const lastPointer = useRef('mouse')
  const pointerFocus = useRef(false)
  const id = useId()

  const close = () => {
    window.clearTimeout(closeTimer.current)
    setOpen(false)
  }

  const scheduleClose = () => {
    window.clearTimeout(closeTimer.current)
    closeTimer.current = window.setTimeout(() => setOpen(false), 220)
  }

  const cancelClose = () => window.clearTimeout(closeTimer.current)

  const reveal = () => {
    cancelClose()
    const box = triggerRef.current?.getBoundingClientRect()
    if (!box) return
    setPlace(placePanel(box))
    setOpen(true)
  }

  useEffect(() => {
    if (!open) return undefined
    const onKeyDown = (event) => { if (event.key === 'Escape') close() }
    const onPointerDown = (event) => {
      const t = event.target
      if (triggerRef.current?.contains(t) || panelRef.current?.contains(t)) return
      close()
    }
    const onScroll = () => close()
    document.addEventListener('keydown', onKeyDown)
    document.addEventListener('pointerdown', onPointerDown)
    window.addEventListener('scroll', onScroll, { passive: true, capture: true })
    window.addEventListener('resize', onScroll)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      document.removeEventListener('pointerdown', onPointerDown)
      window.removeEventListener('scroll', onScroll, { capture: true })
      window.removeEventListener('resize', onScroll)
    }
  }, [open])

  useEffect(() => () => window.clearTimeout(closeTimer.current), [])

  if (!entry) return text

  const rememberPointer = (event) => {
    lastPointer.current = event.pointerType || 'mouse'
    pointerFocus.current = lastPointer.current !== 'mouse'
  }

  const onTap = (event) => {
    event.preventDefault()
    event.stopPropagation()
    pointerFocus.current = false
    if (lastPointer.current === 'mouse') return
    if (open) close()
    else reveal()
  }

  const label = entry.label[lang] ?? entry.label.fr
  const definition = entry.definition[lang] ?? entry.definition.fr

  return (
    <span className="relative inline">
      <span
        ref={triggerRef}
        role="button"
        tabIndex={0}
        data-open={open ? 'true' : undefined}
        onPointerDown={rememberPointer}
        onClick={onTap}
        onKeyDown={(event) => {
          if (event.key !== 'Enter' && event.key !== ' ') return
          event.preventDefault()
          if (open) close()
          else reveal()
        }}
        onFocus={() => { if (!pointerFocus.current) reveal() }}
        onBlur={scheduleClose}
        onPointerEnter={(event) => { if (event.pointerType === 'mouse') reveal() }}
        onMouseLeave={scheduleClose}
        aria-expanded={open}
        aria-describedby={open ? id : undefined}
        className="site-glossary-term"
      >
        {text}
      </span>

      {open && place
        ? createPortal(
          <div
            ref={panelRef}
            id={id}
            role="tooltip"
            data-placement={place.placement}
            onClick={(event) => event.stopPropagation()}
            onMouseEnter={cancelClose}
            onMouseLeave={scheduleClose}
            style={{
              top: place.top,
              bottom: place.bottom,
              left: place.left,
              width: place.width,
              '--glossary-arrow-left': `${place.arrowLeft}px`,
            }}
            className="site-glossary-panel"
          >
            <p className="site-glossary-panel__label">{label}</p>
            <p className="site-glossary-panel__body">{definition}</p>
            {entry.href ? (
              <a
                href={entry.href}
                target="_blank"
                rel="noreferrer"
                className="site-glossary-panel__link"
                onClick={(event) => event.stopPropagation()}
              >
                {(entry.linkLabel?.[lang] ?? entry.linkLabel?.fr) || entry.href}
              </a>
            ) : null}
          </div>,
          portalRoot(),
        )
        : null}
    </span>
  )
}

export default function GlossaryText({ children, lang }) {
  if (typeof children !== 'string') return children

  GLOSSARY_PATTERN.lastIndex = 0
  const parts = children.split(GLOSSARY_PATTERN)
  if (parts.length === 1) return children

  return (
    <span className="site-glossary-inline">
      {parts.map((part, index) => {
        if (index % 2 === 1) return <Term key={`${index}-${part}`} text={part} lang={lang} />
        return part
      })}
    </span>
  )
}
