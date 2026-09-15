#!/usr/bin/env python3
"""The agent's typed door to a manager universe's control socket: `manager_status` and
`manager_observe`, on one lab host, with one manager replica running under the managed profile
(its resident binds to its managed address; its peers are unreachable here, which the resident
tolerates). Environment: PODMESH_SOURCE_SSH (the host), the transient service variables,
PODMESH_MANAGER_CANDIDATE (the attested inspector), PODMESH_REPLICA_SET, PODMESH_NETWORK_PEER_VIAS
(two on-link addresses for the peer-pool routes), and the image `localhost/podmesh-manager-universe:m-u2-lab-a`.

Verified from outside: the resident's status names the replica of the configuration; an
observation appended through PodMesh is in the store (copied out with `podman cp`, inspected by
the attested inspector) with the subject and value named, beside the boot fact the entrypoint
wrote; a replay of the same operation is served from PodMesh's journal and appends nothing; a
new operation appends one more. Refused: a scope this replica does not own, a value beyond the
resident's bound, a universe that is not running, a universe that is not a manager (an Alpine
`sleep` carries no control socket), and a field outside the resident's grammar.
"""
import io, json, os, sys, tarfile, tempfile, time, uuid, hashlib, subprocess

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
CANDIDATE = os.environ['PODMESH_MANAGER_CANDIDATE']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
VIAS = os.environ['PODMESH_NETWORK_PEER_VIAS'].split(',')
control = tempfile.mkdtemp(prefix='podmesh-mctl-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-manager-control'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
replica = next(r for r in replica_set['replicas'] if r['alias'] == 'lab-a')
SCOPE = replica_set['scopes']['lab-a']
FOREIGN_SCOPE = replica_set['scopes']['lab-b']
checks = []

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def routes():
    return A.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def networks():
    return sorted(A.call('podman_run', args=['network', 'ls', '--format', '{{.Name}}'])['stdout'].split())

def image(tag):
    return next(l.split()[0] for l in A.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' ' + tag))

def refused(req, fragment, label):
    r = A.api(req)
    assert not r.get('ok'), (label, 'accepted, expected refusal', r)
    assert fragment in json.dumps(r), (label, 'refused for another reason', r.get('error'))
    checks.append(f'refused ({fragment}): {label}')

def facts_in_store(u):
    d = tempfile.mkdtemp(prefix='podmesh-mctl-store-'); os.chmod(d, 0o700)
    remote = A.ssh('sudo -n mktemp -d').stdout.decode().strip()
    for attempt in range(5):
        try:
            A.ssh(f'sudo -n podman cp podmesh-{u}:/var/lib/podmesh-manager {remote}/state && sudo -n podman cp podmesh-{u}:/etc/podmesh-manager/config.json {remote}/config.json')
            break
        except RuntimeError as e:
            if attempt == 4 or 'copying from container' not in str(e):
                raise
            A.ssh(f'sudo -n rm -rf {remote}/state {remote}/config.json'); time.sleep(1)
    tar = A.ssh(f'sudo -n tar -C {remote} -cf - .').stdout
    A.ssh(f'sudo -n rm -rf {remote}')
    with tarfile.open(fileobj=io.BytesIO(tar)) as t:
        for m in t.getmembers():
            if m.isfile() and not m.name.startswith('/') and '..' not in m.name.split('/'):
                target = os.path.join(d, m.name); os.makedirs(os.path.dirname(target), exist_ok=True)
                with open(target, 'wb') as f:
                    f.write(t.extractfile(m).read())
    config = json.load(open(os.path.join(d, 'config.json')))
    config['network']['database_path'] = os.path.join(d, 'state', 'manager.sqlite'); config['control_socket'] = os.path.join(d, 'control.sock')
    for root, _, files in os.walk(d):
        for f in files:
            os.chmod(os.path.join(root, f), 0o600)
    with open(os.path.join(d, 'config.json'), 'w') as f:
        json.dump(config, f)
    os.chmod(os.path.join(d, 'config.json'), 0o600)
    p = subprocess.run([CANDIDATE, '--inspect-store', '--config', os.path.join(d, 'config.json'), '--state-dir', os.path.join(d, 'state')], capture_output=True, text=True)
    assert p.returncode == 0, ('inspect-store', p.stderr[-400:])
    i = json.loads(p.stdout)
    assert i['sqlite_integrity_result'] == 'ok'
    return [(f['scope'], f['subject'], f['value']) for f in i['ordered_facts']]

initial = {'routes': routes(), 'networks': networks()}
u = str(uuid.uuid4()); plain = str(uuid.uuid4())
declared = False
try:
    peers = [{'pool': POOLS['lab-b'], 'via': VIAS[0]}, {'pool': POOLS['lab-c'], 'via': VIAS[1]}]
    A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=peers)); declared = True
    A.ok(request('create', u, reference, image=image('localhost/podmesh-manager-universe:m-u2-lab-a'), command=['/usr/local/bin/manager-universe'],
                 network_profile='managed', network_address=replica['address']))
    refused(request('manager_status', u, reference), 'not running', 'status of a manager universe that is not running')
    assert A.ok(request('start', u, reference, observe_seconds=3))['application_outcome'] == 'running_when_observed'
    status = A.ok(request('manager_status', u, reference))
    assert status['resident_status']['replica_id'] == replica['replica_id'], status['resident_status']
    assert status['container']['network_profile'] == 'managed' and status['container']['pid'] > 0, status['container']
    checks.append('manager_status: the resident\'s live diagnostic, read through the universe\'s mount namespace, names the replica of the configuration')
    before = facts_in_store(u)
    assert len(before) == 1 and before[0][0] == SCOPE and before[0][1] == 'boot', before

    # the door: one observation in the owned scope, journaled; its replay appends nothing; a second appends one
    oid = str(uuid.uuid4())
    first = A.ok({'operation': 'manager_observe', 'operation_id': oid, 'universe_uuid': u, 'authorization_ref': reference,
                  'scope': SCOPE, 'subject': 'agent-note', 'value': 'first observation through PodMesh'})
    assert first['resident_reply']['response']['result'] == 'observed', first['resident_reply']
    again = A.api({'operation': 'manager_observe', 'operation_id': oid, 'universe_uuid': u, 'authorization_ref': reference,
                   'scope': SCOPE, 'subject': 'agent-note', 'value': 'first observation through PodMesh'})
    assert again['ok'] and again['data'].get('replayed') is True, again
    refused({'operation': 'manager_observe', 'operation_id': oid, 'universe_uuid': u, 'authorization_ref': reference,
             'scope': SCOPE, 'subject': 'agent-note', 'value': 'a different request under the same operation'},
            'different request', 'the same operation ID with another value')
    second = A.ok({'operation': 'manager_observe', 'operation_id': str(uuid.uuid4()), 'universe_uuid': u, 'authorization_ref': reference,
                   'scope': SCOPE, 'subject': 'agent-note', 'value': 'second'})
    assert second['resident_reply']['response']['result'] == 'observed', second
    after = facts_in_store(u)
    assert after[1:] == [(SCOPE, 'agent-note', 'first observation through PodMesh'), (SCOPE, 'agent-note', 'second')], after
    checks.append('manager_observe: an observation named by the agent is in the store with its subject and value, beside the boot fact; the replay served from the journal appended nothing; a second one appended one more')

    # refusals, each its own reason
    refused({'operation': 'manager_observe', 'operation_id': str(uuid.uuid4()), 'universe_uuid': u, 'authorization_ref': reference,
             'scope': FOREIGN_SCOPE, 'subject': 'agent-note', 'value': 'not mine'}, 'resident refused', 'a scope this replica does not own')
    refused({'operation': 'manager_observe', 'operation_id': str(uuid.uuid4()), 'universe_uuid': u, 'authorization_ref': reference,
             'scope': SCOPE, 'subject': 'agent-note', 'value': 'x' * 4097}, 'too large', 'a value beyond the resident\'s bound (the API line limit refuses it first)')
    refused({'operation': 'manager_observe', 'operation_id': str(uuid.uuid4()), 'universe_uuid': u, 'authorization_ref': reference,
             'scope': SCOPE, 'subject': 'agent note', 'value': 'space in the subject'}, 'safe ASCII', 'a subject outside the resident\'s grammar')
    refused({'operation': 'manager_observe', 'operation_id': str(uuid.uuid4()), 'universe_uuid': u, 'authorization_ref': reference,
             'scope': '../' + SCOPE, 'subject': 'agent-note', 'value': 'parent segment'}, 'hierarchical', 'a scope with a parent segment')
    assert facts_in_store(u) == after, 'a refusal appended something'
    A.ok(request('create', plain, reference, image=image('docker.io/library/alpine:3.22'), command=['sleep', '300'], network_profile='isolated'))
    A.ok(request('start', plain, reference, observe_seconds=1))
    refused(request('manager_status', plain, reference), 'no control socket', 'a universe that is not a manager')
    refused(request('manager_status', str(uuid.uuid4()), reference), 'no such universe', 'a universe that is not here')
    checks.append('refusals leave the store as it was')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'facts': after,
                      'not_proven': ['who may hold authorization_ref: provenance recorded, not verified, as everywhere',
                                     'the observation reaching the other replicas: step 4 proved replication; this suite runs one replica']}, indent=2))
finally:
    for x in (u, plain):
        A.api(request('stop', x, reference, timeout_seconds=15, on_timeout='kill'))
        A.api(request('delete', x, reference))
        A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + x], check=False)
    if declared:
        r = A.api(hostwide('network_undeclare', network_uuid=NET))
        if not r.get('ok'):
            print(f'undeclare refused: {r.get("error")}', file=sys.stderr)
    print(f'network state restored: {routes() == initial["routes"] and networks() == initial["networks"]}', file=sys.stderr)
