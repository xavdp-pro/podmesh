import {useEffect,useState} from 'react';import {Copy,Play,Square,Save,RefreshCw} from 'lucide-react';import Select from './Select.jsx';
// A universe's replication to standby hosts, from the drawer: where it replicates, run it now, run it on a schedule, stop.
// Each run stops the universe for its capture; the page says so and shows how long the last one took.
const ago=s=>s==null?'—':s<60?`${s} s ago`:s<3600?`${Math.round(s/60)} min ago`:`${(s/3600).toFixed(1)} h ago`;
const INTERVALS=[[60,'every minute'],[300,'every 5 minutes'],[900,'every 15 minutes'],[3600,'every hour'],[21600,'every 6 hours'],[86400,'every day']];
export default function Replication({universe,host,session,renewSession}){
 const[status,S]=useState(null),[error,E]=useState(''),[busy,B]=useState(''),[target,T]=useState('all'),[interval,I]=useState(900),[auth,A]=useState('');
 async function load(){try{const r=await fetch(`/api/replication/${host.id}/${universe}`,{headers:{'X-Podmesh-Token':session.token}});if(r.status===401){await renewSession();return;}const d=await r.json();S(d);
  if(d.replication){T(d.replication.mode==='all'?'all':Number(d.replication.mode));I(d.replication.interval_seconds);}if(d.error)E(d.error);}catch(e){E(e.message);}}
 useEffect(()=>{load();},[universe,host.id,session?.token]);
 async function act(action){if(!auth.trim()){E('An authorization reference is required');return;}B(action);E('');
  try{const body={action,host:host.id,universe_uuid:universe,authorization_ref:auth,...(action==='configure'?{standbys:target,interval_seconds:interval}:{})};
   const r=await fetch('/api/replication',{method:'POST',headers:{'Content-Type':'application/json','X-Podmesh-Token':session.token},body:JSON.stringify(body)});
   if(r.status===401){await renewSession();E('The console session was renewed; do it again.');return;}const d=await r.json();if(d.result==='refused'||d.result==='unknown'||d.error)E(d.error||'Refused');await load();}
  catch(e){E(e.message);}finally{B('');}}
 if(!status)return <div className="replication"><h3>Replication</h3><p className="replication-note">{error||'Reading the replication…'}</p></div>;
 const rep=status.replication,candidates=status.candidates||[],armed=status.schedule?.armed,last=status.last_run;
 const targets=[{value:'all',label:`All other hosts (${candidates.length})`},...candidates.map((_,i)=>({value:i+1,label:`${i+1} host${i?'s':''}`,hint:'the ones with the most free memory'}))];
 return <div className="replication">
  <div className="replication-head"><h3>Replication</h3><span className={'badge '+(armed?'green':'muted')}>{armed?'scheduled':'not scheduled'}</span><button className="icon" type="button" aria-label="Refresh replication" onClick={load}><RefreshCw size={15}/></button></div>
  <p className="replication-note">Each run stops the universe for a capture, starts it again, and restores the capture into quarantine on every standby, ready for a takeover.{last?.stopped_for_seconds!=null&&` The last run stopped it for ${last.stopped_for_seconds} s.`}</p>
  <div className="op-grid">
   <label>Replicate to<Select value={target} onChange={T} options={targets} label="Replicate to"/></label>
   <label>Schedule<Select value={interval} onChange={I} options={INTERVALS.map(([v,l])=>({value:v,label:l}))} label="Schedule"/></label>
  </div>
  <label className="replication-auth">Authorization reference<input value={auth} onChange={e=>A(e.target.value)} placeholder="Approved task or mandate" spellCheck={false}/></label>
  <div className="actions">
   <button className="button" type="button" disabled={!!busy||!host.allowActions} onClick={()=>act('configure')}><Save size={15}/>{busy==='configure'?'Saving…':rep?'Save target':'Set up replication'}</button>
   <button className="button" type="button" disabled={!!busy||!rep||!host.allowActions} onClick={()=>act('run')}><Copy size={15}/>{busy==='run'?'Replicating…':'Replicate now'}</button>
   {armed?<button className="button" type="button" disabled={!!busy||!host.allowActions} onClick={()=>act('stop')}><Square size={15}/>{busy==='stop'?'Stopping…':'Stop schedule'}</button>
    :<button className="button primary" type="button" disabled={!!busy||!rep||!host.allowActions} onClick={()=>act('start')}><Play size={15}/>{busy==='start'?'Starting…':'Start schedule'}</button>}
  </div>
  {error&&<p className="op-error" role="alert">{error}</p>}
  {rep&&<p className="replication-note">Target: {rep.standbys.length} standby host{rep.standbys.length>1?'s':''} ({rep.chosen_because}).{last&&<> Last run {ago(Math.round(Date.now()/1000)-last.at)}: {last.ok?'ok':`refused — ${last.error}`}.</>}</p>}
  {status.standbys?.length>0&&<table><thead><tr><th>Standby</th><th>Copy</th><th>Age</th><th>On the host</th></tr></thead><tbody>
   {status.standbys.map(s=><tr key={s.host}><td>{candidates.find(c=>c.ssh===s.host)?.name||s.host}</td>
    <td>{s.error?<span className="op-error">{s.error}</span>:s.copy?`generation ${s.copy.generation}`:'none yet'}</td>
    <td>{s.copy?ago(s.copy.age_seconds):'—'}</td><td>{s.copy?(s.copy.present_on_host?<span className="badge green">present</span>:<span className="badge red">missing</span>):'—'}</td></tr>)}
  </tbody></table>}
 </div>;
}
