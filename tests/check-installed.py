#!/usr/bin/env python3
"""Verify an installed standalone PodMesh service on a disposable lab host."""
import json, socket, subprocess

def run(*args):
    return subprocess.check_output(args, text=True)
def api(operation):
    return json.loads(run('/usr/bin/podmesh', operation))
first = api('identity')['data']['host_uuid']
old = api('observations')['data']['observations']
subprocess.run(['systemctl', 'restart', 'podmesh'], check=True)
# systemctl returns before the Unix socket is necessarily ready.
import time
for attempt in range(50):
    try:
        current = api('identity')
        break
    except subprocess.CalledProcessError:
        time.sleep(.1)
else:
    raise RuntimeError('Service did not become ready')
assert current['data']['host_uuid'] == first
new = api('observations')['data']['observations']
assert {x['id'] for x in old[:5]} <= {x['id'] for x in new}
expected = json.loads(run('/usr/bin/podman', 'ps', '--all', '--format', 'json'))
actual = api('inventory')['data']['containers']
assert {x['Id'] for x in expected} == {x['Id'] for x in actual}
failed = subprocess.run(['/usr/bin/podmesh', 'not-an-operation'], capture_output=True, text=True)
assert failed.returncode != 0 and json.loads(failed.stdout)['ok'] is False
for payload in (b'not-json\n', b'x'*4097):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(5)
        s.connect('/run/podmesh/api.sock')
        s.sendall(payload)
        assert json.loads(s.makefile('rb').readline(8192))['ok'] is False
assert api('identity')['ok'] is True
print(json.dumps({'status':'PASS','host_uuid':first,'checks':['identity persistence','journal persistence','independent inventory comparison','unsupported operation','invalid JSON','oversized request','recovery after errors']}))
