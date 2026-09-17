import express from 'express';
import {randomBytes,randomUUID} from 'node:crypto';
import {request} from './transport.mjs';
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const actions=new Set(['create','clone','start','stop','pause','resume','resources','delete']);
// resources: the daemon holds the real bounds (32 MiB to the host's memory, 0.1 core to its cores); the gateway only refuses what is not a limit at all.
const MIN_MEMORY=32*1024*1024;
const MAX_RELATIONSHIP_RESPONSE_BYTES=1024*1024,MAX_RELATIONSHIP_RECORDS=5000,CLOCK_SKEW_SECONDS=5;
function relationshipSource(value){
 if(value===undefined)return null;
 if(!value||typeof value!=='object'||Array.isArray(value)||typeof value.endpoint!=='string')throw Error('Invalid relationship source configuration');
 let endpoint;try{endpoint=new URL(value.endpoint);}catch{throw Error('Invalid relationship source endpoint');}
 if(endpoint.protocol!=='http:'||!['127.0.0.1','[::1]'].includes(endpoint.hostname)||endpoint.username||endpoint.password||endpoint.hash||endpoint.pathname.length>256||endpoint.search.length>256)throw Error('Relationship source must be a loopback HTTP endpoint');
 const maxAgeSeconds=value.maxAgeSeconds===undefined?60:value.maxAgeSeconds;
 if(!Number.isInteger(maxAgeSeconds)||maxAgeSeconds<5||maxAgeSeconds>86400)throw Error('Invalid relationship source freshness limit');
 return {endpoint:endpoint.toString(),maxAgeSeconds};
}
async function boundedJson(response){
 if(!response?.ok){const error=Error('Manager relationship endpoint refused the read');error.relationshipStatus='unavailable';throw error;}
 if(!/^application\/json(?:;|$)/i.test(response.headers?.get?.('content-type')||''))throw Error('Manager relationship endpoint did not return JSON');
 const declared=Number(response.headers.get('content-length'));if(Number.isFinite(declared)&&declared>MAX_RELATIONSHIP_RESPONSE_BYTES)throw Error('Manager relationship response exceeds the 1 MiB safety limit');
 let bytes=0,parts=[];
 if(!response.body?.getReader){const body=await response.text();if(Buffer.byteLength(body)>MAX_RELATIONSHIP_RESPONSE_BYTES)throw Error('Manager relationship response exceeds the 1 MiB safety limit');try{return JSON.parse(body);}catch{throw Error('Manager relationship response is malformed');}}
 const reader=response.body.getReader();for(;;){const {done,value}=await reader.read();if(done)break;bytes+=value.byteLength;if(bytes>MAX_RELATIONSHIP_RESPONSE_BYTES){await reader.cancel();throw Error('Manager relationship response exceeds the 1 MiB safety limit');}parts.push(value);}
 try{return JSON.parse(new TextDecoder().decode(Buffer.concat(parts)));}catch{throw Error('Manager relationship response is malformed');}
}
// A foreign origin is forbidden (403); a missing or stale token is a session that has ended (401) --
// the console restarted, for instance -- and the page renews it instead of showing a refusal the
// operator cannot act on (INTENT.md, "Web surfaces").
function sessionEnded(res){res.status(401).json({error:'The console session has ended; it is renewed from /api/session',session:'renew'});return false;}
function requireReadSession(req,res,origin,token){if(req.headers.origin&&req.headers.origin!==origin){res.status(403).json({error:'Same-origin session required'});return false;}if(req.headers['x-podmesh-token']!==token)return sessionEnded(res);return true;}
function requireMutatingSession(req,res,origin,token){if(req.headers.origin!==origin){res.status(403).json({error:'Same-origin session required'});return false;}if(req.headers['x-podmesh-token']!==token)return sessionEnded(res);return true;}
// A move runs tools/move-universe.py from the PodMesh tree (the workstation is the protocol's transport controller); the tree is named by config.toolsDir or PODMESH_TOOLS_DIR.
import {spawn} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
// A replication runs tools/replicate-universe.py from the PodMesh tree on this workstation, like a move.
function defaultRunReplication({args,host,toolsDir}){
 const tool=path.join(toolsDir,'replicate-universe.py');
 const env={...process.env,PODMESH_SOCKET:host.remoteSocket||'/run/podmesh/api.sock',PODMESH_STATE_DIR:host.remoteStateDir||'/var/lib/podmesh',PODMESH_UNIT:host.remoteUnit||'podmesh.service'};
 return new Promise(resolve=>{const out=[];let done=false;const p=spawn('python3',['-B',tool,...args],{env,stdio:['ignore','pipe','pipe']});
  const timer=setTimeout(()=>{if(!done){done=true;p.kill();resolve({result:'unknown',error:'the replication tool did not finish within ten minutes; read the status again'});}},600000);
  p.stdout.on('data',d=>out.push(d));p.stderr.resume();
  p.on('error',e=>{if(!done){done=true;clearTimeout(timer);resolve({result:'unknown',error:'the replication tool could not be started: '+e.message});}});
  p.on('close',()=>{if(done)return;done=true;clearTimeout(timer);const text=Buffer.concat(out).toString('utf8');try{resolve(JSON.parse(text.slice(text.indexOf('{'))));}catch{resolve({result:'unknown',error:'the replication tool answered nothing readable',raw:text.slice(-600)});}});});
}
function defaultRunMove({source,destination,universe_uuid,authorization_ref,keep_source,toolsDir}){
 const tool=path.join(toolsDir,'move-universe.py');
 const env={...process.env,PODMESH_SOCKET:source.remoteSocket||'/run/podmesh/api.sock',PODMESH_STATE_DIR:source.remoteStateDir||'/var/lib/podmesh',PODMESH_UNIT:source.remoteUnit||'podmesh.service'};
 const args=['-B',tool,'--source',source.ssh,'--destination',destination.ssh,'--universe',universe_uuid,'--reference',authorization_ref,...(keep_source?['--keep-source']:[])];
 return new Promise((resolve)=>{const out=[];let done=false;const p=spawn('python3',args,{env,stdio:['ignore','pipe','pipe']});
  const timer=setTimeout(()=>{if(!done){done=true;p.kill();resolve({result:'unknown',message:'the move tool did not finish within ten minutes; the universe stays where the protocol leaves it -- inspect both hosts',steps:[]});}},600000);
  p.stdout.on('data',d=>out.push(d));p.stderr.resume();
  p.on('error',e=>{if(!done){done=true;clearTimeout(timer);resolve({result:'unknown',message:'the move tool could not be started: '+e.message,steps:[]});}});
  p.on('close',()=>{if(done)return;done=true;clearTimeout(timer);const text=Buffer.concat(out).toString('utf8');try{resolve(JSON.parse(text.slice(text.indexOf('{'))));}catch{resolve({result:'unknown',message:'the move tool answered nothing readable; inspect both hosts',steps:[],raw:text.slice(-800)});}});});
}
export function createApp(config,{call=request,origin='http://127.0.0.1:4175',relationshipFetch=fetch,now=()=>Date.now(),runMove=defaultRunMove,runReplication=defaultRunReplication}={}){
 const app=express();const token=randomBytes(32).toString('hex');
 const hosts=config.hosts||[];if(hosts.length>16)throw Error('Maximum16 hosts');
 const relationships=relationshipSource(config.relationships);
 const ids=new Set();for(const h of hosts){if(!/^[a-z0-9-]+$/.test(h.id)||ids.has(h.id)||h.ssh&&!/^[a-zA-Z0-9_.@-]+$/.test(h.ssh)||h.ssh?.startsWith('-'))throw Error('Invalid host configuration');if(!!h.ssh===!!h.socket||typeof h.name!=='string'||!h.name.trim())throw Error('Explicit host transport and name required');ids.add(h.id);}
 // Inline styles only, as the administration app allows them: the new front's toasts inject their stylesheet; scripts stay 'self'.
 app.use((req,res,next)=>{res.set('Cache-Control','no-store');res.set('X-Content-Type-Options','nosniff');res.set('Referrer-Policy','no-referrer');res.set('X-Frame-Options','DENY');res.set('Content-Security-Policy',"default-src 'self'; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'");if(req.headers.host!==new URL(origin).host)return res.status(403).json({error:'Unexpected host'});next();});
 app.use(express.json({limit:'4kb'}));
 app.get('/api/session',(_req,res)=>res.json({token,mode:'local-operator',hosts:hosts.map(({id,name,allowActions,ssh})=>({id,name,allowActions:!!allowActions,canMove:!!(allowActions&&ssh)}))}));
 // Health, read-only: for every host, what it carries (host_status), what each universe uses (universe_stats), and for
 // every manager universe its replication links as the resident reports them (manager_status). Each host answers on its
 // own; one that fails is reported as such, never hidden, and never blocks the others.
 const replicationToolsDir=()=>config.toolsDir||process.env.PODMESH_TOOLS_DIR||path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../../../../podmesh/tools');
 app.get('/api/health',async(req,res)=>{
  if(!requireReadSession(req,res,origin,token))return;
  const rows=await Promise.all(hosts.map(async h=>{
   const row={id:h.id,name:h.name,errors:{}};
   try{
    const caps=await call(h,{operation:'capabilities'},{timeout:15000});
    const ops=new Set(caps?.ok?caps.data.operations||[]:[]);
    if(!ops.has('host_status')||!ops.has('universe_stats')){row.errors.runtime='This host runs a PodMesh without host_status and universe_stats';return row;}
    const id=()=>randomUUID();
    const [hs,us]=await Promise.all([call(h,{operation:'host_status',operation_id:id(),authorization_ref:'console-health'},{timeout:20000}),call(h,{operation:'universe_stats',operation_id:id(),authorization_ref:'console-health'},{timeout:30000})]);
    if(hs.ok)row.host=hs.data;else row.errors.host_status=hs.error||'refused';
    if(us.ok)row.universes=us.data.universes;else row.errors.universe_stats=us.error||'refused';
    row.managers=[];
    for(const u of (us.ok?us.data.universes:[]).filter(x=>x.state==='running'&&x.manager===true)){
     const ms=await call(h,{operation:'manager_status',operation_id:id(),universe_uuid:u.universe_uuid,authorization_ref:'console-health'},{timeout:20000}).catch(e=>({ok:false,error:e.message}));
     if(!ms.ok){row.managers.push({universe_uuid:u.universe_uuid,error:ms.error||'refused'});continue;}
     const rs=ms.data.resident_status||{};
     row.managers.push({universe_uuid:u.universe_uuid,replica_id:rs.replica_id,store_bytes:ms.data.store_bytes,
      links:Object.entries(rs.peers||{}).map(([peer,v])=>({peer,outcome:v.outcome,last_success_age_ms:v.last_success_age_ms,failures:v.failures,successes:v.authenticated_successes,acknowledged_history_len:v.acknowledged_history_len,local_history_len:v.local_history_len_at_attempt,next_attempt_in_ms:v.next_attempt_in_ms}))});
    }
   }catch(e){row.errors.transport=e.message;}
   return row;
  }));
  // Replication of ordinary universes, from the workstation's ledger alone (no host is reached for it).
  let replication={},replicationError=null;const toolHost=hosts.find(h=>h.ssh);
  if(toolHost){const r=await runReplication({args:['summary'],host:toolHost,toolsDir:replicationToolsDir()});if(r.result==='summary')replication=r.universes||{};else replicationError=r.error||'the replication summary could not be read';}
  res.json({receivedAt:now(),hosts:rows,replication,replicationError});
 });
 // The generic engine: any operation the host advertises with a schema, validated here against that schema -- the same
 // bounds the daemon enforces -- and sent as it was built. Reads need the read session; mutations the mutating one and a
 // host that allows actions; a 'tool' step is refused here (a workstation tool drives it across hosts).
 const UUID_RE=uuid;
 function validateAgainst(schema,p){
  const known=new Set(['operation','operation_id','authorization_ref',...(schema.fields||[]).map(f=>f.name)]);
  const needsUniverse=schema.kind==='universe'||(schema.fields||[]).some(f=>f.name==='universe_uuid');
  if(needsUniverse){known.add('universe_uuid');if(!UUID_RE.test(p.universe_uuid))return 'universe_uuid must be a UUID';}
  for(const k of Object.keys(p))if(!known.has(k))return `Unexpected field ${k}`;
  for(const f of schema.fields||[]){const v=p[f.name];if(v===undefined){if(f.required)return `${f.name} is required`;continue;}
   switch(f.type){
    case 'integer':if(!Number.isInteger(v)||(f.min!==undefined&&v<f.min)||(f.max!==undefined&&v>f.max))return `${f.name} must be an integer${f.min!==undefined?` from ${f.min}`:''}${f.max!==undefined?` to ${f.max}`:''}`;break;
    case 'number':if(typeof v!=='number'||!Number.isFinite(v)||(f.min!==undefined&&v<f.min)||(f.max!==undefined&&v>f.max))return `${f.name} must be a number${f.min!==undefined?` from ${f.min}`:''}${f.max!==undefined?` to ${f.max}`:''}`;break;
    case 'boolean':if(typeof v!=='boolean')return `${f.name} must be true or false`;break;
    case 'enum':if(!f.values?.includes(v))return `${f.name} must be one of ${(f.values||[]).join(', ')}`;break;
    case 'uuid':if(!UUID_RE.test(v))return `${f.name} must be a UUID`;break;
    case 'string':if(typeof v!=='string'||v.length>4096)return `${f.name} must be a string`;break;
    case 'string[]':if(!Array.isArray(v)||!v.every(x=>typeof x==='string')||v.length>256)return `${f.name} must be an array of strings`;break;
    case 'uuid[]':if(!Array.isArray(v)||!v.every(x=>UUID_RE.test(x)))return `${f.name} must be an array of UUIDs`;break;
    case 'object[]':if(!Array.isArray(v)||!v.every(x=>x&&typeof x==='object'&&!Array.isArray(x)))return `${f.name} must be an array of objects`;break;
    case 'object':if(!v||typeof v!=='object'||Array.isArray(v))return `${f.name} must be an object`;break;
    default:return `${f.name} has a type this console does not know (${f.type})`;
   }}
  return null;
 }
 app.post('/api/hosts/:id/operations',express.json({limit:'64kb'}),async(req,res)=>{
  const host=hosts.find(h=>h.id===req.params.id);if(!host)return res.status(404).json({error:'Unknown host'});
  const p=req.body;if(!p||typeof p.operation!=='string'||!/^[a-z_]{1,64}$/.test(p.operation)||!uuid.test(p.operation_id)||typeof p.authorization_ref!=='string'||!p.authorization_ref.trim()||p.authorization_ref.length>256)return res.status(400).json({error:'Invalid operation or identity'});
  if(JSON.stringify(p).length>32768)return res.status(413).json({error:'Request too large'});
  let caps;try{caps=await call(host,{operation:'capabilities'},{timeout:15000});}catch(e){return res.status(502).json({error:e.message});}
  const schema=caps?.ok&&caps.data.schemas?.[p.operation];
  if(!schema)return res.status(409).json({error:caps?.data?.schemas?'Operation not advertised by this host':'This host publishes no operation schemas (older runtime); use the dedicated actions'});
  if(schema.kind==='tool')return res.status(409).json({error:'This operation is one step of a chain a workstation tool drives across hosts; it is not sent alone from here'});
  if(schema.kind==='read'){if(!requireReadSession(req,res,origin,token))return;}
  else{if(!requireMutatingSession(req,res,origin,token))return;if(!host.allowActions)return res.status(403).json({error:'Actions disabled by operator configuration'});}
  const problem=validateAgainst(schema,p);if(problem)return res.status(400).json({error:problem});
  try{const result=await call(host,p,{timeout:schema.kind==='read'?30000:330000});if(schema.kind!=='read'){cached=null;generation++;}res.json(result);}
  catch(e){if(schema.kind!=='read'){cached=null;generation++;}res.status(502).json({error:e.message,operation_id:p.operation_id,outcome:'unknown'});}
 });
 // Replication of a universe to standbys: status (read), and configure / run / start / stop (mutating), each through the
 // replication tool; the active host is the one the universe runs on, the candidates every other SSH host of this console.
 app.get('/api/replication/:host/:universe',async(req,res)=>{
  if(!requireReadSession(req,res,origin,token))return;
  const host=hosts.find(h=>h.id===req.params.host);if(!host)return res.status(404).json({error:'Unknown host'});
  if(!uuid.test(req.params.universe))return res.status(400).json({error:'Invalid universe'});
  const report=await runReplication({args:['status','--universe',req.params.universe],host,toolsDir:replicationToolsDir()});
  res.status(report.result==='status'?200:502).json({...report,candidates:hosts.filter(h=>h.ssh&&h.id!==host.id).map(h=>({id:h.id,name:h.name,ssh:h.ssh}))});
 });
 app.post('/api/replication',express.json({limit:'8kb'}),async(req,res)=>{
  if(!requireMutatingSession(req,res,origin,token))return;
  const p=req.body||{};const host=hosts.find(h=>h.id===p.host);
  if(!host)return res.status(404).json({error:'Unknown host'});
  if(!['configure','run','start','stop','takeover','guard','unguard'].includes(p.action)||!uuid.test(p.universe_uuid)||typeof p.authorization_ref!=='string'||!p.authorization_ref.trim()||p.authorization_ref.length>256)return res.status(400).json({error:'Invalid replication request'});
  if(Object.keys(p).some(k=>!['action','host','universe_uuid','authorization_ref','standbys','interval_seconds','capture','standby','planned','lease_seconds','takeover_margin_seconds','tick_seconds','keep_stale'].includes(k)))return res.status(400).json({error:'Unexpected replication field'});
  if(!host.allowActions)return res.status(403).json({error:'Actions disabled by operator configuration'});
  if(!host.ssh)return res.status(409).json({error:'A replication needs the active host reached over SSH from this console'});
  const others=hosts.filter(h=>h.ssh&&h.id!==host.id);
  let args=['--reference',p.authorization_ref.trim()];
  if(p.action==='configure'){
   if(!others.length)return res.status(409).json({error:'No other host reached over SSH to replicate to'});
   const standbys=p.standbys==='all'?'all':Number.isInteger(p.standbys)&&p.standbys>=1&&p.standbys<=others.length?String(p.standbys):null;
   if(!standbys)return res.status(400).json({error:`standbys must be "all" or 1 to ${others.length}`});
   if(!Number.isInteger(p.interval_seconds)||p.interval_seconds<60||p.interval_seconds>86400)return res.status(400).json({error:'interval_seconds must be from 60 to 86400'});
   // live: the universe is checkpointed with its memory and resumed in place, never stopped; stopped: a quiescent copy.
   const capture=p.capture===undefined?'stopped':p.capture;
   if(!['stopped','live'].includes(capture))return res.status(400).json({error:'capture must be "stopped" or "live"'});
   args.push('configure','--universe',p.universe_uuid,'--active',host.ssh,'--hosts',[host.ssh,...others.map(h=>h.ssh)].join(','),'--standbys',standbys,'--interval',String(p.interval_seconds),'--capture',capture);
  }else if(p.action==='guard'){
   // Continuity: the guardian renews the lease every tick and fails over to the first standby with a copy after lease + margin.
   const lease=p.lease_seconds??30,margin=p.takeover_margin_seconds??15,tick=p.tick_seconds??10;
   if(![lease,margin,tick].every(Number.isInteger)||lease<5||lease>3600||margin<5||margin>3600||tick<2||lease<3*tick)return res.status(400).json({error:'lease_seconds 5-3600, takeover_margin_seconds 5-3600, tick_seconds at least 2 and at most a third of the lease'});
   if(p.keep_stale!==undefined&&typeof p.keep_stale!=='boolean')return res.status(400).json({error:'keep_stale must be true or false'});
   if(p.standbys!==undefined||p.interval_seconds!==undefined||p.capture!==undefined||p.standby!==undefined||p.planned!==undefined)return res.status(400).json({error:'guard takes lease_seconds, takeover_margin_seconds, tick_seconds and keep_stale'});
   args.push('guard','--universe',p.universe_uuid,'--lease',String(lease),'--margin',String(margin),'--tick',String(tick),...(p.keep_stale?['--keep-stale']:[]));
  }else if(p.action==='takeover'){
   // The standby becomes the active host: planned = a switchover while the active host is fine; otherwise the active host is lost.
   const standby=others.find(h=>h.id===p.standby);
   if(!standby)return res.status(400).json({error:'standby must name another host reached over SSH'});
   if(!standby.allowActions)return res.status(403).json({error:'Actions disabled by operator configuration on the standby'});
   if(typeof p.planned!=='boolean')return res.status(400).json({error:'planned must be true or false'});
   if(p.standbys!==undefined||p.interval_seconds!==undefined||p.capture!==undefined)return res.status(400).json({error:'standbys, interval_seconds and capture belong to configure'});
   args.push('takeover','--universe',p.universe_uuid,'--standby',standby.ssh,...(p.planned?['--planned']:[]));
  }else{
   if(p.standbys!==undefined||p.interval_seconds!==undefined||p.capture!==undefined||p.standby!==undefined||p.planned!==undefined||p.lease_seconds!==undefined||p.takeover_margin_seconds!==undefined||p.tick_seconds!==undefined||p.keep_stale!==undefined)return res.status(400).json({error:'standbys, interval_seconds and capture belong to configure; standby and planned to takeover; lease, margin, tick and keep_stale to guard'});
   args.push(p.action,'--universe',p.universe_uuid);
  }
  const report=await runReplication({args,host,toolsDir:replicationToolsDir()});
  if(['run','configure','takeover','guard','unguard'].includes(p.action)){cached=null;generation++;}
  res.status(report.result==='refused'?409:report.result==='unknown'?502:200).json(report);
 });
 // A move: one universe, two hosts of this configuration, both reached over SSH and both allowing actions, on the same runtime paths.
 app.post('/api/moves',express.json({limit:'8kb'}),async(req,res)=>{
  if(!requireMutatingSession(req,res,origin,token))return;
  const p=req.body||{};const source=hosts.find(h=>h.id===p.source),destination=hosts.find(h=>h.id===p.destination);
  if(!source||!destination)return res.status(404).json({error:'Unknown host'});
  if(source.id===destination.id)return res.status(400).json({error:'The source and the destination are the same host'});
  if(!uuid.test(p.universe_uuid)||typeof p.authorization_ref!=='string'||!p.authorization_ref.trim()||p.authorization_ref.length>256||(p.keep_source!==undefined&&typeof p.keep_source!=='boolean'))return res.status(400).json({error:'Invalid move or identity'});
  if(Object.keys(p).some(k=>!['universe_uuid','source','destination','authorization_ref','keep_source'].includes(k)))return res.status(400).json({error:'Unexpected move field'});
  if(!source.allowActions||!destination.allowActions)return res.status(403).json({error:'Actions disabled by operator configuration on one of the hosts'});
  if(!source.ssh||!destination.ssh)return res.status(409).json({error:'A move needs both hosts reached over SSH from this console'});
  if((source.remoteSocket||'')!==(destination.remoteSocket||'')||(source.remoteStateDir||'')!==(destination.remoteStateDir||''))return res.status(409).json({error:'A move needs both hosts on the same runtime paths'});
  const toolsDir=config.toolsDir||process.env.PODMESH_TOOLS_DIR||path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../../../../podmesh/tools');
  try{const report=await runMove({source,destination,universe_uuid:p.universe_uuid,authorization_ref:p.authorization_ref.trim(),keep_source:!!p.keep_source,toolsDir});cached=null;generation++;res.status(report.result==='moved'?200:report.result==='refused'?409:502).json(report);}
  catch(e){cached=null;generation++;res.status(502).json({result:'unknown',message:e.message,steps:[]});}
 });
 app.get('/api/relationships',async(req,res)=>{
  if(!requireReadSession(req,res,origin,token))return;
  if(!relationships)return res.json({status:'unavailable',error:'No manager relationship endpoint is configured'});
  const controller=new AbortController(),timer=setTimeout(()=>controller.abort(),10000);
  try{
   let response;try{response=await relationshipFetch(relationships.endpoint,{method:'GET',headers:{Accept:'application/json'},redirect:'error',signal:controller.signal});}catch(error){error.relationshipStatus='unavailable';throw error;}
   const payload=await boundedJson(response);
   if(!payload||typeof payload!=='object'||Array.isArray(payload)||!Array.isArray(payload.relationships)||!Number.isFinite(payload.observed_at))throw Error('Manager relationship response has an invalid envelope');
   if(payload.relationships.length>MAX_RELATIONSHIP_RECORDS)throw Error(`Manager relationship response exceeds the ${MAX_RELATIONSHIP_RECORDS}-record safety limit`);
   const age=now()/1000-payload.observed_at;
   if(age>relationships.maxAgeSeconds||age < -CLOCK_SKEW_SECONDS)return res.json({status:'stale',error:'Manager relationship data is stale or future-dated',observed_at:payload.observed_at,max_age_seconds:relationships.maxAgeSeconds});
   return res.json({status:'available',relationships:payload.relationships,observed_at:payload.observed_at,max_age_seconds:relationships.maxAgeSeconds});
  }catch(error){const status=error.relationshipStatus||(error.name==='AbortError'?'unavailable':'malformed');return res.json({status,error:error.name==='AbortError'?'Manager relationship read timed out':error.message});}finally{clearTimeout(timer);}
 });
 let cached=null,loading=null,generation=0;const observerBusy=new Set();
 app.get('/api/snapshot',async(_req,res)=>{
  if(!cached||Date.now()-cached.receivedAt>10000){
   const captured=generation;loading??=Promise.all(hosts.map(async h=>{
    const row={id:h.id,name:h.name,allowActions:!!h.allowActions,canMove:!!(h.allowActions&&h.ssh),responses:{},errors:{},optionalErrors:{},receivedAt:Date.now()};
    for(const operation of ['identity','capabilities','inventory','observations']){
     try{const response=await call(h,{operation},{timeout:15000});row.responses[operation]=response;if(!response.ok)row.errors[operation]=response.error||'API refused';}catch(e){row.errors[operation]=e.message;break;}
    }
    const observer=h.detailsSocket?{...h,remoteSocket:h.detailsSocket,socket:h.ssh?h.socket:h.detailsSocket}:h;
    const previous=cached?.hosts.find(previousHost=>previousHost.id===h.id);
    if(observerBusy.has(h.id)){
     row.metricsDeferred=true;
     if(previous?.responses.host_resource_capabilities)row.responses.host_resource_capabilities=previous.responses.host_resource_capabilities;
     if(previous?.responses.host_resource_metrics)row.responses.host_resource_metrics=previous.responses.host_resource_metrics;
    }else{
     observerBusy.add(h.id);
     try{
      const observerCapabilities=h.detailsSocket?await call(observer,{operation:'capabilities'},{timeout:15000}):row.responses.capabilities;
      row.responses.host_resource_capabilities=observerCapabilities;
      if(observerCapabilities?.ok&&observerCapabilities.data.operations?.includes('host_resource_metrics')){
       const response=await call(observer,{operation:'host_resource_metrics'},{timeout:18000});row.responses.host_resource_metrics=response;if(!response.ok)row.optionalErrors.host_resource_metrics=response.error||'Metrics API refused';
      }
     }catch(e){row.optionalErrors.host_resource_metrics=e.message;}finally{observerBusy.delete(h.id);}
    }
    row.receivedAt=Date.now();return row;
   })).then(rows=>{const snapshot={receivedAt:Date.now(),hosts:rows};if(captured===generation)cached=snapshot;return snapshot;}).finally(()=>loading=null);
   const snapshot=await loading;return res.json(snapshot);
  }res.json(cached);
 });
 app.post('/api/hosts/:id/details',async(req,res)=>{
  if(!requireMutatingSession(req,res,origin,token))return;
  const host=hosts.find(h=>h.id===req.params.id);if(!host)return res.status(404).json({error:'Unknown host'});
  const ids=req.body?.container_path;if(!Array.isArray(ids)||ids.length<1||ids.length>4||!ids.every(id=>typeof id==='string'&&/^[a-f0-9]{64}$/.test(id)))return res.status(400).json({error:'Full lowercase container IDs required; maximum depth four'});
  if(observerBusy.has(host.id))return res.status(429).json({error:'An observation is already in progress for this host; retry when it finishes'});observerBusy.add(host.id);
  const observer=host.detailsSocket?{...host,remoteSocket:host.detailsSocket,socket:host.ssh?host.socket:host.detailsSocket}:host;
  try{const caps=await call(observer,{operation:'capabilities'},{timeout:15000});if(!caps.ok||!caps.data.operations?.includes('container_details'))return res.status(409).json({error:'This host requires the container_details API update'});res.json(await call(observer,{operation:'container_details',container_path:ids},{timeout:75000}));}catch(e){res.status(502).json({error:e.message});}finally{observerBusy.delete(host.id);}
 });
 app.post('/api/hosts/:id/actions',async(req,res)=>{
  if(!requireMutatingSession(req,res,origin,token))return;
  const host=hosts.find(h=>h.id===req.params.id);if(!host)return res.status(404).json({error:'Unknown host'});
  if(!host.allowActions)return res.status(403).json({error:'Actions disabled by operator configuration'});
  const p=req.body;if(!p||!actions.has(p.operation)||!uuid.test(p.universe_uuid)||!uuid.test(p.operation_id)||typeof p.authorization_ref!=='string'||!p.authorization_ref.trim()||p.authorization_ref.length>256)return res.status(400).json({error:'Invalid action or identity'});
  if(p.operation==='clone'&&!uuid.test(p.source_uuid))return res.status(400).json({error:'Invalid clone source'});
  const fields=['operation','universe_uuid','operation_id','authorization_ref',...({create:['image','command'],clone:['source_uuid'],start:[],stop:['timeout_seconds','on_timeout'],pause:[],resume:[],resources:['memory_bytes','cpus'],delete:[]}[p.operation])];
  if(Object.keys(p).some(k=>!fields.includes(k)))return res.status(400).json({error:'Unexpected action field'});
  if(p.operation==='create'&&(!/^sha256:[a-f0-9]{64}$/.test(p.image)||!Array.isArray(p.command)||p.command.length>64||!p.command.every(v=>typeof v==='string')))return res.status(400).json({error:'Invalid image or command'});
  if(p.operation==='stop'&&(p.timeout_seconds!==10||p.on_timeout!=='leave_running'))return res.status(400).json({error:'Only a 10-second non-escalating stop is supported'});
  if(p.operation==='resources'){const m=p.memory_bytes,c=p.cpus;const mOk=m===undefined||(Number.isInteger(m)&&m>=MIN_MEMORY);const cOk=c===undefined||(typeof c==='number'&&Number.isFinite(c)&&c>=0.1&&c<=1024);if(!mOk||!cOk||(m===undefined&&c===undefined))return res.status(400).json({error:'resources takes memory_bytes (an integer, at least 32 MiB), cpus (a number from 0.1), or both'});}
  try{const caps=await call(host,{operation:'capabilities'},{timeout:15000});if(!caps.ok||!caps.data.operations?.includes(p.operation))return res.status(409).json({error:'Operation not advertised'});const result=await call(host,p);cached=null;res.json(result);}catch(e){res.status(502).json({error:e.message,operation_id:p.operation_id,outcome:'unknown'});}finally{generation++;cached=null;}
 });
 app.use((err,_req,res,_next)=>res.status(400).json({error:'Invalid request'}));
 return app;
}
