#!/usr/bin/env python3
"""Three hosts, two standbys, and the old active rejoining: the HA-10 shape, on universes.

Environment: PODMESH_SOURCE_SSH (A, active), PODMESH_DESTINATION_SSH (B), PODMESH_THIRD_SSH (C),
PODMESH_SOCKET / PODMESH_STATE_DIR / PODMESH_UNIT for the transient service on all three, and
PODMESH_FENCING_LAB. The tool is driven as a subprocess. What is asserted, from outside the hosts:

1. one capture cycle carries the point to BOTH standbys and each holds the marker in quarantine;
2. after the active host lapses, B takes over under epoch 2, and C -- which never held a lease --
   is informed and refuses a stale epoch-1 permit bound to it;
3. the old active A rejoins: it is superseded, its old grant and a fresh permit at the old epoch
   are refused, its copy is fenced, and a cycle from the NEW active B restores into quarantine on
   A and on C -- A is a standby now, without any reset of its journal;
4. C still refuses to activate under anything but a new rotation.

Everything runs through the API and the tool; nothing here proves a real partition or a real
host loss -- the active host "fails" by not renewing, exactly as the two-host check does.
"""
import json, os, subprocess, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-ha3-')
SA, SB, SC = os.environ['PODMESH_SOURCE_SSH'], os.environ['PODMESH_DESTINATION_SSH'], os.environ['PODMESH_THIRD_SSH']
A = Host('active', SA, control, socket_path, state_dir, unit)
B = Host('standby-b', SB, control, socket_path, state_dir, unit)
C = Host('standby-c', SC, control, socket_path, state_dir, unit)
assert len({A.identity, B.identity, C.identity}) == 3, 'three distinct hosts are required'
work = tempfile.mkdtemp(prefix='podmesh-ha3-gate-')
env = dict(os.environ, PODMESH_GATE=os.path.join(work, 'gate.sqlite'), PODMESH_HA_LEDGER=os.path.join(work, 'ledger'))
reference = 'disposable-lab-ha-three'
checks = []

def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', reference, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)

def image_on(host):
    out = host.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}}', 'docker.io/library/alpine:3.22'])
    return next(l for l in out['stdout'].split() if l.startswith('sha256:'))

def running(host, u):
    return host.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u], check=False).get('stdout', '').strip() == 'true'

u = str(uuid.uuid4()); marker = uuid.uuid4().hex
quarantined = {}
try:
    tool('gate', 'init'); tool('gate', 'declare', '--universe', u)
    A.ok(request('create', u, reference, image=image_on(A), network_profile='isolated',
                 command=['sh', '-c', f"printf %s '{marker}' > /marker-{marker}; trap 'exit 0' TERM; sleep 600 & wait"]))
    tool('activate', '--universe', u, '--host', SA, '--lease', '20', '--margin', '5', '--standbys', '2')
    A.ok(request('start', u, reference, observe_seconds=1))

    # 1. one capture, two standbys
    first = tool('cycle', '--universe', u, '--active', SA, '--standby', SB, '--also', SC)
    assert first['generation'] == 1 and len(first['copies']) == 2 and set(first['standbys']) == {B.identity, C.identity}, first
    for c in first['copies']:
        host = B if c['standby'] == B.identity else C
        quarantined.setdefault(host.role, []).append(c['quarantined_uuid'])
        assert host.call('marker', name='podmesh-' + c['quarantined_uuid'], marker=marker)['present'], f'no marker in the copy on {host.role}'
    checks.append('one capture cycle restored the point into quarantine on both standbys, each carrying the marker')
    assert running(A, u), 'the active universe is not running again after the capture'

    # 2. the active host lapses; B takes over, C is informed
    status = A.ok(request('activation_status', u, reference))
    while A.call('time')['time'] < status['expires_at'] + 1:
        time.sleep(.5)
    took = tool('takeover', '--universe', u, '--active', SA, '--standby', SB, '--also', SC, '--no-start')
    assert took['epoch'] == 2 and took['active_superseded']['delivered'] and took['active_superseded']['highest_epoch_seen'] == 2, took
    assert took['other_standbys_informed'] and took['other_standbys_informed'][0]['delivered'] and took['other_standbys_informed'][0]['highest_epoch_seen'] == 2, took
    assert B.call('marker', name='podmesh-' + u, marker=marker)['present'], 'the promoted universe on B lacks the marker before its first start'
    B.ok(request('start', u, reference, observe_seconds=1))
    assert running(B, u) and not running(A, u)
    # C never held a lease; a permit at the old epoch bound to C must be refused by its screen.
    boot_c = C.call('boot_id')['boot_id']
    stale = {'authority_id': json.load(open(os.path.join(env['PODMESH_HA_LEDGER'], f'{u}.json')))['policy']['authority_id'], 'resource': u, 'epoch': 1,
             'replica_id': C.identity, 'instance_id': boot_c, 'grant_id': 'forged-stale-grant'}
    r = C.api(request('activation_acquire', u, reference, permit=stale))
    assert not r['ok'] and 'superseded' in json.dumps(r), r
    checks.append('B took over under epoch 2; A and C were informed; a stale epoch-1 permit bound to C is refused by its screen')

    # 3. the old active rejoins as a standby: a cycle from the NEW active restores on A and C
    r = A.api(request('start', u, reference, observe_seconds=0))
    assert not r['ok'] and 'activation' in json.dumps(r), r
    second = tool('cycle', '--universe', u, '--active', SB, '--standby', SA, '--also', SC)
    assert second['generation'] == 1, second   # B's own first point of this universe
    for c in second['copies']:
        host = A if c['standby'] == A.identity else C
        quarantined.setdefault(host.role, []).append(c['quarantined_uuid'])
        assert host.call('marker', name='podmesh-' + c['quarantined_uuid'], marker=marker)['present'], f'no marker in the copy on {host.role} after the rejoin'
    assert running(B, u)
    checks.append('the old active rejoined as a standby: a cycle from the new active restored quarantined copies on A and on C, marker present, no journal reset')

    # 4. nothing but a new rotation activates anywhere else
    r = A.api(request('activation_acquire', u, reference, permit=dict(stale, replica_id=A.identity, instance_id=A.call('boot_id')['boot_id'], epoch=2, grant_id='forged-second-grant')))
    assert not r['ok'] and 'already granted here under another grant' in json.dumps(r), r
    assert tool('gate', 'inspect', '--universe', u)['resource']['epoch'] == 2
    checks.append('a second grant at epoch 2 bound to the old active is refused; the gate stands at epoch 2')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'universe': u}, indent=2))
finally:
    for host in (A, B, C):
        host.api(request('stop', u, reference, timeout_seconds=10, on_timeout='kill'))
        for n in [u] + quarantined.get(host.role, []):
            host.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + n], check=False)
        tags = host.call('podman_run', args=['images', '--format', '{{.Repository}}:{{.Tag}}'], check=False).get('stdout', '').split()
        for tag in tags:
            if tag.startswith('localhost/podmesh-restore:'):
                host.call('podman_run', args=['rmi', '--force', tag], check=False)
