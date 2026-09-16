#!/usr/bin/env python3
"""Live replication between two PodMesh hosts through the local API only: a running universe captured with its
memory and resumed in place, carried, staged on a standby, and promoted there running after the active copy stops.

Run on a controller with SSH access to two disposable lab hosts, each running an isolated development service
(PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT are the same on both):

    PODMESH_SOURCE_SSH=user@host-a PODMESH_DESTINATION_SSH=user@host-b \\
    PODMESH_SOCKET=... PODMESH_STATE_DIR=... PODMESH_UNIT=... python3 -B tests/check-live-replication-two-hosts.py

Every product mutation goes through the API. The controller only moves the point's two files from the active
host's outbox to the standby's inbox. Memory continuity is established by an observer reading the application's
own /tmp/state through /proc/<pid>/root: the token lives only in the process's memory, so a fresh start would
print a new one."""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, counter_values, request, transfer  # noqa: E402

REF = 'disposable-lab-live-replication-test'
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']
FILES = ('recovery-point-manifest.json', 'checkpoint.tar.zst')

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-live-replication-')
checks, results = [], {}
A = Host('active', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('standby', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
assert A.identity != B.identity, 'both services report the same host UUID'
U = str(uuid.uuid4())


def inspect(host):
    return host.call('inspect', name='podmesh-' + U)['container']


def counter(host, seconds=3.0):
    return counter_values(host.call('counter', uuid_value=U, seconds=seconds)['samples'])


def check(condition, label, detail=None):
    assert condition, (label, detail)
    checks.append(label)


try:
    alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']
    assert alpine == B.call('image_id', reference='docker.io/library/alpine:3.22')['image'], 'the hosts do not share the alpine image ID'

    # ------------------------------------------------------------------ the active copy, under a lease
    A.ok(request('create', U, REF, image='sha256:' + alpine, network_profile='isolated', command=COUNTER), 'created on the active host', checks)
    A.ok(request('start', U, REF), 'started', checks)
    A.ok(request('activation_require', U, REF, lease_seconds=120, takeover_margin_seconds=5, desired_standbys=1), 'lease-only policy declared', checks)
    A.ok(request('activation_acquire', U, REF), 'lease acquired on the active host', checks)
    time.sleep(3)
    before = counter(A)
    token = before[-1][0]

    # ------------------------------------------------------------------ live capture: never stopped, memory kept
    first = A.ok(request('recovery_point_prepare', U, REF, capture='live'), 'first live capture', checks)
    capture = first['capture']
    check(first['resumed'] is True and first['consistency_class'] == 'memory-coherent', 'the universe resumed in place, memory-coherent', first)
    after = counter(A)
    check({t for t, _ in after} == {token} and after[0][1] >= before[-1][1] and after[-1][1] > after[0][1],
          'the same process continued on the active host: same memory token, counter progressing', (before[-3:], after))
    c = inspect(A)
    check(c['State']['Status'] == 'running' and c['State']['Restored'] is True, 'the active container is running and restored in place', c['State'])
    results['first_capture'] = {k: capture.get(k) for k in ('dump_seconds', 'resume_seconds', 'interruption_seconds', 'kept_images_removed_bytes')}
    results['first_capture']['archive_bytes'] = first['archive']['bytes']

    # ------------------------------------------------------------------ carried and staged; refusals on the standby
    transfer(A, B, first['recovery_point_uuid'], files=FILES)
    staged = B.ok(request('recovery_point_stage', U, REF, recovery_point_uuid=first['recovery_point_uuid']), 'first point staged on the standby', checks)
    check(staged['staged'] is True and staged['image_present_on_this_host'] is True and staged['container'] is None,
          'staging created no container and found the image', staged)
    B.refused(request('recovery_point_stage', U, REF, recovery_point_uuid=first['recovery_point_uuid']), 'second staging of the same point',
              'already staged', checks)
    B.refused(request('recovery_point_promote', U, REF, recovery_point_uuid=first['recovery_point_uuid']), 'promotion without an activation policy',
              'no activation policy', checks)
    B.refused(request('recovery_point_promote', str(uuid.uuid4()), REF, recovery_point_uuid=first['recovery_point_uuid']),
              'promotion naming another universe', 'is staged for universe', checks)

    # A second capture supersedes the first; the first is discarded on the standby.
    time.sleep(2)
    second = A.ok(request('recovery_point_prepare', U, REF, capture='live'), 'second live capture', checks)
    check(second['resumed'] is True and second['generation'] == first['generation'] + 1, 'second capture resumed, next generation', second)
    at_capture = counter(A, seconds=1.0)
    transfer(A, B, second['recovery_point_uuid'], files=FILES)
    B.ok(request('recovery_point_stage', U, REF, recovery_point_uuid=second['recovery_point_uuid']), 'second point staged', checks)
    discarded = B.ok(request('recovery_point_discard', U, REF, recovery_point_uuid=first['recovery_point_uuid']), 'first point discarded', checks)
    check(discarded['discarded'] is True and discarded['bytes_removed'] > 0, 'the discard removed the first archive', discarded)
    B.refused(request('recovery_point_promote', U, REF, recovery_point_uuid=first['recovery_point_uuid']), 'promotion of a discarded point',
              'was discarded', checks)
    listed = B.ok(request('recovery_point_status', U, REF))
    check(len(listed['staged']) == 2 and sum(1 for s in listed['staged'] if s['discarded_at'] is None) == 1,
          'the standby lists both points, one live', listed['staged'])

    # ------------------------------------------------------------------ takeover: the active copy stops, the standby promotes
    A.ok(request('stop', U, REF, timeout_seconds=5, on_timeout='kill'), 'the active copy stopped', checks)
    A.ok(request('activation_release', U, REF), 'the active host released its lease', checks)
    B.ok(request('activation_require', U, REF, lease_seconds=120, takeover_margin_seconds=5, desired_standbys=1), 'policy declared on the standby', checks)
    B.ok(request('activation_acquire', U, REF), 'lease acquired on the standby', checks)
    promote = request('recovery_point_promote', U, REF, recovery_point_uuid=second['recovery_point_uuid'])
    began = time.time()
    promoted = B.ok(promote, 'second point promoted running on the standby', checks)
    results['promotion_seconds'] = round(time.time() - began, 3)
    check(promoted.get('started') is True, 'the promotion is the start: the universe runs with no separate start', promoted)
    c = inspect(B)
    check(c['State']['Status'] == 'running' and c['State']['Restored'] is True and c['HostConfig']['NetworkMode'] == 'none',
          'the standby container is running, restored, network none', c['State'])
    taken = counter(B)
    check({t for t, _ in taken} == {token} and taken[0][1] >= at_capture[0][1] and taken[-1][1] > taken[0][1],
          'the memory continued on the standby: the same token, counter from the capture onward', (at_capture, taken))
    replay = B.ok(promote, 'the same promotion replayed', checks)
    check(replay.get('replayed') is True and inspect(B)['Id'] == c['Id'], 'the replay restored nothing a second time', replay)
    B.refused(request('recovery_point_promote', U, REF, recovery_point_uuid=second['recovery_point_uuid']), 'a second promotion of the same point',
              'already promoted', checks)

    # ------------------------------------------------------------------ the promoted universe is owned here
    stopped = B.ok(request('stop', U, REF, timeout_seconds=5, on_timeout='kill'), 'typed stop of the promoted universe accepted', checks)
    B.ok(request('start', U, REF), 'typed start of the promoted universe accepted', checks)
    results.update(ok=True, universe=U, active=A.identity, standby=B.identity, stop=stopped)
finally:
    for h in (B, A):
        h.api(request('stop', U, REF, timeout_seconds=5, on_timeout='kill'))
        r = h.api(request('delete', U, REF))
        results.setdefault('cleanup', {})[h.role] = r.get('ok') or r.get('error')
    print(json.dumps({'checks': checks, 'results': results}, indent=2))
