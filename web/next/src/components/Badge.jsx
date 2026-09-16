// A state in one word, coloured: green for what runs or holds, amber for what waits, terracotta for what fails, muted otherwise.
const TONES = {
  green: 'bg-green-100 text-green-700',
  amber: 'bg-amber-100 text-amber-600',
  red: 'bg-terra-100 text-terra-600',
  muted: 'bg-cream-200 text-ink-500',
}
export function stateTone(state) {
  if (state === 'running') return 'green'
  if (state === 'paused') return 'amber'
  if (['exited', 'stopped', 'created', 'configured'].includes(state)) return 'muted'
  if (!state) return 'muted'
  return 'muted'
}
export default function Badge({ tone = 'muted', children, className = '' }) {
  return <span className={`inline-flex items-center gap-1 whitespace-nowrap rounded-full px-2 py-0.5 text-xs font-medium ${TONES[tone] || TONES.muted} ${className}`}>{children}</span>
}
