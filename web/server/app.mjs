import express from 'express';
import {randomBytes} from 'node:crypto';
import {request} from './transport.mjs';
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const actions=new Set(['create','clone','start','stop','delete']);
export function createApp(config,{call=request,origin='http://127.0.0.1:4175'}={}){
 const app=express();const token=randomBytes(32).toString('hex');
 const hosts=config.hosts||[];if(hosts.length>16)throw Error('Maximum16 hosts');
 const ids=new Set();for(const h of hosts){if(!/^[a-z0-9-]+$/.test(h.id)||ids.has(h.id)||h.ssh&&!/^[a-zA-Z0-9_.@-]+$/.test(h.ssh)||h.ssh?.startsWith('-'))throw Error('Invalid host configuration');if(!!h.ssh===!!h.socket||typeof h.name!=='string'||!h.name.trim())throw Error('Explicit host transport and name required');ids.add(h.id);}
 app.use((req,res,next)=>{res.set('Cache-Control','no-store');res.set('X-Content-Type-Options','nosniff');res.set('Referrer-Policy','no-referrer');res.set('X-Frame-Options','DENY');res.set('Content-Security-Policy',"default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'");if(req.headers.host!==new URL(origin).host)return res.status(403).json({error:'Unexpected host'});next();});
 app.use(express.json({limit:'4kb'}));
 app.get('/api/session',(_req,res)=>res.json({token,mode:'local-operator',hosts:hosts.map(({id,name,allowActions})=>({id,name,allowActions:!!allowActions}))}));
 let cached=null,loading=null,generation=0;
 app.get('/api/snapshot',async(_req,res)=>{
  if(!cached||Date.now()-cached.receivedAt>10000){
   const captured=generation;loading??=Promise.all(hosts.map(async h=>{
    const row={id:h.id,name:h.name,allowActions:!!h.allowActions,responses:{},errors:{},optionalErrors:{},receivedAt:Date.now()};
    for(const operation of ['identity','capabilities','inventory','observations','host_resource_metrics']){
     if(operation==='host_resource_metrics'&&!row.responses.capabilities?.data?.operations?.includes(operation))continue;
     try{const response=await call(h,{operation},{timeout:operation==='host_resource_metrics'?18000:15000});row.responses[operation]=response;if(!response.ok){if(operation==='host_resource_metrics')row.optionalErrors[operation]=response.error||'Metrics API refused';else row.errors[operation]=response.error||'API refused';}}catch(e){if(operation==='host_resource_metrics')row.optionalErrors[operation]=e.message;else{row.errors[operation]=e.message;break;}}
    }
    row.receivedAt=Date.now();return row;
   })).then(rows=>{const snapshot={receivedAt:Date.now(),hosts:rows};if(captured===generation)cached=snapshot;return snapshot;}).finally(()=>loading=null);
   const snapshot=await loading;return res.json(snapshot);
  }res.json(cached);
 });
 const detailBusy=new Set();
 app.post('/api/hosts/:id/details',async(req,res)=>{
  if(req.headers.origin!==origin||req.headers['x-podmesh-token']!==token)return res.status(403).json({error:'Same-origin session required'});
  const host=hosts.find(h=>h.id===req.params.id);if(!host)return res.status(404).json({error:'Unknown host'});
  const ids=req.body?.container_path;if(!Array.isArray(ids)||ids.length<1||ids.length>4||!ids.every(id=>typeof id==='string'&&/^[a-f0-9]{64}$/.test(id)))return res.status(400).json({error:'Full lowercase container IDs required; maximum depth four'});
  if(detailBusy.has(host.id))return res.status(429).json({error:'An observation is already in progress for this host; retry when it finishes'});detailBusy.add(host.id);
  const observer=host.detailsSocket?{...host,remoteSocket:host.detailsSocket,socket:host.ssh?host.socket:host.detailsSocket}:host;
  try{const caps=await call(observer,{operation:'capabilities'},{timeout:15000});if(!caps.ok||!caps.data.operations?.includes('container_details'))return res.status(409).json({error:'This host requires the container_details API update'});res.json(await call(observer,{operation:'container_details',container_path:ids},{timeout:75000}));}catch(e){res.status(502).json({error:e.message});}finally{detailBusy.delete(host.id);}
 });
 app.post('/api/hosts/:id/actions',async(req,res)=>{
  if(req.headers.origin!==origin||req.headers['x-podmesh-token']!==token)return res.status(403).json({error:'Same-origin session required'});
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
