const MAX_RECORDS=5000;
const MAX_RENDERED_ISSUES=50;
const DEFAULT_MAX_METRIC_AGE_SECONDS=60;
const CLOCK_SKEW_SECONDS=5;
const text=(value,max=256)=>typeof value==='string'&&value.trim()&&value.trim().length<=max?value.trim():null;
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

function summarizeIssues(issues){
 const counts=new Map();for(const issue of issues)counts.set(issue,(counts.get(issue)||0)+1);
 const entries=[...counts];const rendered=entries.slice(0,MAX_RENDERED_ISSUES).map(([issue,count])=>count>1?`${issue} (${count} records)`:issue);
 if(entries.length>MAX_RENDERED_ISSUES){const omitted=entries.slice(MAX_RENDERED_ISSUES).reduce((total,[,count])=>total+count,0);rendered.push(`${omitted} additional relationship records have ${entries.length-MAX_RENDERED_ISSUES} distinct issues`);}
 return rendered;
}

export const isUniverseUuid=value=>Boolean(text(value)&&uuid.test(text(value)));

function observedUniverse(raw){
 const id=text(raw?.uuid);
 if(!id||!uuid.test(id))return {invalid:{uuid:id||'Missing UUID',name:text(raw?.name)||'Unnamed observed record',hostName:text(raw?.hostName),state:text(raw?.state),reason:'Invalid or missing universe UUID',raw}};
 return {value:{uuid:id,name:text(raw.name)||id,role:text(raw.role),hostId:text(raw.hostId),hostName:text(raw.hostName),state:text(raw.state),metrics:raw.metrics||null,raw}};
}

function aggregate(nodes,issues,{nowSeconds,maxMetricAgeSeconds}){
 const unique=[...new Map(nodes.map(node=>[node.uuid,node])).values()];
 const states={},hosts={};
 for(const node of unique){states[node.state||'unknown']=(states[node.state||'unknown']||0)+1;hosts[node.hostName||node.hostId||'unknown']=(hosts[node.hostName||node.hostId||'unknown']||0)+1;}
 const scopes=new Set(),samples=[];let memory=0,storage=0,cpu=0;const reasons=[];
 for(const node of unique){
  const metric=node.metrics,scope=text(metric?.accounting_scope_id);
  if(!metric||!scope){reasons.push(`missing metrics for ${node.uuid}`);continue;}
  if(scopes.has(scope)){reasons.push(`overlapping accounting scope ${scope}`);continue;}
  const values=[metric.memory_used_bytes,metric.storage_used_bytes,metric.cpu_cores],observedAt=metric.observed_at,window=metric.sample_window_seconds;
  if(!values.every(value=>Number.isFinite(value)&&value>=0)||!Number.isFinite(observedAt)||!Number.isFinite(window)||window<=0){reasons.push(`invalid or undated metrics for ${node.uuid}`);continue;}
  const age=nowSeconds-observedAt;
  if(age>maxMetricAgeSeconds||age < -CLOCK_SKEW_SECONDS){reasons.push(`${age < 0?'future-dated':'stale'} metrics for ${node.uuid}`);continue;}
  scopes.add(scope);samples.push({node,values,age:Math.max(0,age),window});memory+=values[0];storage+=values[1];
 }
 const windows=new Set(samples.map(sample=>sample.window));
 const compatibleCpu=windows.size<=1;
 if(!compatibleCpu)reasons.push('CPU sample windows are incompatible');
 if(compatibleCpu)for(const sample of samples)cpu+=sample.values[2];
 const contributors=samples.length,partial=contributors!==unique.length||reasons.length>0,renderedReasons=summarizeIssues(reasons);
 return {universes:unique.length,states,hosts,metrics:{
  memory_used_bytes:contributors?memory:null,
  storage_used_bytes:contributors?storage:null,
  cpu_cores:contributors&&compatibleCpu?cpu:null,
  contributors,expected:unique.length,partial,reasons:renderedReasons,
  max_sample_age_seconds:contributors?Math.max(...samples.map(sample=>sample.age)):null,
  sample_window_seconds:contributors&&compatibleCpu?samples[0].window:null,
 },partial:issues.length>0||partial};
}

function emptyModel({relationshipStatus='available',inputError=null,unassigned=[],invalidObserved=[],issues=[],aggregation}){
 return {relationshipStatus,inputError,fractals:[],unassigned,invalidObserved,issues,aggregation};
}

export function buildFractalModel({universes=[],relationships,relationshipsAvailable=Array.isArray(relationships),relationshipStatus,relationshipError,nowSeconds=Math.floor(Date.now()/1000),maxMetricAgeSeconds=DEFAULT_MAX_METRIC_AGE_SECONDS}={}){
 const aggregation={nowSeconds,maxMetricAgeSeconds};
 const tooMany=universes.length>MAX_RECORDS||Array.isArray(relationships)&&relationships.length>MAX_RECORDS;
 if(tooMany)return emptyModel({relationshipStatus:'error',inputError:`Fractal input exceeds the ${MAX_RECORDS}-record safety limit. Narrow the input before retrying.`,aggregation});
 const observed=new Map(),invalidObserved=[];
 for(const raw of universes){const parsed=observedUniverse(raw);if(parsed.invalid)invalidObserved.push(parsed.invalid);else observed.set(parsed.value.uuid,parsed.value);}
 const sourceStatus=relationshipStatus||(!relationshipsAvailable?'unavailable':'available');
 if(sourceStatus!=='available')return emptyModel({relationshipStatus:sourceStatus,inputError:sourceStatus==='malformed'?relationshipError||'Manager relationship input was rejected.':null,unassigned:[...observed.values()],invalidObserved,issues:[relationshipError||'Manager relationship API is not connected'],aggregation});
 const groups=new Map(),assigned=new Set(),globalIssues=[];
 for(const raw of relationships||[]){
  if(!raw||typeof raw!=='object'||Array.isArray(raw)){globalIssues.push('Malformed or unattested relationship record was ignored');continue;}
  const fractalUuid=text(raw.fractal_uuid,64),universeUuid=text(raw.universe_uuid,64),parentUuid=raw.parent_uuid===null?null:text(raw.parent_uuid,64);
  if(!fractalUuid||!uuid.test(fractalUuid)||!universeUuid||!uuid.test(universeUuid)||raw.parent_uuid!==null&&(!parentUuid||!uuid.test(parentUuid))||!text(raw.provenance,128)||!Number.isInteger(raw.revision)||!Number.isFinite(raw.observed_at)||raw.name!==undefined&&!text(raw.name,256)||raw.role!==undefined&&!text(raw.role,128)||raw.fractal_name!==undefined&&!text(raw.fractal_name,256)){globalIssues.push('Malformed or unattested relationship record was ignored');continue;}
  if(!groups.has(fractalUuid))groups.set(fractalUuid,[]);
  groups.get(fractalUuid).push({...raw,fractalUuid,uuid:universeUuid,parentUuid,name:text(raw.name),role:text(raw.role,128),provenance:text(raw.provenance,128),observedAt:raw.observed_at});
 }
 const membership=new Map();for(const [fractalUuid,records] of groups)for(const record of records){const rows=membership.get(record.uuid)||[];rows.push(fractalUuid);membership.set(record.uuid,rows);}
 const crossFractalConflicts=new Set([...membership].filter(([,fractalUuids])=>new Set(fractalUuids).size>1).map(([universeUuid])=>universeUuid));
 for(const universeUuid of crossFractalConflicts)globalIssues.push(`Conflicting fractal membership for ${universeUuid}`);
 const fractals=[];
 for(const [fractalUuid,records] of groups){
  const issues=[],byUuid=new Map(),conflicts=new Set();
  for(const record of records){
   if(crossFractalConflicts.has(record.uuid)){conflicts.add(record.uuid);issues.push(`Conflicting fractal membership for ${record.uuid}`);continue;}
   const previous=byUuid.get(record.uuid);
   if(previous&&(previous.parentUuid!==record.parentUuid||previous.name!==record.name||previous.role!==record.role||previous.revision!==record.revision||previous.provenance!==record.provenance||previous.observedAt!==record.observedAt)){conflicts.add(record.uuid);issues.push(`Conflicting relationship records for ${record.uuid}`);continue;}
   if(!previous)byUuid.set(record.uuid,record);
  }
  for(const conflict of conflicts)byUuid.delete(conflict);
  for(const universeUuid of byUuid.keys())assigned.add(universeUuid);
  const nodes=new Map();
  for(const record of byUuid.values()){const observation=observed.get(record.uuid);nodes.set(record.uuid,{uuid:record.uuid,parentUuid:record.parentUuid,name:record.name||observation?.name||record.uuid,role:record.role||observation?.role,hostId:observation?.hostId,hostName:observation?.hostName,state:observation?.state,metrics:observation?.metrics,raw:observation?.raw,provenance:record.provenance,observedAt:record.observedAt,children:[],cycle:false});}
  const roots=[],orphans=[];
  for(const node of nodes.values()){if(node.parentUuid===null)roots.push(node);else if(nodes.has(node.parentUuid))nodes.get(node.parentUuid).children.push(node);else{orphans.push(node);issues.push(`Missing parent ${node.parentUuid} for ${node.uuid}`);}}
  const color=new Map(),stack=[],cycleIds=new Set();
  for(const startNode of nodes.values())if(!color.has(startNode.uuid)){
   color.set(startNode.uuid,1);stack.push(startNode);const frames=[{node:startNode,index:0}];
   while(frames.length){const frame=frames.at(-1);if(frame.index<frame.node.children.length){const child=frame.node.children[frame.index++],state=color.get(child.uuid)||0;if(state===0){color.set(child.uuid,1);stack.push(child);frames.push({node:child,index:0});}else if(state===1){const start=stack.findIndex(item=>item.uuid===child.uuid);for(const member of stack.slice(start))cycleIds.add(member.uuid);}}else{color.set(frame.node.uuid,2);stack.pop();frames.pop();}}
  }
  for(const id of cycleIds)nodes.get(id).cycle=true;
  const cycleNodes=[...nodes.values()].filter(node=>node.cycle);
  if(cycleNodes.length)issues.push(`Cycle detected involving ${cycleNodes.length} universes`);
  if(roots.length!==1)issues.push(`Expected one explicit root, observed ${roots.length}`);
  const nodeList=[...nodes.values()];
  const renderedIssues=summarizeIssues(issues);
  fractals.push({uuid:fractalUuid,name:text(records[0]?.fractal_name)||fractalUuid,roots,orphans,cycleNodes,nodes:nodeList,conflicts:[...conflicts],issues:renderedIssues,stats:aggregate(nodeList,renderedIssues,aggregation)});
 }
 return {relationshipStatus:'available',inputError:null,fractals,unassigned:[...observed.values()].filter(row=>!assigned.has(row.uuid)),invalidObserved,issues:summarizeIssues(globalIssues),aggregation};
}

function match(node,filters){const query=(filters.query||'').trim().toLowerCase();return(!query||`${node.name} ${node.uuid}`.toLowerCase().includes(query))&&(!filters.role||node.role===filters.role)&&(!filters.host||node.hostId===filters.host||node.hostName===filters.host)&&(!filters.state||node.state===filters.state);}

export function filterFractalModel(model,filters={}){
 const active=Boolean(filters.fractal||filters.query?.trim()||filters.role||filters.host||filters.state);
 const fractals=model.fractals.filter(fractal=>!filters.fractal||fractal.uuid===filters.fractal).map(fractal=>{
  const byUuid=new Map(fractal.nodes.map(node=>[node.uuid,node])),matchedNodes=fractal.nodes.filter(node=>match(node,filters)),matches=new Set(matchedNodes.map(node=>node.uuid)),visible=active?new Set(matches):new Set(fractal.nodes.map(node=>node.uuid));
  if(active)for(const id of matches){let node=byUuid.get(id);const seen=new Set();while(node?.parentUuid&&!seen.has(node.parentUuid)){seen.add(node.parentUuid);visible.add(node.parentUuid);node=byUuid.get(node.parentUuid);}}
  const flat=[],walked=new Set(),pending=[...fractal.roots,...fractal.orphans,...fractal.cycleNodes].reverse().map(node=>({node,depth:0}));
  while(pending.length){const {node,depth}=pending.pop();if(walked.has(node.uuid)||!visible.has(node.uuid))continue;walked.add(node.uuid);flat.push({...node,depth,contextOnly:!matches.has(node.uuid),children:undefined});for(const child of [...node.children].reverse())pending.push({node:child,depth:depth+1});}
  const stats=aggregate(matchedNodes,fractal.issues,model.aggregation);
  return {...fractal,visibleNodes:flat,stats,statsScope:active?'visible matches':'whole fractal'};
 });
 return {...model,fractals,unassigned:model.unassigned.filter(node=>match(node,filters))};
}

export function filterFractalOptions(fractals=[],query=''){
 const needle=String(query).trim().toLowerCase();
 if(!needle)return fractals;
 return fractals.filter(fractal=>`${fractal.name||''} ${fractal.uuid||''}`.toLowerCase().includes(needle));
}

export function fractalNodePath(nodes=[],focusUuid){
 const byUuid=new Map(nodes.map(node=>[node.uuid,node])),path=[],seen=new Set();
 let node=byUuid.get(focusUuid);
 while(node&&!seen.has(node.uuid)){seen.add(node.uuid);path.push(node);node=byUuid.get(node.parentUuid);}
 return path.reverse();
}

export function projectFractalNodes(nodes=[],{focusUuid='',collapsedUuids=[]}={}){
 const focusIndex=focusUuid?nodes.findIndex(node=>node.uuid===focusUuid):-1;
 const start=focusIndex>=0?focusIndex:0,focusDepth=focusIndex>=0?nodes[focusIndex].depth:0,collapsed=new Set(collapsedUuids);
 const projected=[];let collapsedDepth=null;
 for(let index=start;index<nodes.length;index++){
  const node=nodes[index];
  if(focusIndex>=0&&index>start&&node.depth<=focusDepth)break;
  if(collapsedDepth!==null&&node.depth>collapsedDepth)continue;
  if(collapsedDepth!==null&&node.depth<=collapsedDepth)collapsedDepth=null;
  projected.push({...node,depth:node.depth-focusDepth});
  if(collapsed.has(node.uuid))collapsedDepth=node.depth;
 }
 return projected;
}
