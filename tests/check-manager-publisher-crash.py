#!/usr/bin/env python3
"""Crash and storage-failure safety of the publisher (Codex, P1): a publisher from an operation
reported failed never stays active. One lab host, the daemon restarted under a lab fault at each
point of `publisher_start` -- after the mark, after the connector, before the transition is
recorded effective, and during the compensation -- as a storage failure (refused) or a crash. After
each, from outside: no connector unit active, no governor mark (the origin answers 503), no
transition recorded, and the restart's reconciliation reporting what it withdrew; then a fault-free
start publishes and is stopped. Also: an effective publisher whose lease is superseded is withdrawn
by reconciliation at the next restart.

Environment: as check-manager-publisher-readiness.py (PODMESH_SOURCE_SSH, PODMESH_DAEMON_BINARY,
PODMESH_HOST_ALIAS, the replica set and configs, the tunnel credentials and hostname).
"""
import json, os, pathlib, subprocess, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, prove_takeover  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
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
control = tempfile.mkdtemp(prefix='podmesh-pubcrash-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-publisher-crash'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
peers = [{'pool': POOLS[o], 'via': v} for o, v in zip([a for a in POOLS if a != ALIAS], VIAS)]
LOGICAL = replica_set['logical_manager_id']
address = next(r['address'] for r in replica_set['replicas'] if r['alias'] == ALIAS)
CREDENTIAL = 'cloudflare-tunnel-podmesh-lab'
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
checks = []

def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', reference, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)

def gate_ready():
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
    mark = A.call('restart_with_fault', fault=fault, binary=BINARY)['mark']
    for _ in range(50):
        if A.ssh(f'sudo -n test -S {socket_path}', check=False).returncode == 0 and A.call('ready', seconds=20).get('ready'):
            break
        time.sleep(0.2)
    else:
        raise AssertionError('the daemon did not come up')
    line = A.ssh(f'sudo -n journalctl -u {unit} --since @{mark} --no-pager -o cat | grep "network reconciliation at startup" | tail -1', check=False).stdout.decode().strip()
    return json.loads(line.split(': ', 1)[1]) if line else {}

def api_or_dead(req):
    try:
        r = A.api(req)
    except RuntimeError:
        return None
    return None if r.get('interrupted') else r

def origin():
    return A.ssh(f'curl -s -m 5 -o /dev/null -w "%{{http_code}}" http://{SERVICE}:8080/ready', check=False).stdout.decode().strip()

def unit_active():
    return A.ssh(f'systemctl is-active podmesh-publisher-{LOGICAL}.service', check=False).stdout.decode().strip() == 'active'

def nothing_left(label):
    st = pub('publisher_status')['data']
    assert not unit_active() and st['transition'] is None and st['governor_mark'] is False and origin() == '503', (label, unit_active(), st.get('transition'), st.get('governor_mark'), origin())

u = str(uuid.uuid4())
declared = False
try:
    daemon()
    A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[ALIAS], peer_pools=peers)); declared = True
    declare_replica_config(A, ALIAS, reference, state_dir)
    A.ssh(f'sudo -n mkdir -p -m 0700 {state_dir}/inbox/secrets && sudo -n install -m 0600 -o root -g root /dev/stdin {state_dir}/inbox/secrets/{CREDENTIAL}', input_bytes=CREDENTIALS)
    A.ok(hostwide('secret_declare', name=CREDENTIAL, source=CREDENTIAL))
    replica_create(A, u, ALIAS, reference, address, request)
    started = A.ok(request('start', u, reference, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], A.call('podman_run', args=['logs', 'podmesh-' + u], check=False))
    gate_ready()
    rot = tool('rotate', '--universe', LOGICAL, '--host', os.environ['PODMESH_SOURCE_SSH'], '--lease', '20', '--margin', '5')
    proof, how = prove_takeover(rot, {'host': A}, tool, request, reference, LOGICAL)

    def renew():
        # a 20-second lease under the durable gate, which the proof's barrier or a restart may have
        # let lapse: acquired again with the rotation's permit (idempotent for the holder, the
        # design's answer to a lapsed lease of one's own) before each case, so that the lease gate
        # is not what refuses
        A.ok(request('activation_acquire', LOGICAL, reference, permit=rot['permit']))
    renew()
    A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=address, exclusive_resource=LOGICAL))
    assert pub('publisher_declare', hostname=HOSTNAME, tunnel_uuid=TUNNEL_ID, credential=CREDENTIAL, origin_port=8080).get('ok')
    checks.append(f'a governed replica with the service address and the publisher declared (proof {proof["method"]}, {how})')

    for fault, kind in (('publisher-after-mark', 'failure'), ('publisher-after-mark:crash', 'crash'),
                        ('publisher-after-connector', 'failure'), ('publisher-after-connector:crash', 'crash'),
                        ('publisher-before-effective', 'failure'), ('publisher-before-effective:crash', 'crash'),
                        ('publisher-during-compensation:crash', 'crash')):
        # the last case: a failure after the connector whose compensation then crashes -- two points at once
        rec0 = daemon(fault if not fault.startswith('publisher-during') else f'publisher-after-connector,{fault}')
        renew()
        answer = api_or_dead(dict(operation='publisher_start', operation_id=str(uuid.uuid4()), authorization_ref=reference, resource=LOGICAL, takeover_proof=proof))
        if kind == 'failure':
            assert answer is not None and not answer.get('ok') and 'compensation' in answer['error'], (fault, answer)
            assert not unit_active() and origin() == '503', (fault, 'the failed start left something')
            rec = daemon()
            nothing_left(fault)
            checks.append(f'{fault}: refused with compensation, nothing left, the restart found nothing to withdraw')
        else:
            assert answer is None, (fault, 'the daemon survived a crash fault', answer)
            leftover = {'unit_active': unit_active(), 'origin': origin()}
            rec = daemon()
            renew()
            assert rec.get('publishers'), (fault, 'the restart reported no publisher withdrawn', rec)
            nothing_left(fault)
            checks.append(f'{fault}: the crash left {leftover}; the restart\'s reconciliation withdrew it ({rec["publishers"][0]["why"]}), nothing left')

    # fault-free: published, then the lease superseded -> the next restart withdraws the effective publisher
    renew()
    r = pub('publisher_start', takeover_proof=proof)
    assert r.get('ok') and r['data']['published'] is True and origin() == '200', r
    checks.append('without a fault: published, connector registered, the origin ready')
    gate = fencing_lab.Authority(pathlib.Path(GATE))
    permit = gate.transfer(LOGICAL, gate.inspect(LOGICAL)['epoch'], 'elsewhere', 'elsewhere'); gate.close()
    A.ok(request('activation_supersede', LOGICAL, reference, permit=json.loads(permit.encode())))
    rec = daemon()
    assert rec.get('publishers') and rec['publishers'][0]['why'] == 'no longer entitled', rec
    nothing_left('superseded publisher at restart')
    checks.append('an effective publisher whose lease was superseded: withdrawn by the restart\'s reconciliation, nothing left')
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
    print(f'connector unit active: {unit_active()}', file=sys.stderr)
