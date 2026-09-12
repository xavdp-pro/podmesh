import test from 'node:test';import assert from 'node:assert/strict';import {formatBytes,meshMemory} from '../src/hostMetricsModel.mjs';
const host=(name,identity,memory)=>({name,responses:{identity:{data:{host_uuid:identity}},host_resource_metrics:{data:{memory}}}});
const memory=(available,total)=>({known:true,available_bytes:available,total_bytes:total});

test('formatBytes preserves unknowns and scales beyond TiB',()=>{assert.equal(formatBytes(null),'Unknown');assert.equal(formatBytes(-1),'Unknown');assert.equal(formatBytes(1536),'1.5 KiB');assert.equal(formatBytes(2**50),'1.0 PiB');});
test('mesh memory sums valid unique host identities',()=>{const result=meshMemory([host('A','a',memory(1024,2048)),host('B','b',memory(2048,4096))]);assert.equal(result.value,'3.0 KiB');assert.equal(result.partial,false);assert.match(result.help,/2 of 2 hosts/);});
test('mesh memory rejects duplicate identities and malformed samples as partial',()=>{const result=meshMemory([host('A','same',memory(1024,2048)),host('duplicate','same',memory(1024,2048)),host('bad','bad',memory(5000,1000))]);assert.equal(result.value,'1.0 KiB');assert.equal(result.partial,true);assert.deepEqual(result.reasons,['duplicate host identity','missing or invalid sample for bad']);});
test('mesh memory never renders wholly missing metrics as zero',()=>{const result=meshMemory([host('A','a',{known:false})]);assert.equal(result.value,'Unknown');assert.equal(result.partial,true);});
