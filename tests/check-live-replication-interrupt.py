#!/usr/bin/env python3
"""The two crash paths of live replication, on two PodMesh hosts: the service killed while a live capture's
checkpoint runs, and killed while a live promotion's restore runs. Each command runs in its own scope and outlives
the service; the same operation is then sent again and must settle what happened without doing anything twice.

    PODMESH_SOURCE_SSH=user@host-a PODMESH_DESTINATION_SSH=user@host-b \\
    PODMESH_SOCKET=... PODMESH_STATE_DIR=... PODMESH_UNIT=... python3 -B tests/check-live-replication-interrupt.py

The universe holds about 512 MiB of process memory so that the checkpoint and the restore last long enough for
the kill to land while they run. Memory continuity is read through /proc/<pid>/root as in the other suites."""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, counter_values, request, transfer  # noqa: E402

REF = 'disposable-lab-live-replication-interrupt-test'
HOG = ['sh', '-c',
       'awk "$1" & token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done',
       'hog', 'BEGIN{s="0123456789abcdefghijklmnopqrstuv"; while (length(s) < 536870912) s = s s; while (1) system("sleep 5")}']
FILES = ('recovery-point-manifest.json', 'checkpoint.tar.zst')

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-live-interrupt-')
checks, results = [], {}
A = Host('active', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('standby', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
assert A.identity != B.identity, 'both services report the same host UUID'
U = str(uuid.uuid4())


def counter(host, seconds=4.0):
    return counter_values(host.call('counter', uuid_value=U, seconds=seconds)['samples'])


def check(condition, label, detail=None):
    assert condition, (label, detail)
    checks.append(label)


def inspect(host):
    return host.call('inspect', name='podmesh-' + U)['container']


try:
    alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']
    assert alpine == B.call('image_id', reference='docker.io/library/alpine:3.22')['image'], 'the hosts do not share the alpine image ID'
    A.ok(request('create', U, REF, image='sha256:' + alpine, network_profile='isolated', command=HOG), 'memory-holding universe created', checks)
    A.ok(request('start', U, REF), 'started', checks)
    A.ok(request('activation_require', U, REF, lease_seconds=600, takeover_margin_seconds=5, desired_standbys=1), 'policy declared', checks)
    A.ok(request('activation_acquire', U, REF), 'lease acquired', checks)
    time.sleep(12)
    memory = A.call('memory', uuid_value=U)['memory_current_bytes']
    check(memory > 400 * 1024 ** 2, 'the universe holds more than 400 MiB', memory)
    results['memory_current_bytes'] = memory
    before = counter(A)
    token = before[-1][0]
    points_before = len(A.ok(request('recovery_point_status', U, REF))['recovery_points'])

    # ------------------------------------------------------------------ 1. the service dies during a live capture
    capture = request('recovery_point_prepare', U, REF, capture='live')
    container_id = inspect(A)['Id']
    seen = A.call('interrupt_live', request=capture, needles=['checkpoint', container_id],
                  unit=f"podmesh-live-capture-{capture['operation_id']}.scope")
    results['capture_kill'] = seen['at_kill']
    check(seen['at_kill']['command_alive_after_kill'], 'the checkpoint command outlived the killed service', seen)
    c = inspect(A)
    results['after_capture_kill'] = {'status': c['State']['Status'], 'checkpointed': c['State'].get('Checkpointed')}
    A.call('relaunch_service')
    checks.append('[active] service relaunched')
    retried = A.api(capture)
    results['capture_retry'] = retried
    check(retried.get('ok') is False and 'interrupted' in json.dumps(retried), 'the retried capture reports the interruption and records nothing', retried)
    details = retried.get('details') or {}
    check((details.get('brought_back') or {}).get('verified') is True, 'the universe was brought back from its kept images', details)
    c = inspect(A)
    check(c['State']['Status'] == 'running', 'the universe runs again on the active host', c['State'])
    after = counter(A)
    check({t for t, _ in after} == {token} and after[0][1] >= before[-1][1] and after[-1][1] > after[0][1],
          'its memory survived the interrupted capture: same token, counter progressing', (before[-3:], after))
    check(len(A.ok(request('recovery_point_status', U, REF))['recovery_points']) == points_before, 'no point was recorded from the interrupted capture')
    again = A.api(capture)
    check(again.get('ok') is False and 'interrupted' in json.dumps(again) and inspect(A)['Id'] == c['Id'],
          'a third send of the same capture repeats nothing', again)

    # ------------------------------------------------------------------ 2. the service dies during a live promotion
    point = A.ok(request('recovery_point_prepare', U, REF, capture='live'), 'a complete live capture', checks)
    check(point['resumed'] is True, 'it resumed in place', point)
    at_capture = counter(A, seconds=1.0)
    transfer(A, B, point['recovery_point_uuid'], files=FILES)
    B.ok(request('recovery_point_stage', U, REF, recovery_point_uuid=point['recovery_point_uuid']), 'staged on the standby', checks)
    A.ok(request('stop', U, REF, timeout_seconds=5, on_timeout='kill'), 'the active copy stopped', checks)
    A.ok(request('activation_release', U, REF), 'the active host released its lease', checks)
    B.ok(request('activation_require', U, REF, lease_seconds=600, takeover_margin_seconds=5, desired_standbys=1), 'policy declared on the standby', checks)
    B.ok(request('activation_acquire', U, REF), 'lease acquired on the standby', checks)
    promote = request('recovery_point_promote', U, REF, recovery_point_uuid=point['recovery_point_uuid'])
    seen = B.call('interrupt_live', request=promote, needles=['restore', '--import=', promote['operation_id']],
                  unit=f"podmesh-live-promote-{promote['operation_id']}.scope")
    results['promote_kill'] = seen['at_kill']
    check(seen['at_kill']['command_alive_after_kill'], 'the restore command outlived the killed service', seen)
    B.call('relaunch_service')
    checks.append('[standby] service relaunched')
    finished = B.api(promote)
    results['promote_retry'] = finished
    check(finished.get('ok') is True, 'the retried promotion finished by observation', finished)
    data = finished['data']
    check(data.get('started') is True and (data.get('restore') or {}).get('prevention', {}).get('resumed_after_interruption') is True,
          'it recorded the container the interrupted attempt restored, without restoring again', data)
    c = inspect(B)
    check(c['State']['Status'] == 'running' and c['State']['Restored'] is True, 'the universe runs on the standby', c['State'])
    taken = counter(B)
    check({t for t, _ in taken} == {token} and taken[0][1] >= at_capture[0][1] and taken[-1][1] > taken[0][1],
          'its memory came across: same token, counter from the capture onward', (at_capture, taken))
    replay = B.ok(promote, 'the same promotion sent once more', checks)
    check(replay.get('replayed') is True and inspect(B)['Id'] == c['Id'], 'the replay restored nothing', replay)
    B.refused(request('recovery_point_promote', U, REF, recovery_point_uuid=point['recovery_point_uuid']), 'a new promotion of the same point',
              'already promoted', checks)
    B.ok(request('stop', U, REF, timeout_seconds=5, on_timeout='kill'), 'typed stop of the promoted universe accepted', checks)
    results.update(ok=True, universe=U, active=A.identity, standby=B.identity)
finally:
    for h in (B, A):
        try:
            h.call('ready', seconds=5)
        except Exception:  # noqa: BLE001 -- a service left down by a failed run is brought back before cleanup
            try:
                h.call('relaunch_service')
            except Exception as e:  # noqa: BLE001
                results.setdefault('relaunch_failed', {})[h.role] = str(e)[-300:]
        h.api(request('stop', U, REF, timeout_seconds=5, on_timeout='kill'))
        r = h.api(request('delete', U, REF))
        results.setdefault('cleanup', {})[h.role] = r.get('ok') or r.get('error')
    print(json.dumps({'checks': checks, 'results': results}, indent=2, default=str))
