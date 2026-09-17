#!/usr/bin/env python3
"""The readiness gate of `publisher_start`, alone (the publisher contract, item 8): one lab host,
the daemon restarted with the lab fault `publisher-stale-mark` (the governor mark written one epoch
behind), so that the origin answers ready with the wrong epoch; the start must be refused, its mark
compensated (the origin back to 503), no connector unit left; then, without the fault, the same
start succeeds and is stopped. Environment: PODMESH_SOURCE_SSH (the host), the transient service
variables, PODMESH_DAEMON_BINARY (transient unit only), PODMESH_NETWORK_PEER_VIAS, PODMESH_REPLICA_CONFIGS,
PODMESH_REPLICA_SET, PODMESH_TUNNEL_CREDENTIALS, PODMESH_TUNNEL_ID, PODMESH_PUBLIC_HOSTNAME, and
PODMESH_HOST_ALIAS (which replica's configuration this host runs).
"""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, prove_takeover  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
BINARY = os.environ.get('PODMESH_DAEMON_BINARY')  # needed only for a transient unit systemd has already forgotten
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
VIAS = os.environ['PODMESH_NETWORK_PEER_VIAS'].split(',')
ALIAS = os.environ.get('PODMESH_HOST_ALIAS', 'lab-a')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
TUNNEL_ID = os.environ['PODMESH_TUNNEL_ID']
HOSTNAME = os.environ['PODMESH_PUBLIC_HOSTNAME']
CREDENTIALS = open(os.environ['PODMESH_TUNNEL_CREDENTIALS'], 'rb').read()
control = tempfile.mkdtemp(prefix='podmesh-pubready-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-publisher-readiness'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
peers = [{'pool': POOLS[o], 'via': v} for o, v in zip([a for a in POOLS if a != ALIAS], VIAS)]
LOGICAL = replica_set['logical_manager_id']
address = next(r['address'] for r in replica_set['replicas'] if r['alias'] == ALIAS)
CREDENTIAL = 'cloudflare-tunnel-podmesh-lab'
checks = []

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab, pathlib, subprocess  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)

def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', reference, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)

def gate_ready():
    """The durable gate, as every manager suite: the host has seen epochs for this fixed resource, so a
    lease taken under no authority would be refused as superseded."""
    if not os.path.exists(GATE):
        tool('gate', 'init')
    gate = fencing_lab.Authority(pathlib.Path(GATE))
    try:
        gate.inspect(LOGICAL)
    except fencing_lab.Refused:
        gate.declare(LOGICAL)
    s = A.api(request('activation_status', LOGICAL, reference))
    seen = (s.get('data') or {}).get('highest_epoch_seen') or 0
    while gate.inspect(LOGICAL)['epoch'] < seen:
        gate.transfer(LOGICAL, gate.inspect(LOGICAL)['epoch'], 'gate-recovery', 'gate-recovery')
    gate.close()

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def pub(operation, **extra):
    return A.api(dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, resource=LOGICAL, **extra))

def daemon(fault=None):
    # Through the harness: an installed unit gets a runtime drop-in that carries the fault and turns systemd's own
    # restart off; a transient unit is stopped completely and launched again.
    A.call('restart_with_fault', fault=fault, binary=BINARY)
    for _ in range(50):
        if A.ssh(f'sudo -n test -S {socket_path}', check=False).returncode == 0 and A.call('ready', seconds=20).get('ready'):
            return
        time.sleep(0.2)
    raise AssertionError('the daemon did not come up')

def origin():
    r = A.ssh(f'curl -s -m 5 -o /dev/stderr -w "%{{http_code}}" http://{SERVICE}:8080/ready', check=False)
    return r.stdout.decode().strip(), r.stderr.decode()[-200:]

u = str(uuid.uuid4())
declared = False
try:
    daemon()
    A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[ALIAS], peer_pools=peers)); declared = True
    declare_replica_config(A, ALIAS, reference, state_dir)
    A.ssh(f'sudo -n mkdir -p -m 0700 {state_dir}/inbox/secrets && sudo -n install -m 0600 -o root -g root /dev/stdin {state_dir}/inbox/secrets/{CREDENTIAL}', input_bytes=CREDENTIALS)
    A.ok(hostwide('secret_declare', name=CREDENTIAL, source=CREDENTIAL, replace=True))
    replica_create(A, u, ALIAS, reference, address, request)
    started = A.ok(request('start', u, reference, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], A.call('podman_run', args=['logs', 'podmesh-' + u], check=False))
    gate_ready()
    rot = tool('rotate', '--universe', LOGICAL, '--host', os.environ['PODMESH_SOURCE_SSH'], '--lease', '20', '--margin', '5')
    proof, how = prove_takeover(rot, {'host': A}, tool, request, reference, LOGICAL)
    A.ok(request('activation_acquire', LOGICAL, reference, permit=rot['permit']))  # the proof's barrier may outlast a 20-second lease: acquired again, idempotent for the holder
    A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=address, exclusive_resource=LOGICAL))
    assert pub('publisher_declare', hostname=HOSTNAME, tunnel_uuid=TUNNEL_ID, credential=CREDENTIAL, origin_port=8080).get('ok')
    assert origin()[0] == '503'
    checks.append('a governed replica with the service address, the publisher declared, the origin answering 503 without a mark')

    daemon('publisher-stale-mark')
    r = pub('publisher_start', takeover_proof=proof)
    assert not r.get('ok') and 'not ready for this governor at epoch' in r['error'] and '"gone":true' in r['error'].replace(' ', ''), r
    assert origin()[0] == '503', 'the stale mark was not compensated'
    assert A.ssh(f'systemctl is-active podmesh-publisher-{LOGICAL}.service', check=False).stdout.decode().strip() != 'active'
    st = pub('publisher_status')['data']
    assert st['governor_mark'] is False and st['unit']['state'] != 'active', st
    checks.append('under a stale governor mark (one epoch behind), the start was refused by the readiness check, the mark compensated (origin back to 503), no connector unit')

    daemon()
    A.ok(request('activation_renew', LOGICAL, reference))
    assert pub('publisher_start', takeover_proof=proof).get('ok')
    code, body = origin()
    assert code == '200' and '"ready": true' in body, (code, body)
    assert pub('publisher_stop').get('ok') and origin()[0] == '503'
    checks.append('without the fault the same start succeeded (origin ready at the epoch) and the stop took the mark and the connector away')
    print(json.dumps({'result': 'PASS', 'checks': checks}, indent=2))
finally:
    daemon()
    A.api(dict(operation='publisher_stop', operation_id=str(uuid.uuid4()), authorization_ref=reference, resource=LOGICAL))
    A.api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
    A.api(request('stop', u, reference, timeout_seconds=15, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
    remove_replica_config(A, ALIAS, reference)
    A.api(hostwide('secret_remove', name=CREDENTIAL))
    if declared:
        r = A.api(hostwide('network_undeclare', network_uuid=NET))
        if not r.get('ok'):
            print(f'undeclare refused: {r.get("error")}', file=sys.stderr)
    print(f'connector unit: {A.ssh(f"systemctl is-active podmesh-publisher-{LOGICAL}.service", check=False).stdout.decode().strip()}', file=sys.stderr)
