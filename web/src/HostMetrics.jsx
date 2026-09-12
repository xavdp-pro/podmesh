import React from 'react';
import {Cpu,HardDrive,MemoryStick} from 'lucide-react';
import {formatBytes} from './hostMetricsModel.mjs';
export {formatBytes,meshMemory} from './hostMetricsModel.mjs';

export default function HostMetrics({host}){
 const metrics=host.responses.host_resource_metrics?.data;
 const supported=!!host.responses.host_resource_metrics||!!host.optionalErrors?.host_resource_metrics||host.responses.host_resource_capabilities?.data?.operations?.includes('host_resource_metrics');
 const failed=host.optionalErrors?.host_resource_metrics||host.responses.host_resource_metrics?.ok===false;
 const memory=metrics?.memory;
 const storage=metrics?.storage;
 const count=metrics?.cpu?.count;
 const load=metrics?.cpu?.load;
 const stale=metrics?.observed_at&&Date.now()-metrics.observed_at*1000>60000;
 const status=host.metricsDeferred?(metrics?'Previous metrics retained while inspection runs':'Metrics deferred while inspection runs'):!supported?'Unsupported by installed observer':failed?'Metrics unavailable':metrics?'Metrics observed':'Waiting for metrics';
 return <div className="host-metrics" role="group" aria-live="polite" aria-label={`Observed host resources for ${host.name}. ${status}`}>
  <span title={memory?.source||'No observation'}><MemoryStick size={14}/><b>{memory?.known?formatBytes(memory.available_bytes):'Unknown'}</b><small>RAM available</small></span>
  <span title={storage?.source||'No observation'}><HardDrive size={14}/><b>{storage?.known?formatBytes(storage.available_bytes):'Unknown'}</b><small>Graph-root filesystem free</small></span>
  <span title={load?.source||count?.source||'No observation'}><Cpu size={14}/><b>{count?.known?count.logical_count:'Unknown'}</b><small>{load?.known?`CPU · load ${load.load_1m.toFixed(2)}`:'logical CPUs'}</small></span>
  <time className={stale?'stale-observation':''}>{metrics?.observed_at?`Observed ${new Date(metrics.observed_at*1000).toLocaleTimeString()}${stale?' · stale':''}`:status}</time>
  {metrics&&<small className="metrics-provenance">Sources: kernel procfs and filesystem containing the Podman graph root</small>}
 </div>;
}
