import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
const MAX=8*1024*1024;
const remote=`import socket,sys
b=sys.stdin.buffer.readline(4097)
if len(b)>4096 or not b.endswith(b'\\n'): raise ValueError('Invalid request')
s=socket.socket(socket.AF_UNIX);s.settimeout(390);s.connect('/run/podmesh/api.sock');s.sendall(b)
n=0
while True:
 b=s.recv(65536)
 if not b: break
 n+=len(b)
 if n>8388608: raise ValueError('Response too large')
 sys.stdout.buffer.write(b)
`;
const quote=s=>"'"+s.replaceAll("'","'\\''")+"'";
export function request(host,payload,{timeout=395000}={}) {
 const body=JSON.stringify(payload)+'\n';if(Buffer.byteLength(body)>4096)return Promise.reject(new Error('Request exceeds PodMesh limit'));
 return new Promise((resolve,reject)=>{
  let output=[],done=false,size=0;let process;
  const finish=(err)=>{if(done)return;done=true;clearTimeout(timer);process?.kill();socket?.destroy();if(err)reject(err);else{try{resolve(JSON.parse(Buffer.concat(output).toString('utf8')));}catch{reject(new Error('Invalid PodMesh response'));}}};
  const read=b=>{size+=b.length;if(size>MAX)return finish(new Error('Response too large'));output.push(b);};
  const timer=setTimeout(()=>finish(new Error('Transport timed out; outcome may be unknown. Reconcile before retry.')),timeout);
  let socket;
  if(host.ssh){process=spawn('ssh',['-o','BatchMode=yes','-o','ConnectTimeout=8','-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+(host.knownHostsFile||path.join(os.homedir(),'.ssh/known_hosts')),host.ssh,'sudo -n python3 -c '+quote(remote)],{stdio:['pipe','pipe','pipe']});process.stdout.on('data',read);process.stderr.resume();process.on('error',finish);process.on('close',code=>finish(code?new Error('SSH transport failed; verify host connection and permissions'):null));process.stdin.on('error',finish);process.stdin.end(body);}
  else{socket=net.createConnection(host.socket||'/run/podmesh/api.sock',()=>socket.end(body));socket.on('data',read);socket.on('error',finish);socket.on('end',()=>finish());}
 });
}
