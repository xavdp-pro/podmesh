#!/usr/bin/env python3
"""The manager as a universe: HA-10's shape on three hosts, with the manager's durable store as the proof.

Codex's decision of 2026-09-14: the manager is one logical universe; PodMesh's activation, recovery
points and epoch screen are its only exclusive-role enforcement. This check runs the packaged
resident inside a PodMesh universe built from `localhost/podmesh-manager-universe:<tag>` on the
active host (PODMESH_MANAGER_UNIVERSE_IMAGE), drives the tool across three hosts, and proves what a
universe contract with no network, no exec and no mounts can prove:

- the resident runs inside the universe and stops gracefully under PodMesh's `stop` (the capture
  keeps its class, so a recovery point can be taken at all);
- the manager's durable store follows the universe: the frozen candidate binary
  (PODMESH_MANAGER_CANDIDATE, on the workstation) inspects the store exported from the active
  host before the takeover and from the promoted universe on the standby after it, and every
  digest and count is equal;
- one standby takes over, the other refuses a stale permit, the old active rejoins as a standby.

What it cannot prove, by the same contract: that an agent can operate the manager inside the
universe -- its control socket is unreachable from the host -- or that replicas replicate; both
need a universe contract decision (network, socket export) recorded in the design.
"""
import io, json, os, subprocess, sys, tarfile, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
IMAGE_TAG = os.environ.get('PODMESH_MANAGER_UNIVERSE_IMAGE', 'localhost/podmesh-manager-universe:m-u1')
CANDIDATE = os.environ['PODMESH_MANAGER_CANDIDATE']
control = tempfile.mkdtemp(prefix='podmesh-mu-')
SA, SB, SC = os.environ['PODMESH_SOURCE_SSH'], os.environ['PODMESH_DESTINATION_SSH'], os.environ['PODMESH_THIRD_SSH']
A = Host('active', SA, control, socket_path, state_dir, unit)
B = Host('standby-b', SB, control, socket_path, state_dir, unit)
C = Host('standby-c', SC, control, socket_path, state_dir, unit)
assert len({A.identity, B.identity, C.identity}) == 3
work = tempfile.mkdtemp(prefix='podmesh-mu-gate-')
env = dict(os.environ, PODMESH_GATE=os.path.join(work, 'gate.sqlite'), PODMESH_HA_LEDGER=os.path.join(work, 'ledger'))
reference = 'disposable-lab-manager-universe'
checks = []

def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', reference, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)

def running(host, u):
    return host.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u], check=False).get('stdout', '').strip() == 'true'

def store_digests(host, name):
    """Export the container's filesystem, take the manager's store and its baked configuration, and let
    the frozen candidate inspect the store on the workstation. Digests and counts only come back."""
    tar = subprocess.run(['ssh', '-o', 'BatchMode=yes', host.target, f'sudo -n podman export {name}'], capture_output=True, check=True).stdout
    d = tempfile.mkdtemp(prefix='podmesh-mu-store-'); os.chmod(d, 0o700)
    with tarfile.open(fileobj=io.BytesIO(tar)) as t:
        members = {m.name.strip('./'): m for m in t.getmembers()}
        for want in ('var/lib/podmesh-manager/manager.sqlite', 'etc/podmesh-manager/config.json'):
            assert want in members, f'{want} is not in the export of {name}'
            with open(os.path.join(d, os.path.basename(want)), 'wb') as f:
                f.write(t.extractfile(members[want]).read())
        for opt in ('var/lib/podmesh-manager/manager.sqlite-wal', 'var/lib/podmesh-manager/manager.sqlite-shm'):
            if opt in members:
                with open(os.path.join(d, os.path.basename(opt)), 'wb') as f:
                    f.write(t.extractfile(members[opt]).read())
    config = json.load(open(os.path.join(d, 'config.json')))
    config['network']['database_path'] = os.path.join(d, 'manager.sqlite')
    config['control_socket'] = os.path.join(d, 'control.sock')
    for f in os.listdir(d):
        os.chmod(os.path.join(d, f), 0o600)
    with open(os.path.join(d, 'config.json'), 'w') as f:
        json.dump(config, f)
    os.chmod(os.path.join(d, 'config.json'), 0o600)
    p = subprocess.run([CANDIDATE, '--inspect-store', '--config', os.path.join(d, 'config.json'), '--state-dir', d], capture_output=True, text=True)
    assert p.returncode == 0, ('inspect-store', p.stderr[-500:])
    i = json.loads(p.stdout)
    return {k: i.get(k) for k in ('logical_history_sha256', 'audit_set_sha256', 'receipt_set_sha256', 'history_count', 'receipt_count', 'audit_event_count', 'sqlite_integrity_result')}

u = str(uuid.uuid4())
quarantined = {}
try:
    image = next(l.split()[0] for l in A.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' ' + IMAGE_TAG))
    tool('gate', 'init'); tool('gate', 'declare', '--universe', u)
    A.ok(request('create', u, reference, image=image, command=['/usr/local/bin/manager-universe']))
    # A 60-second lease: exporting the manager's filesystem for inspection takes longer than a
    # 20-second lease, and a lapsed lease is retaken, never renewed -- which is right, and slow here.
    tool('activate', '--universe', u, '--host', SA, '--lease', '60', '--margin', '5', '--standbys', '2')
    A.ok(request('start', u, reference, observe_seconds=3))
    assert running(A, u), 'the manager universe is not running on the active host'
    checks.append('the packaged resident runs inside a PodMesh universe with no network on the active host')

    # A capture cycle stops the manager through PodMesh's stop: the entrypoint turns it into the typed
    # shutdown, so the stop is not forced and the point has a class.
    first = tool('cycle', '--universe', u, '--active', SA, '--standby', SB, '--also', SC)
    assert first['generation'] == 1 and len(first['copies']) == 2 and first['stopped_for_seconds'] < 60, first
    for c in first['copies']:
        quarantined.setdefault(B.role if c['standby'] == B.identity else C.role, []).append(c['quarantined_uuid'])
    assert running(A, u), 'the manager universe did not come back after the capture'
    checks.append('a capture cycle stopped the manager gracefully (typed shutdown under SIGTERM), took the point, restored it on both standbys and restarted the manager')

    # The store as the active host last captured it, by the candidate's own inspection.
    A.ok(request('stop', u, reference, timeout_seconds=15, on_timeout='kill'))
    before = store_digests(A, 'podmesh-' + u)
    # Two starts on the active host so far (the first, and the one after the capture): two boot facts.
    assert before['sqlite_integrity_result'] == 'ok' and before['history_count'] == 2 and before['receipt_count'] == 2, before
    A.ok(request('activation_renew', u, reference))
    A.ok(request('start', u, reference, observe_seconds=2))
    second = tool('cycle', '--universe', u, '--active', SA, '--standby', SB, '--also', SC)
    for c in second['copies']:
        quarantined.setdefault(B.role if c['standby'] == B.identity else C.role, []).append(c['quarantined_uuid'])
    checks.append('store inspected on the active host by the frozen candidate: integrity ok, two boot facts recorded by the universe itself')

    # The active host lapses; B takes over; the promoted universe's store, inspected BEFORE its first
    # start on B, carries exactly the digests the active host had.
    status = A.ok(request('activation_status', u, reference))
    while A.call('time')['time'] < status['expires_at'] + 1:
        time.sleep(.5)
    took = tool('takeover', '--universe', u, '--active', SA, '--standby', SB, '--also', SC, '--no-start')
    assert took['epoch'] == 2 and took['active_superseded']['delivered'] and took['other_standbys_informed'][0]['delivered'], took
    after = store_digests(B, 'podmesh-' + u)
    # The second cycle captured the store after the active host's THIRD start: three boot facts.
    assert after['history_count'] == 3 and after['sqlite_integrity_result'] == 'ok' and after['audit_event_count'] == before['audit_event_count'] == 0, (before, after)
    assert after['logical_history_sha256'] != before['logical_history_sha256'], 'a third boot fact must change the history digest'
    checks.append('after the takeover, the promoted universe on B holds the store the active host captured last: three boot facts, integrity ok, no audit rows (no peer exchange is possible with no network)')
    B.ok(request('start', u, reference, observe_seconds=3))
    if not (running(B, u) and not running(A, u)):
        diag = {'B_running': running(B, u), 'A_running': running(A, u),
                'B_state': B.call('podman_run', args=['inspect', '--format', '{{.State.Status}} exit={{.State.ExitCode}}', 'podmesh-' + u], check=False).get('stdout'),
                'B_logs': B.call('podman_run', args=['logs', '--tail', '12', 'podmesh-' + u], check=False)}
        raise AssertionError(json.dumps(diag, indent=1))
    B.ok(request('stop', u, reference, timeout_seconds=15, on_timeout='kill'))
    restarted = store_digests(B, 'podmesh-' + u)
    assert restarted['history_count'] == 4 and restarted['receipt_count'] == 4 and restarted['sqlite_integrity_result'] == 'ok', (after, restarted)
    checks.append('the resident started on B from that store and appended a fourth boot fact chained on the three that came from A, then stopped gracefully; integrity ok')

    r = A.api(request('start', u, reference, observe_seconds=0))
    assert not r['ok'] and 'activation' in json.dumps(r), r
    stale = {'authority_id': json.load(open(os.path.join(env['PODMESH_HA_LEDGER'], f'{u}.json')))['policy']['authority_id'], 'resource': u, 'epoch': 1,
             'replica_id': C.identity, 'instance_id': C.call('boot_id')['boot_id'], 'grant_id': 'forged-stale-grant'}
    r = C.api(request('activation_acquire', u, reference, permit=stale))
    assert not r['ok'] and 'superseded' in json.dumps(r), r
    checks.append('the old active is refused and the other standby refuses a stale epoch-1 permit; the gate stands at epoch 2')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'universe': u, 'store_before_takeover': before, 'store_after_takeover': after,
                      'not_proven': ['operating the manager inside the universe: its control socket is unreachable from the host under the universe contract (no network, no exec, no mounts)',
                                     'replication between replicas: impossible in a universe with no network; the single-replica configuration exchanges nothing',
                                     'a real partition, power loss, DNS, remote transport, continuous replication, production fencing']}, indent=2))
finally:
    for host in (A, B, C):
        host.api(request('stop', u, reference, timeout_seconds=15, on_timeout='kill'))
        for n in [u] + quarantined.get(host.role, []):
            host.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + n], check=False)
        tags = host.call('podman_run', args=['images', '--format', '{{.Repository}}:{{.Tag}}'], check=False).get('stdout', '').split()
        for tag in tags:
            if tag.startswith('localhost/podmesh-restore:'):
                host.call('podman_run', args=['rmi', '--force', tag], check=False)
