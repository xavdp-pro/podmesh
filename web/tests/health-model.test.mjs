import test from 'node:test';import assert from 'node:assert/strict';
import {linkState,linkBoundMs,replicationSummary,ratio,bytes,age,STORE_WARN_BYTES,LINK_MARGIN_MS} from '../src/healthModel.mjs';
const figures={refresh_ms:600000,max_backoff_ms:30000},bound=600000+30000+LINK_MARGIN_MS;
test('an idle acknowledged link is healthy while its last success is within refresh, backoff and margin',()=>{
 const idle={...figures,outcome:'authenticated_import_receipt',acknowledged_unchanged:true};
 assert.equal(linkBoundMs(idle),bound);
 // Confirmed once per refresh: nine minutes without an exchange is an idle link, not a failing one.
 assert.equal(linkState({...idle,last_success_age_ms:540000,last_attempt_age_ms:540000}),'healthy');
 assert.equal(linkState({...idle,last_success_age_ms:bound,last_attempt_age_ms:bound}),'healthy');
 // Past the bound nothing confirmed the link, acknowledged or not.
 assert.equal(linkState({...idle,last_success_age_ms:bound+1,last_attempt_age_ms:bound+1}),'failing');});
test('a dead peer is degraded from its first failed attempt and failing past the bound',()=>{
 const dead={...figures,outcome:'local_exchange_failure',acknowledged_unchanged:false};
 assert.equal(linkState({...dead,last_success_age_ms:601000,last_attempt_age_ms:500}),'degraded');
 assert.equal(linkState({...dead,last_success_age_ms:bound+1,last_attempt_age_ms:20000}),'failing');
 assert.equal(linkState({...dead,last_success_age_ms:null,last_attempt_age_ms:500}),'failing');
 // An authenticated refusal is a failed attempt, not a success.
 assert.equal(linkState({...figures,outcome:'authenticated_remote_refusal',last_success_age_ms:1000,last_attempt_age_ms:10}),'degraded');
 // So is any attempt the ages place after the last success.
 assert.equal(linkState({...figures,outcome:'authenticated_import_receipt',last_success_age_ms:5000,last_attempt_age_ms:100}),'degraded');});
test('a resident that reports neither figure is judged on a one-minute refresh and a five-minute backoff',()=>{
 // Its idle links succeed about once a minute and no longer flap to degraded past sixty seconds.
 assert.equal(linkState({outcome:'authenticated_import_receipt',last_success_age_ms:61000}),'healthy');
 assert.equal(linkState({outcome:'authenticated_import_receipt',last_success_age_ms:120000}),'healthy');
 assert.equal(linkState({outcome:'local_exchange_failure',last_success_age_ms:5000}),'degraded');
 assert.equal(linkState({outcome:'authenticated_import_receipt',last_success_age_ms:376000}),'failing');
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
