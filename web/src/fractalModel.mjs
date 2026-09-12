const MAX_RECORDS=5000;
const DEFAULT_MAX_METRIC_AGE_SECONDS=60;
const CLOCK_SKEW_SECONDS=5;
const text=value=>typeof value==='string'&&value.trim()?value.trim():null;
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

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
 const contributors=samples.length,partial=contributors!==unique.length||reasons.length>0;
 return {universes:unique.length,states,hosts,metrics:{
  memory_used_bytes:contributors?memory:null,
  storage_used_bytes:contributors?storage:null,
  cpu_cores:contributors&&compatibleCpu?cpu:null,
  contributors,expected:unique.length,partial,reasons,
  max_sample_age_seconds:contributors?Math.max(...samples.map(sample=>sample.age)):null,
  sample_window_seconds:contributors&&compatibleCpu?samples[0].window:null,
 },partial:issues.length>0||partial};
}

function emptyModel({relationshipStatus='available',inputError=null,unassigned=[],invalidObserved=[],issues=[],aggregation}){
 return {relationshipStatus,inputError,fractals:[],unassigned,invalidObserved,issues,aggregation};
}

export function buildFractalModel({universes=[],relationships,relationshipsAvailable=Array.isArray(relationships),nowSeconds=Math.floor(Date.now()/1000),maxMetricAgeSeconds=DEFAULT_MAX_METRIC_AGE_SECONDS}={}){
 const aggregation={nowSeconds,maxMetricAgeSeconds};
 const tooMany=universes.length>MAX_RECORDS||Array.isArray(relationships)&&relationships.length>MAX_RECORDS;
 if(tooMany)return emptyModel({relationshipStatus:'error',inputError:`Fractal input exceeds the ${MAX_RECORDS}-record safety limit. Narrow the input before retrying.`,aggregation});
 const observed=new Map(),invalidObserved=[];
 for(const raw of universes){const parsed=observedUniverse(raw);if(parsed.invalid)invalidObserved.push(parsed.invalid);else observed.set(parsed.value.uuid,parsed.value);}
 if(!relationshipsAvailable)return emptyModel({relationshipStatus:'unavailable',unassigned:[...observed.values()],invalidObserved,issues:['Manager relationship API is not connected'],aggregation});
 const groups=new Map(),assigned=new Set(),globalIssues=[];
 for(const raw of relationships||[]){
  const fractalUuid=text(raw.fractal_uuid),universeUuid=text(raw.universe_uuid),parentUuid=raw.parent_uuid===null?null:text(raw.parent_uuid);
  if(!fractalUuid||!uuid.test(fractalUuid)||!universeUuid||!uuid.test(universeUuid)||raw.parent_uuid!==null&&(!parentUuid||!uuid.test(parentUuid))||!text(raw.provenance)||!Number.isInteger(raw.revision)||!Number.isFinite(raw.observed_at)){globalIssues.push('Malformed or unattested relationship record was ignored');continue;}
  assigned.add(universeUuid);
  if(!groups.has(fractalUuid))groups.set(fractalUuid,[]);
  groups.get(fractalUuid).push({...raw,fractalUuid,uuid:universeUuid,parentUuid,name:text(raw.name),role:text(raw.role),provenance:text(raw.provenance),observedAt:raw.observed_at});
 }
 const fractals=[];
 for(const [fractalUuid,records] of groups){
  const issues=[],byUuid=new Map(),conflicts=new Set();
  for(const record of records){
   const previous=byUuid.get(record.uuid);
   if(previous&&(previous.parentUuid!==record.parentUuid||previous.name!==record.name||previous.role!==record.role||previous.revision!==record.revision||previous.provenance!==record.provenance||previous.observedAt!==record.observedAt)){conflicts.add(record.uuid);issues.push(`Conflicting relationship records for ${record.uuid}`);continue;}
   if(!previous)byUuid.set(record.uuid,record);
  }
  for(const conflict of conflicts)byUuid.delete(conflict);
  const nodes=new Map();
  for(const record of byUuid.values()){const observation=observed.get(record.uuid);nodes.set(record.uuid,{uuid:record.uuid,parentUuid:record.parentUuid,name:record.name||observation?.name||record.uuid,role:record.role||observation?.role,hostId:observation?.hostId,hostName:observation?.hostName,state:observation?.state,metrics:observation?.metrics,raw:observation?.raw,provenance:record.provenance,observedAt:record.observedAt,children:[],cycle:false});}
  const roots=[],orphans=[];
  for(const node of nodes.values()){if(node.parentUuid===null)roots.push(node);else if(nodes.has(node.parentUuid))nodes.get(node.parentUuid).children.push(node);else{orphans.push(node);issues.push(`Missing parent ${node.parentUuid} for ${node.uuid}`);}}
  const color=new Map(),stack=[],cycleIds=new Set();
  function visit(node){
   color.set(node.uuid,1);stack.push(node);
   for(const child of node.children){const state=color.get(child.uuid)||0;if(state===0)visit(child);else if(state===1){const start=stack.findIndex(item=>item.uuid===child.uuid);for(const member of stack.slice(start))cycleIds.add(member.uuid);}}
   stack.pop();color.set(node.uuid,2);
  }
  for(const node of nodes.values())if(!color.has(node.uuid))visit(node);
  for(const id of cycleIds)nodes.get(id).cycle=true;
  const cycleNodes=[...nodes.values()].filter(node=>node.cycle);
  if(cycleNodes.length)issues.push(`Cycle detected: ${cycleNodes.map(node=>node.uuid).join(', ')}`);
  if(roots.length!==1)issues.push(`Expected one explicit root, observed ${roots.length}`);
  const nodeList=[...nodes.values()];
  fractals.push({uuid:fractalUuid,name:text(records[0]?.fractal_name)||fractalUuid,roots,orphans,cycleNodes,nodes:nodeList,conflicts:[...conflicts],issues,stats:aggregate(nodeList,issues,aggregation)});
 }
 return {relationshipStatus:'available',inputError:null,fractals,unassigned:[...observed.values()].filter(row=>!assigned.has(row.uuid)),invalidObserved,issues:globalIssues,aggregation};
}

function match(node,filters){const query=(filters.query||'').trim().toLowerCase();return(!query||`${node.name} ${node.uuid}`.toLowerCase().includes(query))&&(!filters.role||node.role===filters.role)&&(!filters.host||node.hostId===filters.host||node.hostName===filters.host)&&(!filters.state||node.state===filters.state);}

export function filterFractalModel(model,filters={}){
 const active=Boolean(filters.fractal||filters.query?.trim()||filters.role||filters.host||filters.state);
 const fractals=model.fractals.filter(fractal=>!filters.fractal||fractal.uuid===filters.fractal).map(fractal=>{
  const byUuid=new Map(fractal.nodes.map(node=>[node.uuid,node])),matchedNodes=fractal.nodes.filter(node=>match(node,filters)),matches=new Set(matchedNodes.map(node=>node.uuid)),visible=new Set(matches);
  for(const id of matches){let node=byUuid.get(id);const seen=new Set();while(node?.parentUuid&&!seen.has(node.parentUuid)){seen.add(node.parentUuid);visible.add(node.parentUuid);node=byUuid.get(node.parentUuid);}}
  const flat=[],walked=new Set();
  function walk(node,depth){if(walked.has(node.uuid)||!visible.has(node.uuid))return;walked.add(node.uuid);flat.push({...node,depth,contextOnly:!matches.has(node.uuid),children:undefined});for(const child of node.children)walk(child,depth+1);}
  for(const root of [...fractal.roots,...fractal.orphans,...fractal.cycleNodes])walk(root,0);
  const stats=aggregate(matchedNodes,fractal.issues,model.aggregation);
  return {...fractal,visibleNodes:flat,stats,statsScope:active?'visible matches':'whole fractal'};
 });
 return {...model,fractals,unassigned:model.unassigned.filter(node=>match(node,filters))};
}
