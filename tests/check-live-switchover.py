#!/usr/bin/env python3
"""A planned switchover that loses nothing, on two PodMesh hosts, through the local API only: a FINAL live capture
(checkpointed with its memory, not resumed), the way back when the switchover is abandoned (recovery_point_resume),
and the switchover itself (carried, staged, lease moved, promoted running).

    PODMESH_SOURCE_SSH=user@host-a PODMESH_DESTINATION_SSH=user@host-b \\
    PODMESH_SOCKET=... PODMESH_STATE_DIR=... PODMESH_UNIT=... python3 -B tests/check-live-switchover.py

Zero loss is read from the application itself: its counter, written once a second from a token held only in
memory, must not advance on the active host after the final capture, and must continue on the standby from the
very value the active host last wrote."""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, counter_values, request, transfer  # noqa: E402

REF = 'disposable-lab-live-switchover-test'
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']
FILES = ('recovery-point-manifest.json', 'checkpoint.tar.zst')

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-live-switchover-')
checks, results = [], {}
A = Host('active', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('standby', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
assert A.identity != B.identity, 'both services report the same host UUID'
U = str(uuid.uuid4())


def inspect(host):
    return host.call('inspect', name='podmesh-' + U)['container']


def state_file(host):
    """The counter as the stopped container's filesystem holds it: read from the writable layer, not a process."""
    c = inspect(host)
    upper = c['GraphDriver']['Data'].get('UpperDir')
    text = host.call('read', path=f'{upper}/tmp/state')['text'] if upper else ''
    parts = text.split()
    return (parts[0], int(parts[1])) if len(parts) == 2 and parts[1].isdigit() else None


def counter(host, seconds=4.0):
    return counter_values(host.call('counter', uuid_value=U, seconds=seconds)['samples'])


def check(condition, label, detail=None):
    assert condition, (label, detail)
    checks.append(label)


try:
    alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']
    assert alpine == B.call('image_id', reference='docker.io/library/alpine:3.22')['image'], 'the hosts do not share the alpine image ID'
    A.ok(request('create', U, REF, image='sha256:' + alpine, network_profile='isolated', command=COUNTER), 'created', checks)
    A.ok(request('start', U, REF), 'started', checks)
    A.ok(request('activation_require', U, REF, lease_seconds=300, takeover_margin_seconds=5, desired_standbys=1), 'policy declared', checks)
    A.ok(request('activation_acquire', U, REF), 'lease acquired', checks)
    time.sleep(3)
    token = counter(A)[-1][0]

    # ------------------------------------------------------------------ a final capture, then the way back
    first = A.ok(request('recovery_point_prepare', U, REF, capture='live', resume=False), 'final capture', checks)
    check(first['final'] is True and first['resumed'] is False and first['capture']['interruption_seconds'] is None,
          'the final capture left the universe unresumed and claims no interruption end', first)
    c = inspect(A)
    check(c['State']['Status'] != 'running' and c['State']['Checkpointed'] is True, 'the universe is stopped with its checkpoint kept', c['State'])
    frozen = state_file(A)
    time.sleep(3)
    check(state_file(A) == frozen and frozen[0] == token, 'nothing advanced after the final capture', (frozen, state_file(A)))
    A.refused(request('recovery_point_resume', U, REF, recovery_point_uuid=str(uuid.uuid4())), 'resume of an unknown capture', 'No final capture', checks)
    A.refused(request('recovery_point_resume', str(uuid.uuid4()), REF, recovery_point_uuid=first['recovery_point_uuid']),
              'resume naming another universe', 'is of universe', checks)
    resume = request('recovery_point_resume', U, REF, recovery_point_uuid=first['recovery_point_uuid'])
    resumed = A.ok(resume, 'the final capture resumed in place', checks)
    back = counter(A)
    check({t for t, _ in back} == {token} and back[0][1] >= frozen[1] and back[-1][1] > back[0][1],
          'the universe runs again with its memory, from the frozen value', (frozen, back))
    check(A.ok(resume)['replayed'] is True, 'the same resume replayed repeats nothing')
    A.refused(request('recovery_point_resume', U, REF, recovery_point_uuid=first['recovery_point_uuid']), 'a second resume of the same capture',
              'already resumed', checks)
    results['resume_seconds'] = resumed.get('seconds')

    # ------------------------------------------------------------------ the switchover: nothing lost
    began = time.time()
    final = A.ok(request('recovery_point_prepare', U, REF, capture='live', resume=False), 'final capture for the switchover', checks)
    frozen = state_file(A)
    transfer(A, B, final['recovery_point_uuid'], files=FILES)
    B.ok(request('recovery_point_stage', U, REF, recovery_point_uuid=final['recovery_point_uuid']), 'staged on the standby', checks)
    A.ok(request('activation_release', U, REF), 'lease released on the active host', checks)
    B.ok(request('activation_require', U, REF, lease_seconds=300, takeover_margin_seconds=5, desired_standbys=1), 'policy on the standby', checks)
    B.ok(request('activation_acquire', U, REF), 'lease acquired on the standby', checks)
    promoted = B.ok(request('recovery_point_promote', U, REF, recovery_point_uuid=final['recovery_point_uuid']), 'promoted on the standby', checks)
    results['switchover_interruption_seconds'] = round(time.time() - began, 2)
    results['dump_seconds'] = final['capture']['dump_seconds']
    check(promoted['started'] is True, 'running on the standby with no separate start', promoted)
    check(state_file(A) == frozen, 'the active host wrote nothing after the final capture', (frozen, state_file(A)))
    promoted_at = time.time()
    raw = B.call('counter', uuid_value=U, seconds=4.0)['samples']
    taken = counter_values(raw)
    # The counter advances once a second: at the first sample the standby may have written one value per second
    # since the promotion answered (plus one for the tick in progress), never fewer than the frozen value.
    elapsed = max(0.0, raw[0][0] - promoted_at)
    check({t for t, _ in taken} == {token} and frozen[1] <= taken[0][1] <= frozen[1] + int(elapsed) + 2 and taken[-1][1] > taken[0][1],
          'the standby continues from the value the active host last wrote: nothing lost, nothing replayed',
          {'frozen': frozen, 'first_on_standby': taken[0], 'seconds_since_promotion': round(elapsed, 2)})
    results['continuity'] = {'frozen_on_active': frozen[1], 'first_on_standby': taken[0][1], 'seconds_since_promotion': round(elapsed, 2)}
    A.refused(request('recovery_point_resume', U, REF, recovery_point_uuid=final['recovery_point_uuid']), 'resume on the active host once its lease is released',
              'lease', checks)
    A.ok(request('delete', U, REF), 'the stopped copy deleted on the old active host', checks)
    results.update(ok=True, universe=U)
finally:
    for h in (B, A):
        h.api(request('stop', U, REF, timeout_seconds=5, on_timeout='kill'))
        r = h.api(request('delete', U, REF))
        results.setdefault('cleanup', {})[h.role] = r.get('ok') or r.get('error')
    print(json.dumps({'checks': checks, 'results': results}, indent=2, default=str))
