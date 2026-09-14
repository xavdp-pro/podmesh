#!/usr/bin/env python3
"""Level 2 across two hosts: warm standby from a recovery point, with the suite as the agent.

Same environment as check-migration-destination.py: PODMESH_SOURCE_SSH is the active host A,
PODMESH_DESTINATION_SSH the standby B, and PODMESH_SOCKET / PODMESH_STATE_DIR / PODMESH_UNIT name
the transient development service on both. Every product mutation goes through the API; the
suite moves bytes between an outbox and an inbox as the transport controller, and it plays the
part the design gives the agent: it decides WHEN the standby's wait begins and it waits.

The sequence, as UNIVERSE-HIGH-AVAILABILITY.md states it: A runs the universe under a lease;
A captures a recovery point (stop, prepare, start again); the point is carried to B; B restores
it into quarantine; A "fails" -- it stops renewing; A self-fences once its lease has lapsed;
the agent waits the takeover margin measured on A's clock; the gate rotates the epoch to B; B acquires,
promotes and starts; A, back, is superseded and refused.

What this proves and what it does not is printed as one JSON report. The lease on B is B's own
journal's: nothing here proves mutual exclusion, and the wait is the agent's obligation, not a
rule any host enforced.
"""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request, transfer  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-two-hosts-')
A = Host('active', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('standby', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
assert A.identity != B.identity, 'the two targets are the same host'

LEASE, MARGIN = 20, 5
reference = 'disposable-lab-ha'
# The epoch gate is the fencing laboratory's trusted fixture, and here the suite plays it: it
# mints one permit per rotation. PodMesh never sees the gate; it sees permits.
AUTHORITY = 'gate-' + uuid.uuid4().hex[:12]
def permit(host, resource, epoch):
    boot = host.call('boot_id')['boot_id']
    return {'authority_id': AUTHORITY, 'resource': resource, 'epoch': epoch,
            'replica_id': host.identity, 'instance_id': boot, 'grant_id': 'grant-' + uuid.uuid4().hex[:12]}
checks, report = [], {'suite': 'check-recovery-point-two-hosts', 'hosts': {'active': A.identity, 'standby': B.identity}}
u = str(uuid.uuid4())            # the universe's own identity, on both hosts
q = str(uuid.uuid4())            # the quarantined copy on B
marker = uuid.uuid4().hex
point = None

def image_on(host):
    out = host.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}}', 'docker.io/library/alpine:3.22'])
    ids = [l for l in out.get('stdout', '').split() if l.startswith('sha256:')] if isinstance(out, dict) else []
    assert ids, ('no alpine 3.22 as root on', host.role, out)
    return ids[0]

def refused(host, req, expected, label):
    return host.refused(req, label, expected, checks)

def wait_on(host, until, label):
    """Wait until host's OWN clock passes `until`: the margin is a contract in A's time, not ours."""
    while host.call('time')['time'] < until:
        time.sleep(.5)
    checks.append(f'[{host.role}] waited until {label} on its own clock')

try:
    image = image_on(A)
    assert image == image_on(B), 'the two hosts do not hold the same Alpine 3.22 image (identity check)'
    report['image'] = image

    # --- A runs the universe under a lease, and writes the marker while running.
    A.ok(request('create', u, reference, image=image,
                 command=['sh', '-c', f"printf %s '{marker}' > /marker-{marker}; trap 'exit 0' TERM; sleep 600 & wait"]),
         'created', checks)
    A.ok(request('activation_require', u, reference, lease_seconds=LEASE, takeover_margin_seconds=MARGIN, desired_standbys=1,
                 eligible_hosts=[A.identity, B.identity], authority_id=AUTHORITY), 'policy declared: one standby among the two hosts, under the gate', checks)
    refused(A, request('start', u, reference, observe_seconds=0), 'none is held', 'start before any lease')
    refused(A, request('activation_acquire', u, reference), 'requires a permit', 'acquire without a permit under the gate')
    refused(A, request('activation_acquire', u, reference, permit=permit(B, u, 1)), 'bound to another replica', 'acquire with the standby\'s permit')
    epoch1 = permit(A, u, 1)
    lease = A.ok(request('activation_acquire', u, reference, permit=epoch1), 'lease acquired under epoch 1', checks)
    assert lease['epoch'] == 1 and lease['highest_epoch_seen'] == 1, lease
    A.ok(request('start', u, reference, observe_seconds=1), 'started under the lease', checks)

    # --- A captures a recovery point: stop, prepare, start again under a renewed lease.
    stopped = A.ok(request('stop', u, reference, timeout_seconds=10, on_timeout='kill'), 'stopped for capture', checks)
    assert stopped.get('forced') is False, stopped
    prepared = A.ok(request('recovery_point_prepare', u, reference), 'recovery point prepared', checks)
    point = prepared['recovery_point_uuid']
    assert prepared['signed'] is False and prepared['state'] == 'prepared', prepared
    report['recovery_point'] = {k: prepared[k] for k in ('recovery_point_uuid', 'generation', 'rootfs_sha256', 'manifest_sha256', 'rootfs_bytes')}
    A.ok(request('activation_renew', u, reference), 'lease renewed after the capture', checks)
    A.ok(request('start', u, reference, observe_seconds=1), 'started again after the capture', checks)

    # --- Transport: the controller's job, and every byte is compared on both sides.
    carried = transfer(A, B, point, files=('recovery-point-manifest.json', 'rootfs.tar'))
    assert carried['files']['rootfs.tar']['sha256'] == prepared['rootfs_sha256'], carried
    checks.append('[transport] outbox carried to the standby inbox, digests equal on both sides')

    # --- B restores into quarantine, and the marker is there before anything is started on B.
    refused(B, request('recovery_point_restore', u, reference, recovery_point_uuid=point),
            'must not reuse the source universe', 'restore into the source identity on the standby')
    restored = B.ok(request('recovery_point_restore', q, reference, recovery_point_uuid=point), 'restored into quarantine', checks)
    assert restored['quarantined'] and not restored['started'] and restored['manifest_signed'] is False, restored
    assert restored['rootfs_sha256'] == prepared['rootfs_sha256'], restored
    seen = B.call('marker', name='podmesh-' + q, marker=marker)
    assert seen['present'], f'the quarantined copy on the standby does not carry the marker: {seen}'
    checks.append('[standby] marker written on the active host found in the quarantined copy before any start')
    refused(B, request('recovery_point_promote', u, reference, restored_universe_uuid=q),
            'under no activation policy', 'promote before the standby has a policy')

    # --- A fails: it stops renewing. Its lease lapses on its own clock; then it self-fences.
    status = A.ok(request('activation_status', u, reference), 'status read before the failure', checks)
    expires = status['expires_at']
    assert status['live'] and status['holder_host_uuid'] == A.identity, status
    report['active_lease'] = {'generation': status['generation'], 'expires_at': expires, 'margin': MARGIN}
    wait_on(A, expires + 1, 'its lease lapsed')
    refused(A, request('activation_renew', u, reference), 'has expired', 'renew a lapsed lease')
    # The fence is host-wide: it names no universe.
    fenced = A.ok({'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference,
                   'timeout_seconds': 10}, 'self-fence run after the lapse', checks)
    hit = {e['universe_uuid']: e for e in fenced['fenced']}
    assert u in hit and hit[u]['forced'] is False, f'the active host did not fence the universe: {fenced}'
    state_a = A.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u])
    assert state_a.get('stdout', '').strip() == 'false', state_a
    checks.append('[active] universe stopped by the self-fence, without escalation')
    refused(A, request('start', u, reference, observe_seconds=0), 'expired', 'start on the active host after the lapse')

    # --- The agent waits the takeover margin, measured on A's clock, before the standby acts.
    wait_on(A, expires + MARGIN + 1, 'the takeover margin passed')

    # --- B takes over: policy, lease, promotion, start.
    B.ok(request('activation_require', u, reference, lease_seconds=LEASE, takeover_margin_seconds=MARGIN, desired_standbys=1,
                 eligible_hosts=[A.identity, B.identity], authority_id=AUTHORITY), 'policy declared on the standby, under the gate', checks)
    refused(B, request('recovery_point_promote', u, reference, restored_universe_uuid=q), 'none is held', 'promote before the lease')
    # The rotation: the gate (the suite) issues epoch 2 to the standby. The active host's old
    # grant cannot be reused there: it is bound to the other replica.
    refused(B, request('activation_acquire', u, reference, permit=epoch1), 'bound to another replica', 'standby acquiring with the active host\'s permit')
    epoch2 = permit(B, u, 2)
    taken = B.ok(request('activation_acquire', u, reference, permit=epoch2), 'lease acquired on the standby under epoch 2', checks)
    assert taken['epoch'] == 2, taken
    promoted = B.ok(request('recovery_point_promote', u, reference, restored_universe_uuid=q), 'promoted into the universe identity', checks)
    assert promoted['universe_uuid'] == u and promoted['restored_universe_uuid'] == q and not promoted['started'], promoted
    assert 'not mutual exclusion' in promoted['scope'], promoted
    seen = B.call('marker', name='podmesh-' + u, marker=marker)
    assert seen['present'], f'the promoted universe does not carry the marker: {seen}'
    checks.append('[standby] marker found in the promoted universe before its first start')
    B.ok(request('start', u, reference, observe_seconds=1), 'started on the standby under its lease', checks)
    state_b = B.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u])
    assert state_b.get('stdout', '').strip() == 'true', state_b
    checks.append('[standby] universe running on the standby')

    # --- The active host returns. The agent delivers the standby's grant to it: its screen
    # advances, its old grant is refused as superseded, and even a fresh permit at the old
    # epoch is refused. Only a NEW rotation could bring it back.
    over = A.ok(request('activation_supersede', u, reference, permit=epoch2), 'supersession delivered to the active host', checks)
    assert over['highest_epoch_seen'] == 2 and over['superseded'] is True, over
    refused(A, request('activation_acquire', u, reference, permit=epoch1), 'is superseded', 'active host re-acquiring under its old grant')
    refused(A, request('activation_acquire', u, reference, permit=permit(A, u, 1)), 'is superseded', 'active host acquiring a fresh permit at the old epoch')
    refused(A, request('activation_acquire', u, reference, permit=permit(A, u, 2)), 'already granted here under another grant', 'active host acquiring a second grant at the standby\'s epoch')
    refused(A, request('start', u, reference, observe_seconds=0), 'activation', 'start on the active host after supersession')
    checks.append('[active] superseded: old grant, old epoch and the standby\'s epoch all refused; only a new rotation could bring it back')

    # --- What is and is not proven, in the record.
    report['takeover'] = {'standby_lease_generation': taken['generation'], 'promotion_lease_generation': promoted['lease_generation'],
                          'active_epoch': 1, 'standby_epoch': 2, 'active_superseded': True,
                          'active_universe_running_after_takeover': False, 'standby_universe_running_after_takeover': True}
    report['not_proven'] = [
        'mutual exclusion beyond the epoch: the gate was the suite; PodMesh verified permit binding and screen, never the permit origin',
        'failure detection: the suite decided when the wait began and when to rotate; no host did',
        'manifest origin: the point is unsigned and the standby verified bytes against the manifest only',
        'transport: bytes were carried by the suite over SSH, not by any PodMesh mechanism',
    ]
    report['checks'] = checks
    report['result'] = 'PASS'
    print(json.dumps(report, indent=2))
finally:
    # Disposable fixtures only: a stop through the API where it may be running, then removal.
    B.api(request('stop', u, reference, timeout_seconds=10, on_timeout='kill'))
    for host, names in ((A, [u]), (B, [q, u])):
        for n in names:
            host.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + n], check=False)
    if point:
        B.call('podman_run', args=['rmi', '--force', f'localhost/podmesh-restore:{point}'], check=False)
