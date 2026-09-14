#!/usr/bin/env python3
"""Steps 5 and 6 of M-U2: the governor role under the epoch gate while all three replicas keep
running; exactly one exclusive route before and after a takeover; withdrawal observed before the
new publication; stale permit and old active refused; duplicate address refused; a peer lost and
reconnected. Same environment as check-manager-replicas-managed.py, plus PODMESH_FENCING_LAB; PODMESH_GATE and
PODMESH_HA_LEDGER default to ~/.podmesh-ha (private, never in the repository), and the gate is
DURABLE across runs: the resource is the logical manager's fixed UUID and every host keeps the
highest epoch it has seen, so a fresh gate per run would be refused as superseded by the second
run -- which is the laboratory's precondition (one current copy) proven the hard way. A gate found
behind what the hosts have seen (a lost gate) is brought forward by explicit transfers to a
recovery replica id, and the report says so; the epochs of one run are relative to its start.

The exclusive effect is the publication of the logical manager's service route -- a /32 of an
address of the prefix outside every pool, announced by the governor's host only, under the
activation lease on the resource "logical manager UUID". Answering traffic at that address
inside the replica is not built (an address alias in the container is a later step); what is
proven is that the announcement exists on exactly one host at every moment, that the old
host's fence withdraws it before the new governor publishes, and that no replica stops.
"""
import io, json, os, pathlib, subprocess, sys, tarfile, tempfile, time, uuid, hashlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
CANDIDATE = os.environ['PODMESH_MANAGER_CANDIDATE']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
control = tempfile.mkdtemp(prefix='podmesh-mu2g-')
targets = {'lab-a': os.environ['PODMESH_SOURCE_SSH'], 'lab-b': os.environ['PODMESH_DESTINATION_SSH'], 'lab-c': os.environ['PODMESH_THIRD_SSH']}
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
LEASE_GATE_REASONS = ('none is held', 'held by another host', 'expired', 'superseded')
reference = 'disposable-lab-m-u2-governor'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
LOGICAL = replica_set['logical_manager_id']
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
checks = []

def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', reference, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def routes(h):
    return h.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def networks(h):
    return sorted(h.call('podman_run', args=['network', 'ls', '--format', '{{.Name}}'])['stdout'].split())

def service_announced():
    """From the kernels: which hosts hold a route for the service address right now."""
    return sorted(a for a, h in hosts.items() if any(l.startswith(f'{SERVICE} ') for l in routes(h)))

def image_on(h, alias):
    tag = f'localhost/podmesh-manager-universe:m-u2-{alias}'
    return next(l.split()[0] for l in h.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' ' + tag))

def running(h, u):
    return h.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u], check=False).get('stdout', '').strip() == 'true'

def refused(h, req, fragments, label):
    """Refused for one of the named reasons; which one is recorded, since a host that took part in an
    earlier run may hold an expired or superseded lease rather than none at all."""
    r = h.api(req)
    fragments = (fragments,) if isinstance(fragments, str) else fragments
    assert not r.get('ok'), (label, 'accepted, expected refusal', r)
    reason = next((f for f in fragments if f in json.dumps(r)), None)
    assert reason, (label, 'refused for another reason', r.get('error'))
    checks.append(f'refused ({reason}): {label}')


def gate_ready():
    if not os.path.exists(GATE):
        tool('gate', 'init')
    gate = fencing_lab.Authority(pathlib.Path(GATE))
    try:
        gate.inspect(LOGICAL)
    except fencing_lab.Refused:
        gate.declare(LOGICAL)
    seen = 0
    for h in hosts.values():
        s = h.api(request('activation_status', LOGICAL, reference))
        seen = max(seen, (s.get('data') or {}).get('highest_epoch_seen') or 0)
    recovered = []
    while gate.inspect(LOGICAL)['epoch'] < seen:
        e = gate.inspect(LOGICAL)['epoch']
        gate.transfer(LOGICAL, e, 'gate-recovery', 'gate-recovery')
        recovered.append(e + 1)
    state = {'authority_id': gate.authority_id, 'epoch': gate.inspect(LOGICAL)['epoch'], 'hosts_had_seen': seen, 'recovered_epochs': recovered}
    gate.close()
    return state

def inspect_running(h, u):
    d = tempfile.mkdtemp(prefix='podmesh-mu2g-store-'); os.chmod(d, 0o700)
    remote = h.ssh('sudo -n mktemp -d').stdout.decode().strip()
    # The store is copied while the resident runs: SQLite's -shm/-wal files come, go and grow between
    # the copier's listing and its read, so a copy the copier could not complete is retried, never trusted.
    for attempt in range(5):
        try:
            h.ssh(f'sudo -n podman cp podmesh-{u}:/var/lib/podmesh-manager {remote}/state && sudo -n podman cp podmesh-{u}:/etc/podmesh-manager/config.json {remote}/config.json')
            break
        except RuntimeError as e:
            if attempt == 4 or 'copying from container' not in str(e):
                raise
            h.ssh(f'sudo -n rm -rf {remote}/state {remote}/config.json {remote}/podmesh-managerd'); time.sleep(1)
    tar = h.ssh(f'sudo -n tar -C {remote} -cf - .').stdout
    h.ssh(f'sudo -n rm -rf {remote}')
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
    facts = sorted(json.dumps(f, sort_keys=True) for f in i['ordered_facts'])
    return {'history_count': i['history_count'], 'fact_set_sha256': hashlib.sha256('\n'.join(facts).encode()).hexdigest(), 'integrity': i['sqlite_integrity_result']}

def converged(expected_facts, seconds=240):
    deadline = time.time() + seconds
    while time.time() < deadline:
        views = {a: inspect_running(h, universes[a]) for a, h in hosts.items() if running(h, universes[a])}
        if len({v['fact_set_sha256'] for v in views.values()}) == 1 and all(v['history_count'] == expected_facts for v in views.values()):
            return views
        time.sleep(5)
    raise AssertionError(f'not converged to {expected_facts} facts: {views}')

initial = {a: {'routes': routes(h), 'networks': networks(h)} for a, h in hosts.items()}
universes = {a: str(uuid.uuid4()) for a in hosts}
declared = set()
try:
    gate_state = gate_ready()
    for a, h in hosts.items():
        peers = [{'pool': POOLS[o], 'via': lab_hosts[o]} for o in hosts if o != a]
        h.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[a], peer_pools=peers)); declared.add(a)
    for a, h in hosts.items():
        h.ok(request('create', universes[a], reference, image=image_on(h, a), command=['/usr/local/bin/manager-universe'], network_profile='managed', network_address=addresses[a]))
        assert h.ok(request('start', universes[a], reference, observe_seconds=3))['application_outcome'] == 'running_when_observed'
    converged(3)
    checks.append('three replicas running and converged on the managed network (step 4 reproduced)')

    # 5a. no governor yet: no host may publish the service route
    A, B, C = hosts['lab-a'], hosts['lab-b'], hosts['lab-c']
    refused(A, hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-a'], exclusive_resource=str(uuid.uuid4())),
            'under no activation policy', 'exclusive route for a resource under no policy')
    assert service_announced() == []
    # 5b. the gate rotates the role to lab-a; only lab-a may publish
    rot = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-a'], '--lease', '30', '--margin', '5')
    e1 = rot['epoch']
    assert e1 == gate_state['epoch'] + 1 and rot['lease']['live'], (rot, gate_state)
    for other in ('lab-b', 'lab-c'):
        hosts[other].ok(request('activation_require', LOGICAL, reference, lease_seconds=30, takeover_margin_seconds=5, desired_standbys=2, authority_id=rot['permit']['authority_id']))
    refused(B, hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-b'], exclusive_resource=LOGICAL),
            LEASE_GATE_REASONS, 'exclusive route from a host without the lease')
    pub = A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-a'], exclusive_resource=LOGICAL))
    assert any(r['ip'] == SERVICE and r['effective'] and r['exclusive_resource'] == LOGICAL for r in pub['effective']['published_routes']), pub
    assert service_announced() == ['lab-a'], service_announced()
    checks.append(f'exactly one governor: the service route is announced on lab-a only, under epoch {e1}, and refused elsewhere')

    # 5c. takeover to lab-b while every replica keeps running: rotation, supersession, withdrawal
    # observed on the old governor BEFORE the new publication, then the new announcement.
    rot2 = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-b'], '--lease', '30', '--margin', '5')
    assert rot2['epoch'] == e1 + 1, rot2
    over = A.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    assert over['superseded'] is True and over['highest_epoch_seen'] == e1 + 1, over
    C.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    fence = A.ok({'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference, 'timeout_seconds': 10})
    w = [r for r in fence['routes_withdrawn'] if r['ip'] == SERVICE]
    assert w and w[0]['withdrawn'] is True, fence
    assert service_announced() == [], 'the old governor still announces the service address after its fence'
    assert all(running(h, universes[a]) for a, h in hosts.items()), 'a replica stopped during the takeover'
    refused(A, hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-a'], exclusive_resource=LOGICAL),
            'superseded', 'the old governor publishing again under its superseded lease')
    pub2 = B.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-b'], exclusive_resource=LOGICAL))
    assert service_announced() == ['lab-b'], service_announced()
    stale = dict(rot['permit'], replica_id=C.identity, instance_id=C.call('boot_id')['boot_id'], grant_id='forged-stale')
    refused(C, request('activation_acquire', LOGICAL, reference, permit=stale), 'superseded', f'a stale epoch-{e1} permit bound to the third host')
    refused(C, hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-c'], exclusive_resource=LOGICAL),
            LEASE_GATE_REASONS, 'the third host publishing without the role')
    checks.append('takeover with all three replicas running: superseded, the old governor\'s fence withdrew the service route before lab-b published it; exactly one announcement at every observed moment; stale permit and the old governor refused')
    converged(3)
    checks.append('facts still converged after the takeover: replication was never interrupted')

    # 6a. duplicate address: a fourth universe asking for a replica's address is refused
    refused(A, request('create', str(uuid.uuid4()), reference, image=image_on(A, 'lab-a'), command=['sleep', '60'], network_profile='managed', network_address=addresses['lab-a']),
            'allocated to another universe', 'a second universe at an allocated address')
    # 6b. peer loss and reconnection: lab-c's replica stops, the two others stay converged; it comes back with a new boot fact
    C.ok(request('stop', universes['lab-c'], reference, timeout_seconds=15, on_timeout='kill'))
    assert not running(C, universes['lab-c'])
    converged(3)
    assert C.ok(request('start', universes['lab-c'], reference, observe_seconds=3))['application_outcome'] == 'running_when_observed'
    views = converged(4)
    checks.append('peer loss and reconnection: with lab-c stopped the two others stayed converged; back, it appended a fourth boot fact that reached all three')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'service_address': SERVICE, 'announced_at_end': service_announced(), 'facts': views,
                      'gate': gate_state, 'epochs': [e1, e1 + 1],
                      'not_proven': ['answering traffic at the service address inside the replica: the announcement is the exclusive effect proven here',
                                     'a real partition or host loss', 'agent access to the control API']}, indent=2))
finally:
    for a, h in hosts.items():
        h.api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
        h.api(request('stop', universes[a], reference, timeout_seconds=15, on_timeout='kill'))
        h.api(request('delete', universes[a], reference))
        h.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + universes[a]], check=False)
        if a in declared:
            r = h.api(hostwide('network_undeclare', network_uuid=NET))
            if not r.get('ok'):
                print(f'{a}: undeclare refused: {r.get("error")}', file=sys.stderr)
    for a, h in hosts.items():
        print(f'{a}: network state restored: {routes(h) == initial[a]["routes"] and networks(h) == initial[a]["networks"]}', file=sys.stderr)
