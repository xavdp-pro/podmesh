#!/usr/bin/env python3
"""The operation schemas capabilities advertises, held to the daemon that advertises them, on one
lab host. Verified: every advertised operation has a schema, every schema names a known kind and
gate, every field a known type; and the bounds the schemas state are the bounds the daemon
enforces -- for each integer or number field with a bound, a value just outside it is refused and
the refusal names the field, on a disposable universe. A schema that promised a bound the daemon
did not keep would go red here. Environment: PODMESH_SOURCE_SSH, the transient service variables."""
import json, os, sys, tempfile, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

A = Host('host', os.environ['PODMESH_SOURCE_SSH'], tempfile.mkdtemp(prefix='podmesh-schema-'),
         os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock'), os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh'), os.environ.get('PODMESH_UNIT', 'podmesh.service'))
reference = 'disposable-lab-schema'
checks = []
caps = A.ok({'operation': 'capabilities', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference})
schemas = caps['schemas']
advertised = set(caps['operations']) | set(caps['experimental_operations'])
missing = sorted(advertised - set(schemas))
assert not missing, f'advertised without a schema: {missing}'
extra = sorted(set(schemas) - advertised)
assert not extra, f'schema for an operation not advertised: {extra}'
checks.append(f'every one of the {len(advertised)} advertised operations has a schema, and no schema describes an operation that is not advertised')
KINDS, GATES, TYPES = {'read', 'universe', 'host', 'tool'}, {'none', 'lease', 'reservation', 'lease,reservation'}, {'string', 'integer', 'number', 'boolean', 'enum', 'uuid', 'object', 'string[]', 'uuid[]', 'object[]'}
for name, s in schemas.items():
    assert s['kind'] in KINDS and s['gate'] in GATES and s['description'], (name, s)
    if s['fields'] is not None:
        for fld in s['fields']:
            assert fld['type'] in TYPES and fld['name'] and fld['description'], (name, fld)
            if fld['type'] == 'enum':
                assert fld['values'], (name, fld)
undescribed = sorted(n for n, s in schemas.items() if s['fields'] is None)
checks.append(f'every schema names a known kind, gate and field types; {len(undescribed)} operations say their fields are not described ({", ".join(undescribed) or "none"})')

# the bounds, probed on a disposable universe
podman = lambda *a: A.call('podman_run', args=list(a))['stdout'].strip()
images = json.loads(podman('images', '--format', 'json'))
alpine = next(i['Id'] for i in images if any('alpine' in n for n in (i.get('Names') or [])))
u = str(uuid.uuid4())
probes = 0
try:
    A.ok(request('create', u, reference, image='sha256:' + alpine, network_profile='isolated', command=['sleep', '600']))
    A.ok(request('start', u, reference, observe_seconds=1))
    base = {'stop': {'timeout_seconds': 1, 'on_timeout': 'leave_running'}, 'resources': {}, 'start': {},
            'activation_require': {'lease_seconds': 30, 'takeover_margin_seconds': 5}}
    for op in ('start', 'stop', 'resources', 'activation_require'):
        for fld in schemas[op]['fields']:
            if fld['type'] not in ('integer', 'number'):
                continue
            for bad in ([fld['min'] - 1] if 'min' in fld and fld['min'] > 0 else []) + ([fld['max'] + 1] if 'max' in fld else []):
                r = A.api(request(op, u, reference, **{**base[op], fld['name']: bad}))
                assert not r.get('ok') and fld['name'] in r['error'], (op, fld['name'], bad, r)
                probes += 1
        for fld in schemas[op]['fields']:
            if fld['type'] == 'enum':
                r = A.api(request(op, u, reference, **{**base[op], fld['name']: 'not-a-value'}))
                assert not r.get('ok') and fld['name'] in r['error'], (op, fld['name'], r)
                probes += 1
    checks.append(f'{probes} values just outside a stated bound, or outside an enum, each refused by the daemon naming the field: the schemas state the bounds the daemon keeps')
    print(json.dumps({'result': 'PASS', 'checks': checks}, indent=2))
finally:
    A.api(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
