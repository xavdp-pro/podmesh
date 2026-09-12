#!/usr/bin/env python3
"""Interrupted and failed destination restores between two PodMesh hosts.

1. The service is killed with SIGKILL while `migration_restore` runs its Podman command in its own transient
   scope. The retry of the same operation must reconcile to exactly one container and one outcome, without
   restoring twice, and memory must still be continuous.
2. A restore that fails after Podman started (a deliberately damaged archive, delivered with forged documents
   so that it passes every hash check) must hold its claim, write no outcome, and leave a scope and conmon
   state that `migration_restore_abort` then cleans up before recording not_restored.

Same environment as check-migration-destination.py. Every product mutation goes through the API."""
import copy, json, os, shutil, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, event_report, memory_continued, request, transfer  # noqa: E402

REF = 'disposable-lab-migration-destination-interrupt-test'
# About 512 MiB of process memory next to the memory-only token and counter, so that the restore lasts long
# enough to be interrupted while the archive is imported and CRIU restores the pages.
HOG = ['sh', '-c',
       'awk "$1" & token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done',
       'hog', 'BEGIN{s="0123456789abcdefghijklmnopqrstuv"; while (length(s) < 536870912) s = s s; while (1) system("sleep 5")}']
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-two-hosts-')
checks, results = [], {}
A = Host('source', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('destination', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
assert A.identity != B.identity
fixture_ids = {A.role: set(), B.role: set()}
universes = []


def inspect(host, u):
    return host.call('inspect', name='podmesh-' + u)['container']
def gone(host, path, seconds=30):
    """An emptied cgroup is collected by systemd, not by PodMesh: wait briefly before concluding."""
    deadline = time.time() + seconds
    while host.call('path', path=path)['exists']:
        if time.time() > deadline:
            return False
        time.sleep(.5)
    return True
def document(host, box, authorization, name):
    return json.loads(host.call('read', path=f'{state_dir}/{box}/{authorization}/{name}')['text'])
def write(host, box, authorization, name, value):
    return host.call('write_document', box=box, authorization=authorization, name=name, text=json.dumps(value, indent=2) + '\n')
def authorized(u, command, image):
    """A universe created, started, checkpointed and authorized on the source, delivered to the destination."""
    universes.append(u)
    A.ok(request('create', u, REF, image='sha256:' + image, command=command))
    A.ok(request('start', u, REF))
    container = inspect(A, u)
    checkpoint = request('migration_checkpoint', u, REF, container_id=container['Id'], image='sha256:' + image,
                         source_host_uuid=A.identity, destination_host_uuid=B.identity)
    A.ok(checkpoint)
    authorization = A.ok(request('migration_authorize_transfer', u, REF, checkpoint_operation_id=checkpoint['operation_id'],
                                 destination_host_uuid=B.identity))
    return container, checkpoint, authorization


alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']
assert alpine == B.call('image_id', reference='docker.io/library/alpine:3.22')['image']
baseline = {A.role: A.call('snapshot')['podman'], B.role: B.call('snapshot')['podman']}
since = {A.role: int(A.call('time')['time']) - 1, B.role: int(B.call('time')['time']) - 1}
try:
    # ---------------------------------------------------------------- service killed during the restore
    H = str(uuid.uuid4())
    universes.append(H)
    A.ok(request('create', H, REF, image='sha256:' + alpine, command=HOG))
    A.ok(request('start', H, REF))
    deadline = time.time() + 120
    while A.call('memory', uuid_value=H)['memory_current_bytes'] < 400 * 1024 * 1024:
        assert time.time() < deadline, 'the memory fixture did not grow'
        time.sleep(.5)
    memory = A.call('memory', uuid_value=H)['memory_current_bytes']
    source_container = inspect(A, H)
    before_checkpoint = A.call('counter', uuid_value=H, seconds=3)['samples']
    checkpoint = request('migration_checkpoint', H, REF, container_id=source_container['Id'], image='sha256:' + alpine,
                         source_host_uuid=A.identity, destination_host_uuid=B.identity)
    A.ok(checkpoint, f'{memory // (1024 * 1024)} MiB source checkpointed', checks)
    authorization = A.ok(request('migration_authorize_transfer', H, REF, checkpoint_operation_id=checkpoint['operation_id'],
                                 destination_host_uuid=B.identity))
    H1 = authorization['authorization_id']
    transfer(A, B, H1)
    assert B.ok(request('migration_destination_preflight', H, REF, authorization_id=H1))['compatible'] is True

    restore_request = request('migration_restore', H, REF, authorization_id=H1)
    scope_unit = f"podmesh-restore-{restore_request['operation_id']}.scope"
    interruption = B.call('interrupt_restore', request=restore_request,
                          import_path=f"{state_dir}/migrations/{restore_request['operation_id']}/checkpoint.tar.zst", unit=scope_unit)
    B.windows.append((interruption['first_response']['begin_ns'], interruption['first_response']['end_ns'], 'migration_restore',
                      restore_request['operation_id']))
    at_kill = interruption['at_kill']
    assert interruption['first_response']['response'].get('interrupted') is True, (at_kill, interruption['first_response'])
    assert at_kill['restore_command_pids'] and at_kill['scope_before_kill'] == 'active', at_kill
    assert at_kill['restore_command_alive_after_kill'] is True, ('the restore command did not survive the service kill', at_kill)
    checks.append('[destination] the service was killed with SIGKILL while its Podman restore command ran; the command survived in its own '
                  'transient scope (CRIU processes observed at the kill: %d)' % len(at_kill['criu_processes']))
    claim = [k for k in B.call('journal')['migration_restore_claims'] if k['authorization_id'] == H1][0]
    assert claim['state'] == 'restoring' and claim['outcome'] is None, claim
    checks.append('[destination] the claim persisted before the command started and stayed restoring, with no outcome written')
    before_retry = inspect(B, H)
    assert interruption['scope_after_finish'] in ('inactive', 'failed'), interruption

    retry = B.ok(restore_request, 'the retry of the interrupted restore reconciled without restoring again', checks)
    assert retry['finalized_after_interruption'] is True, retry
    after_retry = inspect(B, H)
    assert before_retry and after_retry['Id'] == before_retry['Id'], 'the retry must not create a second container'
    assert after_retry['State']['RestoredAt'] == before_retry['State']['RestoredAt'], 'the retry must not restore again'
    assert len(B.call('labelled', uuid_value=H)['containers']) == 1, 'exactly one container must carry the universe label'
    outcomes = [f for f in B.call('boxes')['outbox'][H1] if f != '_mode']
    assert outcomes == ['outcome.json'], outcomes
    outcome = document(B, 'outbox', H1, 'outcome.json')
    assert outcome['result'] == 'restored' and outcome['restored_container_id'] == after_retry['Id']
    checks.append('[destination] exactly one container and one outcome after the interruption')
    log = B.call('read', path=retry['restore_log']['file'])['text']
    assert '(gitid v3.15.5.3)' in log and 'Restore finished successfully' in log
    checks.append('[destination] the preserved CRIU restore log of the interrupted restore shows a successful restore by the qualified runtime')
    continuity = memory_continued(before_checkpoint, B.call('counter', uuid_value=H, seconds=6)['samples'])
    checks.append('[destination] memory continuity across the interrupted restore: token %s, counter %s -> %s'
                  % (continuity['token'][:8], continuity['last_before_checkpoint'], continuity['last_after_restore']))
    conmon = B.call('conmon', name='podmesh-' + H)
    assert 'libpod-conmon-' in (conmon['conmon_cgroup'] or '') and B.call('scope', unit=scope_unit)['active_state'] in ('inactive', 'failed')
    checks.append('[destination] after the interrupted restore the scope is finished and conmon runs in its own libpod-conmon scope')
    restored_conmon_scope = conmon['conmon_scope']

    transfer(B, A, H1, files=('outcome.json',))
    A.ok(request('migration_complete_transfer', H, REF, authorization_id=H1), 'source completed the interrupted transfer as restored', checks)
    A.ok(request('migration_retire_source', H, REF, authorization_id=H1), 'source retired after the interrupted transfer', checks)
    assert inspect(A, H) is None
    B.ok(request('stop', H, REF, timeout_seconds=10, on_timeout='kill'), 'stop of the restored universe on the destination', checks)
    B.ok(request('delete', H, REF), 'delete of the restored universe on the destination through the API', checks)
    assert inspect(B, H) is None
    assert gone(B, restored_conmon_scope), 'the conmon scope cgroup must disappear with the deleted universe'
    checks.append('[destination] deleting the restored universe removes its container and its conmon scope cgroup')

    # ---------------------------------------------------------------- a restore that fails after Podman started
    # A damaged archive makes CRIU fail and write a log of its own into container storage; on this lab a
    # truncated memory image once produced 6.5 GB of it. The inventory is truncated instead, so the failure is
    # immediate, and the destination must still have room before the attempt.
    free = B.call('space', path='/var/lib/containers/storage')['free']
    assert free > 4 * 1024 ** 3, f'the destination has only {free} bytes free for a deliberately failing restore'
    F = str(uuid.uuid4())
    f_container, f_checkpoint, f_authorization = authorized(F, COUNTER, alpine)
    F1 = f_authorization['authorization_id']
    transfer(A, B, F1)
    archive = f'{state_dir}/inbox/{F1}/checkpoint.tar.zst'
    damaged = B.call('corrupt_archive', source=archive, target=archive, member=None)
    handoff, manifest = document(B, 'inbox', F1, 'handoff.json'), document(B, 'inbox', F1, 'manifest.json')
    forged_manifest = copy.deepcopy(manifest)
    forged_manifest['archive'].update(sha256=damaged['sha256'], bytes=damaged['bytes'])
    written = write(B, 'inbox', F1, 'manifest.json', forged_manifest)
    forged_handoff = copy.deepcopy(handoff)
    forged_handoff['archive'].update(sha256=damaged['sha256'], bytes=damaged['bytes'])
    forged_handoff['manifest']['sha256'] = written['sha256']
    write(B, 'inbox', F1, 'handoff.json', forged_handoff)
    report = B.ok(request('migration_destination_preflight', F, REF, authorization_id=F1))
    assert report['compatible'] is True, ('the damaged archive must pass every hash and structure check', report['blockers'])
    checks.append('[destination] a damaged archive delivered with matching forged documents passes preflight: hashes are not a restore guarantee')

    failing = request('migration_restore', F, REF, authorization_id=F1)
    failure = B.api(failing)
    assert failure['ok'] is False and 'could not be verified' in failure['error'], failure
    claim = [k for k in B.call('journal')['migration_restore_claims'] if k['authorization_id'] == F1][0]
    assert claim['state'] == 'restore_failed' and claim['outcome'] is None
    assert F1 not in B.call('boxes')['outbox'], 'a failed restore must write no outcome'
    checks.append('[destination] a restore that fails after Podman started holds its claim and writes no outcome: ' + failure['details']['reason'])
    failed_scope = f"podmesh-restore-{failing['operation_id']}.scope"
    assert B.call('scope', unit=failed_scope)['active_state'] in ('inactive', 'failed')
    leftover = inspect(B, F)
    conmon = B.call('conmon', name='podmesh-' + F)
    assert leftover is None or leftover['State']['Running'] is False, leftover
    assert conmon['conmon_cgroup'] is None, ('the conmon process of a failed restore must be gone', conmon)
    failed_conmon_scope = conmon['conmon_scope'] if leftover else None
    criu_log = B.call('path', path=leftover['StaticDir'] + '/restore.log') if leftover else {'exists': False}
    results['failed_restore_criu_log_bytes'] = criu_log.get('bytes')
    results['destination_free_bytes_after_failure'] = B.call('space', path='/var/lib/containers/storage')['free']
    checks.append('[destination] after the failure the restore scope is finished and the conmon process is gone (container left: %s; its empty '
                  'conmon scope cgroup still present until the container is removed: %s)'
                  % ('yes, not running' if leftover else 'no', conmon['conmon_scope_exists']))
    B.refused(request('create', F, REF, image='sha256:' + alpine, command=['true']), 'create while a restore claim is unresolved',
              'unresolved restore claim', checks)
    again = B.api(failing)
    assert again['ok'] is False and 'migration_restore_abort' in again['error'], again
    checks.append('[destination] retrying the failed restore keeps the claim held and names the abort operation')

    aborted = B.ok(request('migration_restore_abort', F, REF, authorization_id=F1), 'abort removed only what the failed claim created', checks)
    assert aborted['action'] in ('removed_restore_leftover', 'none_absent') and aborted['outcome']['result'] == 'not_restored'
    assert isinstance(aborted['runtime_processes_remaining'], int), aborted
    checks.append('[destination] the abort reports the runtime processes the failed attempt left behind (%d) instead of killing processes '
                  'PodMesh did not start' % aborted['runtime_processes_remaining'])
    if aborted['removed_container_id']:
        # Test-owned cleanup: a failed CRIU restore was observed to keep writing its log after its container
        # was removed, which fills the host disk.
        results['killed_leftover_processes'] = B.call('kill_container_processes', container_id=aborted['removed_container_id'])['killed']
    assert inspect(B, F) is None and B.call('labelled', uuid_value=F)['containers'] == []
    assert B.call('scope', unit=failed_scope)['active_state'] in ('inactive', 'failed')
    assert failed_conmon_scope is None or gone(B, failed_conmon_scope), 'the conmon scope cgroup must disappear with the removed container'
    checks.append('[destination] after the abort the universe container, its conmon scope cgroup and the restore scope are all gone')
    B.ok(request('create', F, REF, image='sha256:' + alpine, command=['true']), 'the closed claim no longer blocks generic operations', checks)
    B.ok(request('delete', F, REF))
    # The destination claimed, and reports on, the forged handoff it actually received. Its outcome is bound to
    # that handoff, so it cannot end the authorization the source issued: the source stays held, which is the
    # invariant working, and a transport that alters documents leaves a migration needing an explicit decision.
    transfer(B, A, F1, files=('outcome.json',))
    A.refused(request('migration_complete_transfer', F, REF, authorization_id=F1),
              'completion with an outcome bound to the forged handoff the destination actually restored from', 'not to this authorization', checks)
    assert A.status(F)['reservation']['state'] == 'transfer_authorized'
    checks.append('[source] a transport that altered the documents cannot end the source authorization: the reservation stays '
                  'transfer_authorized and no release exists in this version')
    A.remove_fixture('podmesh-' + F, note='reserved source of the forged-document restore, removed directly by the test')
    results['interruption'] = {'at_kill': at_kill, 'retry': retry, 'damaged_archive': damaged, 'abort': aborted}

    # ---------------------------------------------------------------- independent verification
    events = {}
    extra = {B.role: [(interruption['first_response']['begin_ns'], B.windows[-1][1])]}
    removed_by_test = {A.role: {f_container['Id']}, B.role: set()}
    for host in (A, B):
        until = int(host.call('time')['time']) + 1
        events[host.role] = event_report(host, since[host.role], until, universes, fixture_ids[host.role], extra.get(host.role, ()),
                                         removed_by_test=removed_by_test[host.role])
        assert not events[host.role]['outside_api_windows'], (host.role, events[host.role]['outside_api_windows'])
        assert host.call('snapshot')['podman'] == baseline[host.role], (host.role, 'containers, images or volumes differ from the baseline')
    checks.append('every Podman container event on API-managed universes falls inside an API request window of its host, the interrupted '
                  'restore counted from its first attempt to its completing retry: ' + json.dumps({r: e['statuses'] for r, e in events.items()}))
    checks.append('pre-existing containers, images and volumes unchanged on both hosts; no leftovers')
finally:
    for host in (A, B):
        host.cleanup()
    shutil.rmtree(control, ignore_errors=True)

print(json.dumps({'status': 'PASS', 'source_host_uuid': A.identity, 'destination_host_uuid': B.identity, 'universes': universes,
                  'checks': checks, 'check_count': len(checks), 'memory_current_bytes': memory, 'memory_continuity': continuity,
                  'interrupted_authorization_id': H1, 'failed_authorization_id': F1, 'events': events, 'results': results,
                  'reserved_universes_left_in_journals': {A.role: [F], B.role: [H]}}))
