/**
 * Sidebar navigation, the single source of truth for menu groups and items.
 * `icon` is a key of src/components/iconMap.js: a new icon must be imported
 * from lucide-react AND mapped there, otherwise the sidebar shows the fallback.
 * `label` is an i18n key. `end` marks an exact-match route (the home page).
 * `roles` restricts an item to those session roles; a group with no visible
 * item is not shown.
 */
export const nav = [
  {
    id: 'strate0',
    label: 'nav.group.infrastructure',
    items: [
      { to: '/', icon: 'server', label: 'nav.datacenter', end: true },
      { to: '/map', icon: 'network', label: 'nav.fractalMap' },
      { to: '/storage', icon: 'hardDrive', label: 'nav.storageAndRegistries' },
    ],
  },
  {
    id: 'strate12',
    label: 'nav.group.governance',
    items: [
      { to: '/fractals', icon: 'layers', label: 'nav.fractalsAndMakers' },
      { to: '/pmbs', icon: 'shield', label: 'nav.pmbsBackups' },
      { to: '/arbiter', icon: 'flame', label: 'nav.arbiterAndChaos' },
    ],
  },
  {
    id: 'system',
    label: 'nav.group.system',
    items: [
      { to: '/showcase', icon: 'blocks', label: 'nav.showcase' },
      { to: '/api-docs', icon: 'book', label: 'nav.apiDocs' },
      { to: '/admin/users', icon: 'users', label: 'nav.users', roles: ['admin'] },
    ],
  },
]
