// What the drawer shows of a container's observed configuration and metrics (container_details).
import { bytes } from './health'

const knownBytes = v => (Number.isFinite(v) && v >= 0 ? bytes(v) : null)

export function configuredMemory(value) {
  if (value === 0) return { key: 'details.noLimit' }
  const b = knownBytes(value)
  return b ? { text: b } : { key: 'details.notReported' }
}

export function configuredCpu({ cpu_quota: quota, cpu_period: period, cpuset_cpus: cpuset } = {}) {
  const affinity = typeof cpuset === 'string' && cpuset.trim() ? ` · ${cpuset.trim()}` : ''
  if (quota === 0) return { key: 'details.noQuota', suffix: affinity }
  if (!Number.isFinite(quota) || quota < 0) return { key: 'details.notReported', suffix: affinity }
  if (!Number.isFinite(period) || period <= 0) return { text: `${quota} μs`, suffix: affinity }
  return { text: `${(quota / period).toFixed(2)} CPU (${quota} μs / ${period} μs)`, suffix: affinity }
}

// Each row: [label key, value]; a value is {text} or {key} (a translated phrase), with an optional suffix.
export function detailResources(detail = {}) {
  const configuration = detail.configuration || {}, metrics = detail.metrics?.data?.[0] || {}, storage = detail.storage || {}
  const raw = v => (v == null || v === '' ? { key: 'details.unavailable' } : { text: String(v) })
  const sized = v => { const b = knownBytes(v); return b ? { text: b } : { key: 'details.notReported' } }
  return [
    ['details.cpuUsage', raw(metrics.cpu_percent ?? metrics.CPU ?? metrics.CPUPerc)],
    ['details.cpuConfigured', configuredCpu(configuration)],
    ['details.ramUsage', raw(metrics.mem_usage ?? metrics.MemUsage)],
    ['details.ramLimit', configuredMemory(configuration.memory_limit_bytes)],
    ['details.writableLayer', sized(storage.writable_layer_bytes)],
    ['details.rootfs', sized(storage.rootfs_bytes)],
    ['details.diskAvailable', storage.available_bytes == null ? { key: 'details.notReported' } : sized(storage.available_bytes)],
    ['details.netIo', raw(metrics.net_io ?? metrics.NetIO)],
    ['details.blockIo', raw(metrics.block_io ?? metrics.BlockIO)],
  ]
}
