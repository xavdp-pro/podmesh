import Select from './Select'
import { useI18n } from '../store/useLocaleStore'

const NAMES = { fr: 'Français', en: 'English', es: 'Español' }
export default function LanguageSwitcher({ className = 'w-36' }) {
  const { locale, setLocale, t } = useI18n()
  return <Select value={locale} onChange={setLocale} options={Object.entries(NAMES).map(([value, label]) => ({ value, label }))} className={className} label={t('common.language')} />
}
