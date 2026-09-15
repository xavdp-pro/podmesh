#!/usr/bin/env python3
"""Crash and storage-failure safety of the managed network's kernel effects (Codex finding B1):
no route, address, bridge or nftables table PodMesh created may outlive its durable record.

One lab host, PODMESH_SOURCE_SSH, the transient service variables, PODMESH_NETWORK_PEER_VIAS, and
PODMESH_DAEMON_BINARY: the daemon's path on the host, because this suite stops and restarts the
transient unit with `PODMESH_FAULT` set to make the daemon fail -- as a storage failure would --
or die -- as a crash would -- right after a kernel effect and before its record says effective.
Verified from outside at every step: `ip route`, the address inside the universe's namespace,
`podman network exists`, `nft list tables`; and from the daemon's own account: the refusal's
compensation, `network_status` (routes, effects, incomplete effects), the startup reconciliation
line in the journal, and the fence's reconciliation.

Cases: a route publication failing after the alias, after the route, and at the final record
(each refused with everything compensated); a route publication crashing after the route (the
kernel then holds an unowned route and address until the daemon restarts, and the restart's
reconciliation undoes both); a declaration crashing after the bridge and one failing after a
peer route (compensated, the host as before, a new declaration accepted); a withdrawal crashing
after the route (the restart finishes it); an undeclaration crashing after the table (the
restart finishes it). Every mutation is refused while something remains, which no case reaches.
"""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
BINARY = os.environ['PODMESH_DAEMON_BINARY']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
VIAS = os.environ['PODMESH_NETWORK_PEER_VIAS'].split(',')
control = tempfile.mkdtemp(prefix='podmesh-crash-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-crash-safety'
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
PEERS = [{'pool': POOLS['lab-b'], 'via': VIAS[0]}, {'pool': POOLS['lab-c'], 'via': VIAS[1]}]
RESOURCE = str(uuid.uuid4())
checks = []
report = {'cases': []}

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def routes():
    return A.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def route_present(dst):
    return any(l.startswith(dst + ' ') for l in routes())

def bridge_present():
    return A.ssh('sudo -n podman network exists podmesh-managed', check=False).returncode == 0

def table_present():
    return 'table inet podmesh-managed' in A.ssh('sudo -n nft list tables', check=False).stdout.decode()

def carries(u):
    pid = A.call('podman_run', args=['inspect', '--format', '{{.State.Pid}}', 'podmesh-' + u], check=False).get('stdout', '').strip()
    if not pid or pid == '0':
        return False
    return f'{SERVICE}/32' in A.ssh(f'sudo -n nsenter -t {pid} -n -- ip -4 -o addr show', check=False).stdout.decode().split()

def image():
    return next(l.split()[0] for l in A.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' docker.io/library/alpine:3.22'))

def daemon(fault=None):
    """Restart the transient daemon, with or without a fault, and return its startup reconciliation."""
    A.ssh(f'sudo -n systemctl stop {unit} 2>/dev/null; sudo -n systemctl reset-failed {unit} 2>/dev/null', check=False)
    mark = A.ssh('date +%s').stdout.decode().strip()
    env = f'--setenv=PODMESH_FAULT={fault} ' if fault else ''
    A.ssh(f'sudo -n systemd-run --quiet --unit={unit} --property=RuntimeDirectory={os.path.basename(os.path.dirname(socket_path))} --property=RuntimeDirectoryMode=0700 '
          f'--property=StateDirectory={os.path.basename(state_dir)} --property=StateDirectoryMode=0700 --property=UMask=0077 '
          f'--setenv=PODMESH_STATE_DIR={state_dir} --setenv=PODMESH_SOCKET={socket_path} {env}{BINARY}')
    for _ in range(50):
        if A.ssh(f'sudo -n test -S {socket_path}', check=False).returncode == 0:
            break
        time.sleep(0.2)
    else:
        raise AssertionError('the daemon did not come up')
    # The last startup line since the mark: the mark has second granularity and a daemon that died in
    # the same second logged its own, empty, line first.
    line = A.ssh(f'sudo -n journalctl -u {unit} --since @{mark} --no-pager -o cat | grep "network reconciliation at startup" | tail -1', check=False).stdout.decode().strip()
    assert line.startswith('PodMesh network reconciliation at startup: '), line
    return json.loads(line.split(': ', 1)[1])

def api_or_dead(req):
    """A request that may kill the daemon under a crash fault: the answer, or None if the daemon died."""
    try:
        r = A.api(req)
    except RuntimeError as e:
        assert 'returned nothing' in str(e) or 'failed' in str(e), e
        return None
    # The helper reports a connection the daemon closed without answering as an interrupted transport.
    return None if r.get('interrupted') else r

def status():
    return A.ok(hostwide('network_status'))

def clean(expect_declared):
    st = status()
    assert st['effective']['published_routes'] == [], st['effective']['published_routes']
    assert st['incomplete_effects'] == 0, st['effects']
    assert not route_present(SERVICE)
    assert (st['declaration'] is not None) == expect_declared, st['declaration']
    assert bridge_present() == expect_declared and table_present() == expect_declared, (bridge_present(), table_present())

def declare():
    return A.ok(hostwide('network_declare', network_uuid=str(uuid.uuid4()), prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=PEERS))

def undeclare(net):
    return A.ok(hostwide('network_undeclare', network_uuid=net))

def publish():
    return hostwide('network_route_publish', universe_uuid=RESOURCE, ip=SERVICE, via=ip, exclusive_resource=RESOURCE)

initial = routes()
assert not bridge_present() and not table_present(), 'the host already carries the bridge or the table'
u = str(uuid.uuid4())
try:
    daemon()
    d = declare(); net = d['declaration']['network_uuid']
    A.ok(request('create', u, reference, image=image(), command=['sleep', '900'], network_profile='managed'))
    A.ok(request('start', u, reference, observe_seconds=1))
    ip = json.loads(A.call('podman_run', args=['inspect', 'podmesh-' + u])['stdout'])[0]['NetworkSettings']['Networks']['podmesh-managed']['IPAddress']
    A.ok(request('activation_require', RESOURCE, reference, lease_seconds=300, takeover_margin_seconds=5))
    A.ok(request('activation_acquire', RESOURCE, reference))
    checks.append('a declared network, a running managed universe, a lease on the role')

    # storage failures inside a publication: refused, everything compensated, nothing left
    for fault in ('after-alias', 'after-route', 'before-route-effective'):
        daemon(fault)
        r = A.api(publish())
        assert not r.get('ok') and 'compensation' in r['error'] and '"gone":true' in r['error'].replace(' ', ''), (fault, r)
        assert not route_present(SERVICE) and not carries(u), (fault, routes(), carries(u))
        clean(True)
        report['cases'].append({'fault': fault, 'refusal': r['error'][:200]})
        checks.append(f'publication failing at {fault}: refused, alias and route compensated, nothing left in the kernel or the ledger')

    # a crash right after the route: the kernel holds an unowned route and address until the restart,
    # whose reconciliation undoes both -- the exact hazard of finding B1, and its closure
    daemon('after-route:crash')
    answer = api_or_dead(publish())
    assert answer is None, ('the daemon survived a crash fault', answer)
    assert route_present(SERVICE) and carries(u), 'the crash fault did not leave the effects in the kernel'
    rec = daemon()
    undone = {(e['kind'], e['key']) for e in rec['undone']}
    assert ('route', f'{SERVICE}/32') in undone and ('alias', f'{SERVICE}@{u}') in undone and rec['remaining'] == [] and rec['routes_dropped'] == [SERVICE], rec
    assert not route_present(SERVICE) and not carries(u)
    clean(True)
    fence = A.ok({'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference, 'timeout_seconds': 5})
    assert fence['routes_withdrawn'] == [] and fence['network_reconciliation']['remaining'] == [], fence
    report['cases'].append({'fault': 'after-route:crash', 'startup_reconciliation': rec})
    checks.append('publication crashing after the route: the route and the address survived the crash unowned, the restart\'s reconciliation undid both and dropped the route\'s record; the fence then had nothing to find')

    # a withdrawal crashing after the route: the restart finishes it
    pub = A.ok(publish())
    assert route_present(SERVICE) and carries(u)
    daemon('withdraw-after-route:crash')
    assert api_or_dead(hostwide('network_route_withdraw', universe_uuid=RESOURCE)) is None
    assert not route_present(SERVICE) and carries(u), 'the crash fault did not leave the alias behind'
    rec = daemon()
    assert ('alias', f'{SERVICE}@{u}') in {(e['kind'], e['key']) for e in rec['undone']} and rec['remaining'] == [] and rec['routes_dropped'] == [SERVICE], rec
    assert not carries(u)
    clean(True)
    report['cases'].append({'fault': 'withdraw-after-route:crash', 'startup_reconciliation': rec})
    checks.append('withdrawal crashing after the route: the alias survived, the restart\'s reconciliation removed it and dropped the record')

    # declarations: the universe and the network are taken down first
    A.ok(request('stop', u, reference, timeout_seconds=5, on_timeout='kill')); A.ok(request('delete', u, reference))
    A.ok(request('activation_release', RESOURCE, reference))
    undeclare(net)
    clean(False)
    daemon('after-bridge:crash')
    assert api_or_dead(hostwide('network_declare', network_uuid=str(uuid.uuid4()), prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=PEERS)) is None
    assert bridge_present() and not table_present(), 'the crash fault did not leave the bridge alone'
    rec = daemon()
    assert 'bridge' in {e['kind'] for e in rec['undone']} and rec['remaining'] == [] and rec['declarations'] and rec['declarations'][0]['now'] == 'gone', rec
    clean(False)
    report['cases'].append({'fault': 'after-bridge:crash', 'startup_reconciliation': rec})
    checks.append('declaration crashing after the bridge: the bridge survived, the restart\'s reconciliation removed it and the interrupted declaration is gone; the host as before')
    daemon('after-peer_route')
    r = A.api(hostwide('network_declare', network_uuid=str(uuid.uuid4()), prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=PEERS))
    assert not r.get('ok') and 'compensation' in r['error'], r
    st = status()
    assert st['declaration']['state'] == 'failed' and st['declaration']['observed']['compensation'], st['declaration']
    assert not bridge_present() and not route_present(POOLS['lab-b']) and not table_present()
    rec = daemon()
    assert rec['declarations'] and rec['declarations'][0]['was'] == 'failed' and rec['declarations'][0]['now'] == 'gone', rec
    d = declare(); net = d['declaration']['network_uuid']
    clean(True)
    report['cases'].append({'fault': 'after-peer_route', 'refusal': r['error'][:200]})
    checks.append('declaration failing after a peer route: refused with the bridge compensated, recorded failed with the evidence, the failed record cleared by reconciliation once nothing of it remained, a new declaration effective')

    # an undeclaration crashing after the table: the restart finishes it
    daemon('undeclare-after-nat_table:crash')
    assert api_or_dead(hostwide('network_undeclare', network_uuid=net)) is None
    assert not table_present() and bridge_present(), 'the crash fault did not leave the bridge and routes behind'
    rec = daemon()
    assert {e['kind'] for e in rec['undone']} >= {'bridge', 'peer_route'} and rec['remaining'] == [] and rec['declarations'][0]['now'] == 'gone', rec
    clean(False)
    report['cases'].append({'fault': 'undeclare-after-nat_table:crash', 'startup_reconciliation': rec})
    checks.append('undeclaration crashing after the table: the bridge and the peer routes survived, the restart\'s reconciliation removed them and the declaration is gone')
    assert routes() == initial, (routes(), initial)
    print(json.dumps({'result': 'PASS', 'checks': checks, 'report': report,
                      'not_proven': ['a SQLite failure inside the ledger write itself (the row could not be recorded): the operation refuses before any kernel effect, by construction; not injected',
                                     'an effect made by somebody else: reported as drift, never touched']}, indent=2))
finally:
    daemon()
    A.api(request('stop', u, reference, timeout_seconds=5, on_timeout='kill')); A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
    A.api(request('activation_release', RESOURCE, reference))
    st = A.api(hostwide('network_status'))
    d = (st.get('data') or {}).get('declaration')
    if d:
        A.api(hostwide('network_route_withdraw', universe_uuid=RESOURCE))
        r = A.api(hostwide('network_undeclare', network_uuid=d['network_uuid']))
        if not r.get('ok'):
            print(f'undeclare refused: {r.get("error")}', file=sys.stderr)
    print(f'network state restored: {routes() == initial and not bridge_present() and not table_present()}', file=sys.stderr)
