#!/usr/bin/env python3
"""A real partition of the universe network: the governor's host is cut from the two other hosts
(every packet between them dropped by an nftables rule on the governor's host, both directions),
while the agent -- this suite, from the workstation -- still reaches every host. What is shown,
from outside: the three replicas diverge (the two connected ones converge on a fact the cut one
never sees), the service address is unreachable from the cut side, the agent moves the role to a
connected host (rotation, supersession delivered to the cut host, its fence withdrawing route and
alias), the connected hosts reach the service address again, and on reconnection the cut host's
replica converges as a simple replica and reaches the service address through the follow route.
Cleanup removes the rule -- and a dead man's switch on the host removes it anyway after ten
minutes, in case this suite dies with the cut in place.

What it does NOT show, and says so: a partition that also cuts the agent from the governor's host.
PodMesh acts on nothing by itself, so a host no agent can reach keeps its alias until an agent
reaches it -- the self-fence is an operation, and whether it may run on a timer is the operator's
decision (UNIVERSE-HIGH-AVAILABILITY.md). The takeover margin bounds what a correct agent does,
not what an unreachable host does.

Same environment as check-manager-service-address.py; nftables on the hosts.
"""
import json, os, pathlib, subprocess, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
CANDIDATE = os.environ['PODMESH_MANAGER_CANDIDATE']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
PORT = int(os.environ.get('PODMESH_MANAGER_SERVICE_PORT', '9443'))
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
control = tempfile.mkdtemp(prefix='podmesh-mu2p-')
targets = {'lab-a': os.environ['PODMESH_SOURCE_SSH'], 'lab-b': os.environ['PODMESH_DESTINATION_SSH'], 'lab-c': os.environ['PODMESH_THIRD_SSH']}
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
reference = 'disposable-lab-m-u2-partition'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
LOGICAL = replica_set['logical_manager_id']
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
TABLE = 'podmesh-lab-partition'
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
    pid = h.call('podman_run', args=['inspect', '--format', '{{.State.Pid}}', 'podmesh-' + u])['stdout'].strip()
    return f'{SERVICE}/32' in h.ssh(f'sudo -n nsenter -t {pid} -n -- ip -4 -o addr show').stdout.decode().split()

def carriers():
    return sorted(a for a, h in hosts.items() if running(h, universes[a]) and carries(h, universes[a]))

def connect_from(h):
    r = h.ssh(f'python3 -c "import socket,sys,time; t=time.time(); s=socket.socket(); s.settimeout(4)\n'
              f'try:\n s.connect((\'{SERVICE}\', {PORT})); print(\'accepted\', round(time.time()-t, 2))\n'
              f'except OSError as e:\n print(\'failed\', round(time.time()-t, 2), e.errno, e.strerror)"', check=False)
    return r.stdout.decode().strip()

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

def follow(alias, governor):
    return hosts[alias].ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=lab_hosts[governor]))

def withdraw(alias):
    hosts[alias].ok(hostwide('network_route_withdraw', universe_uuid=LOGICAL))

def inspect_running(h, u):
    """The sibling suites' store inspection, copied so that it stays identical."""
    import io, tarfile, hashlib
    d = tempfile.mkdtemp(prefix='podmesh-mu2p-store-'); os.chmod(d, 0o700)
    remote = h.ssh('sudo -n mktemp -d').stdout.decode().strip()
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
    return {'history_count': i['history_count'], 'fact_set_sha256': hashlib.sha256('\n'.join(facts).encode()).hexdigest(), 'integrity': i['sqlite_integrity_result']}

def converged(expected_facts, among, seconds=240):
    deadline = time.time() + seconds
    views = {}
    while time.time() < deadline:
        views = {a: inspect_running(hosts[a], universes[a]) for a in among}
        if len({v['fact_set_sha256'] for v in views.values()}) == 1 and all(v['history_count'] == expected_facts for v in views.values()):
            return views
        time.sleep(5)
    raise AssertionError(f'not converged to {expected_facts} facts among {among}: {views}')

def cut(h, others):
    """Drop every packet between this host and the others -- their host addresses and their pools,
    since universe-to-universe traffic keeps its own addresses inside the prefix -- both directions,
    in a table of its own; a dead man's switch removes the table after ten minutes whatever happens
    to this suite."""
    peers = ', '.join(others)
    # Dropped at prerouting, so that packets forwarded into the bridge (a connection to the service
    # address the replica carries) are cut as well as those delivered to the host; and at output for
    # what the host itself sends. The first attempt hooked input only, and the forwarded connections
    # to the service address crossed the "cut".
    h.ssh(f'sudo -n nft add table inet {TABLE} && sudo -n nft add chain inet {TABLE} prerouting "{{ type filter hook prerouting priority -300; }}" '
          f'&& sudo -n nft add chain inet {TABLE} output "{{ type filter hook output priority -300; }}" '
          f'&& sudo -n nft add rule inet {TABLE} prerouting ip saddr {{ {peers} }} drop && sudo -n nft add rule inet {TABLE} output ip daddr {{ {peers} }} drop')
    h.ssh(f'sudo -n systemd-run --quiet --on-active=600 --unit=podmesh-lab-partition-deadman nft delete table inet {TABLE}', check=False)

def reconnect(h):
    h.ssh(f'sudo -n nft delete table inet {TABLE}', check=False)
    h.ssh('sudo -n systemctl stop podmesh-lab-partition-deadman.timer podmesh-lab-partition-deadman.service 2>/dev/null; sudo -n systemctl reset-failed podmesh-lab-partition-deadman.service 2>/dev/null', check=False)

def is_cut(h):
    return TABLE in h.ssh('sudo -n nft list tables', check=False).stdout.decode()

initial = {a: {'routes': routes(h), 'networks': networks(h)} for a, h in hosts.items()}
universes = {a: str(uuid.uuid4()) for a in hosts}
declared = set()
A, B, C = hosts['lab-a'], hosts['lab-b'], hosts['lab-c']
assert not is_cut(A), 'the partition table already exists on lab-a; refusing to run on top of it'
try:
    gate_state = gate_ready()
    for a, h in hosts.items():
        peers = [{'pool': POOLS[o], 'via': lab_hosts[o]} for o in hosts if o != a]
        h.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[a], peer_pools=peers)); declared.add(a)
    for a, h in hosts.items():
        h.ok(request('create', universes[a], reference, image=image_on(h, a), command=['/usr/local/bin/manager-universe'], network_profile='managed', network_address=addresses[a]))
        started = h.ok(request('start', universes[a], reference, observe_seconds=3))
        assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], h.call('podman_run', args=['logs', 'podmesh-' + universes[a]], check=False))
    converged(3, ['lab-a', 'lab-b', 'lab-c'])
    rot = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-a'], '--lease', '120', '--margin', '5')
    for other in ('lab-b', 'lab-c'):
        hosts[other].ok(request('activation_require', LOGICAL, reference, lease_seconds=120, takeover_margin_seconds=5, desired_standbys=2, authority_id=rot['permit']['authority_id']))
    A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-a'], exclusive_resource=LOGICAL))
    follow('lab-b', 'lab-a'); follow('lab-c', 'lab-a')
    assert carriers() == ['lab-a'] and connect_from(B).startswith('accepted')
    checks.append('three replicas converged, the governor on lab-a carrying the service address, reachable from lab-b')

    # the cut: lab-a's host from the two others, both directions; the agent still reaches lab-a
    # The other hosts' addresses AND their pools: with the source NAT off inside the prefix, the
    # replicas' traffic arrives with the universes' own addresses, which a cut by host address alone
    # let through (found when the NAT exemption landed; the suite then saw the cut side converge).
    cut(A, [lab_hosts['lab-b'], lab_hosts['lab-c'], POOLS['lab-b'], POOLS['lab-c']])
    assert is_cut(A)
    assert connect_from(B).startswith('failed') and connect_from(C).startswith('failed'), 'the service address is still reachable across the cut'
    B.ok(request('stop', universes['lab-b'], reference, timeout_seconds=20, on_timeout='kill'))
    started = B.ok(request('start', universes['lab-b'], reference, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], B.call('podman_run', args=['logs', 'podmesh-' + universes['lab-b']], check=False))
    connected = converged(4, ['lab-b', 'lab-c'])
    cut_side = inspect_running(A, universes['lab-a'])
    assert cut_side['history_count'] == 3, cut_side
    assert running(A, universes['lab-a']) and carriers() == ['lab-a']
    checks.append('partition: the two connected replicas converged on a fourth fact the cut replica never saw (3 facts, still running, still carrying the address); the service address unreachable from the connected side')

    # the agent moves the role to a connected host; the cut host is reachable by the agent and fences itself on request
    rot2 = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-b'], '--lease', '120', '--margin', '5')
    A.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    C.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    fence = A.ok({'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference, 'timeout_seconds': 10})
    w = next(r for r in fence['routes_withdrawn'] if r['ip'] == SERVICE)
    assert w['withdrawn'] is True and w['alias_withdrawn'] is True, w
    withdraw('lab-b'); withdraw('lab-c')
    assert carriers() == []
    B.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-b'], exclusive_resource=LOGICAL))
    follow('lab-c', 'lab-b')
    assert carriers() == ['lab-b'] and connect_from(C).startswith('accepted'), (carriers(), connect_from(C))
    assert all(running(h, universes[x]) for x, h in hosts.items())
    checks.append('takeover across the partition: the role moved to lab-b, lab-a fenced on the agent\'s request (route and alias withdrawn), lab-b\'s replica carries the address and lab-c reaches it; all three still running')

    # reconnection: the cut host's replica converges as a simple replica and reaches the service address through the follow route
    reconnect(A)
    assert not is_cut(A)
    follow('lab-a', 'lab-b')
    healed = converged(4, ['lab-a', 'lab-b', 'lab-c'])
    assert connect_from(A).startswith('accepted'), connect_from(A)
    assert carriers() == ['lab-b']
    checks.append('reconnection: the cut replica converged on the fourth fact as a simple replica, lab-a reaches the service address on lab-b through the follow route, and the address is carried by lab-b\'s replica only')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'facts_during_cut': {'connected': connected, 'cut_side': cut_side}, 'facts_after': healed,
                      'gate': gate_state, 'epochs': [rot['epoch'], rot2['epoch']],
                      'not_proven': ['a partition that also cuts the agent from the governor\'s host: the cut host then keeps its alias until an agent reaches it, since the self-fence is an operation and PodMesh runs no timer',
                                     'a host loss', 'an authenticated exchange at the service address']}, indent=2))
finally:
    reconnect(A)
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
        print(f'{a}: network state restored: {routes(h) == initial[a]["routes"] and networks(h) == initial[a]["networks"]}; cut: {is_cut(h)}', file=sys.stderr)
