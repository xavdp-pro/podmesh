import express from 'express';
import {randomBytes} from 'node:crypto';
import {request} from './transport.mjs';
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const actions=new Set(['create','clone','start','stop','delete']);
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
export function createApp(config,{call=request,origin='http://127.0.0.1:4175',relationshipFetch=fetch,now=()=>Date.now()}={}){
 const app=express();const token=randomBytes(32).toString('hex');
 const hosts=config.hosts||[];if(hosts.length>16)throw Error('Maximum16 hosts');
 const relationships=relationshipSource(config.relationships);
 const ids=new Set();for(const h of hosts){if(!/^[a-z0-9-]+$/.test(h.id)||ids.has(h.id)||h.ssh&&!/^[a-zA-Z0-9_.@-]+$/.test(h.ssh)||h.ssh?.startsWith('-'))throw Error('Invalid host configuration');if(!!h.ssh===!!h.socket||typeof h.name!=='string'||!h.name.trim())throw Error('Explicit host transport and name required');ids.add(h.id);}
 app.use((req,res,next)=>{res.set('Cache-Control','no-store');res.set('X-Content-Type-Options','nosniff');res.set('Referrer-Policy','no-referrer');res.set('X-Frame-Options','DENY');res.set('Content-Security-Policy',"default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'");if(req.headers.host!==new URL(origin).host)return res.status(403).json({error:'Unexpected host'});next();});
 app.use(express.json({limit:'4kb'}));
 app.get('/api/session',(_req,res)=>res.json({token,mode:'local-operator',hosts:hosts.map(({id,name,allowActions})=>({id,name,allowActions:!!allowActions}))}));
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
    const row={id:h.id,name:h.name,allowActions:!!h.allowActions,responses:{},errors:{},optionalErrors:{},receivedAt:Date.now()};
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
  const fields=['operation','universe_uuid','operation_id','authorization_ref',...({create:['image','command'],clone:['source_uuid'],start:[],stop:['timeout_seconds','on_timeout'],delete:[]}[p.operation])];
  if(Object.keys(p).some(k=>!fields.includes(k)))return res.status(400).json({error:'Unexpected action field'});
  if(p.operation==='create'&&(!/^sha256:[a-f0-9]{64}$/.test(p.image)||!Array.isArray(p.command)||p.command.length>64||!p.command.every(v=>typeof v==='string')))return res.status(400).json({error:'Invalid image or command'});
  if(p.operation==='stop'&&(p.timeout_seconds!==10||p.on_timeout!=='leave_running'))return res.status(400).json({error:'Only a 10-second non-escalating stop is supported'});
  try{const caps=await call(host,{operation:'capabilities'},{timeout:15000});if(!caps.ok||!caps.data.operations?.includes(p.operation))return res.status(409).json({error:'Operation not advertised'});const result=await call(host,p);cached=null;res.json(result);}catch(e){res.status(502).json({error:e.message,operation_id:p.operation_id,outcome:'unknown'});}finally{generation++;cached=null;}
 });
 app.use((err,_req,res,_next)=>res.status(400).json({error:'Invalid request'}));
 return app;
}
