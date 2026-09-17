#!/usr/bin/env python3
"""The active manager's replica answers at the logical manager's service address, from every
host, and only the active manager's does. Three replicas run on the managed network (step 4);
the gate rotates the role to lab-a; lab-a publishes the exclusive route, which gives its replica
the service address as an alias inside the universe's network namespace; lab-b and lab-c publish
the plain route that follows the role (the address via the active manager's host). Verified from
outside: a TCP connection to the service address and port from lab-b and from lab-c is accepted;
the address is carried by lab-a's replica only (read inside each universe's namespace from the
host); the resident's status on lab-a counts the connections. Then the role moves to lab-b with
all three running: lab-a's fence withdraws the route and the alias, the follow routes are
withdrawn and republished, lab-b publishes the exclusive route and its replica carries the
address; the connection from lab-a and lab-c now lands on lab-b's replica, lab-a's replica
carries nothing. Between the two, with nothing announced, the connection fails at once. Cleanup
returns the hosts to their initial state.

Same environment as check-manager-active-manager-managed.py. The images must be built from
configurations whose replicas listen on every address (bind 0.0.0.0): a replica bound to its own
address alone does not answer at the alias, which is what the first run of this suite found.
"""
import json, os, pathlib, subprocess, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, secrets_for  # noqa: E402  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
PORT = int(os.environ.get('PODMESH_MANAGER_SERVICE_PORT', '9443'))
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
control = tempfile.mkdtemp(prefix='podmesh-mu2s-')
targets = {'lab-a': os.environ['PODMESH_SOURCE_SSH'], 'lab-b': os.environ['PODMESH_DESTINATION_SSH'], 'lab-c': os.environ['PODMESH_THIRD_SSH']}
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
reference = 'disposable-lab-m-u2-service'
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

def image_on(h, alias):
    tag = f'localhost/podmesh-manager-universe:m-u2-{alias}'
    return next(l.split()[0] for l in h.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' ' + tag))

def running(h, u):
    return h.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u], check=False).get('stdout', '').strip() == 'true'

def carries(h, u):
    """From the host, inside the universe's network namespace: does it carry the service address?"""
    pid = h.call('podman_run', args=['inspect', '--format', '{{.State.Pid}}', 'podmesh-' + u])['stdout'].strip()
    listing = h.ssh(f'sudo -n nsenter -t {pid} -n -- ip -4 -o addr show').stdout.decode()
    return f'{SERVICE}/32' in listing.split()

def carriers():
    return sorted(a for a, h in hosts.items() if running(h, universes[a]) and carries(h, universes[a]))

def connect_from(h):
    """A TCP connection from the host to the service address and port: accepted, refused, or unreachable, with the time it took."""
    r = h.ssh(f'python3 -c "import socket,sys,time; t=time.time(); s=socket.socket(); s.settimeout(4)\n'
              f'try:\n s.connect((\'{SERVICE}\', {PORT})); print(\'accepted\', round(time.time()-t, 2))\n'
              f'except OSError as e:\n print(\'failed\', round(time.time()-t, 2), e.errno, e.strerror)"', check=False)
    return r.stdout.decode().strip()

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
    while gate.inspect(LOGICAL)['epoch'] < seen:
        gate.transfer(LOGICAL, gate.inspect(LOGICAL)['epoch'], 'gate-recovery', 'gate-recovery')
    state = {'authority_id': gate.authority_id, 'epoch': gate.inspect(LOGICAL)['epoch']}
    gate.close()
    return state

def follow(alias, active_manager):
    """On a host that is not the active manager's: the plain route to the service address via the active manager's host."""
    return hosts[alias].ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=lab_hosts[active_manager]))

def withdraw(alias):
    hosts[alias].ok(hostwide('network_route_withdraw', universe_uuid=LOGICAL))

initial = {a: {'routes': routes(h), 'networks': networks(h)} for a, h in hosts.items()}
universes = {a: str(uuid.uuid4()) for a in hosts}
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
    assert carriers() == [] and connect_from(B).startswith('failed'), (carriers(), connect_from(B))
    checks.append('three replicas running; nothing announced: no replica carries the service address and a connection to it fails at once')

    # the active manager on lab-a: exclusive route + alias; the two others follow
    rot = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-a'], '--lease', '120', '--margin', '5')
    for other in ('lab-b', 'lab-c'):
        hosts[other].ok(request('activation_require', LOGICAL, reference, lease_seconds=120, takeover_margin_seconds=5, desired_standbys=2, authority_id=rot['permit']['authority_id']))
    refused(A, hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via='10.86.1.250', exclusive_resource=LOGICAL),
            'nothing runs at', 'an exclusive route pointing at no running universe of this host')
    pub = A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-a'], exclusive_resource=LOGICAL))
    route = next(r for r in pub['effective']['published_routes'] if r['ip'] == SERVICE)
    assert route['alias_universe_uuid'] == universes['lab-a'] and route['alias_effective'] is True and route['effective'] is True, route
    assert carriers() == ['lab-a'], carriers()
    follow('lab-b', 'lab-a'); follow('lab-c', 'lab-a')
    before = A.ok(request('manager_status', universes['lab-a'], reference))['resident_status']
    b, c = connect_from(B), connect_from(C)
    assert b.startswith('accepted') and c.startswith('accepted'), (b, c)
    after = A.ok(request('manager_status', universes['lab-a'], reference))['resident_status']
    counted = {k: (before.get(k), after.get(k)) for k in ('peak_incoming', 'rejected_connections', 'active_incoming')}
    checks.append(f'the active manager\'s replica carries the service address (verified inside its namespace) and accepts a connection to it from lab-b and from lab-c; resident counters before/after: {counted}')

    # the role moves to lab-b with all three running: withdrawal before publication, everywhere
    rot2 = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-b'], '--lease', '120', '--margin', '5')
    A.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    C.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    fence = A.ok({'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference, 'timeout_seconds': 10})
    w = next(r for r in fence['routes_withdrawn'] if r['ip'] == SERVICE)
    assert w['withdrawn'] is True and w['alias_withdrawn'] is True, w
    assert carriers() == [], carriers()
    withdraw('lab-b'); withdraw('lab-c')
    assert connect_from(A).startswith('failed') and connect_from(C).startswith('failed')
    checks.append('after the supersession and lab-a\'s fence: the route and the alias withdrawn, no replica carries the address, the follow routes withdrawn, a connection fails everywhere')
    pub2 = B.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-b'], exclusive_resource=LOGICAL))
    assert carriers() == ['lab-b'], carriers()
    follow('lab-a', 'lab-b'); follow('lab-c', 'lab-b')
    a, c = connect_from(A), connect_from(C)
    assert a.startswith('accepted') and c.startswith('accepted'), (a, c)
    assert all(running(h, universes[x]) for x, h in hosts.items())
    checks.append('the new active manager\'s replica carries the address and accepts the connection from lab-a and from lab-c; lab-a\'s replica carries nothing; all three still running')
    withdraw('lab-b')
    assert carriers() == [], 'the alias outlived the route\'s withdrawal'
    checks.append('a plain withdrawal on the active manager takes the alias with the route')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'service': f'{SERVICE}:{PORT}', 'gate': gate_state, 'epochs': [rot['epoch'], rot2['epoch']],
                      'not_proven': ['an authenticated exchange at the service address: the connection is accepted by the resident\'s listener, its protocol then needs a peer key',
                                     'a real partition or host loss']}, indent=2))
finally:
    for a, h in hosts.items():
        h.api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
        h.api(request('stop', universes[a], reference, timeout_seconds=15, on_timeout='kill'))
        h.api(request('delete', universes[a], reference))
        h.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + universes[a]], check=False)
        remove_replica_config(h, a, reference)
        if a in declared:
            r = h.api(hostwide('network_undeclare', network_uuid=NET))
            if not r.get('ok'):
                print(f'{a}: undeclare refused: {r.get("error")}', file=sys.stderr)
    for a, h in hosts.items():
        print(f'{a}: network state restored: {routes(h) == initial[a]["routes"] and networks(h) == initial[a]["networks"]}', file=sys.stderr)
