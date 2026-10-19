import React, { useId } from 'react'

/**
 * SVG flags, identical to xavdp.pro on gbs-l0.
 * Emoji flags do not render properly on many Linux desktops.
 */

const SHELL = 'h-3.5 w-[1.3125rem] shrink-0 rounded-[2px] shadow-[0_0_0_1px_rgba(0,0,0,0.12)]'

export function Flag({ code, className = '' }) {
  const uid = useId().replace(/:/g, '')
  const shell = `${SHELL} ${className}`

  if (code === 'fr') {
    return (
      <svg viewBox="0 0 3 2" className={shell} aria-hidden="true">
        <rect width="1" height="2" fill="#002395" />
        <rect x="1" width="1" height="2" fill="#fff" />
        <rect x="2" width="1" height="2" fill="#ed2939" />
      </svg>
    )
  }

  if (code === 'es') {
    return (
      <svg viewBox="0 0 3 2" className={shell} aria-hidden="true">
        <rect width="3" height="2" fill="#c60b1e" />
        <rect y="0.5" width="3" height="1" fill="#ffc400" />
      </svg>
    )
  }

  // United Kingdom (language `en`)
  return (
    <svg viewBox="0 0 60 30" className={shell} aria-hidden="true">
      <clipPath id={`${uid}-s`}>
        <path d="M0,0v30h60v-30z" />
      </clipPath>
      <clipPath id={`${uid}-t`}>
        <path d="M30,15h30v15zv15h-30zv-15h-30zv-15z" />
      </clipPath>
      <g clipPath={`url(#${uid}-s)`}>
        <path d="M0,0v30h60v-30z" fill="#012169" />
        <path d="M0,0 60,30 M60,0 0,30" stroke="#fff" strokeWidth="6" />
        <path d="M0,0 60,30 M60,0 0,30" clipPath={`url(#${uid}-t)`} stroke="#c8102e" strokeWidth="4" />
        <path d="M30,0v30 M0,15h60" stroke="#fff" strokeWidth="10" />
        <path d="M30,0v30 M0,15h60" stroke="#c8102e" strokeWidth="6" />
      </g>
    </svg>
  )
}
