#!/usr/bin/env python3
"""On the managed network, a universe reaching a universe of another host is seen there with its
own allocated address, not the host's: the declaration keeps Podman's source NAT off traffic
inside the logical prefix (an nftables table of PodMesh's own; by default a null source NAT of the
local pool's traffic to the prefix, connection tracking kept), and the undeclaration removes it. Two hosts,
PODMESH_SOURCE_SSH and PODMESH_DESTINATION_SSH, with PODMESH_LAB_HOSTS naming their on-link
addresses (lab-a=…,lab-b=…). Verified from outside: the table present after the declaration and
absent after the undeclaration (`nft list tables`); a listener inside the destination universe's
namespace (the host's python through `nsenter -n`, nothing inside the universe) reporting the
peer address of a connection made from inside the source universe's namespace, in both
directions. Cleanup returns both hosts' routes, networks and tables to their initial state.
"""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
control = tempfile.mkdtemp(prefix='podmesh-nonat-')
A = Host('lab-a', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('lab-b', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-no-nat'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24'}
TABLE = 'podmesh-managed'
checks = []

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def routes(h):
    return h.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def networks(h):
    return sorted(h.call('podman_run', args=['network', 'ls', '--format', '{{.Name}}'])['stdout'].split())

def tables(h):
    return sorted(l.strip() for l in h.ssh('sudo -n nft list tables', check=False).stdout.decode().splitlines() if l.strip())

def image(h):
    return next(l.split()[0] for l in h.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' docker.io/library/alpine:3.22'))

def pid(h, u):
    return h.call('podman_run', args=['inspect', '--format', '{{.State.Pid}}', 'podmesh-' + u])['stdout'].strip()

def address(h, u):
    return json.loads(h.call('podman_run', args=['inspect', 'podmesh-' + u])['stdout'])[0]['NetworkSettings']['Networks'][ 'podmesh-managed']['IPAddress']

def peer_seen(listener, u_listener, client, u_client, dst):
    """A connection from inside the client universe to the listener universe: the source address the
    listener's namespace saw, read by the host's own python entered into that namespace."""
    lp, cp = pid(listener, u_listener), pid(client, u_client)
    marker = f'/tmp/podmesh-no-nat-{uuid.uuid4()}'
    listener.ssh(f"sudo -n sh -c 'nohup nsenter -t {lp} -n -- python3 -c \"import socket;s=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);s.bind((\\\"0.0.0.0\\\",9000));s.listen(1);s.settimeout(20);c,a=s.accept();open(\\\"{marker}\\\",\\\"w\\\").write(a[0])\" >/dev/null 2>&1 &'")
    time.sleep(1)
    r = client.ssh(f"sudo -n nsenter -t {cp} -n -- python3 -c \"import socket;s=socket.socket();s.settimeout(5);s.connect(('{dst}',9000));print('connected')\"", check=False)
    assert r.stdout.decode().strip() == 'connected', r.stderr.decode()[-300:]
    time.sleep(1)
    seen = listener.ssh(f'sudo -n cat {marker}; sudo -n rm -f {marker}', check=False).stdout.decode().strip()
    return seen

initial = {h.role: {'routes': routes(h), 'networks': networks(h), 'tables': tables(h)} for h in (A, B)}
for h in (A, B):
    assert f'table inet {TABLE}' not in initial[h.role]['tables'], f'{h.role} already carries the table'
ua, ub = str(uuid.uuid4()), str(uuid.uuid4())
declared = set()
try:
    d = A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=[{'pool': POOLS['lab-b'], 'via': lab_hosts['lab-b']}])); declared.add(A)
    assert d['nat_exemption']['present'] is True and d['nat_exemption']['backend'] == 'null-snat' and any('snat' in r for r in d['nat_exemption']['rules']), d['nat_exemption']
    assert f'table inet {TABLE}' in tables(A)
    B.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS['lab-b'], peer_pools=[{'pool': POOLS['lab-a'], 'via': lab_hosts['lab-a']}])); declared.add(B)
    checks.append('the declaration creates the exemption table on each host, verified from nft, and reports its null-SNAT rule')
    for h, u in ((A, ua), (B, ub)):
        h.ok(request('create', u, reference, image=image(h), command=['sleep', '600'], network_profile='managed'))
        h.ok(request('start', u, reference, observe_seconds=1))
    ip_a, ip_b = address(A, ua), address(B, ub)
    seen_at_b = peer_seen(B, ub, A, ua, ip_b)
    assert seen_at_b == ip_a, f'the universe on lab-b saw {seen_at_b}, not the universe of lab-a at {ip_a}'
    seen_at_a = peer_seen(A, ua, B, ub, ip_a)
    assert seen_at_a == ip_b, f'the universe on lab-a saw {seen_at_a}, not the universe of lab-b at {ip_b}'
    checks.append(f'a universe of one host reaching a universe of the other is seen with its own allocated address, in both directions ({ip_a} seen at lab-b, {ip_b} seen at lab-a): no source NAT inside the prefix')
    r = A.api(hostwide('network_declare', network_uuid=str(uuid.uuid4()), prefix=PREFIX, pool=POOLS['lab-a']))
    assert not r.get('ok'), 'a second declaration was accepted'
    checks.append('a second declaration is refused (the bridge and the table exist)')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'seen': {'at_lab_b': seen_at_b, 'at_lab_a': seen_at_a},
                      'not_proven': ['traffic between a universe and a host address, or leaving the prefix: still Podman\'s NAT, by design']}, indent=2))
finally:
    for h, u in ((A, ua), (B, ub)):
        h.api(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
        h.api(request('delete', u, reference))
        h.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
        if h in declared:
            r = h.api(hostwide('network_undeclare', network_uuid=NET))
            if not r.get('ok'):
                print(f'{h.role}: undeclare refused: {r.get("error")}', file=sys.stderr)
    for h in (A, B):
        same = routes(h) == initial[h.role]['routes'] and networks(h) == initial[h.role]['networks'] and tables(h) == initial[h.role]['tables']
        print(f'{h.role}: network state restored: {same}', file=sys.stderr)
