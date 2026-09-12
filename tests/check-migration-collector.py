#!/usr/bin/env python3
"""Garbage collection on proof: the read-only plan, the terminal reservation classes and the bounded apply.

Contract: docs/GARBAGE-COLLECTION.md. Age never justifies collection, so this suite never waits for one: it
builds the exact shapes the contract's classes describe, proves the plan sees them and changes nothing, and
then proves the apply acts only with the stated fresh proofs and refuses every exclusion.

    PODMESH_SOURCE_SSH=user@host-a PODMESH_DESTINATION_SSH=user@host-b \\
    PODMESH_SOCKET=... PODMESH_STATE_DIR=... PODMESH_UNIT=... PODMESH_BINARY_SHA256=... \\
    PODMESH_TEST_COLLECTOR_BARRIER_DIR=... \\
    python3 -B tests/check-migration-collector.py

The suite builds its own settled and open-authorization rows, so a fresh qualification journal is sufficient.
Earlier lots' rows remain read-only planning input: no apply of this suite ever names one. Every product mutation goes through the API. Direct
Podman writes are limited to uniquely named disposable fixtures of this suite and to deliberate out-of-band
acts on its own universes (removing, starting, pausing and replacing a container), all listed in the output
and excluded from the event correlation. One deliberate journal forgery, on this suite's own authorization
row, exercises the refusal of a record that no longer binds; the original bytes are restored and checked.
"""
import copy, hashlib, json, os, shutil, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import (Host, event_report, memory_continued, request, transfer,
                               signal_metric_counts, validate_predelegation_refusal)  # noqa: E402

REF = 'disposable-lab-migration-collector-test'
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']
# About 512 MiB of process memory, so that a truncated memory image is a real one and the restore runs away.
HOG = ['sh', '-c',
       'awk "$1" & token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done',
       'hog', 'BEGIN{s="0123456789abcdefghijklmnopqrstuv"; while (length(s) < 536870912) s = s s; while (1) system("sleep 5")}']
GRAPH = '/var/lib/containers/storage'
WATCHDOG_FLOOR = 6 * 1024 ** 3
CLASS1 = 'terminal_reservation_all_authorizations_not_restored'
CLASS2 = 'terminal_reservation_container_absent'
CLASS3 = 'failed_restore_claim'

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
binary_sha256 = os.environ['PODMESH_BINARY_SHA256']
collector_barrier_dir = os.environ['PODMESH_TEST_COLLECTOR_BARRIER_DIR']
control = tempfile.mkdtemp(prefix='podmesh-collector-')
checks, results = [], {}
A = Host('source', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('destination', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
assert A.identity != B.identity, 'both services report the same host UUID'
service_identity = {A.role: A.attest(binary_sha256, collector_barrier_dir), B.role: B.attest(binary_sha256)}
fixture_ids = {A.role: set(), B.role: set()}
universes, out_of_band = [], {}
cleanup_evidence = {}


def inspect(host, u):
    return host.call('inspect', name='podmesh-' + u)['container']
def counter(host, u, seconds=3.0):
    return host.call('counter', uuid_value=u, seconds=seconds)['samples']
def document(host, box, authorization, name):
    return json.loads(host.call('read', path=f'{state_dir}/{box}/{authorization}/{name}')['text'])
def write(host, box, authorization, name, value):
    return host.call('write_document', box=box, authorization=authorization, name=name, text=json.dumps(value, indent=2) + '\n')
def collection(operation, **extra):
    """A collector request: host-wide, so it names no universe."""
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=REF, **extra)
def candidate(record, key):
    return next((c for c in record['candidates'] if c['key'] == key), None)
def runs(host):
    return host.call('gc_runs')['count']
def fixture(host, name, *args):
    host.fixture(name, *args)
    fixture_ids[host.role].add(host.ledger[name]['container_id'])
    return name


def forge(host, authorization, damaged):
    """Transport-controller forgery: the delivered documents are rewritten to match a damaged archive, so
    that it passes every hash and structure check and only the runtime discovers the damage."""
    handoff, manifest = document(host, 'inbox', authorization, 'handoff.json'), document(host, 'inbox', authorization, 'manifest.json')
    forged_manifest = copy.deepcopy(manifest)
    forged_manifest['archive'].update(sha256=damaged['sha256'], bytes=damaged['bytes'])
    written = write(host, 'inbox', authorization, 'manifest.json', forged_manifest)
    forged_handoff = copy.deepcopy(handoff)
    forged_handoff['archive'].update(sha256=damaged['sha256'], bytes=damaged['bytes'])
    forged_handoff['manifest']['sha256'] = written['sha256']
    return write(host, 'inbox', authorization, 'handoff.json', forged_handoff)


def checkpointed(host, u, destination, command=COUNTER, samples=False, grow_to=0):
    """A started universe, checkpointed for a recorded destination. Returns its container, the create and
    checkpoint operation IDs, and the counter samples taken before the checkpoint stopped it."""
    universes.append(u)
    create = request('create', u, REF, image='sha256:' + alpine, command=command)
    host.ok(create)
    host.ok(request('start', u, REF))
    deadline = time.time() + 180
    while grow_to and host.call('memory', uuid_value=u)['memory_current_bytes'] < grow_to:
        assert time.time() < deadline, 'the memory fixture did not grow'
        time.sleep(.5)
    container = inspect(host, u)
    before = counter(host, u) if samples else []
    checkpoint = request('migration_checkpoint', u, REF, container_id=container['Id'], image='sha256:' + alpine,
                         source_host_uuid=host.identity, destination_host_uuid=destination)
    host.ok(checkpoint)
    return container, create['operation_id'], checkpoint['operation_id'], before


def declined(u, samples=False):
    """The contract's class 1 shape, built honestly: the destination is asked, it declines the authorization
    it never claimed, and the source completes with that verified not_restored outcome. The reservation is
    checkpointed again with every authorization it ever emitted recorded as ended_not_restored — and lot M3
    left it with no way out, which is why the collector exists."""
    container, create, checkpoint, before = checkpointed(A, u, B.identity, samples=samples)
    authorization = A.ok(request('migration_authorize_transfer', u, REF, checkpoint_operation_id=checkpoint,
                                 destination_host_uuid=B.identity))['authorization_id']
    decline(u, authorization)
    return container, create, checkpoint, authorization, before


def decline(u, authorization):
    """The destination declines an authorization it never claimed; the source completes it as not_restored."""
    transfer(A, B, authorization, files=('handoff.json',))
    B.ok(request('migration_restore_abort', u, REF, authorization_id=authorization))
    transfer(B, A, authorization, files=('outcome.json',))
    A.ok(request('migration_complete_transfer', u, REF, authorization_id=authorization))
    assert A.status(u)['reservation']['state'] == 'checkpointed'


def plan_without_effect(host, note, **extra):
    """A plan, with the proof that it was read-only: no Podman event at all on that host during its window,
    and the migration tables, the delivered documents and every container identical afterwards."""
    before, before_runs = host.call('snapshot'), runs(host)
    # Let anything an earlier operation was still finishing land outside the window this plan is judged on.
    time.sleep(2)
    since = int(host.call('time')['time'])
    record = host.ok(collection('garbage_collect_plan', **extra))
    until = int(host.call('time')['time']) + 1
    events = host.call('events', since=since, until=until)['events']
    after = host.call('snapshot')
    assert not events, (host.role, 'a plan produced Podman events', events)
    assert after == before, (host.role, 'a plan changed state', [k for k in before if before[k] != after.get(k)])
    assert runs(host) == before_runs + 1, 'the plan must record exactly one run'
    checks.append('[%s] %s: %d candidates examined, %d collectable, no Podman event in the window, migration tables, '
                  'delivered documents and containers byte-identical afterwards'
                  % (host.role, note, record['counts']['examined'], record['counts']['collectable']))
    return record


alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']
assert alpine == B.call('image_id', reference='docker.io/library/alpine:3.22')['image'], 'the hosts do not share the alpine image ID'
baseline = {A.role: A.call('snapshot')['podman'], B.role: B.call('snapshot')['podman']}
since = {A.role: int(A.call('time')['time']) - 1, B.role: int(B.call('time')['time']) - 1}
capabilities = A.ok({'operation': 'capabilities'})
assert 'garbage_collect_plan' in capabilities['experimental_operations']
assert 'garbage_collect_apply' in capabilities['experimental_operations']
checks.append('the installed build advertises garbage_collect_plan and garbage_collect_apply with their contracts')
try:
    # ---------------------------------------------------------------- the shapes the contract describes
    U1, U2, U5 = str(uuid.uuid4()), str(uuid.uuid4()), str(uuid.uuid4())
    U3, U4 = str(uuid.uuid4()), str(uuid.uuid4())
    # Self-contained host-wide context: one settled reservation and one reservation blocked by an issued
    # authorization. Earlier suites may leave the same shapes, but this suite never depends on their state.
    SETTLED, OPEN = str(uuid.uuid4()), str(uuid.uuid4())
    settled_container, _, settled_checkpoint, _ = checkpointed(A, SETTLED, str(uuid.uuid4()))
    A.ok(request('migration_release', SETTLED, REF, checkpoint_operation_id=settled_checkpoint))
    A.ok(request('delete', SETTLED, REF))
    open_container, _, open_checkpoint, _ = checkpointed(A, OPEN, str(uuid.uuid4()))
    open_authorization = A.ok(request('migration_authorize_transfer', OPEN, REF,
                                      checkpoint_operation_id=open_checkpoint,
                                      destination_host_uuid=A.status(OPEN)['reservation']['destination_host_uuid']))['authorization_id']
    removed_open = A.remove_fixture('podmesh-' + OPEN,
                                    note='suite-owned source removed after issuing an authorization to build an explicit open blocker')
    assert removed_open['verified'] and removed_open['absent'], removed_open
    out_of_band[OPEN] = 'suite-owned authorized source removed directly after its exact ledger identity was verified'
    c1, create1, ck1, auth1, before_u1 = declined(U1, samples=True)
    c2, create2, ck2, auth2, _ = declined(U2)
    c5, create5, ck5, auth5, _ = declined(U5)
    A.refused(request('migration_release', U1, REF, checkpoint_operation_id=ck1),
              'release of a reservation whose only authorization ended not_restored (the dead end lot M3 named)',
              'transfer authorization(s) were issued', checks)
    A.refused(request('migration_abandon', U1, REF, checkpoint_operation_id=ck1),
              'abandonment of that same reservation', 'transfer authorization(s) were issued', checks)
    c3, create3, ck3, _ = checkpointed(A, U3, str(uuid.uuid4()))
    c4, create4, ck4, _ = checkpointed(A, U4, str(uuid.uuid4()))
    # A reservation that is neither class: its container is still there and it never told another host
    # anything, so it is a release or an abandonment decision, and the collector must say so.
    U6 = str(uuid.uuid4())
    c6, create6, ck6, _ = checkpointed(A, U6, str(uuid.uuid4()))
    for u in (U3, U4):
        A.remove_fixture('podmesh-' + u, note='reserved container removed directly to build the contract class 2 shape')
        out_of_band[u] = 'reserved container removed directly with Podman (no authorization was ever issued for it)'
    assert inspect(A, U3) is None and inspect(A, U4) is None
    checks.append('two class 1 shapes (every authorization ended not_restored, container still checkpointed) and two class 2 shapes '
                  '(container gone before any authorization) built through the API on the source')

    # ---------------------------------------------------------------- the plan is read-only
    survey = plan_without_effect(A, 'host-wide plan over the whole source journal', max_candidates=100)
    results['survey'] = {'counts': survey['counts'], 'limits': survey['limits'],
                         'settled': survey['settled_reservations']['count']}
    blocked = [c for c in survey['candidates'] if not c['collectable']]
    open_candidate = candidate(survey, OPEN)
    assert open_candidate is not None and any('still open (state issued)' in b for b in open_candidate['blockers']), open_candidate
    assert any(s['universe_uuid'] == SETTLED for s in survey['settled_reservations']['states']) and all(
        c['key'] not in [s['universe_uuid'] for s in survey['settled_reservations']['states']] for c in survey['candidates'])
    checks.append('the host-wide plan examined %d of the %d reservations that still owe a decision and found %d collectable and %d blocked, '
                  'each with its reason; the %d settled reservations are counted and never examined'
                  % (survey['limits']['reservations_examined'], survey['limits']['reservations_open'], survey['counts']['collectable'],
                     len(blocked), survey['settled_reservations']['count']))

    # A plan may also be scoped to exactly the universes a caller is deciding about, which is what an
    # operator preparing a collection does, and what a journal larger than any single bound requires.
    universes_of_this_suite = [U1, U2, U3, U4, U5, U6]
    plan = plan_without_effect(A, 'plan scoped to this suite\'s own universes', universe_uuids=universes_of_this_suite)
    plan_id = plan['collection_operation_id']
    assert plan['scope']['universe_uuids'] == universes_of_this_suite
    assert {c['key'] for c in plan['candidates']} == set(universes_of_this_suite)
    results['plan'] = {'counts': plan['counts'], 'scope': plan['scope']}
    assert plan['mode'] == 'plan' and plan['collector_version'] and plan['policy_version']
    assert plan['effects'].startswith('none'), plan['effects']
    one = candidate(plan, U1)
    assert one['class'] == CLASS1 and one['class_number'] == 1 and one['collectable'] is True, one
    assert one['proofs']['container']['same_reserved_container'] is True and one['proofs']['container']['checkpointed'] is True
    assert one['proofs']['container']['state'] == 'exited' and one['proofs']['container']['cgroup_frozen'] is False
    assert one['proofs']['outcomes'][0]['binds_to_this_authorization'] is True
    assert one['proofs']['outcomes'][0]['result'] == 'not_restored' and one['proofs']['open_authorizations'] == 0
    assert one['proposed_effect']['action'] == 'collect_reservation' and one['proposed_effect']['starts_nothing'] is True
    checks.append('the plan classifies a checkpointed reservation whose authorization ended not_restored as class 1, with the outcome '
                  're-hashed and re-bound to its handoff, the source observed stopped and checkpointed, and no open authorization')
    two = candidate(plan, U3)
    assert two['class'] == CLASS2 and two['class_number'] == 2 and two['collectable'] is True, two
    assert two['proofs']['container']['present'] is False
    assert two['proofs']['container']['recorded_container_present_under_another_name'] is False
    assert two['proofs']['authorizations_of_this_reservation'] == 0
    checks.append('and a reservation whose container is absent by name and by recorded ID, with no authorization ever issued, as class 2')
    neither = candidate(plan, U6)
    assert neither['class'] is None and neither['collectable'] is False, neither
    assert any('release or an abandonment decision' in b for b in neither['blockers']), neither['blockers']
    checks.append('a reservation whose container is still there and that never issued an authorization is reported as the decision it really '
                  'needs — a release or an abandonment — and never widened into a collection class')
    small_request = collection('garbage_collect_plan', max_candidates=3)
    small = A.ok(small_request)
    assert len(small['candidates']) == 3 and small['limits']['truncated'] is True
    checks.append('the enumeration is bounded: max_candidates 3 examines exactly 3 and reports the truncation')
    historical = A.ok(dict(small_request))
    assert historical['replayed'] and historical['historical'], 'a repeated plan operation ID must be historical'
    checks.append('a repeated plan operation ID returns its record as history, with a fresh observation beside it')

    # ---------------------------------------------------------------- class 1 applied, with its tombstone
    first = collection('garbage_collect_apply', plan_operation_id=plan_id, candidates=[{'class': CLASS1, 'universe_uuid': U1}])
    applied = A.ok(first, 'class 1 collected: the reservation is terminal, the tombstone is written, nothing was started', checks)
    effect = applied['results'][0]
    assert applied['effects_applied'] == 1 and applied['runtime_reclaims_reserved'] == 0 and applied['verified'] is True
    assert effect['applied'] is True and effect['verified'] is True and effect['verification_blockers'] == []
    assert effect['reservation']['state'] == 'collected' and effect['collected_from_state'] == 'checkpointed'
    assert effect['tombstone']['class'] == CLASS1 and effect['tombstone']['container_absent_at_collection'] is False
    assert effect['verified_from_outside']['create_of_this_universe_uuid_refused'] is True
    assert effect['verified_from_outside']['generic_operations_refused'] is False
    assert effect['verified_from_outside']['container_unchanged_by_the_collection'] is True
    after = inspect(A, U1)
    assert after['Id'] == c1['Id'] and after['State']['Checkpointed'] is True and after['State']['Running'] is False
    checks.append('the collected source is exactly the container it was: same ID, still checkpointed, still stopped — a collection is a '
                  'recorded decision, not a Podman action')
    status = A.status(U1)
    assert status['reservation']['state'] == 'collected' and status['tombstone']['class'] == CLASS1
    assert status['watch']['reservation']['blocks_generic_operations'] is False
    assert status['watch']['reservation']['awaiting_decision'] is False
    checks.append('migration_status reports the tombstone and tells a watcher the reservation no longer blocks anything and owes no decision')
    A.refused(request('create', U1, REF, image='sha256:' + alpine, command=['true']),
              'create reusing a collected universe UUID', 'was collected on this host', checks)
    source_for_clone = str(uuid.uuid4())
    universes.append(source_for_clone)
    A.ok(request('create', source_for_clone, REF, image='sha256:' + alpine, command=['sleep', '3600']))
    A.refused(request('clone', U1, REF, source_uuid=source_for_clone),
              'clone into a collected universe UUID', 'was collected on this host', checks)
    A.ok(request('delete', source_for_clone, REF))

    # The contract's class 1 says a later local memory restore stays a separate explicit operation: prove it.
    assert A.status(U1)['recovery']['restore_local']['permitted'] is True
    local = A.ok(request('migration_restore_local', U1, REF, checkpoint_operation_id=ck1),
                 'the memory a collection released is still restorable: migration_restore_local resumed it in place', checks)
    assert local['in_place'] is True and local['memory_source'] == 'kept_checkpoint_files'
    continuity = memory_continued(before_u1, counter(A, U1, seconds=6))
    checks.append('memory continuity after collecting and then restoring locally, observed from outside the universe: same memory-only '
                  'token %s, counter %s -> %s' % (continuity['token'][:8], continuity['last_before_checkpoint'], continuity['last_after_restore']))
    status = A.status(U1)
    assert status['reservation'] is None and status['reservation_history'][-1]['state'] == 'restored_locally'
    assert status['tombstone']['class'] == CLASS1, 'the tombstone survives the local restore'
    A.refused(request('create', U1, REF, image='sha256:' + alpine, command=['true']),
              'create of that universe UUID even after a verified local restore brought it back', 'was collected on this host', checks)
    checks.append('the tombstone is not lifted by the restore: the identity can be operated and restored, never blindly created again')

    # A tombstone protects identity without preventing later legitimate cycles. Cycle 2 follows the local
    # memory restore. Cycle 3 follows an ordinary start of the collected source and archives the previous
    # collected reservation when the new checkpoint begins.
    repeated_cycles = []
    first_tombstone = copy.deepcopy(A.status(U1)['tombstone'])
    for cycle in (2, 3):
        running = inspect(A, U1)
        ck_request = request('migration_checkpoint', U1, REF, container_id=running['Id'], image='sha256:' + alpine,
                             source_host_uuid=A.identity, destination_host_uuid=B.identity)
        A.ok(ck_request)
        authorization = A.ok(request('migration_authorize_transfer', U1, REF,
                                     checkpoint_operation_id=ck_request['operation_id'],
                                     destination_host_uuid=B.identity))['authorization_id']
        decline(U1, authorization)
        repeat_plan = A.ok(collection('garbage_collect_plan', max_candidates=100, universe_uuids=[U1]))
        assert candidate(repeat_plan, U1)['collectable'] is True
        repeat_apply = collection('garbage_collect_apply', plan_operation_id=repeat_plan['collection_operation_id'],
                                  candidates=[{'class': CLASS1, 'universe_uuid': U1}])
        collected = A.ok(repeat_apply)
        assert collected['effects_applied'] == 1 and collected['results'][0]['verified'] is True
        tombstone = A.status(U1)['tombstone']
        assert tombstone['collected_by_operation'] == first_tombstone['collected_by_operation']
        assert tombstone['proof'] == first_tombstone['proof'], 'later collection must not replace the original proof'
        history = [row for row in A.call('journal')['migration_collection_history'] if row['universe_uuid'] == U1]
        assert len(history) == cycle, (cycle, history)
        assert len({row['collected_by_operation'] for row in history}) == cycle
        before_replay = A.call('snapshot')
        assert A.ok(repeat_apply)['replayed'] is True
        assert A.call('snapshot') == before_replay, 'a repeated apply must not append another collection occurrence'
        repeated_cycles.append({'cycle': cycle, 'checkpoint': ck_request['operation_id'],
                                'collection': repeat_apply['operation_id'], 'history_count': len(history)})
        A.ok(request('start', U1, REF))
        assert inspect(A, U1)['State']['Running'] is True
    checks.append('three collections of the same universe retain all occurrences and the original tombstone; new checkpoints work after '
                  'local memory restore and ordinary start; replay repeats no effect')
    results['repeated_collection_cycles'] = repeated_cycles

    A.ok(request('stop', U1, REF, timeout_seconds=10, on_timeout='kill'))
    A.ok(request('delete', U1, REF), 'the universe a collection released is stopped and deleted through the ordinary API', checks)
    assert inspect(A, U1) is None
    results['class1'] = {'apply': applied, 'restore_local': {k: local[k] for k in ('in_place', 'memory_source', 'container_id')}}

    # ---------------------------------------------------------------- what an apply refuses
    A.refused(collection('garbage_collect_apply', plan_operation_id=plan_id,
                         candidates=[{'class': CLASS2, 'universe_uuid': str(uuid.uuid4())}]),
              'apply naming a candidate the plan never examined', 'did not examine', checks)
    A.refused(collection('garbage_collect_apply', plan_operation_id=plan_id, candidates=[{'class': CLASS2, 'universe_uuid': U2}]),
              'apply naming a candidate under a class the plan did not give it', 'classified', checks)
    A.refused(collection('garbage_collect_apply', plan_operation_id=str(uuid.uuid4()),
                         candidates=[{'class': CLASS1, 'universe_uuid': U2}]),
              'apply naming a plan this host never recorded', 'No garbage collection run', checks)
    A.refused(collection('garbage_collect_apply', plan_operation_id=first['operation_id'],
                         candidates=[{'class': CLASS1, 'universe_uuid': U2}]),
              'apply naming an apply run instead of a plan', 'not as a plan', checks)
    A.refused(collection('garbage_collect_apply', plan_operation_id=plan_id, candidates=[{'class': CLASS1, 'universe_uuid': U2}],
                         max_candidates=0),
              'apply naming more candidates than its own max_candidates bound', 'exceed the max_candidates bound', checks)

    # An authorization issued after the plan: the proofs are repeated, so the race is caught.
    reauthorized = A.ok(request('migration_authorize_transfer', U2, REF, checkpoint_operation_id=ck2,
                                destination_host_uuid=B.identity))['authorization_id']
    A.refused(collection('garbage_collect_apply', plan_operation_id=plan_id, candidates=[{'class': CLASS1, 'universe_uuid': U2}]),
              'apply of a candidate whose reservation was authorized again after the plan found it collectable',
              'still open (state issued)', checks)
    decline(U2, reauthorized)
    second_plan = A.ok(collection('garbage_collect_plan', universe_uuids=[U2]))
    again = candidate(second_plan, U2)
    assert again['class'] == CLASS1 and again['collectable'] is True and again['proofs']['authorizations_of_this_reservation'] == 2
    assert all(o['binds_to_this_authorization'] is True for o in again['proofs']['outcomes'])
    checks.append('a reservation with two authorizations, both ended not_restored with an outcome that still binds, is class 1 again: '
                  'the class is about every authorization it ever emitted, not about the last one')

    # A running, then a paused universe: neither is collectable, whatever the plan said a moment ago.
    A.call('podman_run', args=['start', 'podmesh-' + U2])
    out_of_band[U2] = 'reserved container started, paused, unpaused and stopped directly with Podman to exercise the running and paused refusals'
    A.refused(collection('garbage_collect_apply', plan_operation_id=second_plan['collection_operation_id'],
                         candidates=[{'class': CLASS1, 'universe_uuid': U2}]),
              'apply of a candidate whose universe is running', 'in state running', checks)
    A.call('podman_run', args=['pause', 'podmesh-' + U2])
    A.refused(collection('garbage_collect_apply', plan_operation_id=second_plan['collection_operation_id'],
                         candidates=[{'class': CLASS1, 'universe_uuid': U2}]),
              'apply of a candidate whose universe is paused', 'in state paused', checks)
    A.call('podman_run', args=['unpause', 'podmesh-' + U2])
    A.call('podman_run', args=['stop', '--time', '0', 'podmesh-' + U2])
    deadline = time.time() + 30
    while inspect(A, U2)['State']['Running'] and time.time() < deadline:
        time.sleep(.2)
    A.refused(collection('garbage_collect_apply', plan_operation_id=second_plan['collection_operation_id'],
                         candidates=[{'class': CLASS1, 'universe_uuid': U2}]),
              'apply of a candidate whose universe ran and is no longer in its checkpointed state', 'no longer in its checkpointed state', checks)
    checks.append('a source that ran behind PodMesh\'s back is never collected as if it were still the checkpointed one, even when a '
                  'recorded plan found it collectable a moment earlier')

    # A journal record that no longer binds: the collector re-hashes and re-checks the documents themselves.
    row = A.call('journal_row', table='migration_authorizations', key_column='authorization_id', key=auth5)['rows'][0]
    forged = copy.deepcopy(json.loads(row['outcome']))
    forged['handoff_sha256'] = 'f' * 64
    text = json.dumps(forged, indent=2) + '\n'
    A.call('journal_write', table='migration_authorizations', key_column='authorization_id', key=auth5,
           values={'outcome': text, 'outcome_sha256': hashlib.sha256(text.encode()).hexdigest()})
    tampered = A.ok(collection('garbage_collect_plan', universe_uuids=[U5]))
    assert candidate(tampered, U5)['collectable'] is False
    assert any('does not bind to this authorization' in b for b in candidate(tampered, U5)['blockers'])
    A.refused(collection('garbage_collect_apply', plan_operation_id=plan_id, candidates=[{'class': CLASS1, 'universe_uuid': U5}]),
              'apply of a candidate whose recorded outcome is bound to another handoff', 'does not bind to this authorization', checks)
    A.call('journal_write', table='migration_authorizations', key_column='authorization_id', key=auth5, values={'outcome': None})
    A.refused(collection('garbage_collect_apply', plan_operation_id=plan_id, candidates=[{'class': CLASS1, 'universe_uuid': U5}]),
              'apply of a candidate whose outcome document is missing from the journal', 'no recorded outcome document', checks)
    restored_row = A.call('journal_write', table='migration_authorizations', key_column='authorization_id', key=auth5,
                          values={'outcome': row['outcome'], 'outcome_sha256': row['outcome_sha256']})['rows'][0]
    assert restored_row == row, 'the suite must put back exactly the bytes it forged over'
    checks.append('the class 1 proof is the document, not the state column: an outcome rewritten to name another handoff, and an outcome '
                  'removed from the journal, are both refused; the original bytes were restored and compared')

    # A replaced container under a collected candidate's name.
    forgery = fixture(A, 'podmesh-' + U4, '--label', f'io.podmesh.universe={U4}',
                      '--label', 'io.podmesh.creation-operation=' + str(uuid.uuid4()), alpine, 'sleep', '3600')
    A.refused(collection('garbage_collect_apply', plan_operation_id=plan_id, candidates=[{'class': CLASS2, 'universe_uuid': U4}]),
              'apply of a class 2 candidate whose universe name is now occupied by another container', 'not as ' + CLASS2, checks)
    A.remove_fixture(forgery)
    out_of_band[U4] = 'a forged container was created and removed under the universe name to exercise the replaced-source refusal'

    # ---------------------------------------------------------------- class 2, and the bound on effects
    third_plan = A.ok(collection('garbage_collect_plan', universe_uuids=[U3, U4, U5, U6]))
    third_id = third_plan['collection_operation_id']
    for u in (U3, U4):
        assert candidate(third_plan, u)['class'] == CLASS2 and candidate(third_plan, u)['collectable'] is True
    assert candidate(third_plan, U5)['collectable'] is True, 'the restored journal row makes U5 collectable again'
    bounded = A.ok(collection('garbage_collect_apply', plan_operation_id=third_id, max_effects=1,
                              candidates=[{'class': CLASS2, 'universe_uuid': U3}, {'class': CLASS2, 'universe_uuid': U4}]),
                   'a bounded apply: two candidates named, max_effects 1, one collected and the rest left for another run', checks)
    assert bounded['effects_applied'] == 1 and bounded['results'][0]['universe_uuid'] == U3
    assert bounded['stopped_before_the_rest']['candidate']['universe_uuid'] == U4
    assert 'maximum of 1 effect' in bounded['stopped_before_the_rest']['reason']
    assert A.status(U4)['reservation']['state'] == 'checkpointed' and A.status(U4)['tombstone'] is None
    assert A.status(U3)['reservation']['state'] == 'collected'
    assert A.status(U3)['tombstone']['container_absent_at_collection'] is True
    rest = A.ok(collection('garbage_collect_apply', plan_operation_id=third_id, max_effects=2,
                           candidates=[{'class': CLASS2, 'universe_uuid': U4}, {'class': CLASS1, 'universe_uuid': U5}]),
                'the same plan applied again for the rest: the second class 2 candidate and a class 1 one, both verified', checks)
    assert rest['effects_applied'] == 2 and all(r['verified'] is True for r in rest['results'])
    assert [r['class'] for r in rest['results']] == [CLASS2, CLASS1]
    for u in (U3, U4):
        A.refused(request('create', u, REF, image='sha256:' + alpine, command=['true']),
                  f'create reusing the collected universe UUID of a class 2 collection ({u[:8]})', 'was collected on this host', checks)
    A.refused(collection('garbage_collect_apply', plan_operation_id=third_id, candidates=[{'class': CLASS2, 'universe_uuid': U3}]),
              'a second collection of an already collected reservation, under a new operation ID', 'no longer hold', checks)
    checks.append('an already collected reservation is refused again by its own fresh proofs, not only by the replay of its operation ID')

    # An old container coming back under a collected name never regains ownership through its creation.
    returning = fixture(A, 'podmesh-' + U3, '--label', f'io.podmesh.universe={U3}',
                        '--label', 'io.podmesh.creation-operation=' + create3, alpine, 'sleep', '3600')
    out_of_band[U3] = 'a container carrying the original creation label was recreated and removed under the collected universe name'
    A.refused(request('start', U3, REF), 'start of a container recreated under a collected universe name with its original creation label',
              'does not match its recorded creation', checks)
    A.refused(request('delete', U3, REF), 'delete of that same container', 'does not match its recorded creation', checks)
    A.remove_fixture(returning)
    checks.append('a container recreated under a collected universe name, carrying the very creation operation the collected universe was '
                  'created by, is refused: the identity is tombstoned and the old creation grants nothing')

    # ---------------------------------------------------------------- the replay of an apply
    before_replay = A.call('snapshot')
    replayed = A.ok(dict(first))
    assert replayed['replayed'] is True and replayed['historical'] is True
    assert replayed['original_result'] == applied, 'a replayed apply returns its historical record'
    assert replayed['current']['candidates'][0]['tombstone']['class'] == CLASS1
    assert A.call('snapshot') == before_replay, 'a replayed apply must repeat no effect'
    checks.append('a replayed apply operation ID returns its historical record with a fresh observation, and changes nothing at all')
    results['class2'] = {'bounded': bounded, 'rest': rest}

    # ------------------------------- a real daemon kill after effect commit and before durable completion
    # A root-owned, operation-specific test barrier is emitted only after the class 2 domain state and its
    # pending effect row commit atomically. The node helper validates that marker and the SQLite state, kills
    # only the attested qualification unit MainPID, and proves systemd restarted a different PID.
    R = str(uuid.uuid4())
    r_container, r_create, r_checkpoint, _ = checkpointed(A, R, str(uuid.uuid4()))
    A.remove_fixture('podmesh-' + R, note='reserved container removed directly to build a class 2 shape for the recovery test')
    out_of_band[R] = 'reserved container removed directly with Podman (no authorization was ever issued for it)'
    r_plan = A.ok(collection('garbage_collect_plan', universe_uuids=[R]))
    interrupted_request = collection('garbage_collect_apply', plan_operation_id=r_plan['collection_operation_id'],
                                     candidates=[{'class': CLASS2, 'universe_uuid': R}])
    interruption = A.call('interrupt_collector_after_effect', request=interrupted_request,
                          barrier_dir=collector_barrier_dir,
                          expected_main_pid=service_identity[A.role]['main_pid'],
                          expected_binary_sha256=binary_sha256)
    first_window = interruption['first_response']
    A.windows.append((first_window['begin_ns'], first_window['end_ns'], 'garbage_collect_apply', interrupted_request['operation_id']))
    assert first_window['response'].get('interrupted') is True, first_window
    assert interruption['main_pid_before'] == service_identity[A.role]['main_pid']
    assert interruption['main_pid_after'] != interruption['main_pid_before']
    assert interruption['binary_sha256'] == binary_sha256
    interrupted_state = interruption['state_after_restart']
    assert interrupted_state == interruption['state_at_barrier'], 'restart changed the committed pending effect'
    checks.append('[source] the real qualification daemon was SIGKILLed only after the exact class 2 domain effect and pending progress row '
                  'were externally visible; systemd restarted the same frozen binary under a different MainPID')
    restarted_identity = A.attest(binary_sha256, collector_barrier_dir)
    assert restarted_identity['main_pid'] == interruption['main_pid_after']
    before_recovery = A.call('snapshot')
    recovered = A.ok(dict(interrupted_request),
                     'the request interrupted by a real daemon SIGKILL recovered its pending effect instead of repeating it', checks)
    assert recovered['effects_applied'] == 1 and recovered['effects_recovered'] == 1
    assert recovered['results'][0]['recovered'] is True and recovered['results'][0]['applied'] is True
    assert recovered['results'][0]['verified'] is True and recovered['status'] == 'verified'
    assert A.call('snapshot') == before_recovery, 'a recovered effect must change nothing at all'
    occurrences = [h for h in A.call('journal')['migration_collection_history'] if h['universe_uuid'] == R]
    assert len(occurrences) == 1, ('the effect must be recorded exactly once', occurrences)
    assert A.call('gc_runs')['count'] > 0 and A.call('journal_row', table='garbage_collection_runs',
                                                     key_column='operation_id', key=interrupted_request['operation_id'])['rows']
    historical_after_recovery = A.ok(dict(interrupted_request))
    assert historical_after_recovery['replayed'] is True and historical_after_recovery['historical'] is True
    assert A.call('snapshot') == before_recovery, 'historical replay after recovery repeated an effect'
    checks.append('after restart and retry, the journal, response and observed state agree: one collection occurrence, one tombstone, one '
                  'verified run, one recovered effect and no repeated effect on a later historical replay')
    results['recovery'] = {'interruption': interruption, 'recovered': recovered['results'][0],
                           'historical_status': historical_after_recovery['original_result']['status']}

    # ------------------------------- a collected universe can live and migrate again, and be collected again
    A.ok(request('start', U5, REF), 'a collected universe starts again through the ordinary API, without its checkpointed memory', checks)
    assert A.ok(request('stop', U5, REF, timeout_seconds=10, on_timeout='kill'))
    A.ok(request('start', U5, REF))
    u5_again = request('migration_checkpoint', U5, REF, container_id=inspect(A, U5)['Id'], image='sha256:' + alpine,
                       source_host_uuid=A.identity, destination_host_uuid=B.identity)
    A.ok(u5_again, 'a collected reservation does not block a new checkpoint: it is archived with its history and the universe reserves afresh',
         checks)
    status = A.status(U5)
    assert status['reservation']['operation_id'] == u5_again['operation_id'] and status['reservation']['state'] == 'checkpointed'
    assert [h for h in status['reservation_history'] if h['state'] == 'collected'], status['reservation_history']
    assert status['tombstone']['class'] == CLASS1, 'the identity protection survives the new reservation'
    A.refused(request('create', U5, REF, image='sha256:' + alpine, command=['true']),
              'create of a collected universe UUID that is checkpointed again (the new reservation refuses first, the tombstone behind it)',
              'is reserved by migration operation', checks)
    decline(U5, A.ok(request('migration_authorize_transfer', U5, REF, checkpoint_operation_id=u5_again['operation_id'],
                             destination_host_uuid=B.identity))['authorization_id'])
    cycle_plan = A.ok(collection('garbage_collect_plan', universe_uuids=[U5]))
    assert candidate(cycle_plan, U5)['class'] == CLASS1 and candidate(cycle_plan, U5)['collectable'] is True
    second_cycle = A.ok(collection('garbage_collect_apply', plan_operation_id=cycle_plan['collection_operation_id'],
                                   candidates=[{'class': CLASS1, 'universe_uuid': U5}]),
                        'the same universe is collected a second time: a new occurrence is recorded and the first proof is not overwritten',
                        checks)
    effect = second_cycle['results'][0]
    assert effect['verified'] is True and effect['tombstone_written_by_this_collection'] is False
    history = A.status(U5)['collection_history']
    assert len(history) == 2 and history[0]['collected_by_operation'] != history[1]['collected_by_operation'], history
    assert A.status(U5)['tombstone']['collected_by_operation'] == history[0]['collected_by_operation'], 'the tombstone keeps the first proof'
    A.refused(request('create', U5, REF, image='sha256:' + alpine, command=['true']),
              'create after a second collection of the same universe UUID', 'was collected on this host', checks)
    checks.append('the full cycle ran twice for one universe — collection, life again, checkpoint, a second terminal authorization, collection '
                  'again — and left two occurrences in the collection history under one tombstone that still refuses a blind create')
    results['second_cycle'] = {'history': history, 'effect': effect}

    # ---------------------------------------------------------------- class 3: the plan proposes, the abort acts
    F = str(uuid.uuid4())
    f_container, f_create, f_checkpoint, _ = checkpointed(A, F, B.identity)
    f_authorization = A.ok(request('migration_authorize_transfer', F, REF, checkpoint_operation_id=f_checkpoint,
                                   destination_host_uuid=B.identity))['authorization_id']
    transfer(A, B, f_authorization)
    archive = f'{state_dir}/inbox/{f_authorization}/checkpoint.tar.zst'
    damaged = B.call('corrupt_archive', source=archive, target=archive, member=None)
    forge(B, f_authorization, damaged)
    assert B.ok(request('migration_destination_preflight', F, REF, authorization_id=f_authorization))['compatible'] is True
    failing = B.api(request('migration_restore', F, REF, authorization_id=f_authorization))
    assert failing['ok'] is False and 'could not be verified' in failing['error'], failing
    checks.append('[destination] a restore of a damaged archive delivered with matching forged documents failed as expected, leaving a held '
                  'claim: the shape the contract calls a failed restore claim')

    destination_plan = plan_without_effect(B, 'plan over the destination journal', max_candidates=100)
    destination_id = destination_plan['collection_operation_id']
    proposal = candidate(destination_plan, f_authorization)
    assert proposal['kind'] == 'restore_claim' and proposal['class'] == CLASS3 and proposal['class_number'] == 3
    assert proposal['collectable'] is True and proposal['proposed_effect']['delegates_to'] == 'migration_restore_abort'
    assert proposal['proofs']['restore_scope']['busy'] is False
    left_a_container = proposal['proofs']['container']['present']
    if left_a_container:
        assert proposal['proofs']['container']['created_after_claim'] is True
        assert proposal['proofs']['container']['carries_universe_label'] is True
        assert proposal['proofs']['container']['owned_by_a_verified_operation'] is False
    verified_claims = {k['authorization_id'] for k in B.call('journal')['migration_restore_claims'] if k['state'] == 'restored'}
    listed = {c['key'] for c in destination_plan['candidates'] if c['kind'] == 'restore_claim'}
    assert not (verified_claims & listed), 'a verified restore claim is never a candidate'
    checks.append('[destination] the plan proposes migration_restore_abort for the failed claim, with the container, the scope and the cgroup '
                  'facts it rests on, and never enumerates any of the %d verified claims on this host' % len(verified_claims))
    B.refused(collection('garbage_collect_apply', plan_operation_id=destination_id, reclaim_processes=True,
                         candidates=[{'class': CLASS3, 'universe_uuid': F, 'authorization_id': f_authorization}]),
              'apply asking to end processes without also raising the run\'s runtime reclaim bound', 'max_runtime_reclaims', checks)

    # ------------------------------- the hard shape: a runaway restore, reclaimed through the collector API
    free_before = B.call('df', path=GRAPH)['free']
    assert free_before > WATCHDOG_FLOOR + 4 * 1024 ** 3, f'the destination has only {free_before} bytes free for a deliberate failure'
    H = str(uuid.uuid4())
    h_container, h_create, h_checkpoint, _ = checkpointed(A, H, B.identity, command=HOG, grow_to=400 * 1024 ** 2)
    h_authorization = A.ok(request('migration_authorize_transfer', H, REF, checkpoint_operation_id=h_checkpoint,
                                   destination_host_uuid=B.identity))['authorization_id']
    transfer(A, B, h_authorization)
    damaged = B.call('corrupt_archive', source=f'{state_dir}/inbox/{h_authorization}/checkpoint.tar.zst',
                     target=f'{state_dir}/inbox/{h_authorization}/checkpoint.tar.zst', member='largest-pages')
    assert damaged['member'].startswith('checkpoint/pages-'), damaged
    forge(B, h_authorization, damaged)
    assert B.ok(request('migration_destination_preflight', H, REF, authorization_id=h_authorization))['compatible'] is True
    runaway = request('migration_restore', H, REF, authorization_id=h_authorization)
    watched = B.call('restore_under_watchdog', request=runaway, floor_bytes=WATCHDOG_FLOOR,
                     universe_uuid=H, authorization_id=h_authorization, graph=GRAPH, timeout=1800)
    B.windows.append((watched['begin_ns'], watched['end_ns'], 'migration_restore', runaway['operation_id']))
    assert watched['target_binding']['verified'] is True, watched['target_binding']
    if watched['attempt_container_id']:
        B.record_container('podmesh-' + H, watched['attempt_container_id'], H, runaway['operation_id'], 'failed_restore')
    assert watched['response']['ok'] is False and watched['watchdog_fired'] is None, watched['watchdog_fired']
    assert watched['response']['details']['prevention']['stopped'] is True
    leftover = inspect(B, H)
    assert leftover is not None and leftover['State']['Running'] is False
    frozen_id = leftover['Id']
    checks.append('[destination] the runaway shape reproduced through the API: the bound stopped the attempt after %d MiB, the claim is held '
                  'and its container cgroups still hold the processes CRIU left'
                  % (watched['response']['details']['prevention']['maximum_consumed_bytes'] // 1024 ** 2))
    runaway_plan = plan_without_effect(B, 'plan over a destination holding a runaway failed restore', max_candidates=100)
    runaway_id = runaway_plan['collection_operation_id']
    orphaned = candidate(runaway_plan, h_authorization)
    assert orphaned['class'] == CLASS3 and orphaned['collectable'] is True
    assert orphaned['proposed_effect']['requires_reclaim_processes'] is True
    assert orphaned['proofs']['runtime_processes']['source'] == 'cgroup_residency'
    assert orphaned['proofs']['runtime_processes']['authorizes_reclaim'] is True
    surviving = orphaned['proofs']['runtime_processes']['count']
    assert surviving > 0 and all(p['started_at_or_after_claim'] is True for p in orphaned['proofs']['runtime_processes']['processes'])
    checks.append('[destination] the read-only plan reports the %d processes the failed attempt left, by cgroup residency, and says the effect '
                  'would need reclaim_processes' % surviving)
    before_facts = B.call('cgroup_facts', container_id=frozen_id)
    assert before_facts['total_processes'] == surviving, (before_facts['total_processes'], surviving)
    refusal = B.refused(collection('garbage_collect_apply', plan_operation_id=runaway_id,
                                   candidates=[{'class': CLASS3, 'universe_uuid': H, 'authorization_id': h_authorization}]),
                        'apply of a failed restore whose processes survive, with reclaim_processes false',
                        'They are reported, not ended', checks)
    assert B.call('cgroup_facts', container_id=frozen_id) == before_facts, 'the collector must have signalled nothing'
    refusal_proof = validate_predelegation_refusal(refusal, {
        'class': CLASS3, 'universe_uuid': H, 'authorization_id': h_authorization,
    })
    assert refusal_proof['verified'], refusal_proof
    reported = refusal_proof['runtime_processes']
    assert reported['count'] == surviving and reported['authorizes_reclaim'] is True
    checks.append('[destination] with reclaim_processes false the collector reports those %d processes with their cgroups and start times and '
                  'ends none of them: the same PIDs are still there afterwards' % reported['count'])
    # Two candidates, an allowance of one: the budget is reserved before a delegated call that may signal.
    budgeted = B.ok(collection('garbage_collect_apply', plan_operation_id=runaway_id, reclaim_processes=True,
                               max_runtime_reclaims=1, max_effects=2,
                               candidates=[{'class': CLASS3, 'universe_uuid': H, 'authorization_id': h_authorization},
                                           {'class': CLASS3, 'universe_uuid': F, 'authorization_id': f_authorization}]),
                    'a reclaim through the collector itself: only what the abort could prove was ended, and the run\'s allowance stopped the '
                    'second candidate before it could signal', checks)
    reclaimed = budgeted['results'][0]
    assert budgeted['effects_applied'] == 1 and budgeted['runtime_reclaims_reserved'] == 1
    signal_entries = reclaimed['result']['reclaim']['signalled']
    signal_counts = signal_metric_counts(signal_entries)
    signal_candidates = signal_counts['signal_candidates']
    signal_attempts = signal_counts['signal_attempts']
    signals_delivered = signal_counts['signals_delivered']
    processes_already_gone = signal_counts['processes_already_gone']
    signals_refused = signal_counts['signals_refused']
    assert signal_candidates == signals_delivered + processes_already_gone + signals_refused
    assert 0 < signals_delivered <= signal_attempts <= signal_candidates
    assert all(entry['signal_transport'] == 'pidfd_send_signal' and entry['numeric_pid_fallback'] is False
               for entry in signal_entries)
    assert all(entry['signal_attempted'] is True for entry in signal_entries if entry['signal_outcome'] == 'delivered')
    for record in (reclaimed, reclaimed['result']['reclaim'], budgeted):
        assert record['signal_candidates'] == signal_candidates
        assert record['signal_attempts'] == signal_attempts
        assert record['signals_delivered'] == signals_delivered
        assert record['processes_signalled'] == signals_delivered
        assert record['processes_already_gone'] == processes_already_gone
        assert record['signals_refused'] == signals_refused
    assert budgeted['runtime_reclaims_performed'] == 1
    assert budgeted['verified'] is True and reclaimed['verified'] is True and reclaimed['verification_blockers'] == []
    assert reclaimed['result']['reclaim']['complete'] is True and reclaimed['result']['reclaim']['cgroups_gone'] is True
    assert reclaimed['result']['runtime_processes_remaining'] == 0
    assert all(s['member_of_claimed_cgroup'] is True and s['started_at_or_after_claim'] is True
               for s in reclaimed['result']['reclaim']['signalled'] if s['decision'] == 'sigkill')
    assert budgeted['stopped_before_the_rest']['candidate']['authorization_id'] == f_authorization
    assert 'allowance of 1 runtime reclaim(s) is spent' in budgeted['stopped_before_the_rest']['reason']
    final_facts = B.call('cgroup_facts', container_id=frozen_id)
    assert all(not v['exists'] for v in final_facts.values() if isinstance(v, dict)) and final_facts['total_processes'] == 0
    assert inspect(B, H) is None and B.call('labelled', uuid_value=H)['containers'] == []
    free_after = B.call('df', path=GRAPH)['free']
    checks.append('[destination] verified from outside the collector: %d signal candidates, %d pidfd attempts and %d delivered SIGKILLs; '
                  'both cgroup directories gone, the '
                  'container absent, the claim closed, and the graph root back from %d to %d MiB free'
                  % (signal_candidates, signal_attempts, signals_delivered,
                     watched['minimum_free'] // 1024 ** 2, free_after // 1024 ** 2))
    checks.append('[destination] the second candidate of that run was never acted on: a delegated abort authorized to signal reserves its '
                  'budget before it is called, so what the plan predicted cannot let a run exceed its allowance')
    assert A.status(H)['reservation']['state'] == 'transfer_authorized'
    A.remove_fixture('podmesh-' + H, note='reserved source of the runaway forged-document restore, removed directly by the test')
    out_of_band[H] = 'reserved source of a forged-document restore that can never be completed; container removed directly by the test'
    results['reclaim'] = {'plan': orphaned, 'apply': reclaimed, 'watched': {k: watched[k] for k in
                                                                            ('watchdog_fired', 'minimum_free', 'maximum_consumed')}}

    swept = B.ok(collection('garbage_collect_apply', plan_operation_id=destination_id,
                            candidates=[{'class': CLASS3, 'universe_uuid': F, 'authorization_id': f_authorization}]),
                 'the failed restore claim that left nothing alive is ended the same way, with reclaim_processes false', checks)
    delegated = swept['results'][0]
    assert delegated['delegated_to'] == 'migration_restore_abort' and delegated['reclaim_processes'] is False
    assert delegated['verified'] is True and delegated['verification_blockers'] == [], delegated
    assert delegated['reclaim_performed'] is False and delegated['processes_signalled'] == 0
    assert swept['runtime_reclaims_reserved'] == 0 and swept['verified'] is True
    assert delegated['result']['operation'] == 'migration_restore_abort'
    assert delegated['result']['action'] in ('removed_restore_leftover', 'none_absent'), delegated['result']['action']
    assert delegated['result']['reclaim_processes_requested'] is False and delegated['result']['reclaim'] is None
    assert delegated['result']['runtime_processes_remaining'] == 0
    assert delegated['result']['container_cgroups_absent'] in (True, None)
    assert delegated['verified_from_outside']['claim_unresolved'] is False
    assert delegated['verified_from_outside']['universe_container_present'] is False
    assert inspect(B, F) is None and B.call('labelled', uuid_value=F)['containers'] == []
    checks.append('[destination] the collector sent no signal and manages no process: the abort removed the non-running container the claim '
                  'created, reported %s surviving process, closed the claim and wrote the outcome'
                  % delegated['result']['runtime_processes_remaining'])
    B.refused(collection('garbage_collect_apply', plan_operation_id=destination_id,
                         candidates=[{'class': CLASS3, 'universe_uuid': F, 'authorization_id': f_authorization}]),
              'a second collection of a restore claim this host already closed', 'No unresolved restore claim', checks)
    results['class3'] = {'plan': proposal, 'apply': delegated}

    # The outcome the collector's abort wrote is bound to the handoff the destination actually claimed, which
    # the transport had altered: the source refuses it, and the collector refuses the source. A collection is
    # not a way around a missing break-glass path.
    transfer(B, A, f_authorization, files=('outcome.json',))
    A.refused(request('migration_complete_transfer', F, REF, authorization_id=f_authorization),
              '[source] completion with the outcome bound to the forged handoff the destination actually claimed',
              'not to this authorization', checks)
    assert A.status(F)['reservation']['state'] == 'transfer_authorized'
    closing_plan = A.ok(collection('garbage_collect_plan', universe_uuids=[F]))
    stuck = candidate(closing_plan, F)
    assert stuck['collectable'] is False and any('still open (state issued)' in b for b in stuck['blockers']), stuck
    A.refused(collection('garbage_collect_apply', plan_operation_id=closing_plan['collection_operation_id'],
                         candidates=[{'class': CLASS1, 'universe_uuid': F}]),
              '[source] collection of a source whose authorization no outcome can ever end', 'The plan found', checks)
    checks.append('[source] a transport that altered the documents leaves the source authorized with no outcome that can bind to it: the '
                  'collector reports it as blocked by that open authorization and collects nothing. The break-glass path the protocol '
                  'leaves open is not one a garbage collector may take.')
    A.remove_fixture('podmesh-' + F, note='reserved source of the forged-document restore, removed directly by the test')

    # ---------------------------------------------------------------- independent verification
    A.refused(collection('garbage_collect_apply', plan_operation_id=third_id, candidates=[{'class': CLASS2, 'universe_uuid': U6}]),
              'apply of the reservation the plan refused to classify', 'The plan classified', checks)
    A.ok(request('migration_release', U6, REF, checkpoint_operation_id=ck6),
         'the reservation the collector left alone is ended by the decision it named: migration_release, then an ordinary delete', checks)
    A.ok(request('delete', U6, REF))
    assert inspect(A, U6) is None
    A.remove_fixture('podmesh-' + U2, note='reserved container of the running/paused refusals, removed directly by the test')
    A.ok(request('delete', U5, REF),
         'the second class 1 collection also lifted the gate: its stopped, checkpointed source is deleted through the ordinary API', checks)
    assert inspect(A, U5) is None
    collected_rows = A.call('journal')['migration_universe_tombstones']
    assert {t['universe_uuid'] for t in collected_rows} >= {U1, U3, U4, U5}
    assert all(t['proof'] and t['collected_by_operation'] for t in collected_rows)
    checks.append('every collection left a tombstone carrying its class, its container, its collecting operation and a copy of the proofs: '
                  '%d on this host' % len(collected_rows))
    results['runs'] = {A.role: A.call('gc_runs'), B.role: B.call('gc_runs')}
    events = {}
    observed_universes = [u for u in universes if u not in out_of_band]
    removed_by_test = {A.role: {f_container['Id']}, B.role: set()}
    for host in (A, B):
        until = int(host.call('time')['time']) + 1
        events[host.role] = event_report(host, since[host.role], until, observed_universes, fixture_ids[host.role],
                                         removed_by_test=removed_by_test[host.role])
        assert not events[host.role]['outside_api_windows'], (host.role, events[host.role]['outside_api_windows'])
        assert host.call('snapshot')['podman'] == baseline[host.role], (host.role, 'containers, images or volumes differ from the baseline')
    checks.append('every Podman container event on API-managed universes falls inside an API request window of its host, except this suite\'s '
                  'own fixtures and the universes it deliberately touched out of band: ' + json.dumps({r: e['statuses'] for r, e in events.items()}))
    checks.append('pre-existing containers, images and volumes unchanged on both hosts; no leftovers')
finally:
    for host in (A, B):
        try:
            before_cleanup = host.call('snapshot')
        except Exception as e:
            before_cleanup = {'error': f'{type(e).__name__}: {e}'}
        try:
            cleanup_report = host.cleanup(baseline.get(host.role), before_cleanup.get('podman'))
        except Exception as e:
            cleanup_report = [{'verified': False, 'reason': f'cleanup stopped: {type(e).__name__}: {e}; uncertain state retained'}]
        cleanup_evidence[host.role] = {'before_cleanup': before_cleanup, 'ledger_results': cleanup_report,
                                       'uncertain_retained': [r for r in cleanup_report if not r.get('verified') or not r.get('absent')]}
    print('PODMESH_TEST_CLEANUP=' + json.dumps(cleanup_evidence, sort_keys=True), file=sys.stderr)
    shutil.rmtree(control, ignore_errors=True)

print(json.dumps({'status': 'PASS', 'source_host_uuid': A.identity, 'destination_host_uuid': B.identity, 'universes': universes,
                  'service_identity': service_identity, 'cleanup': cleanup_evidence,
                  'checks': checks, 'check_count': len(checks), 'api_windows': {A.role: len(A.windows), B.role: len(B.windows)},
                  'collected': {'class1': [U1, U5], 'class1_collected_twice': [U5], 'class2': [U3, U4, R],
                                'class3': [f_authorization, h_authorization]},
                  'out_of_band_universes': out_of_band, 'memory_continuity': continuity, 'events': events, 'results': results,
                  'reserved_universes_left_in_journals': {
                      A.role: {'checkpointed_or_authorized': [U2, F, H, OPEN], 'settled': [SETTLED]}, B.role: []}}))
