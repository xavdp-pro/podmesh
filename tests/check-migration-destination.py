#!/usr/bin/env python3
"""Serial migration between two PodMesh hosts through the local API only: authorize, transfer, destination
preflight, restore, completion, retirement and the return trip.

Run on a controller with SSH access to two disposable lab hosts, each running an isolated development
service (PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT are the same on both):

    PODMESH_SOURCE_SSH=user@host-a PODMESH_DESTINATION_SSH=user@host-b \\
    PODMESH_SOCKET=... PODMESH_STATE_DIR=... PODMESH_UNIT=... python3 -B tests/check-migration-destination.py

Every product mutation goes through the API. The controller only moves documents between an outbox and an
inbox, and reads Podman and /proc independently. Direct Podman writes are limited to uniquely named
disposable fixtures of this suite, which are listed in the output. Memory continuity is established by an
observer reading the application's own /tmp/state through /proc/<pid>/root, never by a restore exit code."""
import copy, json, os, shutil, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, event_report, memory_continued, request, transfer  # noqa: E402

REF = 'disposable-lab-migration-destination-test'
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']
RUNTIME_GIT_ID = 'v3.15.5.3'
RUNTIME_BINARY = '/usr/lib/podmesh-vzcriu/criu'

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-two-hosts-')
checks, results = [], {}
A = Host('source', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('destination', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
assert A.identity != B.identity, 'both services report the same host UUID'
fixture_ids = {A.role: set(), B.role: set()}
universes = []


def inspect(host, u):
    return host.call('inspect', name='podmesh-' + u)['container']
def counter(host, u, seconds=3.0):
    return host.call('counter', uuid_value=u, seconds=seconds)['samples']
def document(host, box, authorization, name):
    return json.loads(host.call('read', path=f'{state_dir}/{box}/{authorization}/{name}')['text'])
def write(host, box, authorization, name, value):
    return host.call('write_document', box=box, authorization=authorization, name=name, text=json.dumps(value, indent=2) + '\n')
def fixture(host, name, *args, run=False):
    host.fixtures.append(name)
    host.call('podman_run', args=[('run' if run else 'create'), *(['-d'] if run else []), '--pull=never', '--network=none', '--name', name, *args])
    fixture_ids[host.role].add(host.call('inspect', name=name)['container']['Id'])
    return name
def universe(host, u, image, command):
    universes.append(u)
    host.ok(request('create', u, REF, image='sha256:' + image, command=command))
    host.ok(request('start', u, REF))
    return inspect(host, u)


alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']
assert alpine == B.call('image_id', reference='docker.io/library/alpine:3.22')['image'], 'the hosts do not share the alpine image ID'
baseline = {A.role: A.call('snapshot')['podman'], B.role: B.call('snapshot')['podman']}
since = {A.role: int(A.call('time')['time']) - 1, B.role: int(B.call('time')['time']) - 1}
unrelated = {}
try:
    for host in (A, B):
        name = fixture(host, 'pmfixture-unrelated-' + str(uuid.uuid4()), alpine, 'sleep', '3600', run=True)
        state = host.call('inspect', name=name)['container']
        unrelated[host.role] = (name, state['Id'], state['State']['StartedAt'])

    # ---------------------------------------------------------------- forward migration, source side
    U = str(uuid.uuid4())
    source_container = universe(A, U, alpine, COUNTER)
    before_checkpoint = counter(A, U)
    checkpoint_request = request('migration_checkpoint', U, REF, container_id=source_container['Id'], image='sha256:' + alpine,
                                 source_host_uuid=A.identity, destination_host_uuid=B.identity)
    checkpoint = A.ok(checkpoint_request, 'source checkpointed for the real destination host UUID', checks)
    assert inspect(A, U)['State']['Checkpointed'] is True and inspect(A, U)['State']['Running'] is False

    A.refused(request('migration_authorize_transfer', U, REF, checkpoint_operation_id=str(uuid.uuid4()), destination_host_uuid=B.identity),
              'authorization naming another checkpoint operation', 'belongs to checkpoint operation', checks)
    A.refused(request('migration_authorize_transfer', U, REF, checkpoint_operation_id=checkpoint_request['operation_id'],
                      destination_host_uuid=str(uuid.uuid4())),
              'authorization to a destination other than the one recorded at checkpoint', 'differs from the destination recorded', checks)

    authorize_request = request('migration_authorize_transfer', U, REF, checkpoint_operation_id=checkpoint_request['operation_id'],
                                destination_host_uuid=B.identity)
    authorization = A.ok(authorize_request, 'transfer authorized: authorization recorded, then archive, manifest and handoff in the outbox', checks)
    A1, handoff = authorization['authorization_id'], authorization['handoff']
    outbox = A.call('boxes')['outbox'][A1]
    assert outbox['handoff.json']['sha256'] == authorization['handoff_sha256']
    assert outbox['checkpoint.tar.zst']['sha256'] == checkpoint['archive']['sha256'] == handoff['archive']['sha256']
    assert outbox['manifest.json']['sha256'] == checkpoint['manifest']['sha256'] == handoff['manifest']['sha256']
    assert outbox['_mode'] == '0o700' and all(v['mode'] == '0o600' for k, v in outbox.items() if k != '_mode')
    assert (handoff['universe_uuid'], handoff['source_container_id'], handoff['image_id'], handoff['source_host_uuid'],
            handoff['destination_host_uuid'], handoff['checkpoint_operation_id'], handoff['authorization_ref']) == (
        U, source_container['Id'], alpine, A.identity, B.identity, checkpoint_request['operation_id'], REF)
    assert handoff['runtime']['git_id'] == RUNTIME_GIT_ID and len(handoff['runtime']['binary_sha256']) == 64 and handoff['kernel_release']
    assert A.status(U)['reservation']['state'] == 'transfer_authorized'
    checks.append('[source] handoff binds universe, source container, image, both hosts, checkpoint operation, archive and manifest hashes, '
                  'runtime git ID and binary hash, kernel release and the requester reference')

    replay = A.ok(authorize_request)
    assert replay['historical'] and replay['original_result']['handoff_sha256'] == authorization['handoff_sha256']
    assert all(f['matches'] for f in replay['current_artifacts']['files'].values())
    checks.append('[source] retried authorization is historical and returns the same handoff, with a fresh re-hash of the outbox')
    A.refused(request('migration_authorize_transfer', U, REF, checkpoint_operation_id=checkpoint_request['operation_id'],
                      destination_host_uuid=B.identity),
              'second authorization of an already authorized reservation', 'only a checkpointed reservation can be authorized', checks)

    for operation, extra, label in [('start', {}, 'start'), ('delete', {}, 'delete'), ('create', {'image': 'sha256:' + alpine, 'command': ['true']}, 'create')]:
        A.refused(request(operation, U, REF, **extra), f'{label} of a transfer_authorized source', 'reserved', checks)
    A.refused(request('clone', str(uuid.uuid4()), REF, source_uuid=U), 'clone from a transfer_authorized source', 'reserved', checks)
    A.refused(dict(checkpoint_request, operation_id=str(uuid.uuid4())), 'second checkpoint of a transfer_authorized source', 'already reserved', checks)

    # ---------------------------------------------------------------- destination refusals without effect
    restore_request = request('migration_restore', U, REF, authorization_id=A1)
    preflight_request = request('migration_destination_preflight', U, REF, authorization_id=A1)
    B.refused(restore_request, 'restore before the handoff was delivered', 'No handoff for this authorization in the inbox', checks)
    B.refused(dict(preflight_request, operation_id=str(uuid.uuid4())), 'destination preflight before the handoff was delivered', 'No handoff', checks)

    delivery = transfer(A, B, A1)
    checks.append('[destination] archive, manifest and handoff arrive with the hashes the source recorded')
    archive_path = f'{state_dir}/inbox/{A1}/checkpoint.tar.zst'
    B.call('corrupt', path=archive_path, mode='append', data='tampered')
    report = B.ok(dict(preflight_request, operation_id=str(uuid.uuid4())))
    assert report['compatible'] is False and any('does not match the handoff' in b for b in report['blockers']), report['blockers']
    B.refused(restore_request, 'restore of a tampered archive', 'Restore preconditions not met', checks)
    B.call('corrupt', path=archive_path, mode='truncate', data='tampered')

    genuine = {name: document(B, 'inbox', A1, name) for name in ('handoff.json', 'manifest.json')}
    for field, forge, expected in [
        ('runtime', lambda h, m: (h['runtime'].update(binary_sha256='f' * 64), m['runtime']['sha256'].update({RUNTIME_BINARY: 'f' * 64})),
         'differs from the source runtime'),
        ('kernel', lambda h, m: (h.update(kernel_release='0.0.0-forged'), m['runtime'].update(kernel='0.0.0-forged')),
         'differs from the source kernel'),
    ]:
        forged_handoff, forged_manifest = copy.deepcopy(genuine['handoff.json']), copy.deepcopy(genuine['manifest.json'])
        forge(forged_handoff, forged_manifest)
        written = write(B, 'inbox', A1, 'manifest.json', forged_manifest)
        forged_handoff['manifest']['sha256'] = written['sha256']
        write(B, 'inbox', A1, 'handoff.json', forged_handoff)
        report = B.ok(dict(preflight_request, operation_id=str(uuid.uuid4())))
        assert report['compatible'] is False and any(expected in b for b in report['blockers']), (field, report['blockers'])
        B.refused(restore_request, f'restore of a self-consistent handoff claiming another {field} (forged documents)', 'Restore preconditions not met', checks)
    transfer(A, B, A1)  # deliver the genuine documents again

    occupant = fixture(B, 'podmesh-' + U, alpine, 'sleep', '3600')
    report = B.ok(dict(preflight_request, operation_id=str(uuid.uuid4())))
    assert any('is occupied by container' in b for b in report['blockers']), report['blockers']
    B.refused(restore_request, 'restore onto an occupied universe name', 'Restore preconditions not met', checks)
    B.remove_fixture(occupant)
    labelled = fixture(B, 'pmfixture-occupant-' + str(uuid.uuid4()), '--label', f'io.podmesh.universe={U}', alpine, 'sleep', '3600')
    report = B.ok(dict(preflight_request, operation_id=str(uuid.uuid4())))
    assert any('carries this universe label' in b for b in report['blockers']), report['blockers']
    B.refused(restore_request, 'restore while another container carries the universe label', 'Restore preconditions not met', checks)
    B.remove_fixture(labelled)

    # ---------------------------------------------------------------- an image the destination does not have
    V = str(uuid.uuid4())
    image_fixture = fixture(A, 'pmfixture-image-' + str(uuid.uuid4()), alpine, 'true')
    reference = 'localhost/pmfixture-image-' + V
    A.call('podman_run', args=['commit', '--change', f'LABEL io.podmesh.test-image={V}', image_fixture, reference])
    A.remove_fixture(image_fixture)
    v_image = A.call('image_id', reference=reference + ':latest')['image']
    assert v_image and v_image != alpine
    v_container = universe(A, V, v_image, COUNTER)
    v_checkpoint = request('migration_checkpoint', V, REF, container_id=v_container['Id'], image='sha256:' + v_image,
                           source_host_uuid=A.identity, destination_host_uuid=B.identity)
    A.ok(v_checkpoint)
    v_authorization = A.ok(request('migration_authorize_transfer', V, REF, checkpoint_operation_id=v_checkpoint['operation_id'],
                                   destination_host_uuid=B.identity))
    V1 = v_authorization['authorization_id']
    transfer(A, B, V1)
    report = B.ok(request('migration_destination_preflight', V, REF, authorization_id=V1))
    assert report['compatible'] is False and any('is not present in the local store' in b for b in report['blockers']), report['blockers']
    B.refused(request('migration_restore', V, REF, authorization_id=V1), 'restore of a universe whose image the destination does not have',
              'Restore preconditions not met', checks)

    # ---------------------------------------------------------------- the restore itself
    report = B.ok(preflight_request, 'destination preflight compatible, read-only', checks)
    assert report['compatible'] is True and report['blockers'] == [], report
    assert report['facts']['handoff_sha256'] == authorization['handoff_sha256']
    assert report['facts']['archive']['matches_handoff'] is True and report['facts']['manifest']['matches_handoff'] is True
    assert report['facts']['archive']['config']['universe_label'] == U and report['facts']['image_present'] is True
    assert B.ok(preflight_request)['historical'], 'a repeated preflight operation ID must be historical'
    assert not [k for k in B.call('journal')['migration_restore_claims'] if k['authorization_id'] == A1], 'preflight must not claim'

    restored = B.ok(restore_request, 'restore verified from the delivered handoff', checks)
    restored_container = inspect(B, U)
    assert restored['container_id'] == restored_container['Id'] != source_container['Id']
    assert restored_container['State']['Running'] is True and restored_container['State']['Restored'] is True
    assert restored_container['Config']['Labels']['io.podmesh.universe'] == U
    assert restored_container['HostConfig']['NetworkMode'] == 'none' and restored_container['Mounts'] == []
    assert restored_container['Image'].replace('sha256:', '') == alpine
    checks.append('[destination] restored universe runs with the handoff universe label, a new container ID, network none and no mounts')
    log = B.call('read', path=restored['restore_log']['file'])['text']
    assert f'(gitid {RUNTIME_GIT_ID})' in log and 'Restore finished successfully' in log
    assert restored['restore_log']['sha256'] == B.call('path', path=restored['restore_log']['file'])['sha256']
    checks.append(f'[destination] preserved CRIU restore log shows a successful restore by the qualified private runtime {RUNTIME_GIT_ID}')
    after_restore = counter(B, U, seconds=6)
    continuity = memory_continued(before_checkpoint, after_restore)
    checks.append('[destination] memory continuity observed from outside the universe: same memory-only token %s, counter %s -> %s'
                  % (continuity['token'][:8], continuity['last_before_checkpoint'], continuity['last_after_restore']))
    scope = B.call('scope', unit=restored['restore_scope']['unit'])
    conmon = B.call('conmon', name='podmesh-' + U)
    assert scope['active_state'] in ('inactive', 'failed') and restored['restore_scope']['finished'] is True, scope
    assert 'libpod-conmon-' in (conmon['conmon_cgroup'] or '') and conmon['conmon_scope_exists'] is True, conmon
    checks.append('[destination] after a successful restore the transient restore scope is finished and conmon lives in its own '
                  'libpod-conmon scope, outside it: ' + conmon['conmon_cgroup'])
    outcome = document(B, 'outbox', A1, 'outcome.json')
    assert outcome['result'] == 'restored' and outcome['handoff_sha256'] == authorization['handoff_sha256']
    assert outcome['restored_container_id'] == restored_container['Id'] and outcome['universe_uuid'] == U
    assert outcome['destination_host_uuid'] == B.identity and outcome['source_host_uuid'] == A.identity
    checks.append('[destination] outcome bound to the handoff hash names the restored container and this host')
    assert inspect(A, U)['State']['Running'] is False, 'the source must stay stopped'

    assert B.ok(restore_request)['historical'] and B.ok(restore_request)['current_outcome']['matches'] is True
    checks.append('[destination] retried restore is historical, with a fresh re-hash of the written outcome')
    B.refused(request('migration_restore', U, REF, authorization_id=A1), 'second restore of an already claimed authorization',
              'already claimed on this host', checks)
    B.refused(request('migration_restore_abort', U, REF, authorization_id=A1), 'abort of a verified restore', 'aborting would remove the active universe', checks)

    # ---------------------------------------------------------------- completion and retirement
    transfer(B, A, A1, files=('outcome.json',))
    write(A, 'inbox', V1, 'outcome.json', document(A, 'inbox', A1, 'outcome.json'))
    A.refused(request('migration_complete_transfer', V, REF, authorization_id=V1),
              'completion with an outcome bound to another handoff', 'not to this authorization', checks)

    completion = A.ok(request('migration_complete_transfer', U, REF, authorization_id=A1), 'transfer completed as restored', checks)
    assert completion['destination_result'] == 'restored' and completion['restored_container_id'] == restored_container['Id']
    assert A.status(U)['reservation']['state'] == 'transferred'
    A.refused(request('migration_complete_transfer', U, REF, authorization_id=A1), 'second completion of the same authorization',
              'already completed by operation', checks)
    for operation, extra, label in [('start', {}, 'start'), ('delete', {}, 'delete'), ('create', {'image': 'sha256:' + alpine, 'command': ['true']}, 'create')]:
        A.refused(request(operation, U, REF, **extra), f'{label} of a transferred source', 'reserved', checks)

    retirement = A.ok(request('migration_retire_source', U, REF, authorization_id=A1), 'source retired: only the stopped checkpointed container removed', checks)
    assert retirement['action'] == 'removed' and retirement['kept_checkpoint_files_removed'] is True
    assert inspect(A, U) is None and retirement['evidence_directory_kept'] is True
    status = A.status(U)
    assert status['reservation']['state'] == 'transferred' and status['artifacts']['archive_sha256_matches'] is True
    assert status['transfer_authorizations'][0]['state'] == 'completed_restored'
    A.refused(request('create', U, REF, image='sha256:' + alpine, command=['true']), 'create reusing a transferred universe UUID on the source', 'reserved', checks)

    # The source must refuse the handoff it issued: restoring it here would activate a second copy.
    transfer(A, A, A1, files=('handoff.json', 'manifest.json', 'checkpoint.tar.zst'))
    report = A.ok(request('migration_destination_preflight', U, REF, authorization_id=A1))
    assert any('is not this host' in b for b in report['blockers']) and any('source is this host' in b for b in report['blockers']), report['blockers']
    A.refused(request('migration_restore', U, REF, authorization_id=A1), 'restore of an outbound handoff on the source host itself',
              'Restore preconditions not met', checks)

    # ---------------------------------------------------------------- declining an authorization
    declined = B.ok(request('migration_restore_abort', V, REF, authorization_id=V1), 'destination declined an authorization it never claimed', checks)
    assert declined['action'] == 'declined_without_restore' and declined['outcome']['result'] == 'not_restored'
    B.refused(request('migration_restore', V, REF, authorization_id=V1), 'restore of an authorization this host declined', 'state not_restored', checks)
    transfer(B, A, V1, files=('outcome.json',))
    ended = A.ok(request('migration_complete_transfer', V, REF, authorization_id=V1), 'authorization ended by a not_restored outcome', checks)
    assert ended['destination_result'] == 'not_restored' and A.status(V)['reservation']['state'] == 'checkpointed'
    assert A.status(V)['transfer_authorizations'][0]['state'] == 'ended_not_restored'
    A.refused(request('start', V, REF), 'start of a source whose authorization ended without a restore', 'reserved', checks)

    # ---------------------------------------------------------------- lifecycle on the destination, then the return trip
    B.ok(request('stop', U, REF, timeout_seconds=10, on_timeout='kill'), 'stop of the restored universe through the API', checks)
    assert inspect(B, U)['State']['Running'] is False
    B.ok(request('start', U, REF), 'start of the restored universe through the API', checks)
    assert inspect(B, U)['State']['Running'] is True
    before_return = counter(B, U, seconds=4)
    return_container = inspect(B, U)
    return_checkpoint = request('migration_checkpoint', U, REF, container_id=return_container['Id'], image='sha256:' + alpine,
                                source_host_uuid=B.identity, destination_host_uuid=A.identity)
    B.ok(return_checkpoint, 'restored universe checkpointed for the return trip (ownership from the verified import)', checks)
    return_authorization = B.ok(request('migration_authorize_transfer', U, REF, checkpoint_operation_id=return_checkpoint['operation_id'],
                                        destination_host_uuid=A.identity))
    A2 = return_authorization['authorization_id']
    transfer(B, A, A2)
    report = A.ok(request('migration_destination_preflight', U, REF, authorization_id=A2))
    assert report['compatible'] is True and report['blockers'] == [], report['blockers']
    checks.append('[source] a local transferred history does not block the return trip')
    returned = A.ok(request('migration_restore', U, REF, authorization_id=A2), 'return restore verified on the original host', checks)
    returned_container = inspect(A, U)
    assert returned['container_id'] == returned_container['Id'] and returned_container['State']['Restored'] is True
    assert returned_container['Id'] not in (source_container['Id'], restored_container['Id'])
    continuity_back = memory_continued(before_return, counter(A, U, seconds=6))
    checks.append('[source] memory continuity after the return trip: token %s, counter %s -> %s'
                  % (continuity_back['token'][:8], continuity_back['last_before_checkpoint'], continuity_back['last_after_restore']))
    status = A.status(U)
    assert status['reservation'] is None and returned['reservation_history_archived'] is True
    assert status['reservation_history'][0]['state'] == 'transferred'
    checks.append('[source] the earlier transferred reservation is archived by the verified return restore, so the universe is operable again')
    transfer(A, B, A2, files=('outcome.json',))
    B.ok(request('migration_complete_transfer', U, REF, authorization_id=A2), 'return transfer completed on the first destination', checks)
    assert B.status(U)['reservation']['state'] == 'transferred'
    B.ok(request('migration_retire_source', U, REF, authorization_id=A2), 'first destination retired its stopped source after the return', checks)
    assert inspect(B, U) is None
    B.refused(request('create', U, REF, image='sha256:' + alpine, command=['true']), 'create reusing the universe UUID on the host it left', 'reserved', checks)

    A.ok(request('stop', U, REF, timeout_seconds=10, on_timeout='kill'), 'stop of the returned universe', checks)
    A.ok(request('start', U, REF))
    A.ok(request('stop', U, REF, timeout_seconds=10, on_timeout='kill'))
    A.ok(request('delete', U, REF), 'delete of the returned universe through the API', checks)
    assert inspect(A, U) is None and A.call('labelled', uuid_value=U)['containers'] == []

    # ---------------------------------------------------------------- cleanup and independent verification
    A.remove_fixture('podmesh-' + V, note='reserved checkpointed source of the declined authorization, removed directly by the test')
    A.call('podman_run', args=['image', 'rm', reference + ':latest'])
    results['restored'] = {'forward': restored, 'return': returned, 'completion': completion, 'retirement': retirement}
    for host in (A, B):
        name, container_id, started = unrelated[host.role]
        now = host.call('inspect', name=name)['container']
        assert (now['Id'], now['State']['StartedAt'], now['State']['Status']) == (container_id, started, 'running'), (host.role, 'unrelated container touched')
        host.remove_fixture(name)
    checks.append('unrelated running containers on both hosts untouched')
    events = {}
    removed_by_test = {A.role: {v_container['Id']}, B.role: set()}
    for host in (A, B):
        until = int(host.call('time')['time']) + 1
        events[host.role] = event_report(host, since[host.role], until, universes, fixture_ids[host.role],
                                         removed_by_test=removed_by_test[host.role])
        assert not events[host.role]['outside_api_windows'], (host.role, events[host.role]['outside_api_windows'])
        assert host.call('snapshot')['podman'] == baseline[host.role], (host.role, 'containers, images or volumes differ from the baseline')
    checks.append('every Podman container event on API-managed universes on both hosts falls inside an API request window of that host, '
                  'except this suite\'s own fixture containers: ' + json.dumps({r: e['statuses'] for r, e in events.items()}))
    checks.append('pre-existing containers, images and volumes unchanged on both hosts; no leftovers')
finally:
    for host in (A, B):
        host.cleanup()
    shutil.rmtree(control, ignore_errors=True)

print(json.dumps({'status': 'PASS', 'source_host_uuid': A.identity, 'destination_host_uuid': B.identity, 'universes': universes,
                  'checks': checks, 'check_count': len(checks), 'api_windows': {A.role: len(A.windows), B.role: len(B.windows)},
                  'forward_authorization_id': A1, 'return_authorization_id': A2, 'declined_authorization_id': V1,
                  'memory_continuity': {'forward': continuity, 'return': continuity_back}, 'events': events,
                  'delivery': delivery, 'results': results,
                  'reserved_universes_left_in_journals': {A.role: [V], B.role: [U]}}))
