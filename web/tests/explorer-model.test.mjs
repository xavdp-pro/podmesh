import test from 'node:test';
import assert from 'node:assert/strict';
import {configuredCpu,configuredMemory,detailResources} from '../src/explorerModel.mjs';

test('configuration summaries preserve unlimited, unknown and measured resource states',()=>{
 assert.equal(configuredMemory(0),'No explicit limit');
 assert.equal(configuredMemory(undefined),'Not reported');
 assert.equal(configuredMemory(1024*1024),'1.0 MiB');
 assert.equal(configuredCpu({cpu_quota:0}),'No explicit quota');
 assert.equal(configuredCpu({cpu_quota:25000,cpu_period:100000,cpuset_cpus:'0-1'}),'0.25 CPU cores (25000 μs / 100000 μs) · affinity 0-1');
 assert.equal(configuredCpu({cpu_quota:25000}),'Quota 25000 μs; period not reported');
});

test('detail resource cards never turn absent disk capacity into free capacity',()=>{
 const cards=Object.fromEntries(detailResources({configuration:{memory_limit_bytes:0,cpu_quota:0},storage:{writable_layer_bytes:0,rootfs_bytes:null,available_bytes:null}}));
 assert.equal(cards['RAM configured limit'],'No explicit limit');
 assert.equal(cards['Writable disk layer'],'0 B');
 assert.equal(cards['Root filesystem layers'],'Not reported');
 assert.equal(cards['Disk space available'],'Not reported by API');
});
