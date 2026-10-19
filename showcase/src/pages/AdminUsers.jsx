import { useCallback, useMemo, useState } from 'react'
import { Navigate } from 'react-router-dom'
import { Pencil, Plus, UserCheck, UserX } from 'lucide-react'
import { api } from '../api/client'
import { useAuth } from '../auth/AuthProvider'
import { useI18n } from '../i18n/I18nProvider'
import DataTable from '../components/ui/DataTable'
import SlideOver from '../components/ui/SlideOver'
import ConfirmModal from '../components/ui/ConfirmModal'
import Select from '../components/ui/Select'
import Field from '../components/ui/Field'
import TextInput from '../components/ui/TextInput'
import { useForm } from '../components/ui/useForm'
import { email as emailRule, maxLength, oneOf, required } from '../components/ui/formValidators'
import { toast } from '../components/ui/ToastHost'

/** Same whitelist and caps as server/routes/users.js. */
export const ROLES = ['admin', 'member']
const EMAIL_MAX = 190
const DISPLAY_NAME_MAX = 120

const EMPTY = { email: '', displayName: '', role: 'member' }
const RULES = {
  email: [required(), emailRule(), maxLength(EMAIL_MAX)],
  displayName: [required(), maxLength(DISPLAY_NAME_MAX)],
  role: [required(), oneOf(ROLES)],
}

function Badge({ children, className = '' }) {
  return <span className={`inline-block rounded-full px-2 py-0.5 text-xs font-semibold ${className}`}>{children}</span>
}

/**
 * Admin page: the allowlist (auth_users) as a server-mode DataTable, with a
 * SlideOver form to add or edit and a ConfirmModal to remove (deactivate).
 * The API enforces the guards; this page only shows their messages.
 */
export default function AdminUsers() {
  const { user: me } = useAuth()
  const { t, tError, locale } = useI18n()
  const [reloadKey, setReloadKey] = useState(0)
  const [panel, setPanel] = useState(null) // null | { mode: 'add' } | { mode: 'edit', user }
  const [removing, setRemoving] = useState(null)
  const [removeBusy, setRemoveBusy] = useState(false)

  const dateFmt = useMemo(() => new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short' }), [locale])
  const refresh = useCallback(() => setReloadKey((n) => n + 1), [])
  const fetchUsers = useCallback((params) => api.listUsers(params), [])

  const editing = panel?.mode === 'edit' ? panel.user : null
  const form = useForm({
    initialValues: EMPTY,
    rules: RULES,
    submit: (values) => (editing
      ? api.updateUser(editing.email, { displayName: values.displayName, role: values.role })
      : api.createUser(values)),
    onSuccess: (result) => {
      toast(t(editing ? 'users.updated' : 'users.created', { email: result.user.email }))
      setPanel(null)
      refresh()
    },
  })

  function openAdd() {
    form.reset(EMPTY)
    setPanel({ mode: 'add' })
  }

  function openEdit(row) {
    form.reset({ email: row.email, displayName: row.displayName, role: row.role })
    setPanel({ mode: 'edit', user: row })
  }

  async function restore(row) {
    try {
      await api.updateUser(row.email, { active: true })
      toast(t('users.restored', { email: row.email }))
      refresh()
    } catch (err) {
      toast(tError(err.code), { type: 'error' })
    }
  }

  async function confirmRemove() {
    if (!removing) return
    setRemoveBusy(true)
    try {
      await api.removeUser(removing.email)
      toast(t('users.removed', { email: removing.email }))
      setRemoving(null)
      refresh()
    } catch (err) {
      toast(tError(err.code), { type: 'error' })
    } finally {
      setRemoveBusy(false)
    }
  }

  const columns = useMemo(() => [
    {
      key: 'email',
      header: 'users.email',
      sortable: true,
      hideable: false,
      mobile: 'primary',
      render: (row) => (
        <span className="inline-flex max-w-full items-center gap-1.5">
          <span className="truncate">{row.email}</span>
          {row.email === me?.email && <Badge className="bg-accent-soft text-accent">{t('users.you')}</Badge>}
        </span>
      ),
    },
    { key: 'displayName', header: 'users.displayName', sortable: true, width: '11rem', mobile: 'secondary' },
    {
      key: 'role',
      header: 'users.role',
      sortable: true,
      width: '8.5rem',
      render: (row) => <Badge className={row.role === 'admin' ? 'bg-warning-soft text-warning' : 'bg-surface-2 text-fg-muted'}>{t(row.role === 'admin' ? 'users.roleAdmin' : 'users.roleMember')}</Badge>,
    },
    {
      key: 'active',
      header: 'users.status',
      sortable: true,
      width: '6.5rem',
      render: (row) => <Badge className={row.active ? 'bg-success-soft text-success' : 'bg-danger-soft text-danger'}>{t(row.active ? 'users.active' : 'users.inactive')}</Badge>,
    },
    { key: 'createdAt', header: 'users.createdAt', sortable: true, width: '10rem', hideBelow: 'lg', render: (row) => (row.createdAt ? dateFmt.format(new Date(row.createdAt)) : ''), mobile: 'meta' },
    { key: 'updatedAt', header: 'users.updatedAt', sortable: true, width: '10rem', hideBelow: 'xl', render: (row) => (row.updatedAt ? dateFmt.format(new Date(row.updatedAt)) : ''), mobile: 'hidden' },
  ], [t, dateFmt, me?.email])

  const filters = useMemo(() => [
    { key: 'role', label: 'users.filterRole', options: [{ value: 'admin', label: t('users.roleAdmin') }, { value: 'member', label: t('users.roleMember') }] },
    { key: 'active', label: 'users.filterStatus', options: [{ value: '1', label: t('users.active') }, { value: '0', label: t('users.inactive') }] },
  ], [t])

  // Removal is confirmed by this page's own ConfirmModal (the message names the address).
  const rowActions = (row) => [
    { key: 'edit', label: 'users.edit', icon: Pencil, onAction: openEdit },
    row.active
      ? { key: 'remove', label: 'users.remove', icon: UserX, danger: true, onAction: (target) => setRemoving(target) }
      : { key: 'restore', label: 'users.restore', icon: UserCheck, onAction: restore },
  ]

  if (me && me.role !== 'admin') return <Navigate to="/" replace />

  return (
    <div className="space-y-6">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-2xl font-black tracking-tight text-fg">{t('users.title')}</h1>
          <p className="mt-1 max-w-2xl text-sm text-fg-muted">{t('users.intro')}</p>
        </div>
        <button type="button" onClick={openAdd} className="inline-flex items-center gap-1.5 rounded-lg bg-accent px-4 py-2 text-sm font-semibold text-accent-fg hover:bg-accent-hover">
          <Plus size={16} /> {t('users.add')}
        </button>
      </header>

      <DataTable
        id="admin-users"
        columns={columns}
        columnPicker
        rowKey="email"
        fetcher={fetchUsers}
        reloadKey={reloadKey}
        filters={filters}
        defaultSort={{ key: 'email', dir: 'asc' }}
        rowActions={rowActions}
        onRowClick={openEdit}
      />

      <SlideOver
        isOpen={Boolean(panel)}
        onClose={() => setPanel(null)}
        title={editing ? t('users.editTitle', { email: editing.email }) : t('users.addTitle')}
        footer={(
          <div className="flex justify-end gap-3">
            <button type="button" onClick={() => setPanel(null)} className="rounded-md border border-line px-4 py-2 text-sm font-medium text-fg hover:bg-surface-2">
              {t('common.cancel')}
            </button>
            <button type="submit" form="user-form" disabled={form.submitting} className="rounded-md bg-accent px-4 py-2 text-sm font-semibold text-accent-fg hover:bg-accent-hover disabled:opacity-60">
              {form.submitting ? t('form.saving') : t('common.save')}
            </button>
          </div>
        )}
      >
        <form
          id="user-form"
          noValidate
          onSubmit={(event) => {
            event.preventDefault()
            form.handleSubmit(event)
          }}
          className="space-y-4"
        >
          <Field label={t('users.email')} hint={t('users.emailHint')} error={form.error('email')} required>
            <TextInput type="email" inputMode="email" autoComplete="off" disabled={Boolean(editing)} {...form.field('email')} />
          </Field>
          <Field label={t('users.displayName')} error={form.error('displayName')} required>
            <TextInput autoComplete="off" maxLength={DISPLAY_NAME_MAX} {...form.field('displayName')} />
          </Field>
          <Field label={t('users.role')} error={form.error('role')} required>
            <Select
              value={form.values.role}
              onChange={(value) => form.setValue('role', value)}
              options={ROLES.map((role) => ({ value: role, label: t(role === 'admin' ? 'users.roleAdmin' : 'users.roleMember') }))}
              disabled={Boolean(editing) && editing.email === me?.email}
            />
          </Field>
          {form.submitError && (
            <p role="alert" className="rounded-md border border-danger/40 bg-danger-soft px-3 py-2 text-xs font-medium text-danger">
              {tError(form.submitError)}
            </p>
          )}
        </form>
      </SlideOver>

      <ConfirmModal
        open={Boolean(removing)}
        title={t('users.removeTitle')}
        message={t('users.removeMessage', { email: removing?.email })}
        confirmLabel={t('users.remove')}
        pending={removeBusy}
        onCancel={() => setRemoving(null)}
        onConfirm={confirmRemove}
      />
    </div>
  )
}
