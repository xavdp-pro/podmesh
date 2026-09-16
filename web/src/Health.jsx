import React,{useEffect,useState} from 'react';import {Cpu,MemoryStick,HardDrive,Activity,RefreshCw,AlertTriangle,CheckCircle2,CircleDashed} from 'lucide-react';
import {replicationSummary,linkState,ratio,bytes,age,STORE_WARN_BYTES} from './healthModel.mjs';
const short=s=>s?String(s).slice(0,8):'—';
function Bar({value,label,tone}){const pct=value==null?0:Math.round(value*100);return <div className="hbar" aria-label={label}><span className={'hbar-fill '+(tone||(pct>=90?'bad':pct>=70?'warn':'ok'))} style={{width:pct+'%'}}/></div>;}
const badge={healthy:['green','healthy'],degraded:['amber','degraded'],failing:['red','failing'],absent:['muted','no manager']};
export default function Health({session,renewSession}){
 const[data,D]=useState(null),[error,E]=useState(''),[busy,B]=useState(false);
 async function load(){B(true);try{const r=await fetch('/api/health',{headers:{'X-Podmesh-Token':session.token}});if(r.status===401){await renewSession();E('The console session was renewed; refreshing.');return;}if(!r.ok)throw Error((await r.json().catch(()=>({}))).error||'Health unavailable');D(await r.json());E('');}catch(e){E(e.message);}finally{B(false);}}
 useEffect(()=>{if(!session)return;load();const t=setInterval(load,15000);return()=>clearInterval(t);},[session?.token]);
 if(!session)return <section className="panel"><div className="empty">Waiting for the local session.</div></section>;
 if(!data)return <section className="panel"><div className="empty">{error||'Reading every host…'}</div></section>;
 const hosts=data.hosts,rep=replicationSummary(hosts),[tone,word]=badge[rep.state];
 return <>
  <section className="panel health-summary">
   <div className="panel-title"><div><h2>Manager replication <span className={'badge '+tone}>{word}</span></h2>
    <p>{rep.replicas} replicas · {rep.links} links: {rep.healthy} healthy, {rep.degraded} degraded, {rep.failing} failing. Read from each resident, every 15 s.</p></div>
    <button className="button" type="button" onClick={load} disabled={busy}><RefreshCw size={15}/>{busy?'Reading…':'Refresh'}</button></div>
   {rep.storeWarning&&<p className="health-warn"><AlertTriangle size={16}/> The largest manager store is {bytes(rep.largestStoreBytes)}. Its exchange audit table is never compacted and each audit write re-verifies all of it; past {bytes(STORE_WARN_BYTES)} exchanges near the 2 s network deadline and links start failing although facts still arrive. The fix belongs to the resident (reported to Codex).</p>}
   {error&&<p className="op-error" role="alert">{error}</p>}
  </section>
  <section className="health-hosts">
   {hosts.map(h=>{const hs=h.host;const mem=hs?ratio(hs.memory_total_bytes-hs.memory_available_bytes,hs.memory_total_bytes):null;const disk=hs?ratio(hs.storage.used_bytes,hs.storage.size_bytes):null;const load=hs?ratio(hs.load_average['1m'],hs.cpu_count):null;
    return <article className="panel health-host" key={h.id}><h3>{h.name}</h3>
     {Object.entries(h.errors||{}).map(([k,v])=><p className="op-error" key={k}>{k}: {v}</p>)}
     {hs&&<dl>
      <dt><Cpu size={15}/> CPU</dt><dd><Bar value={load} label="load per core"/><small>{hs.cpu_count} cores · load {hs.load_average['1m']?.toFixed(2)}</small></dd>
      <dt><MemoryStick size={15}/> Memory</dt><dd><Bar value={mem} label="memory used"/><small>{bytes(hs.memory_total_bytes-hs.memory_available_bytes)} of {bytes(hs.memory_total_bytes)}</small></dd>
      <dt><HardDrive size={15}/> Disk</dt><dd><Bar value={disk} label="disk used"/><small>{bytes(hs.storage.used_bytes)} of {bytes(hs.storage.size_bytes)} · {hs.storage.backend}{hs.storage.dedicated?' dedicated':' shared with the system'} · growth {hs.storage.growth}</small></dd>
     </dl>}
    </article>;})}
  </section>
  <section className="panel">
   <div className="panel-title"><div><h2>Universes</h2><p>CPU as a share of one core over a 500 ms sample, memory and its limit from the cgroup, disk written from Podman.</p></div></div>
   <div className="table-wrap"><table><thead><tr><th>Universe</th><th>Host</th><th>State</th><th>CPU</th><th>Memory</th><th>Disk written</th></tr></thead><tbody>
    {hosts.flatMap(h=>(h.universes||[]).map(u=><tr key={h.id+u.universe_uuid}>
     <td><code>{short(u.universe_uuid)}</code>{u.manager&&<small className="tag">manager</small>}</td><td>{h.name}</td>
     <td><span className={'badge '+(u.state==='running'?'green':u.state==='paused'?'amber':'muted')}>{u.state}</span></td>
     <td>{u.cpu_percent_of_one_core==null?'—':<><Bar value={ratio(u.cpu_percent_of_one_core,100*(u.cpus_allowed||h.host?.cpu_count||1))} label="cpu"/><small>{u.cpu_percent_of_one_core} %{u.cpus_allowed?` of ${u.cpus_allowed} core`:''}</small></>}</td>
     <td>{u.memory_current_bytes==null?'—':<><Bar value={ratio(u.memory_current_bytes,u.memory_max_bytes||h.host?.memory_total_bytes)} label="memory"/><small>{bytes(u.memory_current_bytes)}{u.memory_max_bytes?` of ${bytes(u.memory_max_bytes)}`:' (no limit)'}</small></>}</td>
     <td>{bytes(u.disk_written_bytes)}</td></tr>))}
   </tbody></table></div>
  </section>
  <section className="panel">
   <div className="panel-title"><div><h2>Replication links</h2><p>Each replica's outgoing exchange to each peer, as its resident reports it.</p></div></div>
   <div className="table-wrap"><table><thead><tr><th>From</th><th>To</th><th>State</th><th>Last success</th><th>Failures</th><th>Facts acknowledged</th><th>Store</th></tr></thead><tbody>
    {hosts.flatMap(h=>(h.managers||[]).flatMap(m=>m.error?[<tr key={h.id+m.universe_uuid}><td>{h.name}</td><td colSpan={6} className="op-error">{m.error}</td></tr>]:m.links.map(l=>{const s=linkState(l);return <tr key={h.id+l.peer}>
     <td>{h.name} <code>{short(m.replica_id)}</code></td><td><code>{short(l.peer)}</code></td>
     <td><span className={'badge '+badge[s][0]}>{s==='healthy'?<CheckCircle2 size={13}/>:s==='degraded'?<CircleDashed size={13}/>:<AlertTriangle size={13}/>} {badge[s][1]}</span><small className="outcome">{l.outcome}</small></td>
     <td>{age(l.last_success_age_ms)}</td><td>{l.failures} <small>/ {l.successes} ok</small></td><td>{l.acknowledged_history_len??'—'}</td>
     <td className={m.store_bytes>=STORE_WARN_BYTES?'warn-text':''}>{bytes(m.store_bytes)}</td></tr>;})))}
   </tbody></table></div>
  </section>
 </>;
}
