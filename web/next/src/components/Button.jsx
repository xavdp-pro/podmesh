// The console's buttons: primary (green), danger (terracotta), plain (bordered) -- always type="button".
const KINDS = {
  primary: 'bg-green-600 text-cream-50 hover:opacity-90',
  danger: 'bg-terra-600 text-cream-50 hover:opacity-90',
  plain: 'border border-cream-400 text-ink-700 hover:bg-cream-200',
}
export default function Button({ kind = 'plain', icon: Icon = null, children, className = '', ...rest }) {
  return (
    <button type="button" className={`inline-flex items-center gap-1.5 rounded-md px-3 py-2 text-sm font-medium transition disabled:opacity-50 ${KINDS[kind]} ${className}`} {...rest}>
      {Icon && <Icon size={15} />}{children}
    </button>
  )
}
