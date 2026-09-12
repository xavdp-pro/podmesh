import http from 'node:http';import test from 'node:test';import assert from 'node:assert/strict';import {createApp} from '../server/app.mjs';import {request}from'../server/transport.mjs';import net from'node:net';import fs from'node:fs/promises';import os from'node:os';import path from'node:path';
const payload={operation:'start',universe_uuid:'00000000-0000-4000-8000-000000000001',operation_id:'00000000-0000-4000-8000-000000000002',authorization_ref:'fixture'};
async function gateway(t,actions=true){
 const calls=[];let app;const s=http.createServer((req,res)=>app(req,res));s.listen(0,'127.0.0.1');await new Promise(r=>s.once('listening',r));t.after(()=>s.close());const url='http://127.0.0.1:'+s.address().port;
 app=createApp({hosts:[{id:'a',name:'A',socket:'/fixture.sock',allowActions:actions}]},{origin:url,call:async(_h,p)=>{calls.push(p);return {ok:true,data:p.operation==='capabilities'?{operations:['start']}:{}};}});
 const session=await fetch(url+'/api/session').then(r=>r.json());return{calls,url,post:(body=payload,extra={})=>fetch(url+'/api/hosts/a/actions',{method:'POST',headers:{Origin:url,'Content-Type':'application/json','X-Podmesh-Token':session.token,...extra},body:JSON.stringify(body)})};
}
test('action forwards exact identity through capability check',async t=>{const g=await gateway(t);assert.equal((await g.post()).status,200);assert.deepEqual(g.calls,[{operation:'capabilities'},payload]);});
test('missing session, foreign origin and unknown action refused',async t=>{const g=await gateway(t);assert.equal((await g.post(payload,{'X-Podmesh-Token':''})).status,403);assert.equal((await g.post(payload,{Origin:'https://foreign.test'})).status,403);assert.equal((await g.post({...payload,operation:'garbage_collect_apply'})).status,400);assert.deepEqual(g.calls,[]);});
test('read-only and unadvertised capabilities refuse writes',async t=>{const a=await gateway(t,false);assert.equal((await a.post()).status,403);assert.deepEqual(a.calls,[]);const b=await gateway(t);assert.equal((await b.post({...payload,operation:'delete'})).status,409);assert.equal(b.calls.length,1);});
test('DNS rebinding host refused',async t=>{const g=await gateway(t);const status=await new Promise((resolve,reject)=>{http.get(g.url+'/api/session',{headers:{Host:'foreign.test'}},r=>{r.resume();resolve(r.statusCode);}).on('error',reject);});assert.equal(status,403);});
test('Unix transport forwards newline JSON and bounds response',async t=>{const dir=await fs.mkdtemp(path.join(os.tmpdir(),'podmesh-web-'));const socket=path.join(dir,'api.sock');const server=net.createServer({allowHalfOpen:true},c=>{let b='';c.on('data',x=>{b+=x;if(b.endsWith('\n')){assert.deepEqual(JSON.parse(b),{operation:'identity'});c.end('{"ok":true,"data":{"host_uuid":"fixture"}}\n');}});});await new Promise(r=>server.listen(socket,r));t.after(async()=>{server.close();await fs.rm(dir,{recursive:true,force:true});});assert.deepEqual(await request({socket},{operation:'identity'}),{ok:true,data:{host_uuid:'fixture'}});await assert.rejects(request({socket},{operation:'x',value:'x'.repeat(4096)}),/limit/);});

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
