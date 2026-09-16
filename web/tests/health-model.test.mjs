import test from 'node:test';import assert from 'node:assert/strict';
import {linkState,replicationSummary,ratio,bytes,age,STORE_WARN_BYTES} from '../src/healthModel.mjs';
test('a link is healthy only when authenticated within a minute, degraded within five, failing beyond',()=>{
 assert.equal(linkState({outcome:'authenticated_import_receipt',last_success_age_ms:5000}),'healthy');
 assert.equal(linkState({outcome:'local_exchange_failure',last_success_age_ms:5000}),'degraded');
 assert.equal(linkState({outcome:'authenticated_import_receipt',last_success_age_ms:120000}),'degraded');
 assert.equal(linkState({outcome:'local_exchange_failure',last_success_age_ms:1_300_000}),'failing');
 assert.equal(linkState({outcome:'local_exchange_failure',last_success_age_ms:null}),'failing');});
test('the replication summary is as bad as its worst link, and warns on a large store',()=>{
 const ok={outcome:'authenticated_import_receipt',last_success_age_ms:1000};const bad={outcome:'local_exchange_failure',last_success_age_ms:2_000_000};
 assert.equal(replicationSummary([{name:'a',managers:[{links:[ok,ok],store_bytes:100}]}]).state,'healthy');
 const s=replicationSummary([{name:'a',managers:[{links:[ok,bad],store_bytes:STORE_WARN_BYTES+1}]},{name:'b',managers:[{error:'refused'}]}]);
 assert.equal(s.state,'failing');assert.equal(s.failing,1);assert.equal(s.healthy,1);assert.equal(s.storeWarning,true);assert.equal(s.replicas,2);
 assert.equal(replicationSummary([{name:'a',managers:[]}]).state,'absent');});
test('ratios, sizes and ages are drawn honestly',()=>{
 assert.equal(ratio(null,10),null);assert.equal(ratio(5,0),null);assert.equal(ratio(20,10),1);
 assert.equal(bytes(null),'—');assert.equal(bytes(1536),'1.5 KiB');assert.equal(bytes(11788288),'11 MiB');
 assert.equal(age(null),'never');assert.equal(age(4000),'4 s ago');assert.equal(age(1_300_000),'22 min ago');});
