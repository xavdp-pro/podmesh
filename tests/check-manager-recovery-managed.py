#!/usr/bin/env python3
"""Step 7 of M-U2: a recovery point combined with the running replica set -- rescue, not the only
copy. Three replicas run on the managed network with an active manager announced; one replica
(lab-c's) is stopped through the typed stop, a recovery point is prepared from it, and the
replica is then DELETED, its address released. While it is gone the set moves on (lab-b's replica
restarts and appends a boot fact the lost replica never saw). The point is restored on lab-c into
quarantine (isolated, as every restore is), promoted into the replica's own identity under the
managed profile at the address the restore reported as the source's, and started: it comes back
with the facts of the point, imports the one it missed from its peers, appends its own new boot
fact, and the three replicas converge again -- all while the active manager's announcement never
moved.

Same environment as check-manager-governor-managed.py. Verified from outside: the promoted
container's address from Podman, the fact sets from `podman cp` copies inspected by the attested
inspector, the service route from the kernels. Not shown: a host loss (the rescue is on the same
host, since a replica's address lives in its host's pool), a real partition, a signed manifest.
"""
import io, json, os, pathlib, subprocess, sys, tarfile, tempfile, time, uuid, hashlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request, transfer  # noqa: E402
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, secrets_for  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
CANDIDATE = os.environ['PODMESH_MANAGER_CANDIDATE']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
control = tempfile.mkdtemp(prefix='podmesh-mu2r-')
targets = {'lab-a': os.environ['PODMESH_SOURCE_SSH'], 'lab-b': os.environ['PODMESH_DESTINATION_SSH'], 'lab-c': os.environ['PODMESH_THIRD_SSH']}
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
reference = 'disposable-lab-m-u2-recovery'
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
    return sorted(a for a, h in hosts.items() if any(l.startswith(f'{SERVICE} ') for l in routes(h)))

def image_on(h, alias):
    tag = f'localhost/podmesh-manager-universe:m-u2-{alias}'
    return next(l.split()[0] for l in h.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' ' + tag))

def running(h, u):
    return h.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u], check=False).get('stdout', '').strip() == 'true'

def present(h, u):
    return h.call('podman_run', args=['inspect', '--format', '{{.Id}}', 'podmesh-' + u], check=False).get('stdout', '').strip() != ''

def refused(h, req, fragment, label):
    r = h.api(req)
    assert not r.get('ok'), (label, 'accepted, expected refusal', r)
    assert fragment in json.dumps(r), (label, 'refused for another reason', r.get('error'))
    checks.append(f'refused ({fragment}): {label}')

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
    d = tempfile.mkdtemp(prefix='podmesh-mu2r-store-'); os.chmod(d, 0o700)
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
            h.ssh(f'sudo -n rm -rf {remote}/state {remote}/config.json'); time.sleep(1)
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
    return {'history_count': i['history_count'], 'fact_set_sha256': hashlib.sha256('\n'.join(facts).encode()).hexdigest(), 'integrity': i['sqlite_integrity_result'],
            'import_sources': len({r.get('source_replica_id') for r in i['ordered_receipts'] if r.get('kind') == 'authenticated_import'}),
            'scopes': sorted({f['scope'] for f in i['ordered_facts']})}

def converged(expected_facts, seconds=240):
    deadline = time.time() + seconds
    views = {}
    while time.time() < deadline:
        views = {a: inspect_running(h, universes[a]) for a, h in hosts.items() if running(h, universes[a])}
        if len({v['fact_set_sha256'] for v in views.values()}) == 1 and all(v['history_count'] == expected_facts for v in views.values()):
            return views
        time.sleep(5)
    raise AssertionError(f'not converged to {expected_facts} facts: {views}')

initial = {a: {'routes': routes(h), 'networks': networks(h)} for a, h in hosts.items()}
universes = {a: str(uuid.uuid4()) for a in hosts}
quarantine = str(uuid.uuid4())
declared = set()
A, B, C = hosts['lab-a'], hosts['lab-b'], hosts['lab-c']
try:
    gate_state = gate_ready()
    for a, h in hosts.items():
        peers = [{'pool': POOLS[o], 'via': lab_hosts[o]} for o in hosts if o != a]
        h.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[a], peer_pools=peers)); declared.add(a)
    for a, h in hosts.items():
        declare_replica_config(h, a, reference, state_dir)
        replica_create(h, universes[a], a, reference, addresses[a], request)
        started = h.ok(request('start', universes[a], reference, observe_seconds=3))
        assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], h.call('podman_run', args=['logs', 'podmesh-' + universes[a]], check=False))
    converged(3)
    rot = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-a'], '--lease', '120', '--margin', '5')
    A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-a'], exclusive_resource=LOGICAL))
    assert service_announced() == ['lab-a']
    checks.append(f'three replicas running and converged, the active manager on lab-a under epoch {rot["epoch"]} with the service route announced there only')

    # 1. the replica on lab-c is stopped through the typed stop, a point is prepared, and the replica is deleted
    stopped = C.ok(request('stop', universes['lab-c'], reference, timeout_seconds=20, on_timeout='kill'))
    assert stopped['forced'] is False, stopped
    prepared = C.ok(request('recovery_point_prepare', universes['lab-c'], reference))
    point = prepared['recovery_point_uuid']
    manifest = json.loads(C.ssh(f"sudo -n cat {prepared['outbox']}/recovery-point-manifest.json").stdout)
    assert manifest['podman_config']['labels']['io.podmesh.universe-ip'] == addresses['lab-c'], manifest['podman_config']['labels']
    deleted = C.ok(request('delete', universes['lab-c'], reference))
    assert deleted['network_address_released'] == addresses['lab-c'], deleted
    assert not present(C, universes['lab-c'])
    checks.append('lab-c\'s replica stopped cleanly, a recovery point prepared from it (the manifest records its managed address), then deleted and its address released')

    # 2. the set moves on without it: lab-b's replica restarts and appends a fact the lost replica never saw
    B.ok(request('stop', universes['lab-b'], reference, timeout_seconds=20, on_timeout='kill'))
    started = B.ok(request('start', universes['lab-b'], reference, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], B.call('podman_run', args=['logs', 'podmesh-' + universes['lab-b']], check=False))
    before = converged(4)
    assert set(before) == {'lab-a', 'lab-b'}, before
    assert service_announced() == ['lab-a']
    checks.append('with the replica gone, the two others converged on a fourth fact it never saw; the active manager\'s announcement did not move')

    # 3. rescue: the point restored on lab-c into quarantine (isolated), then promoted into the replica's own identity
    #    under the managed profile at the address the restore reported, under a lease on that universe
    transfer(C, C, point, files=('recovery-point-manifest.json', 'rootfs.tar'))
    restored = C.ok(request('recovery_point_restore', quarantine, reference, recovery_point_uuid=point))
    assert restored['quarantined'] is True and restored['network'] == 'none', restored
    assert restored['source_network'] == {'profile': 'managed', 'ip': addresses['lab-c'], 'network_uuid': NET}, restored['source_network']
    C.ok(request('activation_require', universes['lab-c'], reference, lease_seconds=120, takeover_margin_seconds=5))
    C.ok(request('activation_acquire', universes['lab-c'], reference))
    refused(C, request('recovery_point_promote', universes['lab-c'], reference, restored_universe_uuid=quarantine, network_profile='managed', network_address=addresses['lab-a']),
            'outside', 'a promotion at an address outside this host\'s pool')
    assert restored['source_secrets'] == secrets_for('lab-c'), restored['source_secrets']
    promoted = C.ok(request('recovery_point_promote', universes['lab-c'], reference, restored_universe_uuid=quarantine, network_profile='managed', network_address=restored['source_network']['ip'], secrets=restored['source_secrets']))
    assert promoted['network'] == {'profile': 'managed', 'requested_address': addresses['lab-c']}, promoted['network']
    assert promoted['secrets'] == secrets_for('lab-c'), promoted['secrets']
    insp = json.loads(C.call('podman_run', args=['inspect', 'podmesh-' + universes['lab-c']])['stdout'])[0]
    assert insp['Config']['Labels']['io.podmesh.universe-ip'] == addresses['lab-c'] and insp['Config']['Labels']['io.podmesh.network-profile'] == 'managed', insp['Config']['Labels']
    assert insp['State']['Status'] == 'created', insp['State']
    checks.append('the point restored into quarantine (isolated, the source\'s managed address reported), and promoted into the replica\'s identity at that address under a lease; an address outside the pool refused')

    # 4. started, it rejoins: the facts of the point, the one it missed, and its own new boot fact, on all three
    started = C.ok(request('start', universes['lab-c'], reference, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], C.call('podman_run', args=['logs', 'podmesh-' + universes['lab-c']], check=False))
    insp = json.loads(C.call('podman_run', args=['inspect', 'podmesh-' + universes['lab-c']])['stdout'])[0]
    assert insp['NetworkSettings']['Networks']['podmesh-managed']['IPAddress'] == addresses['lab-c'], insp['NetworkSettings']['Networks']
    after = converged(5)
    assert set(after) == {'lab-a', 'lab-b', 'lab-c'} and after['lab-c']['import_sources'] == 2 and after['lab-c']['integrity'] == 'ok', after
    assert service_announced() == ['lab-a']
    checks.append('the rescued replica came back at its address, imported the fact it had missed from both peers, appended its own boot fact, and the three converged on five facts; the active manager unchanged throughout')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'recovery_point': point, 'facts_before_rescue': before, 'facts_after_rescue': after, 'gate': gate_state,
                      'not_proven': ['a host loss: the rescue is on the same host, a replica\'s address living in its host\'s pool',
                                     'a real partition', 'a signed manifest', 'the replica serving at the service address']}, indent=2))
finally:
    for a, h in hosts.items():
        h.api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
        h.api(request('stop', universes[a], reference, timeout_seconds=15, on_timeout='kill'))
        h.api(request('delete', universes[a], reference))
        h.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + universes[a]], check=False)
        remove_replica_config(h, a, reference)
    C.api(request('delete', quarantine, reference))
    C.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + quarantine], check=False)
    C.api(request('activation_release', universes['lab-c'], reference))
    for a, h in hosts.items():
        if a in declared:
            r = h.api(hostwide('network_undeclare', network_uuid=NET))
            if not r.get('ok'):
                print(f'{a}: undeclare refused: {r.get("error")}', file=sys.stderr)
    for a, h in hosts.items():
        print(f'{a}: network state restored: {routes(h) == initial[a]["routes"] and networks(h) == initial[a]["networks"]}', file=sys.stderr)
