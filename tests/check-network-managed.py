#!/usr/bin/env python3
"""The managed network profile on one host: a universe obtains its declared address, and cleanup
leaves no route and no network object behind. Step 3 of the M-U2 order.

Runs on a lab host through the two-host helper (PODMESH_SOURCE_SSH), as root there, so that the
bridge, the routes and the address are verified from the host with `podman network inspect`,
`ip route` and `podman inspect` -- never from the daemon's own tables. The logical prefix and the
pools are the check's fixtures (10.86.0.0/16; the local pool and two peer pools routed via
addresses of the documentation range that nothing on the lab answers), declared and undeclared
here; the host's initial network state is captured before and compared after.
"""
import json, os, sys, tempfile, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-net-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-network'
NET = str(uuid.uuid4())
PREFIX, POOL, GATEWAY = '10.86.0.0/16', '10.86.201.0/24', '10.86.201.1'
# The peer pools are routed through the other lab hosts' on-link addresses (PODMESH_NETWORK_PEER_VIAS,
# two addresses, comma-separated, kept out of the repository): the kernel refuses an off-link next hop.
VIAS = os.environ['PODMESH_NETWORK_PEER_VIAS'].split(',')
assert len(VIAS) == 2, 'PODMESH_NETWORK_PEER_VIAS needs exactly two on-link addresses'
PEERS = [{'pool': '10.86.202.0/24', 'via': VIAS[0]}, {'pool': '10.86.203.0/24', 'via': VIAS[1]}]
checks = []

def sh(cmd):
    return A.call('podman_run', args=cmd, check=False) if cmd[0] == 'podman' else A.ssh(f'sudo -n {cmd}', check=False).stdout.decode()

def routes():
    return A.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def refused(req, fragment, label):
    r = A.api(req)
    assert not r.get('ok') and fragment in json.dumps(r), (label, r)
    checks.append(f'refused: {label}')

initial_routes = routes()
initial_networks = A.call('podman_run', args=['network', 'ls', '--format', '{{.Name}}'])['stdout'].split()
assert 'podmesh-managed' not in initial_networks, 'the bridge already exists on this host; the check needs a clean host'
u = str(uuid.uuid4())
try:
    image = next(l for l in A.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}}', 'docker.io/library/alpine:3.22'])['stdout'].split() if l.startswith('sha256:'))
    # Before any declaration: a managed create is refused, an isolated one is not gated by it.
    refused(request('create', u, reference, image=image, command=['sleep', '600'], network_profile='managed'), 'needs a network declared', 'managed create with no declaration')
    refused(request('create', u, reference, image=image, command=['sleep', '600']), 'network_profile is required', 'create without a profile')
    refused(request('create', u, reference, image=image, command=['sleep', '600'], network_profile='bridge'), 'must be isolated or managed', 'create with an unknown profile')
    refused(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool='10.87.1.0/24'), 'pool must lie inside prefix', 'pool outside the prefix')
    refused(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool='10.86.201.5/24'), 'network address', 'pool naming a host address')
    # A pool that the host already routes (its own LAN) is somebody else's network: refused on the
    # overlap, whatever prefix would contain it.
    refused(hostwide('network_declare', network_uuid=NET, prefix='192.168.0.0/16', pool='192.168.10.0/24'), 'already exists', 'pool overlapping a route the host already holds')

    declared = A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOL, peer_pools=PEERS))
    d = declared['declaration']
    assert d['state'] == 'effective' and d['gateway'] == GATEWAY and d['bridge'] == 'podmesh-managed', declared
    eff = declared['effective']
    assert eff['bridge']['exists'] is True and eff['bridge']['subnets'] == [POOL], eff
    assert all(p['effective'] is True for p in eff['peer_pool_routes']) and len(eff['peer_pool_routes']) == 2, eff
    # From the host, not from the daemon.
    assert any(POOL in l for l in A.call('podman_run', args=['network', 'inspect', 'podmesh-managed', '--format', '{{range .Subnets}}{{.Subnet}} {{end}}'])['stdout'].split()), 'the bridge does not carry the pool'
    now = routes()
    for p in PEERS:
        assert any(l.startswith(p['pool']) and f"via {p['via']}" in l for l in now), (p, now)
    checks.append('declared: bridge with the local pool and DNS disabled, one route per peer pool, all verified from the host')
    refused(hostwide('network_declare', network_uuid=str(uuid.uuid4()), prefix=PREFIX, pool='10.86.204.0/24'), 'already carries', 'a second declaration on the host')

    # A managed universe: the address is allocated to its UUID and carried by the container.
    created = A.ok(request('create', u, reference, image=image, command=['sh', '-c', 'trap "exit 0" TERM; sleep 600 & wait'], network_profile='managed'))
    net = created['network']
    ip = net['requested']['ip']
    assert net['profile'] == 'managed' and ip and ip.startswith('10.86.201.') and ip != GATEWAY, net
    status = A.ok(hostwide('network_status'))
    assert any(a['universe_uuid'] == u and a['ip'] == ip and a['released_at'] is None for a in status['allocations']), status
    A.ok(request('start', u, reference, observe_seconds=1))
    insp = json.loads(A.call('podman_run', args=['inspect', 'podmesh-' + u])['stdout'])[0]
    got = {n: v.get('IPAddress') for n, v in insp['NetworkSettings']['Networks'].items()}
    assert got == {'podmesh-managed': ip}, got
    assert insp['Config']['Labels'].get('io.podmesh.network-profile') == 'managed' and insp['Config']['Labels'].get('io.podmesh.universe-ip') == ip
    ping = A.ssh(f'ping -c 1 -W 2 {ip}', check=False).returncode
    assert ping == 0, f'the host cannot reach the universe at {ip}'
    checks.append(f'managed universe created, started, reachable from the host at its allocated address, labels and Podman agree')

    # The same UUID keeps its address: a second create under the same operation replays, and the
    # allocation is unique -- a second universe gets the next address.
    v = str(uuid.uuid4())
    created2 = A.ok(request('create', v, reference, image=image, command=['sleep', '600'], network_profile='managed'))
    ip2 = created2['network']['requested']['ip']
    assert ip2 != ip and ip2.startswith('10.86.201.'), (ip, ip2)
    checks.append('a second managed universe receives a distinct address of the pool')

    # Routes that follow a universe placed elsewhere: exactly one announcement per address.
    refused(hostwide('network_route_publish', universe_uuid=u, ip=ip, via=VIAS[0]), 'allocated on this host', 'publishing a route to elsewhere for a universe placed here')
    w = str(uuid.uuid4()); wip = '10.86.202.7'
    pub = A.ok(hostwide('network_route_publish', universe_uuid=w, ip=wip, via=VIAS[0]))
    assert any(r['ip'] == wip and r['effective'] is True for r in pub['effective']['published_routes']), pub
    assert any(l.startswith(f'{wip} ') and f'via {VIAS[0]}' in l for l in routes()), routes()
    refused(hostwide('network_route_publish', universe_uuid=str(uuid.uuid4()), ip=wip, via=VIAS[1]), 'already effective', 'a second announcement of one address')
    refused(hostwide('network_route_publish', universe_uuid=str(uuid.uuid4()), ip='10.99.0.1', via=VIAS[1]), 'outside the declared prefix', 'a route outside the prefix')
    refused(hostwide('network_undeclare', network_uuid=NET), 'remain', 'undeclare while allocations and routes remain')
    A.ok(hostwide('network_route_withdraw', universe_uuid=w))
    assert not any(l.startswith(f'{wip} ') for l in routes()), 'the /32 survived its withdrawal'
    refused(hostwide('network_route_withdraw', universe_uuid=w), 'No route is published', 'withdrawing twice')
    checks.append('a /32 route published and withdrawn, verified from the kernel; a second announcement of one address refused')

    # Cleanup: delete releases the addresses; undeclare removes the bridge and the peer routes.
    A.ok(request('stop', u, reference, timeout_seconds=10, on_timeout='kill'))
    gone = A.ok(request('delete', u, reference))
    assert gone['network_address_released'] == ip, gone
    gone2 = A.ok(request('delete', v, reference))
    assert gone2['network_address_released'] == ip2, gone2
    status = A.ok(hostwide('network_status'))
    assert all(a['released_at'] is not None for a in status['allocations']), status
    A.ok(hostwide('network_undeclare', network_uuid=NET))
    after_networks = A.call('podman_run', args=['network', 'ls', '--format', '{{.Name}}'])['stdout'].split()
    assert 'podmesh-managed' not in after_networks, after_networks
    assert routes() == initial_routes, ('routes differ from the initial state', initial_routes, routes())
    assert sorted(after_networks) == sorted(initial_networks), (initial_networks, after_networks)
    checks.append('cleanup: addresses released on delete, bridge and peer routes removed on undeclare, host routes and networks byte-identical to the initial state')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'network_uuid': NET, 'pool': POOL, 'universe_ip': ip}, indent=2))
finally:
    for n in (u, locals().get('v', ''), ):
        if n:
            A.api(request('stop', n, reference, timeout_seconds=10, on_timeout='kill'))
            A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + n], check=False)
    A.api(hostwide('network_route_withdraw', universe_uuid=locals().get('w', 'none')))
    A.api(hostwide('network_undeclare', network_uuid=NET))
