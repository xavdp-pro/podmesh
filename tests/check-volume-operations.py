#!/usr/bin/env python3
"""volume_declare and volume_grow: advertised in capabilities, and refused on a host whose Podman
storage cannot grow (the same rule as storage_status). Environment: PODMESH_SOURCE_SSH, the transient
service variables."""
import json, os, sys, tempfile, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

A = Host('host', os.environ['PODMESH_SOURCE_SSH'], tempfile.mkdtemp(prefix='podmesh-volume-'),
         os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock'), os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh'), os.environ.get('PODMESH_UNIT', 'podmesh.service'))
reference = 'disposable-lab-volume'
checks = []
caps = A.ok({'operation': 'capabilities', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference})
ops = set(caps['operations']) | set(caps.get('experimental_operations', []))
for name in ('volume_declare', 'volume_grow'):
    assert name in ops, f'{name} is not advertised'
    schema = caps['schemas'][name]
    assert schema['kind'] == 'universe' and schema['gate'] == 'reservation', (name, schema)
checks.append('volume_declare and volume_grow are advertised as universe mutations with reservation gate and schemas')

storage = A.ok({'operation': 'storage_status', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference})
growth = storage['growth']
assert growth in ('possible', 'refused'), growth

podman = lambda *a: A.call('podman_run', args=list(a))['stdout'].strip()
images = json.loads(podman('images', '--format', 'json'))
alpine = next(i['Id'] for i in images if any('alpine' in n for n in (i.get('Names') or [])))
u = str(uuid.uuid4())
try:
    A.ok(request('create', u, reference, image='sha256:' + alpine, network_profile='isolated', command=['sleep', '600']))
    A.ok(request('start', u, reference, observe_seconds=1))
    A.ok(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
    declare = request('volume_declare', u, reference, capacity_bytes=64 * 1024 * 1024)
    r = A.api(declare)
    if growth == 'refused':
        assert not r.get('ok'), r
        assert storage['reason'] in r.get('error', ''), (r, storage['reason'])
        checks.append('volume_declare refused on this host with the same reason storage_status gives for growth')
    else:
        assert r.get('ok'), r
        grow = request('volume_grow', u, reference, additional_bytes=32 * 1024 * 1024)
        g = A.ok(grow)
        assert g['action'] == 'grown', g
        again = A.ok({'operation': 'storage_status', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference})
        decls = again['universe_volumes']['declarations']
        assert any(row['universe_uuid'] == u for row in decls), decls
        checks.append('volume_declare and volume_grow succeeded on a growable host and storage_status lists the declaration')
    print(json.dumps({'result': 'PASS', 'growth': growth, 'checks': checks}, indent=2))
finally:
    A.api(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
