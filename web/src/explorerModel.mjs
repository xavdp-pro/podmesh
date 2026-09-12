import {formatBytes} from './hostMetricsModel.mjs';

const knownBytes=value=>Number.isFinite(value)&&value>=0?formatBytes(value):'Not reported';

export function configuredMemory(value){
 if(value===0)return 'No explicit limit';
 return knownBytes(value);
}

export function configuredCpu({cpu_quota:quota,cpu_period:period,cpuset_cpus:cpuset}={}){
 const affinity=typeof cpuset==='string'&&cpuset.trim()?` · affinity ${cpuset.trim()}`:'';
 if(quota===0)return `No explicit quota${affinity}`;
 if(!Number.isFinite(quota)||quota<0)return `Not reported${affinity}`;
 if(!Number.isFinite(period)||period<=0)return `Quota ${quota} μs; period not reported${affinity}`;
 return `${(quota/period).toFixed(2)} CPU cores (${quota} μs / ${period} μs)${affinity}`;
}

export function detailResources(detail={}){
 const configuration=detail.configuration||{},metrics=detail.metrics?.data?.[0]||{},storage=detail.storage||{};
 return [
  ['CPU usage',metrics.cpu_percent??metrics.CPU??metrics.CPUPerc??'Unavailable'],
  ['CPU configured',configuredCpu(configuration)],
  ['RAM usage',metrics.mem_usage??metrics.MemUsage??'Unavailable'],
  ['RAM configured limit',configuredMemory(configuration.memory_limit_bytes)],
  ['Writable disk layer',knownBytes(storage.writable_layer_bytes)],
  ['Root filesystem layers',knownBytes(storage.rootfs_bytes)],
  ['Disk space available',storage.available_bytes===null||storage.available_bytes===undefined?'Not reported by API':knownBytes(storage.available_bytes)],
  ['Network I/O',metrics.net_io??metrics.NetIO??'Unavailable'],
  ['Block I/O',metrics.block_io??metrics.BlockIO??'Unavailable'],
 ];
}
