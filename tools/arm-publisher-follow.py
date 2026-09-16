#!/usr/bin/env python3
"""Arm laboratory publisher-follow on podmesh-dev-ha: replicas, governor, declared publisher,
installed proof, host-side timer. Does not package, push, or touch podmesh.service.

Environment: same as the manager publisher suites, plus the private Cloudflare files.
"""
import json, os, pathlib, subprocess, sys, tempfile, time, uuid, urllib.request, urllib.error

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tests'))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, replica_create, prove_takeover  # noqa: E402

TOOL = str(ROOT / 'tools' / 'ha-standby.py')
FOLLOW = str(ROOT / 'packaging' / 'podmesh-publisher-follow')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh-dev-ha/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh-dev-ha')
unit = os.environ.get('PODMESH_UNIT', 'podmesh-dev-ha.service')
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
TUNNEL_ID = os.environ['PODMESH_TUNNEL_ID']
HOSTNAME = os.environ['PODMESH_PUBLIC_HOSTNAME']
CREDENTIALS = open(os.environ['PODMESH_TUNNEL_CREDENTIALS'], 'rb').read()
control = tempfile.mkdtemp(prefix='podmesh-pubfollow-')
targets = {'lab-a': os.environ['PODMESH_SOURCE_SSH'], 'lab-b': os.environ['PODMESH_DESTINATION_SSH'], 'lab-c': os.environ['PODMESH_THIRD_SSH']}
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
REFERENCE = 'mandate:lab-publisher-follow'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
LOGICAL = replica_set['logical_manager_id']
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
CREDENTIAL = 'cloudflare-tunnel-podmesh-lab'
TIMER = 'podmesh-publisher-follow-lab'
PROOF_REMOTE = '/run/podmesh-publisher-follow/proof.json'
MANDATE_REMOTE = '/run/podmesh-publisher-follow/mandate'
FOLLOW_REMOTE = '/usr/local/lib/podmesh-publisher-follow-lab/podmesh-publisher-follow'
LEASE, MARGIN = 3600, 30
# The mandate's own life, and how near the lease's end a tick renews.
MANDATE_SECONDS = int(os.environ.get('PODMESH_FOLLOW_MANDATE_SECONDS', 24 * 3600))
RENEW_BELOW = int(os.environ.get('PODMESH_FOLLOW_RENEW_BELOW', 900))
STATE_DIR = pathlib.Path.home() / 'Bureau/REMOTE3/podmesh-lab/cursor/publisher-follow'
STATE_DIR.mkdir(parents=True, exist_ok=True)
G = 'lab-a'


def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', REFERENCE, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)


def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=REFERENCE, **extra)


def pub(operation, alias, **extra):
    return hosts[alias].api(dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=REFERENCE, resource=LOGICAL, **extra))


def detect_cli(h):
    """The CLI beside the daemon this host is actually running -- not whatever sorts last in
    /opt: a tick calling a client from another build fails silently every ten seconds."""
    out = h.ssh('D=$(sudo -n readlink -f /proc/$(systemctl show -p ExecMainPID --value ' + unit + ')/exe); echo "$(dirname "$D")/podmesh"').stdout.decode().strip()
    assert out.endswith('/podmesh') and h.ssh(f'test -x {out}', check=False).returncode == 0, (h.role, out)
    return out


def declare_credential(h):
    h.ssh(f'sudo -n mkdir -p -m 0700 {state_dir}/inbox/secrets && sudo -n install -m 0600 -o root -g root /dev/stdin {state_dir}/inbox/secrets/{CREDENTIAL}', input_bytes=CREDENTIALS)
    r = h.api(hostwide('secret_declare', name=CREDENTIAL, source=CREDENTIAL, replace=True))
    assert r.get('ok'), (h.role, r)


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
        s = h.api(request('activation_status', LOGICAL, REFERENCE))
        seen = max(seen, (s.get('data') or {}).get('highest_epoch_seen') or 0)
    while gate.inspect(LOGICAL)['epoch'] < seen:
        gate.transfer(LOGICAL, gate.inspect(LOGICAL)['epoch'], 'gate-recovery', 'gate-recovery')
    state = {'authority_id': gate.authority_id, 'epoch': gate.inspect(LOGICAL)['epoch']}
    gate.close()
    return state


def key_fields():
    import importlib.util
    spec = importlib.util.spec_from_file_location('ha_standby', TOOL)
    ha = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(ha)
    key = ha.signing_key(gate_state['authority_id'])
    return {'authority_key': ha.public_hex(key)} if key else {}


def public_ready(seconds=90):
    deadline = time.time() + seconds
    last = None
    while time.time() < deadline:
        try:
            req = urllib.request.Request(f'https://{HOSTNAME}/ready', headers={'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) PodMesh-lab-check/1.0'})
            with urllib.request.urlopen(req, timeout=10) as r:
                return r.status, json.loads(r.read().decode())
        except urllib.error.HTTPError as e:
            last = (e.code, e.read().decode()[:200])
        except Exception as e:  # noqa: BLE001
            last = (None, str(e)[:200])
        time.sleep(3)
    return last


def current_proof():
    path = pathlib.Path(LEDGER) / f'{LOGICAL}.current-proof.json'
    if path.is_file():
        return json.loads(path.read_text())
    raise SystemExit(f'no current takeover proof at {path}; rotate first')


def install_follow(h, cli, proof):
    h.ssh(f'sudo -n install -D -m 0755 /dev/stdin {FOLLOW_REMOTE}', input_bytes=open(FOLLOW, 'rb').read())
    h.ssh('sudo -n mkdir -p -m 0700 /run/podmesh-publisher-follow')
    h.ssh(f'sudo -n install -m 0600 /dev/stdin {PROOF_REMOTE}', input_bytes=json.dumps(proof).encode())
    # Bounded: the mandate dies on its own clock, and renewal happens near the lease's end, not
    # every tick. An unbounded renewal would make the holder's lease immortal, and lease expiry
    # is what withdraws a governor nobody can reach (docs/PUBLISHER-FOLLOW-LAB.md).
    mandate = (f'authorization_ref={REFERENCE}\nresource={LOGICAL}\nproof={PROOF_REMOTE}\nrenew=1\n'
               f'not_after={int(time.time()) + MANDATE_SECONDS}\nrenew_below={RENEW_BELOW}\n')
    h.ssh(f'sudo -n install -m 0600 /dev/stdin {MANDATE_REMOTE}', input_bytes=mandate.encode())
    h.ssh(f'sudo -n systemctl stop {TIMER}.timer {TIMER}.service 2>/dev/null; sudo -n systemctl reset-failed {TIMER}.timer {TIMER}.service 2>/dev/null', check=False)
    h.ssh(
        f'sudo -n systemd-run --quiet --unit={TIMER} --on-active=2 --on-unit-active=10 --timer-property=AccuracySec=1s '
        f'--setenv=PODMESH_SOCKET={socket_path} --setenv=PODMESH_CLI={cli} '
        f'--setenv=PODMESH_PUBLISHER_FOLLOW_MANDATE={MANDATE_REMOTE} {FOLLOW_REMOTE}'
    )


def refresh_only():
    global gate_state
    gate_state = gate_ready()
    rot = tool('rotate', '--universe', LOGICAL, '--host', targets[G], '--lease', str(LEASE), '--margin', str(MARGIN), '--standbys', '2')
    for other in ('lab-b', 'lab-c'):
        hosts[other].ok(request('activation_require', LOGICAL, REFERENCE, lease_seconds=LEASE, takeover_margin_seconds=MARGIN, desired_standbys=2, authority_id=rot['permit']['authority_id'], **key_fields()))
    proof, how = prove_takeover(rot, hosts, tool, request, REFERENCE, LOGICAL)
    for a in hosts:
        r = pub('publisher_declare', a, hostname=HOSTNAME, tunnel_uuid=TUNNEL_ID, credential=CREDENTIAL, origin_port=8080)
        assert r.get('ok'), (a, r)
    r = hosts[G].api(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses[G], exclusive_resource=LOGICAL))
    if not r.get('ok'):
        err = r.get('error') or ''
        assert 'already effective' in err and addresses[G] in err, (G, r)
    follow_route('lab-b', G)
    follow_route('lab-c', G)
    clis = {a: detect_cli(h) for a, h in hosts.items()}
    for a, h in hosts.items():
        install_follow(h, clis[a], proof)
    status, body = public_ready(120)
    report = {
        'result': 'PASS' if status == 200 and isinstance(body, dict) and body.get('ready') is True else 'WAIT',
        'mode': 'refresh',
        'hostname': HOSTNAME,
        'http': status,
        'body': body if isinstance(body, dict) else {'raw': body},
        'governor': G,
        'epoch': rot['epoch'],
        'lease_seconds': LEASE,
        'lease_expires_at': rot['lease']['expires_at'],
        'takeover_method': proof.get('method'),
        'takeover_how': how,
        'renew': 1,
        'note': 'replicas kept; role re-acquired under a 1h lease; follow tick renews it on the eligible host',
    }
    path = STATE_DIR / 'refresh.json'
    path.write_text(json.dumps(report, indent=2, sort_keys=True))
    print(json.dumps(report, indent=2, sort_keys=True))
    sys.exit(0 if report['result'] == 'PASS' else 1)


def follow_route(alias, governor):
    r = hosts[alias].api(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=lab_hosts[governor]))
    if r.get('ok'):
        return r
    err = r.get('error') or ''
    assert 'already effective' in err, (alias, r)
    return r


if '--refresh' in sys.argv:
    refresh_only()

gate_state = gate_ready()
universes = {a: str(uuid.uuid4()) for a in hosts}
declared = set()
for a, h in hosts.items():
    cf = h.ssh('test -x /usr/local/bin/cloudflared && echo yes', check=False).stdout.decode().strip()
    assert cf == 'yes', f'{a}: /usr/local/bin/cloudflared missing'
    peers = [{'pool': POOLS[o], 'via': lab_hosts[o]} for o in hosts if o != a]
    h.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[a], peer_pools=peers))
    declared.add(a)
    declare_replica_config(h, a, REFERENCE, state_dir)
    declare_credential(h)
    replica_create(h, universes[a], a, REFERENCE, addresses[a], request)
    started = h.ok(request('start', universes[a], REFERENCE, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', (a, started['application_outcome'])

rot = tool('rotate', '--universe', LOGICAL, '--host', targets[G], '--lease', str(LEASE), '--margin', str(MARGIN))
for other in ('lab-b', 'lab-c'):
    hosts[other].ok(request('activation_require', LOGICAL, REFERENCE, lease_seconds=LEASE, takeover_margin_seconds=MARGIN, desired_standbys=2, authority_id=rot['permit']['authority_id'], **key_fields()))
proof, how = prove_takeover(rot, hosts, tool, request, REFERENCE, LOGICAL)
for a in hosts:
    r = pub('publisher_declare', a, hostname=HOSTNAME, tunnel_uuid=TUNNEL_ID, credential=CREDENTIAL, origin_port=8080)
    assert r.get('ok'), (a, r)
hosts[G].ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses[G], exclusive_resource=LOGICAL))
follow_route('lab-b', G)
follow_route('lab-c', G)

clis = {a: detect_cli(h) for a, h in hosts.items()}
for a, h in hosts.items():
    install_follow(h, clis[a], proof)

status, body = public_ready(120)
report = {
    'result': 'PASS' if status == 200 and body.get('ready') is True else 'WAIT',
    'hostname': HOSTNAME,
    'http': status,
    'body': body if isinstance(body, dict) else {'raw': body},
    'governor': G,
    'epoch': rot['epoch'],
    'takeover_method': proof.get('method'),
    'takeover_how': how,
    'universes': universes,
    'network_uuid': NET,
    'lease_seconds': LEASE,
    'renew': 1,
    'note': 'follow timer armed on all three hosts; eligible host renews the lease each tick; a rotation still needs a new proof dropped by the agent',
}
path = STATE_DIR / 'state.json'
path.write_text(json.dumps(report, indent=2, sort_keys=True))
os.chmod(path, 0o600)
print(json.dumps(report, indent=2, sort_keys=True))
sys.exit(0 if report['result'] == 'PASS' else 1)
