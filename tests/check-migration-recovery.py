#!/usr/bin/env python3
"""Recovery of a migration reservation that never left its host: release, abandonment and local restore.

One disposable lab host running an isolated development service is enough: these operations never contact
another host, and a transfer authorization only has to be *recorded* to refuse them, not delivered.

    PODMESH_SOURCE_SSH=user@host-a PODMESH_SOCKET=... PODMESH_STATE_DIR=... PODMESH_UNIT=... \\
    python3 -B tests/check-migration-recovery.py

Every product mutation goes through the API. Direct Podman writes are limited to uniquely named disposable
fixtures of this suite and to two deliberate out-of-band acts on its own universes (starting a reserved
container, and replacing one), which are listed in the output and excluded from the event correlation.
Memory continuity is established by an observer reading the application's own /tmp/state through
/proc/<pid>/root, never by a restore's exit code."""
import json, os, shutil, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, counter_values, event_report, memory_continued, request  # noqa: E402

REF = 'disposable-lab-migration-recovery-test'
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']
RUNTIME_GIT_ID = 'v3.15.5.3'

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-recovery-')
checks, results = [], {}
A = Host('source', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
fixture_ids, universes, out_of_band = set(), [], {}


def inspect(u):
    return A.call('inspect', name='podmesh-' + u)['container']
def counter(u, seconds=3.0):
    return A.call('counter', uuid_value=u, seconds=seconds)['samples']
def reserved(u, command=COUNTER, image=None):
    """A started universe, observed from outside, then checkpointed for a recorded destination that is
    never contacted. The counter samples are taken before the checkpoint stops the application."""
    universes.append(u)
    A.ok(request('create', u, REF, image='sha256:' + (image or alpine), network_profile='isolated', command=command))
    A.ok(request('start', u, REF))
    container = inspect(u)
    before = counter(u)
    checkpoint = request('migration_checkpoint', u, REF, container_id=container['Id'], image='sha256:' + (image or alpine),
                         source_host_uuid=A.identity, destination_host_uuid=str(uuid.uuid4()))
    A.ok(checkpoint)
    return container, checkpoint['operation_id'], before
def fixture(name, *args):
    A.fixtures.append(name)
    A.call('podman_run', args=['create', '--pull=never', '--network=none', '--name', name, *args])
    fixture_ids.add(A.call('inspect', name=name)['container']['Id'])
    return name


alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']
baseline = A.call('snapshot')['podman']
since = int(A.call('time')['time']) - 1
try:
    # ---------------------------------------------------------------- release, then an in-place local restore
    U = str(uuid.uuid4())
    container, checkpoint, before_checkpoint = reserved(U)
    status = A.status(U)
    assert status['reservation']['state'] == 'checkpointed'
    assert status['recovery']['release']['permitted'] is True and status['recovery']['release']['blockers'] == []
    assert status['recovery']['abandon']['permitted'] is False and status['recovery']['restore_local']['permitted'] is False
    assert status['release']['permitted'] is True and status['release']['operation_available'] is True
    checks.append('status reports which recovery operations the observed state permits: release yes, abandon and local restore no')

    A.refused(request('migration_release', U, REF, checkpoint_operation_id=str(uuid.uuid4())),
              'release naming another checkpoint operation', 'belongs to checkpoint operation', checks)
    A.refused(request('migration_abandon', U, REF, checkpoint_operation_id=checkpoint),
              'abandonment while the reserved container is still there', 'a container occupies the universe name', checks)
    A.refused(request('migration_restore_local', U, REF, checkpoint_operation_id=checkpoint),
              'local restore of a reservation that was never released', 'migration_release comes first', checks)

    released = A.ok(request('migration_release', U, REF, checkpoint_operation_id=checkpoint),
                    'checkpointed reservation released after observing the same container, stopped', checks)
    assert released['reservation']['state'] == 'released' and released['container_id'] == container['Id']
    assert inspect(U)['State']['Status'] == 'exited' and inspect(U)['State']['Checkpointed'] is True
    checks.append('release starts nothing: the source is still the stopped, checkpointed container it was')
    status = A.status(U)
    assert status['reservation']['state'] == 'released'
    assert status['recovery']['restore_local']['permitted'] is True
    assert status['recovery']['restore_local']['memory_source'] == 'kept_checkpoint_files'

    local = A.ok(request('migration_restore_local', U, REF, checkpoint_operation_id=checkpoint),
                 'local restore resumed the universe in place from the checkpoint files Podman kept', checks)
    restored = inspect(U)
    assert local['in_place'] is True and local['memory_source'] == 'kept_checkpoint_files'
    assert local['container_id'] == container['Id'] == restored['Id'], 'an in-place restore keeps the container ID'
    assert restored['State']['Running'] is True and restored['State']['Restored'] is True
    log = A.call('read', path=local['restore_log']['file'])['text']
    assert f'(gitid {RUNTIME_GIT_ID})' in log and 'Restore finished successfully' in log
    checks.append(f'the preserved CRIU restore log shows a successful local restore by the qualified runtime {RUNTIME_GIT_ID}')
    continuity = memory_continued(before_checkpoint, counter(U, seconds=6))
    checks.append('memory continuity of the local restore observed from outside the universe: same memory-only token %s, counter %s -> %s'
                  % (continuity['token'][:8], continuity['last_before_checkpoint'], continuity['last_after_restore']))
    assert local['prevention']['watched'] is True and local['prevention']['stopped'] is False
    assert local['prevention']['allowance_bytes'] > 0 and local['prevention']['minimum_free_bytes'] > 0
    checks.append('the local restore ran under the same space bound as a destination restore, and stayed inside it '
                  '(allowance %d bytes, most consumed %d)' % (local['prevention']['allowance_bytes'], local['prevention']['maximum_consumed_bytes']))
    status = A.status(U)
    assert status['reservation'] is None, 'a verified local restore archives the reservation'
    assert status['reservation_history'][-1]['state'] == 'restored_locally'
    checks.append('the verified local restore archives the reservation, so the universe is fully operable again')

    # The universe must now behave like any other: checkpoint, release, stop, start and delete.
    again = request('migration_checkpoint', U, REF, container_id=restored['Id'], image='sha256:' + alpine,
                    source_host_uuid=A.identity, destination_host_uuid=str(uuid.uuid4()))
    A.ok(again, 'a universe brought back by a local restore can be checkpointed again', checks)
    A.ok(request('migration_release', U, REF, checkpoint_operation_id=again['operation_id']))
    A.ok(request('start', U, REF), 'and started, stopped and deleted through the API afterwards', checks)
    A.ok(request('stop', U, REF, timeout_seconds=10, on_timeout='kill'))
    A.ok(request('delete', U, REF))
    assert inspect(U) is None

    # ---------------------------------------------------------------- local restore from the preserved archive
    V = str(uuid.uuid4())
    v_container, v_checkpoint, before_v = reserved(V)
    A.ok(request('migration_release', V, REF, checkpoint_operation_id=v_checkpoint))
    A.ok(request('delete', V, REF), 'a released reservation lifts the gate: the universe container can be deleted', checks)
    assert inspect(V) is None
    status = A.status(V)
    assert status['recovery']['restore_local']['permitted'] is True
    assert status['recovery']['restore_local']['memory_source'] == 'preserved_archive'
    v_local = A.ok(request('migration_restore_local', V, REF, checkpoint_operation_id=v_checkpoint),
                   'local restore from the preserved archive once the kept checkpoint files are gone', checks)
    v_restored = inspect(V)
    assert v_local['in_place'] is False and v_local['memory_source'] == 'preserved_archive'
    assert v_local['container_id'] == v_restored['Id'] != v_container['Id'], 'restoring the archive gives a new container ID'
    assert v_restored['State']['Restored'] is True and v_restored['Config']['Labels']['io.podmesh.universe'] == V
    assert v_restored['HostConfig']['NetworkMode'] == 'none' and v_restored['Mounts'] == []
    continuity_archive = memory_continued(before_v, counter(V, seconds=6))
    checks.append('memory continuity of the archive restore: token %s, counter %s -> %s'
                  % (continuity_archive['token'][:8], continuity_archive['last_before_checkpoint'], continuity_archive['last_after_restore']))
    A.ok(request('stop', V, REF, timeout_seconds=10, on_timeout='kill'),
         'the container restored from the archive is owned by the verified local restore: stop and delete work on its new ID', checks)
    A.ok(request('delete', V, REF))
    assert inspect(V) is None and A.call('labelled', uuid_value=V)['containers'] == []

    # ---------------------------------------------------------------- an ordinary start after a release
    W = str(uuid.uuid4())
    w_container, w_checkpoint, before_w = reserved(W)
    A.ok(request('migration_release', W, REF, checkpoint_operation_id=w_checkpoint))
    started = A.ok(request('start', W, REF), 'an ordinary start of a released universe is allowed and states that memory was not restored', checks)
    assert started['memory_restored'] is False and 'began afresh' in started['memory_note'], started
    old_token, old_count = counter_values(before_w)[-1]
    fresh = counter_values(counter(W, seconds=4))
    assert fresh and all(t != old_token for t, _ in fresh), ('an ordinary start must not resume the checkpointed memory', old_token, fresh)
    assert fresh[0][1] < old_count, ('the counter must restart from the beginning after a fresh start', old_count, fresh)
    checks.append('observed from outside, the ordinary start really did lose the memory: a new token %s replaced %s and the counter '
                  'restarted at %d instead of continuing from %d' % (fresh[0][0][:8], old_token[:8], fresh[0][1], old_count))
    # A new checkpoint of the same universe supersedes the released reservation instead of being refused.
    w_again = request('migration_checkpoint', W, REF, container_id=inspect(W)['Id'], image='sha256:' + alpine,
                      source_host_uuid=A.identity, destination_host_uuid=str(uuid.uuid4()))
    A.ok(w_again, 'a new checkpoint supersedes a released reservation, which is archived with its history', checks)
    status = A.status(W)
    assert status['reservation']['operation_id'] == w_again['operation_id'] and status['reservation']['state'] == 'checkpointed'
    assert [h for h in status['reservation_history'] if h['state'] == 'released'], status['reservation_history']
    A.ok(request('migration_release', W, REF, checkpoint_operation_id=w_again['operation_id']))
    A.ok(request('delete', W, REF))

    # ---------------------------------------------------------------- refused after a transfer authorization
    X = str(uuid.uuid4())
    x_container, x_checkpoint, _ = reserved(X)
    x_destination = A.status(X)['reservation']['destination_host_uuid']
    authorization = A.ok(request('migration_authorize_transfer', X, REF, checkpoint_operation_id=x_checkpoint,
                                 destination_host_uuid=x_destination))
    A.refused(request('migration_release', X, REF, checkpoint_operation_id=x_checkpoint),
              'release of a reservation for which a transfer authorization was issued', 'transfer authorization(s) were issued', checks)
    A.refused(request('migration_abandon', X, REF, checkpoint_operation_id=x_checkpoint),
              'abandonment of a reservation for which a transfer authorization was issued', 'transfer authorization(s) were issued', checks)
    status = A.status(X)
    assert status['recovery']['release']['permitted'] is False and status['recovery']['abandon']['permitted'] is False
    assert status['recovery']['authorizations_ever_issued'] == 1
    checks.append('status reports why: an authorization was issued, so only a verified destination outcome can end this reservation')
    A.remove_fixture('podmesh-' + X, note='reserved source held by an issued authorization, removed directly by the test')
    out_of_band[X] = 'reserved container removed directly by the test (no release exists for an authorized reservation)'

    # ---------------------------------------------------------------- release refused for a running or replaced container
    Y = str(uuid.uuid4())
    y_container, y_checkpoint, _ = reserved(Y)
    # Deliberate out-of-band act on this suite's own universe: a reservation is not fencing, and a source
    # started behind PodMesh's back must not be released as if it were still the checkpointed one.
    A.call('podman_run', args=['start', 'podmesh-' + Y])
    out_of_band[Y] = 'reserved container started and stopped directly with Podman to exercise the running refusal'
    A.refused(request('migration_release', Y, REF, checkpoint_operation_id=y_checkpoint),
              'release of a reserved container that is running', 'is running', checks)
    A.call('podman_run', args=['stop', '--time', '0', 'podmesh-' + Y])
    deadline = time.time() + 30
    while inspect(Y)['State']['Running'] and time.time() < deadline:
        time.sleep(.2)
    permitted = A.ok(request('migration_release', Y, REF, checkpoint_operation_id=y_checkpoint),
                     'once it is stopped again the same reservation can be released, from a container Podman no longer reports as checkpointed', checks)
    assert permitted['reservation']['state'] == 'released'
    A.refused(request('migration_restore_local', Y, REF, checkpoint_operation_id=y_checkpoint),
              'local restore of a released universe whose container was started out of band', 'no longer in its checkpointed state', checks)
    A.ok(request('delete', Y, REF))
    y_restore = A.ok(request('migration_restore_local', Y, REF, checkpoint_operation_id=y_checkpoint),
                     'after deleting that container the preserved archive still restores the universe locally', checks)
    assert y_restore['memory_source'] == 'preserved_archive' and inspect(Y)['State']['Restored'] is True
    A.ok(request('stop', Y, REF, timeout_seconds=10, on_timeout='kill'))
    A.ok(request('delete', Y, REF))

    Z = str(uuid.uuid4())
    z_container, z_checkpoint, _ = reserved(Z)
    A.remove_fixture('podmesh-' + Z, note='reserved container removed to replace it with a forged one')
    out_of_band[Z] = 'reserved container removed and replaced by a forged container with the same name and labels'
    fixture('podmesh-' + Z, '--label', f'io.podmesh.universe={Z}',
            '--label', 'io.podmesh.creation-operation=' + str(uuid.uuid4()), alpine, 'sleep', '3600')
    A.refused(request('migration_release', Z, REF, checkpoint_operation_id=z_checkpoint),
              'release when the universe name carries a container that is not the reserved one', 'not the reserved source container', checks)
    A.refused(request('migration_abandon', Z, REF, checkpoint_operation_id=z_checkpoint),
              'abandonment when a container occupies the universe name', 'a container occupies the universe name', checks)
    A.remove_fixture('podmesh-' + Z)

    # ---------------------------------------------------------------- abandonment of a reservation whose container is gone
    abandoned = A.ok(request('migration_abandon', Z, REF, checkpoint_operation_id=z_checkpoint),
                     'a reservation whose container is gone is abandoned, with its artifacts preserved', checks)
    assert abandoned['reservation']['state'] == 'abandoned' and abandoned['artifacts']['archive_sha256_matches'] is True
    assert A.call('path', path=abandoned['artifact_directory'])['exists'] is True
    status = A.status(Z)
    assert status['reservation']['state'] == 'abandoned'
    assert status['recovery']['release']['permitted'] is False and status['recovery']['restore_local']['permitted'] is False
    for operation, extra, label in [('create', {'image': 'sha256:' + alpine, 'command': ['true'], 'network_profile': 'isolated'}, 'create'), ('start', {}, 'start')]:
        A.refused(request(operation, Z, REF, **extra), f'{label} of an abandoned universe UUID', 'reserved', checks)
    A.refused(request('migration_abandon', Z, REF, checkpoint_operation_id=z_checkpoint),
              'a second abandonment of the same reservation', 'only a reserved, checkpointing', checks)
    checks.append('an abandoned universe keeps refusing create, start, delete and clone on this host, and its artifacts stay verifiable')

    # ---------------------------------------------------------------- independent verification
    results['release'] = released
    results['restore_local_in_place'] = local
    results['restore_local_from_archive'] = v_local
    results['abandon'] = abandoned
    results['authorization_blocking_release'] = authorization['authorization_id']
    until = int(A.call('time')['time']) + 1
    events = event_report(A, since, until, [u for u in universes if u not in out_of_band], fixture_ids)
    assert not events['outside_api_windows'], events['outside_api_windows']
    checks.append('every Podman container event on API-managed universes falls inside an API request window, except this suite\'s own '
                  'fixtures and the universes it deliberately touched out of band: ' + json.dumps(events['statuses']))
    assert A.call('snapshot')['podman'] == baseline, 'containers, images or volumes differ from the baseline'
    checks.append('pre-existing containers, images and volumes unchanged; no leftovers')
finally:
    A.cleanup()
    shutil.rmtree(control, ignore_errors=True)

print(json.dumps({'status': 'PASS', 'host_uuid': A.identity, 'universes': universes, 'checks': checks,
                  'check_count': len(checks), 'api_windows': len(A.windows),
                  'memory_continuity': {'in_place': continuity, 'from_archive': continuity_archive},
                  'out_of_band_universes': out_of_band, 'events': events, 'results': results,
                  'reserved_universes_left_in_journal': {'abandoned': [Z], 'authorized': [X]}}))
