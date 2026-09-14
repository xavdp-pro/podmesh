#!/usr/bin/env python3
"""Failed-restore cleanup on a migration destination: a bounded attempt, and a verified reclaim.

The hard failure shape is a checkpoint archive whose largest memory image is truncated, delivered with
forged documents so that it passes every hash and structure check. On this laboratory that makes CRIU
spin and write its restore log at about 20 MB/s while `podman container restore` never returns, in
cgroups that the transient scope PodMesh created does not contain. The suite measures three things:

1. **Prevention.** The attempt is stopped when it has consumed more of the Podman graph root than its own
   preflight required: the container's cgroup is frozen, which ends nothing, and the transient scope is
   stopped. The disk must stay far above the floor and the service must survive.
2. **The control case.** `migration_restore_abort` without `reclaim_processes` must report the surviving
   processes with their cgroups and start times and signal nothing at all.
3. **The reclaim.** With the explicit `reclaim_processes: true`, only processes proven to be members of
   the claimed container's own cgroups, with a start time at or after the claim, are ended; both cgroups
   must then disappear, the container must be absent, and the graph root must recover.

A successful restore is measured beside them, to show that its cgroups are never reclaim candidates.

    PODMESH_SOURCE_SSH=user@host-a PODMESH_DESTINATION_SSH=user@host-b \\
    PODMESH_SOCKET=... PODMESH_STATE_DIR=... PODMESH_UNIT=... python3 -B tests/check-migration-cleanup.py

The deliberate failure runs under a TEST-OWNED free-space watchdog (floor 6 GiB) that ends the attempt's
cgroups if the product's own bound ever fails to. That watchdog is this suite's cleanup, not a product
mechanism, and the run records whether it had to fire (it must not)."""
import copy, json, os, shutil, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, event_report, request, transfer  # noqa: E402

REF = 'disposable-lab-migration-cleanup-test'
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']
# About 512 MiB of process memory, so that the truncated memory image is a real one.
HOG = ['sh', '-c',
       'awk "$1" & token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done',
       'hog', 'BEGIN{s="0123456789abcdefghijklmnopqrstuv"; while (length(s) < 536870912) s = s s; while (1) system("sleep 5")}']
GRAPH = '/var/lib/containers/storage'
WATCHDOG_FLOOR = 6 * 1024 ** 3
# Declared recovery tolerance: the graph root must come back to within this of its pre-attempt reading.
RECOVERY_TOLERANCE = 64 * 1024 ** 2

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-cleanup-')
checks, results, space = [], {}, {}
A = Host('source', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('destination', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
assert A.identity != B.identity, 'both services report the same host UUID'
fixture_ids = {A.role: set(), B.role: set()}
universes = []


def inspect(host, u):
    return host.call('inspect', name='podmesh-' + u)['container']
def document(host, box, authorization, name):
    return json.loads(host.call('read', path=f'{state_dir}/{box}/{authorization}/{name}')['text'])
def write(host, box, authorization, name, value):
    return host.call('write_document', box=box, authorization=authorization, name=name, text=json.dumps(value, indent=2) + '\n')
def df(label):
    """A raw `df -B1` capture of the destination graph root, kept in the evidence."""
    space[label] = B.call('df', path=GRAPH)
    return space[label]['free']
def claim_of(host, authorization):
    return [k for k in host.call('journal')['migration_restore_claims'] if k['authorization_id'] == authorization][0]
def authorized(u, command, grow_to=0):
    """A universe created, started, checkpointed and authorized on the source, delivered to the destination.
    `grow_to` waits for the fixture's memory before checkpointing, so that the archive really holds one."""
    universes.append(u)
    A.ok(request('create', u, REF, image='sha256:' + alpine, network_profile='isolated', command=command))
    A.ok(request('start', u, REF))
    deadline = time.time() + 180
    while grow_to and A.call('memory', uuid_value=u)['memory_current_bytes'] < grow_to:
        assert time.time() < deadline, 'the memory fixture did not grow'
        time.sleep(.5)
    container = inspect(A, u)
    checkpoint = request('migration_checkpoint', u, REF, container_id=container['Id'], image='sha256:' + alpine,
                         source_host_uuid=A.identity, destination_host_uuid=B.identity)
    A.ok(checkpoint)
    authorization = A.ok(request('migration_authorize_transfer', u, REF, checkpoint_operation_id=checkpoint['operation_id'],
                                 destination_host_uuid=B.identity))
    transfer(A, B, authorization['authorization_id'])
    return container, checkpoint, authorization


alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']
assert alpine == B.call('image_id', reference='docker.io/library/alpine:3.22')['image'], 'the hosts do not share the alpine image ID'
baseline = {A.role: A.call('snapshot')['podman'], B.role: B.call('snapshot')['podman']}
since = {A.role: int(A.call('time')['time']) - 1, B.role: int(B.call('time')['time']) - 1}
started_at = time.time()
try:
    # ---------------------------------------------------------------- a successful restore is not a candidate
    G = str(uuid.uuid4())
    g_container, g_checkpoint, g_authorization = authorized(G, COUNTER)
    G1 = g_authorization['authorization_id']
    good = B.ok(request('migration_restore', G, REF, authorization_id=G1), 'a good archive restores normally under the same bound', checks)
    assert good['prevention']['watched'] is True and good['prevention']['stopped'] is False, good['prevention']
    assert good['prevention']['container_cgroup_frozen'] is False and good['prevention']['maximum_consumed_bytes'] >= 0
    checks.append('the bound watched the successful restore without acting: allowance %d bytes, most consumed %d, free never below %d'
                  % (good['prevention']['allowance_bytes'], good['prevention']['maximum_consumed_bytes'], good['prevention']['minimum_free_bytes']))
    g_claim = claim_of(B, G1)
    g_facts = B.call('cgroup_facts', container_id=good['container_id'])
    running_cgroups = [p for p, v in g_facts.items() if isinstance(v, dict) and v['exists']]
    assert len(running_cgroups) == 2 and g_facts['total_processes'] > 0, g_facts
    assert all(p['start_epoch'] >= g_claim['created_at'] for v in g_facts.values() if isinstance(v, dict) for p in v['processes'])
    checks.append('[destination] the verified restore owns the same two cgroups a reclaim would look at, holding %d processes started after '
                  'its claim: they are never candidates, because a verified claim is never aborted' % g_facts['total_processes'])
    B.refused(request('migration_restore_abort', G, REF, authorization_id=G1, reclaim_processes=True),
              'abort with reclaim_processes of a verified restore', 'aborting would remove the active universe', checks)
    results['successful_restore'] = {'claim': g_claim, 'cgroups': g_facts, 'prevention': good['prevention']}
    transfer(B, A, G1, files=('outcome.json',))
    A.ok(request('migration_complete_transfer', G, REF, authorization_id=G1))
    A.ok(request('migration_retire_source', G, REF, authorization_id=G1))
    B.ok(request('stop', G, REF, timeout_seconds=10, on_timeout='kill'))
    B.ok(request('delete', G, REF))

    # ---------------------------------------------------------------- the hard failure shape, bounded
    free_before = df('before_attempt')
    assert free_before > WATCHDOG_FLOOR + 4 * 1024 ** 3, f'the destination has only {free_before} bytes free for a deliberate failure'
    F = str(uuid.uuid4())
    f_container, f_checkpoint, f_authorization = authorized(F, HOG, grow_to=400 * 1024 ** 2)
    F1 = f_authorization['authorization_id']
    archive = f'{state_dir}/inbox/{F1}/checkpoint.tar.zst'
    damaged = B.call('corrupt_archive', source=archive, target=archive, member='largest-pages')
    assert damaged['member'].startswith('checkpoint/pages-'), damaged
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
    checks.append('[destination] a %d MiB memory image truncated in half, delivered with matching forged documents, passes preflight'
                  % (damaged['member_bytes_before'] // 1024 ** 2))

    failing = request('migration_restore', F, REF, authorization_id=F1)
    watched = B.call('restore_under_watchdog', request=failing, floor_bytes=WATCHDOG_FLOOR, graph=GRAPH, timeout=1800)
    B.windows.append((watched['begin_ns'], watched['end_ns'], 'migration_restore', failing['operation_id']))
    failure = watched['response']
    assert failure['ok'] is False and 'could not be verified' in failure['error'], failure
    assert watched['watchdog_fired'] is None, ('the product bound must act before this suite\'s watchdog', watched['watchdog_fired'])
    prevention = failure['details']['prevention']
    assert prevention['stopped'] is True and prevention['container_cgroup_frozen'] is True, prevention
    assert prevention['freeze_confirmed_by_observation'] is True, prevention
    assert prevention['transient_scope_stopped'] is True and 'more than' in prevention['reason'], prevention
    consumed = prevention['maximum_consumed_bytes']
    assert consumed <= prevention['allowance_bytes'] + 128 * 1024 ** 2, (consumed, prevention['allowance_bytes'])
    assert watched['minimum_free'] > WATCHDOG_FLOOR, watched['minimum_free']
    checks.append('[destination] prevention measured on the runaway restore: stopped after %d MiB consumed against an allowance of %d MiB, '
                  'graph root never below %d MiB (this suite\'s 6 GiB watchdog never fired)'
                  % (consumed // 1024 ** 2, prevention['allowance_bytes'] // 1024 ** 2, watched['minimum_free'] // 1024 ** 2))
    assert B.ok(request('migration_destination_preflight', F, REF, authorization_id=F1))['compatible'] is False
    checks.append('[destination] the service survived the runaway attempt and answers the next request')
    f_claim = claim_of(B, F1)
    assert f_claim['state'] == 'restore_failed' and f_claim['outcome'] is None
    assert F1 not in B.call('boxes')['outbox'], 'a failed restore must write no outcome'
    checks.append('[destination] the claim is held as restore_failed with no outcome written')
    leftover_container = inspect(B, F)
    assert leftover_container is not None and leftover_container['State']['Running'] is False
    frozen_id = leftover_container['Id']
    free_before_reclaim = df('before_reclaim')

    # ------------------------------------------- what a watching agent can read without running anything
    watch = B.status(F)['watch']
    claim_watch = [k for k in watch['unresolved_restore_claims'] if k['authorization_id'] == F1][0]
    assert claim_watch['state'] == 'restore_failed' and claim_watch['container_id'] == frozen_id
    assert claim_watch['runtime_processes']['source'] == 'cgroup_residency'
    assert claim_watch['runtime_processes']['authorizes_reclaim'] is True and claim_watch['runtime_processes']['count'] > 0
    assert claim_watch['prevention_stopped_the_attempt'] is True and watch['graph_root']['known'] is True
    assert all(p['started_at_or_after_claim'] is True for p in claim_watch['runtime_processes']['processes'])
    assert watch['observed_at'] and claim_watch['runtime_processes']['observed_at']
    checks.append('[destination] migration_status alone tells an observer that this claim is unresolved and since when, that the attempt it '
                  'stopped left %d processes in the container cgroups (by residency, not by command line), and how much room is left where they '
                  'were writing: %d bytes against an allowance of %s' % (claim_watch['runtime_processes']['count'],
                                                                         watch['graph_root']['available_bytes'], claim_watch['allowance_bytes']))
    results['watch'] = watch

    # ---------------------------------------------------------------- the control case: report, never signal
    before_facts = B.call('cgroup_facts', container_id=frozen_id)
    assert before_facts['total_processes'] > 0, ('the failure must leave processes to reclaim', before_facts)
    refusal = B.refused(request('migration_restore_abort', F, REF, authorization_id=F1),
                        'abort without reclaim_processes while processes of the attempt survive', 'They are reported, not ended', checks)
    reported = refusal['details']['detail']['runtime_processes']
    assert reported['source'] == 'cgroup_residency' and reported['authorizes_reclaim'] is True
    assert reported['count'] == before_facts['total_processes'], (reported['count'], before_facts['total_processes'])
    assert all(p['started_at_or_after_claim'] is True for p in reported['processes']), reported
    assert {p['pid'] for p in reported['processes']} == {p['pid'] for v in before_facts.values() if isinstance(v, dict) for p in v['processes']}
    after_facts = B.call('cgroup_facts', container_id=frozen_id)
    assert after_facts == before_facts, ('the control case must not signal anything', before_facts, after_facts)
    checks.append('[destination] without the field the abort reports %d surviving processes by cgroup residency, each with its cgroup and a '
                  'start time at or after the claim, and signals none of them: the same PIDs are still there afterwards' % reported['count'])
    results['control_no_reclaim'] = {'reported': reported, 'independent_facts': before_facts}

    # ---------------------------------------------------------------- the reclaim
    aborted = B.ok(request('migration_restore_abort', F, REF, authorization_id=F1, reclaim_processes=True),
                   'abort with reclaim_processes ended only the processes it could prove belonged to the failed attempt', checks)
    reclaim = aborted['reclaim']
    assert aborted['reclaim_processes_requested'] is True and aborted['authorization_ref'] == REF
    assert reclaim['complete'] is True and reclaim['cgroups_gone'] is True and reclaim['surviving_processes'] == 0
    assert reclaim['before']['count'] == reported['count']
    # Every candidate is either signalled on proof re-read immediately before the signal, or skipped because
    # it had already exited between the two reads. No candidate may be signalled without both proofs, and
    # none may be skipped for failing them: that would mean the cgroup held something older than the claim.
    signalled = [s for s in reclaim['signalled'] if s['decision'] == 'sigkill']
    skipped = [s for s in reclaim['signalled'] if s['decision'] != 'sigkill']
    assert signalled, reclaim['signalled']
    assert all(s['member_of_claimed_cgroup'] is True and s['started_at_or_after_claim'] is True for s in signalled), signalled
    assert all('gone' in s['result'] for s in skipped), skipped
    vanished = [s for s in reclaim['signalled'] if 'No such process' in s['result'] or 'gone' in s['result']]
    assert aborted['container_cgroups_absent'] is True and aborted['runtime_processes_remaining'] == 0
    checks.append('[destination] the reclaim proved %d processes members of the claimed container\'s own cgroups with a start time at or after '
                  'the claim, re-reading each proof immediately before its signal, and both cgroups disappeared in %.2f s'
                  % (len(signalled), reclaim['waited_seconds']))
    if vanished:
        checks.append('[destination] %d of them exited between the proof and the signal (a child of one already ended): reported as such, '
                      'never counted as a successful signal, and completeness is judged by the cgroups disappearing' % len(vanished))
    final_facts = B.call('cgroup_facts', container_id=frozen_id)
    assert all(not v['exists'] for v in final_facts.values() if isinstance(v, dict)) and final_facts['total_processes'] == 0
    assert inspect(B, F) is None and B.call('labelled', uuid_value=F)['containers'] == []
    checks.append('[destination] verified from outside the producer: both cgroup directories are gone, no process of the attempt survives, '
                  'the container is absent and no container carries the universe label')
    free_after = df('after_reclaim')
    recovered = free_after - free_before
    assert abs(recovered) <= RECOVERY_TOLERANCE, ('the graph root did not recover within the declared tolerance', free_before, free_after)
    checks.append('[destination] the graph root recovered to within %d bytes of its pre-attempt reading (declared tolerance %d MiB): '
                  '%d -> %d -> %d bytes free' % (abs(recovered), RECOVERY_TOLERANCE // 1024 ** 2, free_before, free_before_reclaim, free_after))
    written_reclaim = B.call('path', path=f"{state_dir}/migrations/{f_claim['operation_id']}")
    assert any(e.startswith('reclaim-') for e in written_reclaim['entries']), written_reclaim
    checks.append('[destination] the operation directory keeps the reclaim record beside the failure diagnostics: '
                  + ', '.join(e for e in written_reclaim['entries'] if e.startswith(('reclaim-', 'failure-', 'abort-'))))
    assert B.ok(request('create', F, REF, image='sha256:' + alpine, network_profile='isolated', command=['true']))
    B.ok(request('delete', F, REF), 'the closed claim no longer blocks generic operations on the destination', checks)
    results['reclaim'] = {'abort': aborted, 'claim': f_claim, 'watched': watched}

    # ---------------------------------------------------------------- the source is left honestly held
    transfer(B, A, F1, files=('outcome.json',))
    A.refused(request('migration_complete_transfer', F, REF, authorization_id=F1),
              'completion with an outcome bound to the forged handoff the destination actually claimed', 'not to this authorization', checks)
    assert A.status(F)['reservation']['state'] == 'transfer_authorized'
    A.refused(request('migration_release', F, REF, checkpoint_operation_id=f_checkpoint['operation_id']),
              'release of a source whose authorization is still live', 'transfer authorization(s) were issued', checks)
    checks.append('[source] a transport that altered the documents leaves the source authorized and unreleasable: the break-glass path is '
                  'still open by design, and lot M3 did not add one')
    A.remove_fixture('podmesh-' + F, note='reserved source of the forged-document restore, removed directly by the test')

    # ---------------------------------------------------------------- evidence and independent verification
    results['journal'] = {'destination': B.call('journal_text', since=started_at, lines=400)['text'][-6000:]}
    results['space'] = space
    events = {}
    removed_by_test = {A.role: {f_container['Id']}, B.role: set()}
    for host in (A, B):
        until = int(host.call('time')['time']) + 1
        events[host.role] = event_report(host, since[host.role], until, universes, fixture_ids[host.role],
                                         removed_by_test=removed_by_test[host.role])
        assert not events[host.role]['outside_api_windows'], (host.role, events[host.role]['outside_api_windows'])
        assert host.call('snapshot')['podman'] == baseline[host.role], (host.role, 'containers, images or volumes differ from the baseline')
    checks.append('every Podman container event on API-managed universes falls inside an API request window of its host: '
                  + json.dumps({r: e['statuses'] for r, e in events.items()}))
    checks.append('pre-existing containers, images and volumes unchanged on both hosts; no leftovers')
finally:
    for host in (A, B):
        host.cleanup()
    shutil.rmtree(control, ignore_errors=True)

print(json.dumps({'status': 'PASS', 'source_host_uuid': A.identity, 'destination_host_uuid': B.identity, 'universes': universes,
                  'checks': checks, 'check_count': len(checks), 'api_windows': {A.role: len(A.windows), B.role: len(B.windows)},
                  'failed_authorization_id': F1, 'successful_authorization_id': G1, 'damaged_archive': damaged,
                  'recovery_tolerance_bytes': RECOVERY_TOLERANCE, 'watchdog_floor_bytes': WATCHDOG_FLOOR,
                  'events': events, 'results': results,
                  'reserved_universes_left_in_journals': {A.role: [F], B.role: []}}))
