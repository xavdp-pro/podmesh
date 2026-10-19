/**
 * Explicit icon registry for the navigation. lucide-react is tree-shaken, so an
 * icon used by name in src/data/nav.js must be imported here AND added to the
 * map; a missing key falls back to `Circle` and stays visible instead of crashing.
 */
import {
  Blocks,
  BookOpen,
  Circle,
  Flame,
  HardDrive,
  Home,
  Layers,
  Network,
  Server,
  Shield,
  Table,
  Terminal,
  Users,
  Workflow,
} from 'lucide-react'

export const iconMap = {
  home: Home,
  server: Server,
  network: Network,
  layers: Layers,
  shield: Shield,
  flame: Flame,
  hardDrive: HardDrive,
  terminal: Terminal,
  workflow: Workflow,
  book: BookOpen,
  blocks: Blocks,
  table: Table,
  users: Users,
}

export const FallbackIcon = Circle

export function resolveIcon(name) {
  return iconMap[name] || FallbackIcon
}
