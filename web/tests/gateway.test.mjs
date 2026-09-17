import http from 'node:http';import test from 'node:test';import assert from 'node:assert/strict';import {createApp} from '../server/app.mjs';import {request}from'../server/transport.mjs';import net from'node:net';import fs from'node:fs/promises';import os from'node:os';import path from'node:path';
const payload={operation:'start',universe_uuid:'00000000-0000-4000-8000-000000000001',operation_id:'00000000-0000-4000-8000-000000000002',authorization_ref:'fixture'};
async function gateway(t,actions=true){
 const calls=[];let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/fixture.sock',allowActions:actions}]},{origin:url,call:async(_h,p)=>{calls.push(p);return {ok:true,data:p.operation==='capabilities'?{operations:['start','pause','resume','resources']}:{}};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());return{calls,url,session,post:(body=payload,extra={})=>fetch(url+'/api/hosts/a/actions',{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':session.token,...extra},body:JSON.stringify(body)})};
}
test('action forwards exact identity through capability check',async t=>{const g=await gateway(t);assert.equal((await g.post()).status,200);assert.deepEqual(g.calls,[{operation:'capabilities'},payload]);});
test('missing session, foreign origin and unknown action refused',async t=>{const g=await gateway(t);assert.equal((await g.post(payload,{'X-Podmesh-Token':''})).status,401);assert.equal((await g.post(payload,{Origin:'https://foreign.test'})).status,403);assert.equal((await g.post({...payload,operation:'garbage_collect_apply'})).status,400);assert.deepEqual(g.calls,[]);});
test('mutating routes require an explicit same-origin header even with a valid session token',async t=>{const g=await gateway(t);const action=await fetch(g.url+'/api/hosts/a/actions',{method:'POST',headers:{'Content-Type':'application/json','X-Podmesh-Token':g.session.token},body:JSON.stringify(payload)});const details=await fetch(g.url+'/api/hosts/a/details',{method:'POST',headers:{'Content-Type':'application/json','X-Podmesh-Token':g.session.token},body:JSON.stringify({container_path:['a'.repeat(64)]})});assert.equal(action.status,403);assert.equal(details.status,403);assert.deepEqual(g.calls,[]);});
test('read-only and unadvertised capabilities refuse writes',async t=>{const a=await gateway(t,false);assert.equal((await a.post()).status,403);assert.deepEqual(a.calls,[]);const b=await gateway(t);assert.equal((await b.post({...payload,operation:'delete'})).status,409);assert.equal(b.calls.length,1);});
test('DNS rebinding host refused',async t=>{const g=await gateway(t);const status=await new Promise((resolve,reject)=>{http.get(g.url+'/api/session',{headers:{Host:'foreign.test'}},r=>{r.resume();resolve(r.statusCode);}).on('error',reject);});assert.equal(status,403);});
test('Unix transport forwards newline JSON and bounds response',async t=>{const dir=await fs.mkdtemp(path.join(os.tmpdir(),'podmesh-web-'));const socket=path.join(dir,'api.sock');const server=net.createServer({allowHalfOpen:true},c=>{let b='';c.on('data',x=>{b+=x;if(b.endsWith('\n')){assert.deepEqual(JSON.parse(b),{operation:'identity'});c.end('{"ok":true,"data":{"host_uuid":"fixture"}}\n');}});});await new Promise(r=>server.listen(socket,r));t.after(async()=>{server.close();await fs.rm(dir,{recursive:true,force:true});});assert.deepEqual(await request({socket},{operation:'identity'}),{ok:true,data:{host_uuid:'fixture'}});await assert.rejects(request({socket},{operation:'x',value:'x'.repeat(4096)}),/limit/);});

test('pause and resume forward as they are; resources is bounded at the gateway',async t=>{const g=await gateway(t);
 for(const operation of ['pause','resume'])assert.equal((await g.post({...payload,operation})).status,200);
 assert.equal((await g.post({...payload,operation:'resources',memory_bytes:512*1024*1024,cpus:1.5})).status,200);
 assert.equal((await g.post({...payload,operation:'resources',memory_bytes:16*1024*1024})).status,400);
 assert.equal((await g.post({...payload,operation:'resources',cpus:0})).status,400);
 assert.equal((await g.post({...payload,operation:'resources'})).status,400);
 assert.equal((await g.post({...payload,operation:'resources',memory_bytes:'512m'})).status,400);
 assert.deepEqual(g.calls.filter(c=>c.operation!=='capabilities').map(c=>c.operation),['pause','resume','resources']);});
test('a move runs the tool between two SSH hosts of the configuration, and refuses what is not a move',async t=>{
 const calls=[];const moves=[];let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',ssh:'lab@a',remoteSocket:'/run/x/api.sock',remoteStateDir:'/var/lib/x',allowActions:true},{id:'b',name:'B',ssh:'lab@b',remoteSocket:'/run/x/api.sock',remoteStateDir:'/var/lib/x',allowActions:true},{id:'c',name:'C',socket:'/fixture.sock',allowActions:true},{id:'d',name:'D',ssh:'lab@d',remoteSocket:'/run/other/api.sock',allowActions:true}]},{origin:url,call:async(_h,p)=>{calls.push(p);return {ok:true,data:{}};},runMove:async m=>{moves.push(m);return {result:'moved',message:'ok',steps:[{step:'restore',host:'destination',ok:true}]};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());
 assert.deepEqual(session.hosts.map(h=>h.canMove),[true,true,false,true]);
 const post=(body,extra={})=>fetch(url+'/api/moves',{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':session.token,...extra},body:JSON.stringify(body)});
 const good={universe_uuid:'00000000-0000-4000-8000-000000000001',source:'a',destination:'b',authorization_ref:'fixture'};
 assert.equal((await post(good,{'X-Podmesh-Token':''})).status,401);
 assert.equal((await post({...good,destination:'a'})).status,400);
 assert.equal((await post({...good,destination:'zz'})).status,404);
 assert.equal((await post({...good,authorization_ref:''})).status,400);
 assert.equal((await post({...good,extra:1})).status,400);
 assert.equal((await post({...good,destination:'c'})).status,409);
 assert.equal((await post({...good,destination:'d'})).status,409);
 assert.deepEqual(moves,[]);
 const ok=await post({...good,keep_source:true});assert.equal(ok.status,200);assert.equal((await ok.json()).result,'moved');
 assert.equal(moves.length,1);assert.equal(moves[0].source.id,'a');assert.equal(moves[0].destination.id,'b');assert.equal(moves[0].keep_source,true);assert.equal(moves[0].authorization_ref,'fixture');
 app=createApp({hosts:[{id:'a',name:'A',ssh:'lab@a',allowActions:true},{id:'b',name:'B',ssh:'lab@b',allowActions:true}]},{origin:url,call:async()=>({ok:true,data:{}}),runMove:async()=>({result:'refused',message:'inspect: only a network-disabled universe is moved today',steps:[]})});
 const s2=await fetch(url+'/api/session').then(r=>r.json());
 const refused=await fetch(url+'/api/moves',{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':s2.token},body:JSON.stringify(good)});
 assert.equal(refused.status,409);assert.equal((await refused.json()).result,'refused');});
test('unexpected action fields refused',async t=>{const g=await gateway(t);assert.equal((await g.post({...payload,on_timeout:'kill'})).status,400);assert.deepEqual(g.calls,[]);});
test('missing host transport refused',()=>assert.throws(()=>createApp({hosts:[{id:'a',name:'A'}]}),/transport/));
test('UTF-8 survives split socket chunks',async t=>{
 const dir=await fs.mkdtemp(path.join(os.tmpdir(),'podmesh-utf8-'));const socket=path.join(dir,'api.sock');const expected={ok:true,data:'électricité'};const bytes=Buffer.from(JSON.stringify(expected));const at=bytes.indexOf(0xc3)+1;
 const server=net.createServer({allowHalfOpen:true},c=>c.once('data',()=>{c.write(bytes.subarray(0,at));setTimeout(()=>c.end(bytes.subarray(at)),20);}));await new Promise(r=>server.listen(socket,r));t.after(async()=>{server.close();await fs.rm(dir,{recursive:true,force:true});});assert.deepEqual(await request({socket},{operation:'identity'}),expected);
});
test('unknown action invalidates completed and in-flight observations',async t=>{
 let app,release;let reads=0,hold=false;
 const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/fixture.sock',allowActions:true}]},{origin:url,call:async(_h,p)=>{if(p.operation==='start')throw Error('lost response');if(p.operation==='identity'){reads++;if(hold)await new Promise(r=>release=r);}return{ok:true,data:{operations:['start']}};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());const post=()=>fetch(url+'/api/hosts/a/actions',{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':session.token},body:JSON.stringify(payload)});
 await fetch(url+'/api/snapshot');assert.equal(reads,1);assert.equal((await post()).status,502);await fetch(url+'/api/snapshot');assert.equal(reads,2);
 await post();hold=true;const pending=fetch(url+'/api/snapshot');while(!release)await new Promise(r=>setTimeout(r,1));await post();hold=false;release();await pending;await fetch(url+'/api/snapshot');assert.equal(reads,4);
});
test('details rejects command-like IDs and refuses missing API capability',async t=>{const g=await gateway(t);const session=await fetch(g.url+'/api/session').then(r=>r.json());const post=ids=>fetch(g.url+'/api/hosts/a/details',{method:'POST',headers:{Origin:g.url,'Content-Type':'application/json','X-Podmesh-Token':session.token},body:JSON.stringify({container_path:ids})});assert.equal((await post(['--latest'])).status,400);assert.equal((await post(['a'.repeat(64)])).status,409);});

test('snapshot includes advertised host metrics and skips them on older runtimes',async t=>{
 let app;const calls=[];const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'new',name:'New',socket:'/new.sock'},{id:'old',name:'Old',socket:'/old.sock'}]},{origin:url,call:async(h,p)=>{calls.push([h.id,p.operation]);if(p.operation==='capabilities')return{ok:true,data:{operations:h.id==='new'?['host_resource_metrics']:[]}};if(p.operation==='host_resource_metrics')return{ok:true,data:{observed_at:1,memory:{known:true,total_bytes:20,available_bytes:10}}};if(p.operation==='inventory')return{ok:true,data:{containers:[]}};if(p.operation==='observations')return{ok:true,data:{observations:[]}};return{ok:true,data:{host_uuid:h.id}};}});
 const snapshot=await fetch(url+'/api/snapshot').then(r=>r.json());
 assert.equal(snapshot.hosts[0].responses.host_resource_metrics.data.memory.available_bytes,10);
 assert.equal(snapshot.hosts[1].responses.host_resource_metrics,undefined);
 assert.deepEqual(calls.filter(([,operation])=>operation==='host_resource_metrics'),[['new','host_resource_metrics']]);
});

test('snapshot reads metrics from the dedicated observation socket',async t=>{
 let app;const calls=[];const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',ssh:'lab@example.test',detailsSocket:'/run/observer.sock'}]},{origin:url,call:async(h,p)=>{calls.push([h.remoteSocket||'/run/podmesh/api.sock',p.operation]);if(p.operation==='capabilities')return{ok:true,data:{operations:h.remoteSocket?['host_resource_metrics']:[]}};if(p.operation==='host_resource_metrics')return{ok:true,data:{memory:{known:true,total_bytes:20,available_bytes:10}}};if(p.operation==='inventory')return{ok:true,data:{containers:[]}};if(p.operation==='observations')return{ok:true,data:{observations:[]}};return{ok:true,data:{host_uuid:'a'}};}});
 const host=(await fetch(url+'/api/snapshot').then(r=>r.json())).hosts[0];
 assert.equal(host.responses.host_resource_metrics.data.memory.available_bytes,10);
 assert.deepEqual(host.responses.host_resource_capabilities.data.operations,['host_resource_metrics']);
 assert.deepEqual(calls.filter(([,operation])=>operation==='host_resource_metrics'),[['/run/observer.sock','host_resource_metrics']]);
});

test('dedicated observer capability state is truthful when metrics are unsupported',async t=>{
 let app;const calls=[];const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/main.sock',detailsSocket:'/observer.sock'}]},{origin:url,call:async(h,p)=>{calls.push([h.socket,p.operation]);if(p.operation==='capabilities')return{ok:true,data:{operations:h.socket==='/observer.sock'?[]:['host_resource_metrics']}};if(p.operation==='inventory')return{ok:true,data:{containers:[]}};if(p.operation==='observations')return{ok:true,data:{observations:[]}};return{ok:true,data:{host_uuid:'a'}};}});
 const host=(await fetch(url+'/api/snapshot').then(r=>r.json())).hosts[0];
 assert.deepEqual(host.responses.host_resource_capabilities.data.operations,[]);
 assert.equal(host.responses.host_resource_metrics,undefined);
 assert.deepEqual(calls.filter(([,operation])=>operation==='host_resource_metrics'),[]);
 assert.ok(calls.some(([socket,operation])=>socket==='/observer.sock'&&operation==='capabilities'));
});

test('details socket never redirects lifecycle actions',async t=>{
 let app;const calls=[];const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',ssh:'lab@example.test',detailsSocket:'/run/observer.sock',allowActions:true}]},{origin:url,call:async(h,p)=>{calls.push([h.remoteSocket,p.operation]);return{ok:true,data:p.operation==='capabilities'?{operations:['start','pause','resume','resources']}:{}};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());const response=await fetch(url+'/api/hosts/a/actions',{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':session.token},body:JSON.stringify(payload)});
 assert.equal(response.status,200);assert.deepEqual(calls,[[undefined,'capabilities'],[undefined,'start']]);
});

test('deep inspection excludes concurrent observer polling without hiding main inventory',async t=>{
 let app,releaseDetails,detailsStarted;const started=new Promise(resolve=>detailsStarted=resolve);const calls=[];const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/main.sock',detailsSocket:'/observer.sock'}]},{origin:url,call:async(h,p)=>{calls.push([h.socket,p.operation]);if(p.operation==='capabilities')return{ok:true,data:{operations:h.socket==='/observer.sock'?['container_details','host_resource_metrics']:[]}};if(p.operation==='container_details'){detailsStarted();await new Promise(resolve=>releaseDetails=resolve);return{ok:true,data:{}};}if(p.operation==='inventory')return{ok:true,data:{containers:[{Id:'kept'}]}};if(p.operation==='observations')return{ok:true,data:{observations:[]}};return{ok:true,data:{host_uuid:'a'}};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());const detailRequest=fetch(url+'/api/hosts/a/details',{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':session.token},body:JSON.stringify({container_path:['a'.repeat(64)]})});await started;
 const host=(await fetch(url+'/api/snapshot').then(r=>r.json())).hosts[0];
 assert.equal(host.responses.inventory.data.containers[0].Id,'kept');assert.equal(host.metricsDeferred,true);assert.equal(calls.filter(([socket])=>socket==='/observer.sock').length,2);
 releaseDetails();assert.equal((await detailRequest).status,200);
});

test('observer capability failure remains optional for main observations',async t=>{
 let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/main.sock',detailsSocket:'/observer.sock'}]},{origin:url,call:async(h,p)=>{if(h.socket==='/observer.sock')throw Error('observer unavailable');if(p.operation==='inventory')return{ok:true,data:{containers:[{Id:'kept'}]}};if(p.operation==='observations')return{ok:true,data:{observations:[{id:1}]}};if(p.operation==='capabilities')return{ok:true,data:{operations:[]}};return{ok:true,data:{host_uuid:'a'}};}});
 const host=(await fetch(url+'/api/snapshot').then(r=>r.json())).hosts[0];assert.equal(host.responses.inventory.data.containers[0].Id,'kept');assert.equal(host.responses.observations.data.observations[0].id,1);assert.equal(host.optionalErrors.host_resource_metrics,'observer unavailable');assert.deepEqual(host.errors,{});
});

test('failed optional metrics cannot suppress inventory or journal observations',async t=>{
 let app;const calls=[];const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/a.sock'}]},{origin:url,call:async(_host,p)=>{calls.push(p.operation);if(p.operation==='capabilities')return{ok:true,data:{operations:['host_resource_metrics']}};if(p.operation==='host_resource_metrics')throw Error('fixture metrics timeout');if(p.operation==='inventory')return{ok:true,data:{containers:[{Id:'kept'}]}};if(p.operation==='observations')return{ok:true,data:{observations:[{id:1,operation:'create'}]}};return{ok:true,data:{host_uuid:'a'}};}});
 const host=(await fetch(url+'/api/snapshot').then(r=>r.json())).hosts[0];
 assert.deepEqual(calls,['identity','capabilities','inventory','observations','host_resource_metrics']);assert.equal(host.responses.inventory.data.containers[0].Id,'kept');assert.equal(host.responses.observations.data.observations[0].operation,'create');assert.equal(host.errors.host_resource_metrics,undefined);assert.equal(host.optionalErrors.host_resource_metrics,'fixture metrics timeout');
});

async function relationshipGateway(t,relationshipFetch,now=100000){
 let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/fixture.sock'}],relationships:{endpoint:'http://127.0.0.1:8787/v1/fractals',maxAgeSeconds:60}},{origin:url,now:()=>now,relationshipFetch,call:async(_h,p)=>p.operation==='inventory'?{ok:true,data:{containers:[{Id:'kept'}]}}:{ok:true,data:p.operation==='observations'?{observations:[]}:{}}});
 const session=await fetch(url+'/api/session').then(r=>r.json());const read=()=>fetch(url+'/api/relationships',{headers:{Origin:url,'X-Podmesh-Token':session.token}}).then(r=>r.json());return{url,session,read};
}
const relationship=(fractal_uuid,universe_uuid,parent_uuid=null)=>({fractal_uuid,fractal_name:'Fixture',universe_uuid,parent_uuid,name:'Fixture universe',role:'worker',revision:1,provenance:'fixture',observed_at:100});
const relationIds={first:'00000000-0000-4000-8000-000000000010',second:'00000000-0000-4000-8000-000000000011',one:'00000000-0000-4000-8000-000000000001',two:'00000000-0000-4000-8000-000000000002'};

test('relationship gateway reads multiple manager-attested fractals through the local session',async t=>{
 const g=await relationshipGateway(t,async(url,options)=>{assert.equal(url,'http://127.0.0.1:8787/v1/fractals');assert.equal(options.method,'GET');return new Response(JSON.stringify({observed_at:100,relationships:[relationship(relationIds.first,relationIds.one),relationship(relationIds.second,relationIds.two)]}),{headers:{'content-type':'application/json'}});});
 const body=await g.read();assert.equal(body.status,'available');assert.equal(body.relationships.length,2);assert.equal((await fetch(g.url+'/api/relationships')).status,401);assert.equal((await fetch(g.url+'/api/relationships',{headers:{'X-Podmesh-Token':g.session.token}}).then(r=>r.json())).status,'available');
});

test('missing relationship endpoint remains explicit and cannot block host inventory',async t=>{
 let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/fixture.sock'}]},{origin:url,call:async(_h,p)=>p.operation==='inventory'?{ok:true,data:{containers:[{Id:'kept'}]}}:{ok:true,data:p.operation==='observations'?{observations:[]}:{}}});const session=await fetch(url+'/api/session').then(r=>r.json());
 const relationships=await fetch(url+'/api/relationships',{headers:{Origin:url,'X-Podmesh-Token':session.token}}).then(r=>r.json());const snapshot=await fetch(url+'/api/snapshot').then(r=>r.json());assert.equal(relationships.status,'unavailable');assert.equal(snapshot.hosts[0].responses.inventory.data.containers[0].Id,'kept');
});

test('malformed and over-limit manager responses are rejected before the view model',async t=>{
 const malformed=await relationshipGateway(t,async()=>new Response(JSON.stringify({relationships:[]}),{headers:{'content-type':'application/json'}}));assert.equal((await malformed.read()).status,'malformed');
 const bounded=await relationshipGateway(t,async()=>new Response(JSON.stringify({observed_at:100,relationships:Array.from({length:5001},()=>({}))}),{headers:{'content-type':'application/json'}}));const body=await bounded.read();assert.equal(body.status,'malformed');assert.match(body.error,/5000-record/);
});

test('stale relationship data remains explicit and leaves observed universes unassigned',async t=>{
 const g=await relationshipGateway(t,async()=>new Response(JSON.stringify({observed_at:1,relationships:[relationship(relationIds.first,relationIds.one)]}),{headers:{'content-type':'application/json'}}));const body=await g.read();assert.equal(body.status,'stale');assert.equal(body.relationships,undefined);
});

test('manager relationship failure is isolated from host snapshot collection',async t=>{
 const g=await relationshipGateway(t,async()=>{throw Error('fixture manager unavailable');});const relationships=await g.read();const snapshot=await fetch(g.url+'/api/snapshot').then(r=>r.json());assert.equal(relationships.status,'unavailable');assert.match(relationships.error,/fixture manager unavailable/);assert.equal(snapshot.hosts[0].responses.inventory.data.containers[0].Id,'kept');
});
test('the generic operation route validates against the schema the host publishes',async t=>{
 const calls=[];let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 const schemas={stop:{kind:'universe',gate:'none',description:'stop',fields:[{name:'timeout_seconds',type:'integer',required:true,min:0,max:300},{name:'on_timeout',type:'enum',required:true,values:['kill','leave_running']}]},
  storage_status:{kind:'read',gate:'none',description:'storage',fields:[]},activation_require:{kind:'universe',gate:'none',description:'policy',fields:[{name:'lease_seconds',type:'integer',required:true,min:5,max:3600},{name:'eligible_hosts',type:'uuid[]',required:false}]},
  migration_checkpoint:{kind:'tool',gate:'reservation',description:'step',fields:null}};
 app=createApp({hosts:[{id:'a',name:'A',socket:'/fixture.sock',allowActions:true},{id:'r',name:'R',socket:'/fixture.sock',allowActions:false}]},{origin:url,call:async(h,p)=>{calls.push([h.id,p]);return p.operation==='capabilities'?{ok:true,data:{operations:Object.keys(schemas),schemas}}:{ok:true,data:{echo:p.operation}};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());
 const post=(host,body,extra={})=>fetch(url+`/api/hosts/${host}/operations`,{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':session.token,...extra},body:JSON.stringify(body)});
 const id=()=>'00000000-0000-4000-8000-00000000000'+(Math.floor(Math.random()*9)+1);
 const U='00000000-0000-4000-8000-000000000001';
 const good={operation:'stop',operation_id:id(),universe_uuid:U,authorization_ref:'fixture',timeout_seconds:10,on_timeout:'kill'};
 assert.equal((await post('a',good)).status,200);
 assert.equal((await post('a',{...good,operation_id:id(),timeout_seconds:301})).status,400);
 assert.equal((await post('a',{...good,operation_id:id(),timeout_seconds:'10'})).status,400);
 assert.equal((await post('a',{...good,operation_id:id(),on_timeout:'maybe'})).status,400);
 assert.equal((await post('a',{...good,operation_id:id(),extra:1})).status,400);
 assert.equal((await post('a',{...good,operation_id:id(),universe_uuid:'nope'})).status,400);
 assert.equal((await post('a',{operation:'stop',operation_id:id(),universe_uuid:U,authorization_ref:'fixture',timeout_seconds:10})).status,400);
 assert.equal((await post('a',{operation:'activation_require',operation_id:id(),universe_uuid:U,authorization_ref:'fixture',lease_seconds:30,eligible_hosts:[U]})).status,200);
 assert.equal((await post('a',{operation:'activation_require',operation_id:id(),universe_uuid:U,authorization_ref:'fixture',lease_seconds:30,eligible_hosts:['x']})).status,400);
 assert.equal((await post('a',{operation:'migration_checkpoint',operation_id:id(),universe_uuid:U,authorization_ref:'fixture'})).status,409);
 assert.equal((await post('a',{operation:'unknown_op',operation_id:id(),authorization_ref:'fixture'})).status,409);
 assert.equal((await post('r',good)).status,403);
 const read=await fetch(url+'/api/hosts/r/operations',{method:'POST',headers:{'Content-Type':'application/json','X-Podmesh-Token':session.token},body:JSON.stringify({operation:'storage_status',operation_id:id(),authorization_ref:'fixture'})});
 assert.equal(read.status,200);
 assert.equal((await post('a',good,{Origin:'https://foreign.test'})).status,403);
 const sent=calls.filter(([,p])=>p.operation!=='capabilities').map(([h,p])=>h+':'+p.operation);
 assert.deepEqual(sent,['a:stop','a:activation_require','r:storage_status']);});
test('health reads every host on its own, reports a failing one, and reads manager links only for manager universes',async t=>{
 const calls=[];let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 const M='00000000-0000-4000-8000-00000000000a',U='00000000-0000-4000-8000-00000000000b';
 app=createApp({hosts:[{id:'a',name:'A',socket:'/f.sock'},{id:'b',name:'B',socket:'/g.sock'},{id:'c',name:'C',socket:'/h.sock'}]},{origin:url,call:async(h,p)=>{calls.push([h.id,p.operation,p.universe_uuid]);
  if(h.id==='c')throw Error('ssh unreachable');
  if(p.operation==='capabilities')return {ok:true,data:{operations:h.id==='b'?['identity']:['host_status','universe_stats','manager_status']}};
  if(p.operation==='host_status')return {ok:true,data:{cpu_count:2,load_average:{'1m':0.5},memory_total_bytes:100,memory_available_bytes:40,storage:{backend:'plain',dedicated:false,growth:'refused',size_bytes:1000,used_bytes:400}}};
  if(p.operation==='universe_stats')return {ok:true,data:{universes:[{universe_uuid:M,state:'running',manager:true,image:'sha256:aa'},{universe_uuid:U,state:'running',manager:false,image:'alpine'}]}};
  if(p.operation==='manager_status')return {ok:true,data:{store_bytes:123,resident_status:{replica_id:'r1',peers:{p2:{outcome:'local_exchange_failure',last_success_age_ms:1000000,failures:9,authenticated_successes:3,acknowledged_history_len:25}}}}};
  return {ok:false,error:'unexpected'};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());
 assert.equal((await fetch(url+'/api/health')).status,401);
 const body=await fetch(url+'/api/health',{headers:{'X-Podmesh-Token':session.token}}).then(r=>r.json());
 const [a,b,c]=body.hosts;
 assert.equal(a.host.cpu_count,2);assert.equal(a.universes.length,2);assert.equal(a.managers.length,1);assert.equal(a.managers[0].links[0].outcome,'local_exchange_failure');assert.equal(a.managers[0].store_bytes,123);
 assert.match(b.errors.runtime,/without host_status/);assert.match(c.errors.transport,/unreachable/);
 assert.deepEqual(calls.filter(([,op])=>op==='manager_status').map(([h,,u])=>h+':'+u),['a:'+M]);});
test('replication runs the tool for the active host with every other SSH host as candidate, and refuses what is not a replication',async t=>{
 const runs=[];let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',ssh:'lab@a',allowActions:true},{id:'b',name:'B',ssh:'lab@b',allowActions:true},{id:'c',name:'C',ssh:'lab@c',allowActions:true},{id:'r',name:'R',ssh:'lab@r',allowActions:false},{id:'l',name:'L',socket:'/l.sock',allowActions:true}],toolsDir:'/tools'},
  {origin:url,call:async()=>({ok:true,data:{}}),runReplication:async r=>{runs.push(r.args);return r.args.includes('status')?{result:'status',configured:false}:r.args.includes('refuse')?{result:'refused',error:'no'}:{result:r.args[2]};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());
 const U='00000000-0000-4000-8000-000000000001';
 const post=(body,extra={})=>fetch(url+'/api/replication',{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':session.token,...extra},body:JSON.stringify(body)});
 const st=await fetch(url+`/api/replication/a/${U}`,{headers:{'X-Podmesh-Token':session.token}}).then(r=>r.json());
 assert.equal(st.result,'status');assert.deepEqual(st.candidates.map(c=>c.id),['b','c','r']);
 const base={host:'a',universe_uuid:U,authorization_ref:'fixture'};
 assert.equal((await post({...base,action:'configure',standbys:'all',interval_seconds:900})).status,200);
 assert.deepEqual(runs.at(-1),['--reference','fixture','configure','--universe',U,'--active','lab@a','--hosts','lab@a,lab@b,lab@c,lab@r','--standbys','all','--interval','900','--capture','stopped']);
 assert.equal((await post({...base,action:'configure',standbys:2,interval_seconds:60})).status,200);assert.equal(runs.at(-1)[runs.at(-1).indexOf('--standbys')+1],'2');
 assert.equal((await post({...base,action:'configure',standbys:1,interval_seconds:300,capture:'live'})).status,200);assert.deepEqual(runs.at(-1).slice(-2),['--capture','live']);
 for(const bad of [{...base,action:'configure',standbys:4,interval_seconds:900},{...base,action:'configure',standbys:'all',interval_seconds:30},{...base,action:'run',standbys:'all'},{...base,action:'explode'},{...base,action:'run',extra:1},{...base,action:'run',authorization_ref:''},{...base,action:'configure',standbys:'all',interval_seconds:900,capture:'hot'},{...base,action:'run',capture:'live'}])
  assert.equal((await post(bad)).status,400,JSON.stringify(bad));
 assert.equal((await post({...base,host:'r',action:'run'})).status,403);
 assert.equal((await post({...base,host:'l',action:'run'})).status,409);
 assert.equal((await post({...base,action:'run'},{'X-Podmesh-Token':''})).status,401);
 for(const action of ['run','start','stop']){assert.equal((await post({...base,action})).status,200);assert.deepEqual(runs.at(-1),['--reference','fixture',action,'--universe',U]);}
 assert.equal(runs.filter(a=>a.includes('configure')).length,3);
 assert.equal((await post({...base,action:'takeover',standby:'b',planned:true})).status,200);assert.deepEqual(runs.at(-1),['--reference','fixture','takeover','--universe',U,'--standby','lab@b','--planned']);
 assert.equal((await post({...base,action:'takeover',standby:'c',planned:false})).status,200);assert.deepEqual(runs.at(-1).slice(-4),['--universe',U,'--standby','lab@c']);
 for(const bad of [{...base,action:'takeover',standby:'a',planned:true},{...base,action:'takeover',standby:'l',planned:true},{...base,action:'takeover',standby:'b'},{...base,action:'run',standby:'b'},{...base,action:'takeover',standby:'b',planned:true,capture:'live'}])
  assert.equal((await post(bad)).status,400,JSON.stringify(bad));
 assert.equal((await post({...base,action:'takeover',standby:'r',planned:true})).status,403);
 assert.equal((await post({...base,action:'guard'})).status,200);assert.deepEqual(runs.at(-1),['--reference','fixture','guard','--universe',U,'--lease','30','--margin','15','--tick','10']);
 assert.equal((await post({...base,action:'guard',lease_seconds:60,takeover_margin_seconds:20,tick_seconds:15,keep_stale:true})).status,200);assert.deepEqual(runs.at(-1).slice(-7),['--lease','60','--margin','20','--tick','15','--keep-stale']);
 assert.equal((await post({...base,action:'unguard'})).status,200);assert.deepEqual(runs.at(-1),['--reference','fixture','unguard','--universe',U]);
 for(const bad of [{...base,action:'guard',lease_seconds:20,tick_seconds:10},{...base,action:'guard',takeover_margin_seconds:2},{...base,action:'guard',keep_stale:'yes'},{...base,action:'guard',standby:'b'},{...base,action:'unguard',lease_seconds:30},{...base,action:'run',tick_seconds:10}])
  assert.equal((await post(bad)).status,400,JSON.stringify(bad));});
test('health carries the replication summary from the ledger, and says when it cannot be read',async t=>{
 let app,answer;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 const U='00000000-0000-4000-8000-00000000000b',seen=[];
 app=createApp({hosts:[{id:'a',name:'A',ssh:'lab@a'}]},{origin:url,call:async()=>({ok:true,data:{operations:[]}}),runReplication:async r=>{seen.push(r.args);return answer;}});
 const session=await fetch(url+'/api/session').then(r=>r.json());const get=()=>fetch(url+'/api/health',{headers:{'X-Podmesh-Token':session.token}}).then(r=>r.json());
 answer={result:'summary',universes:{[U]:{mode:'all',standbys:2,armed:true,interval_seconds:900,last_copy_age_seconds:30,last_run:{ok:true,stopped_for_seconds:3.4}}}};
 const body=await get();assert.equal(body.replication[U].standbys,2);assert.equal(body.replicationError,null);assert.deepEqual(seen[0],['summary']);
 answer={result:'unknown',error:'the replication tool answered nothing readable'};
 const bad=await get();assert.deepEqual(bad.replication,{});assert.match(bad.replicationError,/nothing readable/);});
