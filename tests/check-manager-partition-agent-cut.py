#!/usr/bin/env python3
"""The partition that also cuts the agent from the governor's host, with the packaged timer on that
host: the host withdraws itself once its lease lapses on its own clock, before the standby that
waited lease plus margin takes the role (Codex's decision 4, the takeover margin's assumption
measured). Same environment as check-manager-partition.py plus PODMESH_CLI (the CLI beside the
daemon on lab-a) and PODMESH_REPLICA_CONFIGS.

Timeline, all from the workstation's clock unless said: the three replicas converge, lab-a is the
governor under a 20-second lease with a 5-second margin, its self-withdrawal timer runs every
two seconds under a mandate; then an nftables table on lab-a drops every packet to and from lab-b,
lab-c, their pools AND this workstation -- lab-a is alone, and nothing can reach it for 100 seconds,
when a dead man's switch on the host removes the table. The suite waits lease + margin + 1 second,
as the takeover tool does for an unreachable active host, then rotates the role to lab-b (the gate
is on the workstation), delivers the supersession to lab-c, and lab-b publishes the exclusive route.
After the reconnection: lab-a's own journal shows the timer's fence withdrawing the route and the
address at a time BEFORE lab-b's publication (both hosts' clocks recorded), lab-a's replica carries
nothing, the supersession delivered to lab-a is accepted, it follows the role and converges as a
simple replica. What it does not show: clocks that lie (the lapse is on lab-a's clock, the wait on
the workstation's; the two are compared with a one-second allowance and both are reported).
"""
import json, os, pathlib, subprocess, sys, tempfile, time, uuid, io, tarfile, hashlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
FENCE = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'packaging', 'podmesh-fence')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
CLI = os.environ['PODMESH_CLI']
CANDIDATE = os.environ['PODMESH_MANAGER_CANDIDATE']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
PORT = int(os.environ.get('PODMESH_MANAGER_SERVICE_PORT', '9443'))
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
control = tempfile.mkdtemp(prefix='podmesh-mu2ac-')
targets = {'lab-a': os.environ['PODMESH_SOURCE_SSH'], 'lab-b': os.environ['PODMESH_DESTINATION_SSH'], 'lab-c': os.environ['PODMESH_THIRD_SSH']}
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
reference = 'disposable-lab-m-u2-agent-cut'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
LOGICAL = replica_set['logical_manager_id']
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
TABLE = 'podmesh-lab-partition'
TIMER = 'podmesh-fence-lab'
LEASE, MARGIN, CUT_SECONDS = 20, 5, 100
checks = []
A, B, C = None, None, None

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

def running(h, u):
    return h.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u], check=False).get('stdout', '').strip() == 'true'

def carries(h, u):
    pid = h.call('podman_run', args=['inspect', '--format', '{{.State.Pid}}', 'podmesh-' + u])['stdout'].strip()
    return f'{SERVICE}/32' in h.ssh(f'sudo -n nsenter -t {pid} -n -- ip -4 -o addr show').stdout.decode().split()

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

def inspect_running(h, u):
    d = tempfile.mkdtemp(prefix='podmesh-mu2ac-store-'); os.chmod(d, 0o700)
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

def workstation_address_seen_by(h):
    return h.ssh('echo $SSH_CLIENT').stdout.decode().split()[0]

def cut(h, others, seconds):
    """The dead man's switch FIRST, verified armed, then the cut in one nftables transaction whose
    SSH session is expected to die under it: the workstation is among the cut, so the switch is the
    only way back. The first run of this suite did it the other way round -- the rules dropped the
    SSH session before the switch was armed, and lab-a stayed cut with nobody able to reach it."""
    peers = ', '.join(others)
    h.ssh(f'sudo -n systemd-run --quiet --on-active={seconds} --unit=podmesh-lab-partition-deadman /usr/sbin/nft delete table inet {TABLE}')
    armed = h.ssh('systemctl is-active podmesh-lab-partition-deadman.timer', check=False).stdout.decode().strip()
    assert armed == 'active', f'the dead man\'s switch is not armed ({armed}); refusing to cut'
    # One-line chain bodies parse `} drop }` as a set closer (nftables on this lab rejects them).
    ruleset = (
        f'table inet {TABLE} {{\n'
        f'  chain prerouting {{\n'
        f'    type filter hook prerouting priority -300;\n'
        f'    ip saddr {{ {peers} }} drop\n'
        f'  }}\n'
        f'  chain output {{\n'
        f'    type filter hook output priority -300;\n'
        f'    ip daddr {{ {peers} }} drop\n'
        f'  }}\n'
        f'}}\n'
    )
    h.ssh(f'sudo -n install -m 0600 /dev/stdin /run/podmesh-lab-partition.nft', input_bytes=ruleset.encode())
    parsed = h.ssh('sudo -n /usr/sbin/nft -c -f /run/podmesh-lab-partition.nft', check=False)
    assert parsed.returncode == 0, f'the partition ruleset does not parse: {parsed.stderr.decode()}'
    # Apply from a systemd unit: the session dies under the cut; nft must not die with it.
    h.ssh('sudo -n systemctl reset-failed podmesh-lab-partition-apply.service 2>/dev/null', check=False)
    h.ssh('sudo -n systemd-run --quiet --collect --unit=podmesh-lab-partition-apply /usr/sbin/nft -f /run/podmesh-lab-partition.nft', check=False)
    time.sleep(2)

def reconnect_cleanup(h):
    h.ssh(f'sudo -n nft delete table inet {TABLE}', check=False)
    h.ssh('sudo -n systemctl stop podmesh-lab-partition-deadman.timer podmesh-lab-partition-deadman.service podmesh-lab-partition-apply.service 2>/dev/null; sudo -n systemctl reset-failed podmesh-lab-partition-deadman.service podmesh-lab-partition-apply.service 2>/dev/null', check=False)

def is_cut(h):
    return TABLE in h.ssh('sudo -n nft list tables', check=False).stdout.decode()

def reset_master(h):
    subprocess.run(['ssh', '-o', f'ControlPath={h.control}/%C', '-O', 'exit', h.target], capture_output=True)

def timer_start(h, mandate):
    h.ssh('sudo -n install -D -m 0755 /dev/stdin /usr/local/lib/podmesh-fence-lab/podmesh-fence', input_bytes=open(FENCE, 'rb').read())
    h.ssh(f'sudo -n sh -c \'umask 077; printf "authorization_ref=mandate:lab-agent-cut\\ntimeout_seconds=5\\n" > {mandate}\'')
    h.ssh(f'sudo -n systemd-run --quiet --unit={TIMER} --on-active=1 --on-unit-active=2 --timer-property=AccuracySec=1s '
          f'--setenv=PODMESH_SOCKET={socket_path} --setenv=PODMESH_CLI={CLI} --setenv=PODMESH_FENCE_MANDATE={mandate} /usr/local/lib/podmesh-fence-lab/podmesh-fence')

def timer_stop(h, mandate):
    h.ssh(f'sudo -n systemctl stop {TIMER}.timer {TIMER}.service 2>/dev/null; sudo -n systemctl reset-failed {TIMER}.service {TIMER}.timer 2>/dev/null; sudo -n rm -rf {mandate} /usr/local/lib/podmesh-fence-lab', check=False)

A, B, C = hosts['lab-a'], hosts['lab-b'], hosts['lab-c']
initial = {a: {'routes': routes(h), 'networks': networks(h)} for a, h in hosts.items()}
universes = {a: str(uuid.uuid4()) for a in hosts}
mandate = f'/run/podmesh-fence-lab-{uuid.uuid4()}'
declared = set()
assert not is_cut(A), 'the partition table already exists on lab-a; refusing to run on top of it'
A.ssh(f'sudo -n systemctl stop {TIMER}.timer {TIMER}.service 2>/dev/null; sudo -n systemctl reset-failed {TIMER}.service {TIMER}.timer 2>/dev/null', check=False)
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
    converged(3, ['lab-a', 'lab-b', 'lab-c'])
    rot = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-a'], '--lease', str(LEASE), '--margin', str(MARGIN))
    for other in ('lab-b', 'lab-c'):
        hosts[other].ok(request('activation_require', LOGICAL, reference, lease_seconds=LEASE, takeover_margin_seconds=MARGIN, desired_standbys=2, authority_id=rot['permit']['authority_id']))
    A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-a'], exclusive_resource=LOGICAL))
    follow('lab-b', 'lab-a'); follow('lab-c', 'lab-a')
    timer_start(A, mandate)
    time.sleep(4)
    assert carries(A, universes['lab-a']) and connect_from(B).startswith('accepted')
    lease_expires = A.ok(request('activation_status', LOGICAL, reference))['expires_at']
    checks.append(f'three replicas converged, lab-a the governor under a {LEASE}-second lease (margin {MARGIN}), its self-withdrawal timer running under a mandate, the service address reachable from lab-b')

    # the cut: lab-a alone -- the other hosts, their pools, and this workstation; back only by the dead man's switch
    agent = workstation_address_seen_by(A)
    clock_a_at_cut = A.call('time')['time']
    clock_ws_at_cut = time.time()
    cut(A, [lab_hosts['lab-b'], lab_hosts['lab-c'], POOLS['lab-b'], POOLS['lab-c'], agent], CUT_SECONDS)
    reset_master(A)
    checks.append(f'lab-a cut from lab-b, lab-c and the agent for {CUT_SECONDS} seconds (dead man\'s switch); lease to expire at {lease_expires} on lab-a\'s clock ({round(lease_expires - clock_a_at_cut, 1)} s after the cut)')
    # the agent's side: wait lease + margin + 1 on its own clock, as the takeover tool does for an unreachable host
    wait = LEASE + MARGIN + 1
    time.sleep(wait)
    seen = connect_from(B)
    assert seen.startswith('failed'), f'the service address still reachable from lab-b across the cut: {seen!r}'
    rot2 = tool('rotate', '--universe', LOGICAL, '--host', targets['lab-b'], '--lease', str(LEASE), '--margin', str(MARGIN))
    C.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    B.ok(hostwide('network_route_withdraw', universe_uuid=LOGICAL)); C.ok(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
    publish_at_b = B.call('time')['time']
    B.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses['lab-b'], exclusive_resource=LOGICAL))
    follow('lab-c', 'lab-b')
    assert carries(B, universes['lab-b']) and connect_from(C).startswith('accepted')
    checks.append(f'after waiting lease + margin + 1 = {wait} s on the agent\'s clock without reaching lab-a: the role rotated to lab-b, lab-c superseded, lab-b carrying the address and reachable from lab-c')

    # the reconnection: only the dead man's switch brings lab-a back
    deadline = time.time() + CUT_SECONDS + 60
    while time.time() < deadline:
        try:
            reset_master(A)
            if not is_cut(A):
                break
        except Exception:
            pass
        time.sleep(10)
    assert not is_cut(A), 'lab-a did not come back'
    reconnect_cleanup(A)
    # lab-a's own account: its timer's fence withdrew the route and the address, at what time on its clock
    log = A.ssh(f'sudo -n journalctl -u {TIMER}.service --no-pager -o short-unix --since @{int(clock_a_at_cut)}', check=False).stdout.decode()
    withdrawn_lines = [l for l in log.splitlines() if '"withdrawn": true' in l]
    assert withdrawn_lines, log[-1500:]
    withdrawn_at_a = float(withdrawn_lines[0].split()[0])
    assert not carries(A, universes['lab-a']) and running(A, universes['lab-a'])
    assert withdrawn_at_a < publish_at_b + 1.0, (withdrawn_at_a, publish_at_b)
    st = A.ok(hostwide('network_status'))
    assert st['effective']['published_routes'] == [], st['effective']['published_routes']
    checks.append(f'lab-a\'s own journal: its timer\'s fence withdrew the route and the address at {withdrawn_at_a} (lab-a\'s clock), {round(withdrawn_at_a - lease_expires, 1)} s after the lapse and {round(publish_at_b - withdrawn_at_a, 1)} s BEFORE lab-b published (lab-b\'s clock); the replica still running, carrying nothing')
    over = A.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    assert over['superseded'] is True
    follow('lab-a', 'lab-b')
    healed = converged(3, ['lab-a', 'lab-b', 'lab-c'])
    assert connect_from(A).startswith('accepted')
    checks.append('reconnected: lab-a superseded, following the role, converged as a simple replica, reaching the service address on lab-b')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'clocks': {'lab_a_at_cut': clock_a_at_cut, 'workstation_at_cut': clock_ws_at_cut, 'lease_expires_lab_a': lease_expires,
                      'withdrawn_at_lab_a': withdrawn_at_a, 'publish_at_lab_b': publish_at_b}, 'facts_after': healed, 'gate': gate_state, 'epochs': [rot['epoch'], rot2['epoch']],
                      'not_proven': ['clocks that lie: the lapse is on lab-a\'s clock, the wait on the agent\'s, compared with a one-second allowance',
                                     'a wedged daemon or a dead host on the cut side: the timer runs through the daemon (lease-expiry self-withdrawal, not host fencing)']}, indent=2))
finally:
    try:
        reset_master(A)
        reconnect_cleanup(A)
        timer_stop(A, mandate)
    except Exception as e:
        print(f'lab-a cleanup: {e}', file=sys.stderr)
    for a, h in hosts.items():
        try:
            h.api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
            h.api(request('stop', universes[a], reference, timeout_seconds=15, on_timeout='kill'))
            h.api(request('delete', universes[a], reference))
            h.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + universes[a]], check=False)
            remove_replica_config(h, a, reference)
            if a in declared:
                r = h.api(hostwide('network_undeclare', network_uuid=NET))
                if not r.get('ok'):
                    print(f'{a}: undeclare refused: {r.get("error")}', file=sys.stderr)
        except Exception as e:
            print(f'{a} cleanup: {e}', file=sys.stderr)
    for a, h in hosts.items():
        print(f'{a}: network state restored: {routes(h) == initial[a]["routes"] and networks(h) == initial[a]["networks"]}; cut: {is_cut(h)}', file=sys.stderr)
