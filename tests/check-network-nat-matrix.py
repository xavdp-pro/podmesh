#!/usr/bin/env python3
"""The source-NAT exemption's matrix (Codex's finding B3), on two hosts (PODMESH_SOURCE_SSH and
PODMESH_DESTINATION_SSH, PODMESH_LAB_HOSTS naming their on-link addresses). For each backend --
`null-snat` (the default: a null source NAT of the local pool's traffic to the prefix, in a nat
chain before netavark's, conntrack kept) and `notrack` (prefix-to-prefix traffic untracked) --
the network is declared on both hosts with one universe each, and measured from inside the
universes' namespaces with the hosts' own python through `nsenter -n`:

- TCP and UDP, both directions: the peer seen at the destination universe is the source
  universe's own address (no source NAT inside the prefix);
- a stateful firewall on the destination host that drops untracked and invalid forwarded
  traffic: passes under `null-snat`, blocks under `notrack` -- the consequence Codex named,
  measured rather than described;
- external destinations keep Podman's NAT: a universe reaching the other HOST's address is seen
  there as its host;
- host-address traffic: a host reaching the other host's universe is seen there as the host;
- reconciliation keeps the table: after a fence (which reconciles), the rules are intact and
  nothing is reported as drift; the undeclaration removes the table.

The firewall table is the suite's own, with a dead man's switch, removed at cleanup.
"""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
control = tempfile.mkdtemp(prefix='podmesh-natm-')
A = Host('lab-a', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('lab-b', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-nat-matrix'
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24'}
FIREWALL = 'podmesh-lab-stateful'
checks, report = [], {}

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def routes(h):
    return h.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def tables(h):
    return sorted(l.strip() for l in h.ssh('sudo -n nft list tables', check=False).stdout.decode().splitlines() if l.strip())

def image(h):
    return next(l.split()[0] for l in h.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' docker.io/library/alpine:3.22'))

def pid(h, u):
    return h.call('podman_run', args=['inspect', '--format', '{{.State.Pid}}', 'podmesh-' + u])['stdout'].strip()

def address(h, u):
    return json.loads(h.call('podman_run', args=['inspect', 'podmesh-' + u])['stdout'])[0]['NetworkSettings']['Networks']['podmesh-managed']['IPAddress']

LISTEN_TCP = 'import socket,sys;s=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);s.bind(("0.0.0.0",int(sys.argv[2])));s.listen(1);s.settimeout(12);c,a=s.accept();c.sendall(b"ok");open(sys.argv[1],"w").write(a[0])'
LISTEN_UDP = 'import socket,sys;s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.bind(("0.0.0.0",int(sys.argv[2])));s.settimeout(12);d,a=s.recvfrom(64);s.sendto(b"ok",a);open(sys.argv[1],"w").write(a[0])'
CLIENT_TCP = 'import socket,sys;s=socket.socket();s.settimeout(5);s.connect((sys.argv[1],int(sys.argv[2])));print("reply" if s.recv(2)==b"ok" else "no-reply")'
CLIENT_UDP = 'import socket,sys;s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.settimeout(5);s.sendto(b"hi",(sys.argv[1],int(sys.argv[2])));print("reply" if s.recvfrom(64)[0]==b"ok" else "no-reply")'

PROGRAMS = {'listen-tcp': LISTEN_TCP, 'listen-udp': LISTEN_UDP, 'client-tcp': CLIENT_TCP, 'client-udp': CLIENT_UDP}

def install_programs(h):
    """The four small programs as files on the host, so that no quoting crosses ssh, sh and nsenter."""
    for name, prog in PROGRAMS.items():
        h.ssh(f'sudo -n install -m 0644 /dev/stdin /tmp/podmesh-natm-{name}.py', input_bytes=prog.encode())

def remove_programs(h):
    h.ssh('sudo -n rm -f /tmp/podmesh-natm-*.py', check=False)

def exchange(proto, listener, lpid, client, cpid, dst, port):
    """Listener in a namespace (or on the host when lpid is None), client in a namespace (or on the
    host when cpid is None): the peer address the listener saw and whether the client got a reply."""
    marker = f'/tmp/podmesh-natm-{uuid.uuid4()}'
    ns = f'nsenter -t {lpid} -n -- ' if lpid else ''
    listener.ssh(f"sudo -n sh -c 'nohup {ns}python3 /tmp/podmesh-natm-listen-{proto}.py {marker} {port} >/dev/null 2>&1 &'")
    time.sleep(1)
    cns = f'sudo -n nsenter -t {cpid} -n -- ' if cpid else 'sudo -n '
    r = client.ssh(f'{cns}python3 /tmp/podmesh-natm-client-{proto}.py {dst} {port}', check=False)
    got = r.stdout.decode().strip() or ('failed: ' + r.stderr.decode().strip()[-120:])
    time.sleep(1)
    peer = listener.ssh(f'sudo -n cat {marker} 2>/dev/null; sudo -n rm -f {marker}', check=False).stdout.decode().strip()
    listener.ssh(f'sudo -n pkill -f "{marker}" 2>/dev/null', check=False)
    return got, peer

def firewall(h, on):
    """On the destination host: forwarded traffic that conntrack calls untracked or invalid is dropped."""
    if on:
        h.ssh(f'sudo -n nft add table inet {FIREWALL} && sudo -n nft add chain inet {FIREWALL} forward "{{ type filter hook forward priority 10; }}" '
              f'&& sudo -n nft add rule inet {FIREWALL} forward ct state untracked,invalid drop')
        h.ssh(f'sudo -n systemd-run --quiet --on-active=600 --unit=podmesh-lab-stateful-deadman nft delete table inet {FIREWALL}', check=False)
    else:
        h.ssh(f'sudo -n nft delete table inet {FIREWALL}', check=False)
        h.ssh('sudo -n systemctl stop podmesh-lab-stateful-deadman.timer podmesh-lab-stateful-deadman.service 2>/dev/null; sudo -n systemctl reset-failed podmesh-lab-stateful-deadman.service 2>/dev/null', check=False)

initial = {h.role: {'routes': routes(h), 'tables': tables(h)} for h in (A, B)}
assert f'table inet {FIREWALL}' not in initial['lab-b']['tables']
install_programs(A); install_programs(B)
try:
    for backend, expect_stateful in (('null-snat', 'reply'), ('notrack', 'blocked')):
        net = str(uuid.uuid4()); ua, ub = str(uuid.uuid4()), str(uuid.uuid4())
        outcome = {}
        try:
            d = A.ok(hostwide('network_declare', network_uuid=net, prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=[{'pool': POOLS['lab-b'], 'via': lab_hosts['lab-b']}], nat_exemption=backend))
            assert d['nat_exemption']['backend'] == backend and d['nat_exemption']['present'] is True and d['nat_exemption']['rules'], d['nat_exemption']
            B.ok(hostwide('network_declare', network_uuid=net, prefix=PREFIX, pool=POOLS['lab-b'], peer_pools=[{'pool': POOLS['lab-a'], 'via': lab_hosts['lab-a']}], nat_exemption=backend))
            for h, u in ((A, ua), (B, ub)):
                h.ok(request('create', u, reference, image=image(h), command=['sleep', '600'], network_profile='managed'))
                h.ok(request('start', u, reference, observe_seconds=1))
            ip_a, ip_b = address(A, ua), address(B, ub)
            pa, pb = pid(A, ua), pid(B, ub)
            outcome['rules'] = d['nat_exemption']['rules']
            # inside the prefix, both protocols, both directions: the universe's own address, and a reply
            for proto, port in (('tcp', 9000), ('udp', 9001)):
                got_b, peer_b = exchange(proto, B, pb, A, pa, ip_b, port)
                got_a, peer_a = exchange(proto, A, pa, B, pb, ip_a, port)
                assert got_b == 'reply' and got_a == 'reply' and peer_b == ip_a and peer_a == ip_b, (backend, proto, got_b, peer_b, got_a, peer_a)
                outcome[proto] = {'a_to_b_seen': peer_b, 'b_to_a_seen': peer_a}
            checks.append(f'{backend}: TCP and UDP exchanges inside the prefix, both directions, each universe seen with its own address and answered')
            # a stateful firewall on the destination host
            firewall(B, True)
            got, peer = exchange('tcp', B, pb, A, pa, ip_b, 9000)
            outcome['stateful_firewall'] = got
            if expect_stateful == 'reply':
                assert got == 'reply' and peer == ip_a, (backend, got, peer)
                checks.append(f'{backend}: a stateful firewall dropping untracked and invalid forwarded traffic lets the exchange through: conntrack is kept')
            else:
                assert got != 'reply', (backend, got)
                checks.append(f'{backend}: the same stateful firewall blocks the exchange: the traffic is untracked, the consequence of this backend')
            firewall(B, False)
            # external destination: the other host's own address, seen as this host (Podman's NAT kept)
            got, peer = exchange('tcp', B, None, A, pa, lab_hosts['lab-b'], 9002)
            assert got == 'reply' and peer == lab_hosts['lab-a'], (backend, got, peer)
            outcome['to_host_seen'] = peer
            checks.append(f'{backend}: a universe reaching the other host\'s address is seen there as its host: Podman\'s NAT outside the prefix is kept')
            # host-address traffic to a universe
            got, peer = exchange('tcp', A, pa, B, None, ip_a, 9003)
            assert got == 'reply' and peer == lab_hosts['lab-b'], (backend, got, peer)
            outcome['from_host_seen'] = peer
            checks.append(f'{backend}: a host reaching the other host\'s universe is seen there as the host')
            # reconciliation keeps the table
            fence = A.ok({'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference, 'timeout_seconds': 5})
            assert fence['network_reconciliation']['drift'] == [] and fence['network_reconciliation']['remaining'] == [], fence['network_reconciliation']
            st = A.ok(hostwide('network_status'))
            assert st['nat_exemption']['rules'] == d['nat_exemption']['rules'] and st['incomplete_effects'] == 0, st['nat_exemption']
            checks.append(f'{backend}: after a fence\'s reconciliation the table and its rules are intact, nothing drifted')
        finally:
            firewall(B, False)
            for h, u in ((A, ua), (B, ub)):
                h.api(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
                h.api(request('delete', u, reference))
                h.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
                r = h.api(hostwide('network_undeclare', network_uuid=net))
                if not r.get('ok'):
                    print(f'{h.role}: undeclare refused: {r.get("error")}', file=sys.stderr)
            assert 'table inet podmesh-managed' not in tables(A) and 'table inet podmesh-managed' not in tables(B)
        report[backend] = outcome
    checks.append('the undeclaration removed the table on both hosts, for both backends')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'report': report,
                      'not_proven': ['a NAT backend other than netavark\'s nftables driver', 'traffic leaving the prefix to the Internet (the lab hosts have no route to prove it beyond the LAN)']}, indent=2))
finally:
    remove_programs(A); remove_programs(B)
    for h in (A, B):
        same = routes(h) == initial[h.role]['routes'] and tables(h) == initial[h.role]['tables']
        print(f'{h.role}: network state restored: {same}', file=sys.stderr)
