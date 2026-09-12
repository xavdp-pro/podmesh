export function formatBytes(value){
 if(!Number.isFinite(value)||value<0)return 'Unknown';
 const units=['B','KiB','MiB','GiB','TiB','PiB','EiB'];let amount=value,index=0;
 while(amount>=1024&&index<units.length-1){amount/=1024;index++;}
 return `${amount>=10||index===0?amount.toFixed(0):amount.toFixed(1)} ${units[index]}`;
}

export function formatCpuCores(value){
 if(!Number.isFinite(value)||value<0)return 'Unknown';
 return new Intl.NumberFormat('en',{maximumFractionDigits:2}).format(value);
}

export function meshMemory(hosts){
 const identities=new Set();let available=0,total=0,contributors=0;const reasons=[];
 for(const host of hosts){
  const identity=host.responses.identity?.data?.host_uuid,memory=host.responses.host_resource_metrics?.data?.memory;
  if(typeof identity!=='string'||!identity){reasons.push('missing host identity');continue;}
  if(identities.has(identity)){reasons.push('duplicate host identity');continue;}identities.add(identity);
  if(!memory?.known||![memory.available_bytes,memory.total_bytes].every(Number.isFinite)||memory.total_bytes<=0||memory.available_bytes<0||memory.available_bytes>memory.total_bytes){reasons.push(`missing or invalid sample for ${host.name||identity}`);continue;}
  contributors++;available+=memory.available_bytes;total+=memory.total_bytes;
 }
 const partial=contributors!==hosts.length||reasons.length>0;
 if(!contributors)return {value:'Unknown',help:'No valid unique-host memory observation · partial',partial,reasons};
 return {value:formatBytes(available),help:`Available across ${contributors} of ${hosts.length} hosts · ${formatBytes(total)} total${partial?' · partial':''}`,partial,reasons};
}
