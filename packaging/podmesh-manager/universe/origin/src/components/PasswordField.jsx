import { useId, useState } from 'react'
import { Eye, EyeOff } from 'lucide-react'
import { useI18n } from '../store/useLocaleStore'

// A password field with the eye: what is really in the field can be seen -- a capital a keyboard
// added, a value a password manager filled -- which is most of what makes a sign-in fail.
export default function PasswordField({ label, value, onChange, autoComplete = 'current-password', autoFocus = false, hint = '' }) {
  const [shown, setShown] = useState(false)
  const id = useId()
  const { t } = useI18n()
  return (
    <div>
      <label htmlFor={id} className="mb-1.5 block text-xs font-medium text-ink-500">{label}</label>
      <div className="relative">
        <input id={id} type={shown ? 'text' : 'password'} value={value} onChange={e => onChange(e.target.value)} autoComplete={autoComplete} autoFocus={autoFocus} spellCheck={false} autoCapitalize="off"
          className="w-full rounded-md border border-cream-400 bg-cream-50 px-3 py-2.5 pr-11 text-sm text-ink-900 transition focus:border-green-600 focus:outline-none focus:ring-2 focus:ring-green-600/20" />
        <button type="button" onClick={() => setShown(v => !v)} aria-label={shown ? t('password.hide') : t('password.show')} title={shown ? t('password.hide') : t('password.show')} tabIndex={-1}
          className="absolute right-3 top-1/2 -translate-y-1/2 text-ink-500 hover:text-green-600">
          {shown ? <EyeOff size={16} /> : <Eye size={16} />}
        </button>
      </div>
      {hint && <p className="mt-1 text-xs text-ink-500">{hint}</p>}
    </div>
  )
}
