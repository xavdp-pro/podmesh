import {useMemo,useState} from 'react';import Select from './Select.jsx';
// One mechanism for every operation: the form is drawn from the schema the host itself publishes in
// its capabilities -- kind, gate, fields, types, bounds -- validated here against the same schema,
// sent as one JSON request, and answered with the host's typed result. Nothing is a native form post.
const NEEDS_UUID=kind=>kind==='universe';
function initial(schema){const o={};for(const f of schema?.fields||[])o[f.name]=f.type==='boolean'?false:'';return o;}
function toValue(f,raw){
 if(raw===''||raw===undefined||raw===null)return undefined;
 switch(f.type){
  case 'integer':{if(!/^-?\d+$/.test(String(raw).trim()))throw Error(`${f.name} must be an integer`);const n=Number(raw);if(f.min!==undefined&&n<f.min)throw Error(`${f.name} is at least ${f.min}`);if(f.max!==undefined&&n>f.max)throw Error(`${f.name} is at most ${f.max}`);return n;}
  case 'number':{const n=Number(raw);if(!Number.isFinite(n))throw Error(`${f.name} must be a number`);if(f.min!==undefined&&n<f.min)throw Error(`${f.name} is at least ${f.min}`);if(f.max!==undefined&&n>f.max)throw Error(`${f.name} is at most ${f.max}`);return n;}
  case 'boolean':return !!raw;
  case 'enum':if(!f.values.includes(raw))throw Error(`${f.name} must be one of ${f.values.join(', ')}`);return raw;
  case 'uuid':if(!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(raw))throw Error(`${f.name} must be a UUID`);return raw;
  case 'string[]':case 'uuid[]':case 'object[]':case 'object':{let v;try{v=JSON.parse(raw);}catch{throw Error(`${f.name} must be valid JSON`);}
   if(f.type==='object'){if(!v||typeof v!=='object'||Array.isArray(v))throw Error(`${f.name} must be a JSON object`);return v;}
   if(!Array.isArray(v))throw Error(`${f.name} must be a JSON array`);
   if(f.type==='string[]'&&!v.every(x=>typeof x==='string'))throw Error(`${f.name} must be an array of strings`);
   if(f.type==='object[]'&&!v.every(x=>x&&typeof x==='object'))throw Error(`${f.name} must be an array of objects`);return v;}
  default:return String(raw);
 }
}
export function buildRequest(schema,operation,fields,universe,authorization){
 const req={operation,operation_id:crypto.randomUUID(),authorization_ref:authorization};
 if(NEEDS_UUID(schema.kind)||schema.fields?.some(f=>f.name==='universe_uuid')){if(!/^[0-9a-f-]{36}$/i.test(universe||''))throw Error('universe_uuid must be a UUID');req.universe_uuid=universe;}
 for(const f of schema.fields||[]){const v=toValue(f,fields[f.name]);if(v===undefined){if(f.required)throw Error(`${f.name} is required`);continue;}req[f.name]=v;}
 if(!authorization.trim())throw Error('An authorization reference is required');
 return req;
}
export default function OperationForm({hosts,universes,session,onSent,renewSession}){
 const[hostId,H]=useState(hosts[0]?.id||''),[operation,Op]=useState(''),[fields,Fd]=useState({}),[universe,U]=useState(''),[auth,Au]=useState(''),[error,E]=useState(''),[result,R]=useState(null),[busy,B]=useState(false);
 const host=hosts.find(h=>h.id===hostId);const schemas=host?.responses?.capabilities?.data?.schemas||{};
 const ops=useMemo(()=>Object.entries(schemas).sort(([a],[b])=>a.localeCompare(b)).map(([name,s])=>({value:name,label:name.replaceAll('_',' '),hint:`${s.kind} · ${s.gate}`})),[schemas]);
 const schema=schemas[operation];
 const pickOp=name=>{Op(name);Fd(initial(schemas[name]));E('');R(null);};
 const universeOptions=(universes||[]).filter(c=>c.host.id===hostId&&c.uuid).map(c=>({value:c.uuid,label:c.name,hint:c.State}));
 async function submit(e){e.preventDefault();if(busy||!schema)return;B(true);E('');R(null);
  try{const req=buildRequest(schema,operation,fields,universe,auth);
   const r=await fetch(`/api/hosts/${hostId}/operations`,{method:'POST',headers:{'Content-Type':'application/json','X-Podmesh-Token':session.token},body:JSON.stringify(req)});
   if(r.status===401){await renewSession();E('The console restarted and its session was renewed. Send again.');return;}
   const body=await r.json();R({status:r.status,body,request:req});if(onSent&&schema.kind!=='read')onSent();}
  catch(err){E(err.message);}finally{B(false);}}
 return <section className="panel op-form">
  <div className="panel-title"><div><h2>Run an operation</h2><p>Every operation the host advertises, drawn from the contract it publishes; validated here against the same bounds, sent as one JSON request under a mandate.</p></div></div>
  <form onSubmit={submit} noValidate>
   <div className="op-grid">
    <label>Host<Select value={hostId} onChange={id=>{H(id);Op('');R(null);}} options={hosts.map(h=>({value:h.id,label:h.name,hint:h.allowActions?'actions allowed':'read only'}))} label="Host"/></label>
    <label>Operation<Select value={operation} onChange={pickOp} options={ops} searchable placeholder={ops.length?'Choose…':'This host publishes no schema (older runtime)'} searchPlaceholder="Search an operation…" label="Operation" disabled={!ops.length}/></label>
   </div>
   {schema&&<>
    <p className="op-desc"><span className={'badge '+(schema.kind==='read'?'muted':schema.kind==='tool'?'muted':'green')}>{schema.kind}</span>{schema.gate!=='none'&&<span className="badge muted">gate: {schema.gate}</span>} {schema.description}</p>
    {schema.kind==='tool'&&<p className="op-tool">This is one step of a chain across hosts. It is driven by a tool from this workstation (tools/move-universe.py, tools/ha-standby.py), not sent alone from here.</p>}
    {schema.fields===null&&<p className="op-tool">Its fields are not described by the host yet.</p>}
    {schema.kind!=='tool'&&<>
     {(NEEDS_UUID(schema.kind)||schema.fields?.some(f=>f.name==='universe_uuid'))&&<label>Universe{universeOptions.length?<Select value={universe} onChange={U} options={universeOptions} searchable placeholder="Choose a universe…" searchPlaceholder="Search a universe…" label="Universe"/>:<input value={universe} onChange={e=>U(e.target.value)} placeholder="universe UUID" spellCheck={false}/>}</label>}
     {(schema.fields||[]).map(f=><label key={f.name}>{f.name}{f.required?' *':''}<small>{f.description}{f.min!==undefined?` · min ${f.min}`:''}{f.max!==undefined?` · max ${f.max}`:''}</small>
      {f.type==='enum'?<Select value={fields[f.name]} onChange={v=>Fd({...fields,[f.name]:v})} options={f.values.map(v=>({value:v,label:v}))} placeholder="Choose…" label={f.name}/>
      :f.type==='boolean'?<span className="check"><input type="checkbox" checked={!!fields[f.name]} onChange={e=>Fd({...fields,[f.name]:e.target.checked})} aria-label={f.name}/> yes</span>
      :['string[]','uuid[]','object[]','object'].includes(f.type)?<textarea value={fields[f.name]} onChange={e=>Fd({...fields,[f.name]:e.target.value})} placeholder={f.type==='object'?'{ }':'[ ]'} rows={3} spellCheck={false} aria-label={f.name}/>
      :<input value={fields[f.name]} onChange={e=>Fd({...fields,[f.name]:e.target.value})} inputMode={['integer','number'].includes(f.type)?'decimal':'text'} spellCheck={false} aria-label={f.name}/>}</label>)}
     <label>Authorization reference *<input value={auth} onChange={e=>Au(e.target.value)} placeholder="Approved task or mandate" spellCheck={false}/></label>
     {error&&<p className="op-error" role="alert">{error}</p>}
     <button className="button primary" type="submit" disabled={busy||!session||(schema.kind!=='read'&&!host?.allowActions)}>{busy?'Sending…':schema.kind==='read'?'Read':'Send '+operation.replaceAll('_',' ')}</button>
     {schema.kind!=='read'&&!host?.allowActions&&<p className="op-tool">Actions are disabled for this host in the console's configuration; reads stay available.</p>}
    </>}
   </>}
  </form>
  {result&&<div className="result"><strong>{result.body?.ok?'The host answered ok':result.status===401?'Session renewed — send again':'The host refused'}</strong>{result.body?.error&&<p className="op-error">{result.body.error}</p>}<pre>{JSON.stringify(result.body,null,2)}</pre><details><summary>The request that was sent</summary><pre>{JSON.stringify(result.request,null,2)}</pre></details></div>}
 </section>;
}
