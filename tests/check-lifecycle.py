#!/usr/bin/env python3
import json, socket, subprocess, uuid, os
endpoint=os.environ.get('PODMESH_SOCKET','/run/podmesh/api.sock')
def api(r):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(30); s.connect(endpoint)
        s.sendall(json.dumps(r).encode()+b'\n')
        return json.loads(s.makefile('rb').readline())
def inventory():
    return json.loads(subprocess.check_output(['podman','ps','-a','--format','json']))
before={c['Id'] for c in inventory()}
u=str(uuid.uuid4()); name='podmesh-'+u
images=json.loads(subprocess.check_output(['podman','images','--format','json']))
image=next(i['Id'] for i in images if any('alpine' in n for n in i.get('Names') or []))
r={'operation':'create','operation_id':str(uuid.uuid4()),'universe_uuid':u,'authorization_ref':'disposable-lab-acceptance','image':'sha256:'+image,'command':['sleep','300'],'network_profile':'isolated'}
a=api(r); assert a['ok'], a
b=api(r); assert b['ok'] and b['data']['replayed'],b
changed=dict(r,command=['false']); assert not api(changed)['ok']
after={c['Id'] for c in inventory()}; assert len(after-before)==1 and before<=after
subprocess.run(['podman','start',name],check=True,stdout=subprocess.DEVNULL)
d={'operation':'delete','operation_id':str(uuid.uuid4()),'universe_uuid':u,'authorization_ref':'disposable-lab-acceptance'}
assert not api(d)['ok'], 'Running container must be protected'
subprocess.run(['podman','stop','--time','1',name],check=True,stdout=subprocess.DEVNULL)
assert api(d)['ok']; assert api(d)['ok']
assert {c['Id'] for c in inventory()}==before
print(json.dumps({'status':'PASS','checks':['create','idempotent retry','conflicting operation ID rejected','running deletion rejected','stopped deletion','unrelated containers preserved'],'universe_uuid':u}))
