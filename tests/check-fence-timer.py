#!/usr/bin/env python3
"""The self-fence on a timer, under a mandate: a host whose lease lapses on its own clock, with no
agent calling anything, withdraws the role's route and the address its replica carried. One lab
host (PODMESH_SOURCE_SSH), the transient service variables, PODMESH_NETWORK_PEER_VIAS, and the
`podmesh` CLI installed beside the daemon (PODMESH_CLI on the host; default /usr/bin/podmesh).

The shape is the active manager's: a universe on the managed network (an Alpine `sleep`, under no
policy of its own, so it is never stopped), a resource under an activation policy with a short
lease (leases alone, no authority), the lease acquired here, and the exclusive route published
so that the universe carries the address. The packaged script `packaging/podmesh-fence` is run
by a transient systemd timer on the host every 2 seconds. Verified from outside: without a
mandate file the script refuses (exit 3) and the route stays; with the mandate, while the lease
is live nothing is withdrawn; once the lease has lapsed, within one interval and one fence, the
route is gone from the kernel and the address from the universe's namespace, the universe still
running, and the fence's report in the journal names the mandate's provenance. Nothing here was
called by this suite after the mandate was written but the observation. Cleanup stops the timer
and removes the mandate.
"""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
CLI = os.environ.get('PODMESH_CLI', '/usr/bin/podmesh')
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
VIAS = os.environ['PODMESH_NETWORK_PEER_VIAS'].split(',')
control = tempfile.mkdtemp(prefix='podmesh-ftimer-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-fence-timer'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
RESOURCE = str(uuid.uuid4())
LEASE = 20
TIMER = 'podmesh-fence-lab'
SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'packaging', 'podmesh-fence')
checks = []

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def routes():
    return A.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def networks():
    return sorted(A.call('podman_run', args=['network', 'ls', '--format', '{{.Name}}'])['stdout'].split())

def image():
    return next(l.split()[0] for l in A.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' docker.io/library/alpine:3.22'))

def running(u):
    return A.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u], check=False).get('stdout', '').strip() == 'true'

def carries(u):
    pid = A.call('podman_run', args=['inspect', '--format', '{{.State.Pid}}', 'podmesh-' + u])['stdout'].strip()
    return f'{SERVICE}/32' in A.ssh(f'sudo -n nsenter -t {pid} -n -- ip -4 -o addr show').stdout.decode().split()

def announced():
    return any(l.startswith(f'{SERVICE} ') for l in routes())

def timer_running():
    return A.ssh(f'systemctl is-active {TIMER}.timer', check=False).stdout.decode().strip() == 'active'

def fence_operations():
    """How many journaled fence operations the host's journal holds: the timer must add none while
    there is nothing to fence (Codex, I2), and one when there is."""
    return int(A.ssh(f'sudo -n python3 -c "import sqlite3;print(sqlite3.connect(\'file:{state_dir}/state.sqlite?mode=ro\',uri=True).execute(\\"select count(*) from operations where request like \'%activation_fence%\'\\").fetchone()[0])"').stdout.decode().strip())

initial = {'routes': routes(), 'networks': networks()}
u = str(uuid.uuid4())
mandate = f'/run/podmesh-fence-lab-{uuid.uuid4()}'
declared = False
A.ssh(f'sudo -n systemctl stop {TIMER}.timer {TIMER}.service 2>/dev/null; sudo -n systemctl reset-failed {TIMER}.service {TIMER}.timer 2>/dev/null', check=False)
assert not timer_running(), 'a lab fence timer is already running on the host'
try:
    # /run is noexec on Debian: the packaged script goes under /usr/local/lib for the run, removed at cleanup
    A.ssh('sudo -n install -D -m 0755 /dev/stdin /usr/local/lib/podmesh-fence-lab/podmesh-fence', input_bytes=open(SCRIPT, 'rb').read())
    peers = [{'pool': POOLS['lab-b'], 'via': VIAS[0]}, {'pool': POOLS['lab-c'], 'via': VIAS[1]}]
    A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=peers)); declared = True
    A.ok(request('create', u, reference, image=image(), command=['sleep', '600'], network_profile='managed'))
    A.ok(request('start', u, reference, observe_seconds=1))
    ip = json.loads(A.call('podman_run', args=['inspect', 'podmesh-' + u])['stdout'])[0]['NetworkSettings']['Networks']['podmesh-managed']['IPAddress']
    A.ok(request('activation_require', RESOURCE, reference, lease_seconds=LEASE, takeover_margin_seconds=5))
    lease = A.ok(request('activation_acquire', RESOURCE, reference))
    A.ok(hostwide('network_route_publish', universe_uuid=RESOURCE, ip=SERVICE, via=ip, exclusive_resource=RESOURCE))
    assert announced() and carries(u)
    acquired_at = A.call('time')['time']
    checks.append('a universe carrying the role\'s address under a live lease on this host')

    # 1. no mandate: the script refuses, and nothing is withdrawn
    r = A.ssh(f'sudo -n env PODMESH_SOCKET={socket_path} PODMESH_CLI={CLI} PODMESH_FENCE_MANDATE={mandate} /usr/local/lib/podmesh-fence-lab/podmesh-fence', check=False)
    assert r.returncode == 3 and b'no mandate' in r.stderr, (r.returncode, r.stderr[-200:])
    assert announced() and carries(u)
    checks.append('refused (no mandate): the script fences nothing without the operator\'s mandate file, exit 3')

    # 2. the mandate written, the timer started: while the lease is live, nothing is withdrawn
    A.ssh(f'sudo -n sh -c \'umask 077; printf "authorization_ref=mandate:lab-fence-timer\\ntimeout_seconds=5\\n" > {mandate}\'')
    A.ssh(f'sudo -n systemd-run --quiet --unit={TIMER} --on-active=1 --on-unit-active=2 --timer-property=AccuracySec=1s '
          f'--setenv=PODMESH_SOCKET={socket_path} --setenv=PODMESH_CLI={CLI} --setenv=PODMESH_FENCE_MANDATE={mandate} /usr/local/lib/podmesh-fence-lab/podmesh-fence')
    journaled_before = fence_operations()
    time.sleep(6)
    assert timer_running(), 'the transient timer is not running'
    assert announced() and carries(u), 'a live lease was fenced'
    ticks = int(A.ssh(f'sudo -n journalctl -u {TIMER}.service --no-pager -o short 2>/dev/null | grep -c "Finished\\|Deactivated"', check=False).stdout.decode().strip() or 0)
    assert fence_operations() == journaled_before, 'a tick with nothing to fence journaled a fence'
    checks.append(f'the timer ticked while the lease was live: nothing withdrawn, and nothing journaled (the preview answered nothing_to_fence), the universe still carrying the address')

    # 3. the lease lapses on the host's own clock, nobody renews it, nobody calls anything: within one
    #    interval after the lapse the route and the address are gone, the universe still runs
    deadline = lease['expires_at']
    while A.call('time')['time'] <= deadline:
        time.sleep(1)
    withdrawn_at = None
    for _ in range(15):
        if not announced() and not carries(u):
            withdrawn_at = A.call('time')['time']
            break
        time.sleep(1)
    assert withdrawn_at is not None, f'the lapsed role is still announced ({announced()}) or carried ({carries(u)}) fifteen seconds after the lapse'
    assert running(u), 'the universe under no policy was stopped'
    assert fence_operations() == journaled_before + 1, (fence_operations(), journaled_before)
    report = A.ssh(f'sudo -n journalctl -u {TIMER}.service --no-pager -o cat | grep -m1 "mandate:lab-fence-timer"', check=False).stdout.decode()
    status = A.ok(hostwide('network_status'))
    assert not any(r['ip'] == SERVICE for r in status['effective']['published_routes']), status['effective']['published_routes']
    checks.append(f'lease lapsed at {deadline} on the host\'s clock; the route and the address withdrawn by the timer\'s fence by {withdrawn_at} ({withdrawn_at - deadline} s after the lapse), the universe still running, nothing called by the agent; exactly one fence journaled')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'lease_seconds': LEASE, 'acquired_at': acquired_at, 'expires_at': deadline, 'withdrawn_at': withdrawn_at,
                      'not_proven': ['the packaged timer unit itself: the lab runs the packaged script under a transient timer with a 2-second interval, the package ships 5 seconds',
                                     'a host whose clock lies: the lapse is on the host\'s own clock, as the design states']}, indent=2))
finally:
    A.ssh(f'sudo -n systemctl stop {TIMER}.timer {TIMER}.service 2>/dev/null; sudo -n systemctl reset-failed {TIMER}.service {TIMER}.timer 2>/dev/null; sudo -n rm -rf {mandate} /usr/local/lib/podmesh-fence-lab', check=False)
    A.api(hostwide('network_route_withdraw', universe_uuid=RESOURCE))
    A.api(request('activation_release', RESOURCE, reference))
    A.api(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
    if declared:
        r = A.api(hostwide('network_undeclare', network_uuid=NET))
        if not r.get('ok'):
            print(f'undeclare refused: {r.get("error")}', file=sys.stderr)
    print(f'network state restored: {routes() == initial["routes"] and networks() == initial["networks"]}; timer running: {timer_running()}', file=sys.stderr)
