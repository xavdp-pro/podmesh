import React,{useMemo,useState} from 'react';
import {AlertTriangle,GitFork,Search} from 'lucide-react';
import {buildFractalModel,filterFractalModel} from './fractalModel.mjs';
import {formatBytes} from './hostMetricsModel.mjs';

const all=(rows,key)=>[...new Set(rows.map(row=>row[key]).filter(Boolean))].sort();
const short=id=>id?.slice(0,8)||'unknown';
function Chips({label,values,value,onChange}){return <fieldset className="filter-group"><legend>{label}</legend><div className="chips"><button type="button" aria-pressed={!value} className={!value?'selected':''} onClick={()=>onChange('')}>All</button>{values.map(item=>{const option=typeof item==='string'?{value:item,label:item}:item;return <button type="button" key={option.value} aria-pressed={value===option.value} className={value===option.value?'selected':''} onClick={()=>onChange(option.value)}>{option.label}</button>})}</div></fieldset>}

export default function Fractals({containers,relationships,relationshipsAvailable=false,onSelect}){
 const [filters,setFilters]=useState({fractal:'',query:'',role:'',host:'',state:''});
 const universes=useMemo(()=>containers.filter(row=>row.uuid).map(row=>({uuid:row.uuid,name:row.name,hostId:row.host.id,hostName:row.host.name,state:row.State,raw:row})),[containers]);
 // Relationship records will come from the manager API. Physical Podman nesting is deliberately absent.
 const base=useMemo(()=>buildFractalModel({universes,relationships,relationshipsAvailable}),[universes,relationships,relationshipsAvailable]);
 const model=useMemo(()=>filterFractalModel(base,filters),[base,filters]);
 const related=base.fractals.flatMap(fractal=>fractal.nodes),filterSources=[...universes,...related];
 const set=(key,value)=>setFilters(current=>({...current,[key]:value}));
 return <section className="fractal-view">
  {base.inputError&&<div className="relationship-state" role="alert"><AlertTriangle size={21}/><div><strong>Fractal input was not processed</strong><p>{base.inputError}</p></div></div>}
  {base.relationshipStatus==='unavailable'&&<div className="relationship-state"><AlertTriangle size={21}/><div><strong>Logical relationships unavailable</strong><p>The manager relationship API is not connected. Podman nesting and host placement are never used to invent parentage.</p></div></div>}
  {base.relationshipStatus==='available'&&base.issues.map(issue=><div className="relationship-state" key={issue}><AlertTriangle size={21}/><div><strong>Relationship input is partial</strong><p>{issue}</p></div></div>)}
  <div className="fractal-filters">
   <label className="search"><Search size={16}/><input aria-label="Filter by universe name or UUID" placeholder="Filter by name or UUID…" value={filters.query} onChange={event=>set('query',event.target.value)}/></label>
   <Chips label="Fractal" values={base.fractals.map(fractal=>({value:fractal.uuid,label:`${fractal.name} · ${short(fractal.uuid)}`}))} value={filters.fractal} onChange={value=>set('fractal',value)}/>
   <Chips label="Role" values={all(filterSources,'role')} value={filters.role} onChange={value=>set('role',value)}/>
   <Chips label="Host" values={all(filterSources,'hostId')} value={filters.host} onChange={value=>set('host',value)}/>
   <Chips label="State" values={all(filterSources,'state')} value={filters.state} onChange={value=>set('state',value)}/>
  </div>
  {model.fractals.map(fractal=><article className="fractal-card" key={fractal.uuid}>
   <header><div><span className="tile"><GitFork size={18}/></span><h2>{fractal.name}</h2><code>{fractal.uuid}</code></div><span className={'badge '+(fractal.stats.partial?'amber':'green')}>{fractal.stats.partial?'Partial statistics':'Complete observations'}</span></header>
   <p className="stats-scope">Statistics for {fractal.statsScope}. Context-only ancestors are excluded.</p>
   <div className="fractal-stats"><span><b>{fractal.stats.universes}</b> universes</span><span><b>{fractal.stats.metrics.contributors}/{fractal.stats.metrics.expected}</b> metric coverage</span><span><b>{formatBytes(fractal.stats.metrics.memory_used_bytes)}</b> RAM use</span><span><b>{formatBytes(fractal.stats.metrics.storage_used_bytes)}</b> storage use</span><span><b>{fractal.stats.metrics.max_sample_age_seconds===null?'Unknown':`${Math.round(fractal.stats.metrics.max_sample_age_seconds)} s`}</b> oldest included sample</span><span><b>{fractal.stats.metrics.sample_window_seconds===null?'Mixed / unknown':`${fractal.stats.metrics.sample_window_seconds} s`}</b> metric window</span></div>
   {fractal.issues.map(issue=><p className="fractal-issue" key={issue}>{issue}</p>)}
   <div className="fractal-tree" role="tree" aria-label={`${fractal.name} logical relationship tree`}>{fractal.visibleNodes.map(node=>{const unavailable=!node.raw;const qualifiers=[node.contextOnly&&'context only',node.cycle&&'cycle member',unavailable&&'not currently observed'].filter(Boolean).join(', ');return <button key={node.uuid} role="treeitem" aria-level={node.depth+1} aria-label={`${node.name}${qualifiers?`, ${qualifiers}`:''}`} disabled={unavailable} title={unavailable?'Relationship exists, but no current host observation can be opened.':undefined} className={[node.contextOnly?'context-node':'',node.cycle?'cycle-node':''].filter(Boolean).join(' ')} style={{'--depth':node.depth}} onClick={()=>node.raw&&onSelect?.(node.raw)}><GitFork size={15}/><span><strong>{node.name}</strong><small>{node.role||'Role unknown'} · {node.hostName||'Host unknown'} · {node.state||'State unknown'}</small><small className="node-qualifiers">{node.contextOnly&&'Context only · '}{node.cycle&&'Cycle member · '}{unavailable&&'Not currently observed'}</small></span><code>{short(node.uuid)}</code></button>})}</div>
  </article>)}
  {!!base.invalidObserved.length&&<section className="panel invalid-observed" aria-labelledby="invalid-observed-title"><div className="panel-title"><div><h2 id="invalid-observed-title">Invalid observed records</h2><p>These inventory records are visible but cannot join a logical tree until their universe UUID is corrected.</p></div><span className="badge amber">{base.invalidObserved.length}</span></div>{base.invalidObserved.map((node,index)=><div className="invalid-record" key={`${node.uuid}-${index}`}><span><strong>{node.name}</strong><small>{node.uuid}</small></span><span>{node.hostName||'Host unknown'}</span><span>{node.reason}</span></div>)}</section>}
  <section className="panel unassigned"><div className="panel-title"><div><h2>Unassigned observed universes</h2><p>Observed through host inventories, with no manager relationship asserted.</p></div><span className="badge muted">{model.unassigned.length} shown</span></div>{model.unassigned.map(node=><button key={node.uuid} onClick={()=>node.raw&&onSelect?.(node.raw)}><span><strong>{node.name}</strong><small>{node.uuid}</small></span><span>{node.hostName||'Host unknown'}</span><span className={'badge '+(node.state==='running'?'green':'muted')}>{node.state||'unknown'}</span></button>)}{!model.unassigned.length&&<div className="empty">No matching unassigned universes.</div>}</section>
 </section>;
}
