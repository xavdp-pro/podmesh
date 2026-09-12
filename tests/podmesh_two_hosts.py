#!/usr/bin/env python3
"""Two-host laboratory helper for the destination-side migration suites.

Runs in two roles from the same file.

* **Node** (as root on a lab host, through SSH): executes one bounded function against the local
  PodMesh service, Podman or /proc and prints one JSON line. Every API call is timed with the host's
  own clock, so Podman event correlation never compares clocks across machines.
* **Controller** (imported by a suite on the workstation): drives both hosts through SSH, carries
  documents between an outbox and an inbox as the transport controller, and keeps the API windows,
  refusal snapshots and checks of each host.

Direct Podman writes are limited to uniquely named disposable fixtures owned by the suite; every
product mutation goes through the PodMesh API. The controller never holds authority: it moves bytes.
"""
import base64, hashlib, json, os, shutil, signal, socket, sqlite3, stat, struct, subprocess, sys, threading, time, uuid

MIGRATION_TABLES = ('migration_reservations', 'migration_authorizations', 'migration_restore_claims', 'migration_reservation_history',
                    'migration_universe_tombstones', 'migration_collection_history')


# ---------------------------------------------------------------- node side

def _endpoint():
    return os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
def _state():
    return os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
def _unit():
    return os.environ.get('PODMESH_UNIT', 'podmesh.service')
def _podman(*args, check=True, timeout=120):
    p = subprocess.run(['podman', *args], capture_output=True, timeout=timeout)
    if check and p.returncode:
        raise RuntimeError(f'podman {args}: {p.stderr.decode()}')
    return p
def _out(*args, timeout=120):
    return _podman(*args, timeout=timeout).stdout.decode().strip()
def _sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()
def stat_is_socket(path):
    return stat.S_ISSOCK(os.stat(path).st_mode)
def _raw_api_with_peer(request, timeout=600):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(timeout)
        s.connect(_endpoint())
        peer_pid, peer_uid, peer_gid = struct.unpack('3i', s.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize('3i')))
        s.sendall(json.dumps(request).encode() + b'\n')
        return json.loads(s.makefile('rb').readline()), {'pid': peer_pid, 'uid': peer_uid, 'gid': peer_gid}
def _raw_api(request, timeout=600):
    return _raw_api_with_peer(request, timeout)[0]


def validate_service_identity(proof, expected):
    """Pure fail-closed binding of socket peer, systemd unit, executable and SQLite state."""
    errors = []
    if proof.get('active_state') != 'active' or proof.get('load_state') != 'loaded':
        errors.append('qualification unit is not loaded and active')
    if proof.get('peer_pid') != proof.get('main_pid') or not isinstance(proof.get('main_pid'), int) or proof['main_pid'] <= 0:
        errors.append('socket peer PID is not the qualification unit MainPID')
    if proof.get('peer_uid') != 0:
        errors.append('socket peer is not root')
    if proof.get('binary_sha256') != expected.get('binary_sha256'):
        errors.append('service executable hash differs from the frozen binary')
    if proof.get('socket') != expected.get('socket') or not proof.get('socket_is_unix'):
        errors.append('service socket is not the expected Unix socket')
    if proof.get('state_dir') != expected.get('state_dir') or not proof.get('state_db_regular'):
        errors.append('service state is not the expected SQLite path')
    if proof.get('process_socket') != expected.get('socket') or proof.get('process_state_dir') != expected.get('state_dir'):
        errors.append('service process environment does not bind the expected socket and state')
    if ('collector_barrier_dir' in expected
            and proof.get('process_collector_barrier_dir') != expected.get('collector_barrier_dir')):
        errors.append('service process environment does not bind the expected collector test barrier')
    try:
        host_uuid_valid = str(uuid.UUID(proof.get('api_host_uuid', ''))) == proof.get('api_host_uuid')
    except (ValueError, TypeError, AttributeError):
        host_uuid_valid = False
    if not host_uuid_valid or proof.get('api_host_uuid') != proof.get('db_host_uuid'):
        errors.append('API host UUID differs from the configured SQLite state')
    if not proof.get('machine_id') or not proof.get('db_machine_id') or proof.get('db_machine_id') != proof.get('machine_id'):
        errors.append('SQLite state belongs to another machine identity')
    return {'verified': not errors, 'errors': errors}


def watchdog_binding(expected, observed, claim):
    """Pure proof that a container is the exact failed-restore fixture the watchdog may contain."""
    errors = []
    expected_name = 'podmesh-' + expected['universe_uuid']
    if not isinstance(observed, dict):
        errors.append('the expected universe container is not observable')
    else:
        names = observed.get('Name') or observed.get('Names') or []
        if isinstance(names, str):
            names = [names]
        names = [name.lstrip('/') for name in names]
        if expected_name not in names:
            errors.append('the observed container does not own the expected universe name')
        if (observed.get('Config', {}).get('Labels') or {}).get('io.podmesh.universe') != expected['universe_uuid']:
            errors.append('the observed container lacks the expected universe label')
    if not isinstance(claim, dict):
        errors.append('the expected restore claim is not observable')
    else:
        if claim.get('authorization_id') != expected['authorization_id'] or claim.get('universe_uuid') != expected['universe_uuid']:
            errors.append('the restore claim does not bind the expected authorization and universe')
        if not isinstance(observed, dict) or claim.get('container_id') != observed.get('Id'):
            errors.append('the restore claim does not bind the observed container ID')
        if claim.get('state') not in ('restoring', 'restore_failed'):
            errors.append('the restore claim is not in a reclaimable in-progress or failed state')
    return {'verified': not errors, 'errors': errors,
            'container_id': observed.get('Id') if isinstance(observed, dict) and not errors else None}


def owned_removal_verdict(entry, observed):
    """Pure ledger check used before any test cleanup removes a container."""
    if not isinstance(entry, dict) or not isinstance(observed, dict):
        return {'verified': False, 'reason': 'ledger entry or observed container is unavailable'}
    names = observed.get('Name') or observed.get('Names') or []
    if isinstance(names, str):
        names = [names]
    names = [name.lstrip('/') for name in names]
    if observed.get('Id') != entry.get('container_id') or entry.get('name') not in names:
        return {'verified': False, 'reason': 'observed name and ID do not match the cleanup ledger'}
    universe = entry.get('universe_uuid')
    if universe and (observed.get('Config', {}).get('Labels') or {}).get('io.podmesh.universe') != universe:
        return {'verified': False, 'reason': 'observed universe label does not match the cleanup ledger'}
    return {'verified': True, 'reason': 'exact ledger name, ID and ownership facts match'}


def untracked_container_additions(baseline_podman, observed_podman, ledger):
    """Pure inventory delta: additions outside the exact ledger are evidence to retain, never cleanup targets."""
    baseline_ids = set((baseline_podman or {}).get('containers') or {})
    observed = (observed_podman or {}).get('containers') or {}
    tracked_ids = {entry.get('container_id') for entry in (ledger or {}).values() if isinstance(entry, dict)}
    return [{'container_id': container_id, 'observed': facts}
            for container_id, facts in observed.items()
            if container_id not in baseline_ids and container_id not in tracked_ids]


def validate_predelegation_refusal(response, expected):
    """Pure fail-closed decoder for a collector refusal made before migration_restore_abort is entered."""
    errors = []
    details = response.get('details') if isinstance(response, dict) else None
    detail = details.get('detail') if isinstance(details, dict) else None
    runtime = detail.get('runtime_processes') if isinstance(detail, dict) else None
    if not isinstance(details, dict) or details.get('applied') != []:
        errors.append('the refusal does not prove that no earlier candidate was applied')
    if not isinstance(detail, dict) or detail.get('delegated') is not False or detail.get('effects_applied') != 0:
        errors.append('the refusal does not prove that delegation and effects stayed at zero')
    candidate = detail.get('candidate') if isinstance(detail, dict) else None
    if not isinstance(candidate, dict) or any(candidate.get(key) != expected.get(key)
                                               for key in ('class', 'universe_uuid', 'authorization_id')):
        errors.append('the refusal candidate does not match the requested failed restore claim')
    outer_candidate = details.get('candidate') if isinstance(details, dict) else None
    if not isinstance(outer_candidate, dict) or any(outer_candidate.get(key) != expected.get(key)
                                                     for key in ('class', 'universe_uuid', 'authorization_id')):
        errors.append('the outer refusal candidate does not match the requested failed restore claim')
    message = response.get('error', '') if isinstance(response, dict) else ''
    status = response.get('ok') if isinstance(response, dict) else None
    if (not isinstance(response, dict) or (status is not None and status is not False) or 'nothing was collected' not in message
            or 'migration_restore_abort was not entered' not in message):
        errors.append('the response is not an explicit pre-delegation no-effect refusal')
    if not isinstance(runtime, dict):
        errors.append('the refusal carries no runtime-process proof')
    else:
        container_id = runtime.get('container_id')
        claim_time = runtime.get('claim_created_at')
        processes = runtime.get('processes')
        count = runtime.get('count')
        if (runtime.get('known') is not True or runtime.get('source') != 'cgroup_residency'
                or runtime.get('authorizes_reclaim') is not True or runtime.get('observation_errors') != []):
            errors.append('the runtime-process observation is not known, authoritative cgroup residency')
        if (not isinstance(container_id, str) or len(container_id) != 64
                or any(c not in '0123456789abcdef' for c in container_id)):
            errors.append('the runtime-process proof has no valid immutable container ID')
        if not isinstance(claim_time, int):
            errors.append('the runtime-process proof has no valid claim time')
        if not isinstance(processes, list) or not isinstance(count, int) or count <= 0 or len(processes) != count:
            errors.append('the runtime-process proof has no complete non-empty process list')
        elif isinstance(container_id, str) and isinstance(claim_time, int):
            scopes = (f'/machine.slice/libpod-{container_id}.scope',
                      f'/machine.slice/libpod-conmon-{container_id}.scope')
            for process in processes:
                cgroup = process.get('cgroup') if isinstance(process, dict) else None
                start = process.get('start_epoch') if isinstance(process, dict) else None
                exact_member = isinstance(cgroup, str) and any(cgroup == scope or cgroup.startswith(scope + '/') for scope in scopes)
                if (not exact_member or process.get('started_at_or_after_claim') is not True
                        or not isinstance(start, int) or start < claim_time or not isinstance(process.get('pid'), int)
                        or process['pid'] <= 0):
                    errors.append('a reported process lacks exact cgroup, PID, or claim-time ownership proof')
                    break
    return {'verified': not errors, 'errors': errors, 'runtime_processes': runtime if not errors else None}


def validate_collector_barrier_marker(marker, expected):
    """Pure exact-schema check for the externally observed post-commit test barrier."""
    required = {'format', 'operation_id', 'candidate_key', 'universe_uuid', 'class', 'phase'}
    errors = []
    if not isinstance(marker, dict) or set(marker) != required:
        errors.append('the reached marker does not have the exact barrier schema')
    else:
        wanted = dict(expected, format='podmesh-test-collector-barrier/1',
                      phase='effect_committed_verification_pending')
        for key, value in wanted.items():
            if marker.get(key) != value:
                errors.append(f'the reached marker has the wrong {key}')
    return {'verified': not errors, 'errors': errors}


def validate_interrupted_collection_state(state, expected):
    """Pure proof that the domain effect and pending progress committed, but run completion did not."""
    if not isinstance(state, dict):
        state = {}
    errors = []
    operation = state.get('operation')
    effects = state.get('effects')
    reservation = state.get('reservation')
    history = state.get('history')
    tombstone = state.get('tombstone')
    if (not isinstance(operation, dict) or operation.get('id') != expected.get('operation_id')
            or operation.get('status') != 'pending' or operation.get('result') is not None):
        errors.append('the interrupted operation is not durably pending without a result')
    if state.get('runs') != []:
        errors.append('a durable collector run completion already exists')
    if not isinstance(effects, list) or len(effects) != 1:
        errors.append('the exact pending collector effect is not present once')
    else:
        effect = effects[0]
        result = effect.get('result') if isinstance(effect, dict) else None
        if (effect.get('operation_id') != expected.get('operation_id')
                or effect.get('candidate_key') != expected.get('universe_uuid')
                or effect.get('class') != expected.get('class')
                or effect.get('universe_uuid') != expected.get('universe_uuid')):
            errors.append('the pending effect identity differs from the interrupted request')
        if (not isinstance(result, dict) or result.get('verification') != 'pending'
                or result.get('effect_state') != 'committed' or result.get('universe_uuid') != expected.get('universe_uuid')
                or result.get('class') != expected.get('class')):
            errors.append('the effect is not committed with verification still pending')
    if (not isinstance(reservation, dict) or reservation.get('universe_uuid') != expected.get('universe_uuid')
            or reservation.get('state') != 'collected'):
        errors.append('the reservation domain effect is not collected')
    else:
        detail = reservation.get('detail')
        if not isinstance(detail, dict) or detail.get('collected_by_operation') != expected.get('operation_id'):
            errors.append('the collected reservation is not bound to this operation')
    occurrence = history[0] if isinstance(history, list) and len(history) == 1 and isinstance(history[0], dict) else None
    if (not isinstance(occurrence, dict) or occurrence.get('universe_uuid') != expected.get('universe_uuid')
            or occurrence.get('collected_by_operation') != expected.get('operation_id')
            or occurrence.get('class') != expected.get('class') or occurrence.get('container_absent_at_collection') not in (1, True)):
        errors.append('the collection occurrence is not present exactly once for this operation')
    if (not isinstance(tombstone, dict)
            or tombstone.get('universe_uuid') != expected.get('universe_uuid')
            or tombstone.get('collected_by_operation') != expected.get('operation_id')
            or tombstone.get('class') != expected.get('class')
            or tombstone.get('container_absent_at_collection') not in (1, True)):
        errors.append('the tombstone is not bound to this interrupted collection')
    attempts = state.get('attempts')
    if (not isinstance(attempts, list) or not attempts or not isinstance(attempts[-1], dict)
            or attempts[-1].get('finished_at') is not None or attempts[-1].get('outcome') is not None):
        errors.append('the interrupted attempt is not durably open')
    return {'verified': not errors, 'errors': errors}


def signal_metric_counts(entries):
    """Truthful counts: candidates, attempts and confirmed pidfd deliveries are distinct facts."""
    entries = entries if isinstance(entries, list) else []
    return {
        'signal_candidates': len(entries),
        'signal_attempts': sum(isinstance(entry, dict) and entry.get('signal_attempted') is True for entry in entries),
        'signals_delivered': sum(isinstance(entry, dict) and entry.get('signal_outcome') == 'delivered' for entry in entries),
        'processes_already_gone': sum(isinstance(entry, dict) and entry.get('signal_outcome') == 'already_gone' for entry in entries),
        'signals_refused': sum(not isinstance(entry, dict) or entry.get('signal_outcome') == 'refused' for entry in entries),
    }


def n_api(request, timeout=600):
    """One API request, with the window measured by this host's clock."""
    begin = time.time_ns()
    try:
        response = _raw_api(request, timeout)
    except Exception as e:
        return {'response': {'ok': False, 'error': f'transport: {type(e).__name__}: {e}', 'interrupted': True},
                'begin_ns': begin, 'end_ns': time.time_ns()}
    return {'response': response, 'begin_ns': begin, 'end_ns': time.time_ns()}
def n_ready(seconds=60):
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            if _raw_api({'operation': 'capabilities'}, 5)['ok']:
                return {'ready': True}
        except (OSError, ValueError):
            pass
        time.sleep(.1)
    raise RuntimeError('Service did not become ready')
def n_service_identity(expected_binary_sha256, expected_collector_barrier_dir=None):
    """Read-only proof that socket, systemd MainPID, executable and SQLite state are one service."""
    assert len(expected_binary_sha256) == 64 and all(c in '0123456789abcdef' for c in expected_binary_sha256)
    response, peer = _raw_api_with_peer({'operation': 'identity'}, 5)
    assert response.get('ok') is True, response
    def show(prop):
        p = subprocess.run(['systemctl', 'show', _unit(), '--property=' + prop, '--value'], capture_output=True, text=True)
        if p.returncode:
            raise RuntimeError(f'systemctl show {_unit()} {prop}: {p.stderr.strip()}')
        return p.stdout.strip()
    main_pid_text = show('MainPID')
    main_pid = int(main_pid_text) if main_pid_text.isdigit() else 0
    process_env = {}
    if main_pid > 0:
        with open(f'/proc/{main_pid}/environ', 'rb') as f:
            for item in f.read().split(b'\0'):
                key, separator, value = item.partition(b'=')
                if separator:
                    process_env[key.decode(errors='replace')] = value.decode(errors='replace')
    state_real = os.path.realpath(_state())
    db_path = os.path.join(state_real, 'state.sqlite')
    db = sqlite3.connect(f'file:{db_path}?mode=ro', uri=True)
    metadata = dict(db.execute("SELECT key,value FROM metadata WHERE key IN ('host_uuid','machine_id')"))
    db.close()
    proof = {
        'unit': _unit(), 'load_state': show('LoadState'), 'active_state': show('ActiveState'),
        'main_pid': main_pid, 'peer_pid': peer['pid'], 'peer_uid': peer['uid'], 'peer_gid': peer['gid'],
        'executable': os.path.realpath(f'/proc/{main_pid}/exe') if main_pid > 0 else None,
        'binary_sha256': _sha256(f'/proc/{main_pid}/exe') if main_pid > 0 else None,
        'socket': os.path.realpath(_endpoint()), 'socket_is_unix': os.path.exists(_endpoint()) and stat_is_socket(_endpoint()),
        'state_dir': state_real, 'state_db': db_path, 'state_db_regular': os.path.isfile(db_path),
        'process_socket': process_env.get('PODMESH_SOCKET'), 'process_state_dir': process_env.get('PODMESH_STATE_DIR'),
        'process_collector_barrier_dir': process_env.get('PODMESH_TEST_COLLECTOR_BARRIER_DIR'),
        'api_host_uuid': response['data']['host_uuid'], 'db_host_uuid': metadata.get('host_uuid'),
        'db_machine_id': metadata.get('machine_id'), 'machine_id': open('/etc/machine-id').read().strip(),
    }
    main_pid_after_text = show('MainPID')
    main_pid_after = int(main_pid_after_text) if main_pid_after_text.isdigit() else 0
    if main_pid_after != main_pid:
        raise RuntimeError(f'Qualification service MainPID changed during attestation: {main_pid} -> {main_pid_after}')
    expected = {'binary_sha256': expected_binary_sha256, 'socket': os.path.realpath(_endpoint()), 'state_dir': state_real}
    if expected_collector_barrier_dir is not None:
        expected['collector_barrier_dir'] = os.path.realpath(expected_collector_barrier_dir)
        proof['process_collector_barrier_dir'] = (os.path.realpath(proof['process_collector_barrier_dir'])
                                                  if proof['process_collector_barrier_dir'] else None)
    verdict = validate_service_identity(proof, expected)
    proof.update(verdict)
    if not proof['verified']:
        raise RuntimeError('Qualification service identity failed: ' + '; '.join(proof['errors']))
    return proof
def n_time():
    return {'time': time.time(), 'time_ns': time.time_ns()}
def n_podman_state():
    """Independent view of every container, image and volume on this host."""
    containers = {c['Id']: [tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'), c.get('ExitCode'), c.get('ImageID')]
                  for c in json.loads(_out('ps', '--all', '--format', 'json'))}
    images = {i['Id']: sorted(i.get('Names') or []) for i in json.loads(_out('images', '--all', '--format', 'json'))}
    return {'containers': containers, 'images': images, 'volumes': sorted(_out('volume', 'ls', '--quiet').split())}
def n_journal():
    """The migration tables, which a refused request must leave untouched."""
    db = sqlite3.connect(f'file:{_state()}/state.sqlite?mode=ro', uri=True)
    rows = {}
    for table in MIGRATION_TABLES:
        try:
            cursor = db.execute(f'SELECT * FROM {table}')
            names = [d[0] for d in cursor.description]
            rows[table] = [dict(zip(names, r)) for r in cursor.fetchall()]
        except sqlite3.OperationalError:
            # A table the service has not created yet holds no rows either: the first request of a fresh
            # journal creates them, which is not an effect of the request under test.
            rows[table] = []
    db.close()
    return rows
def n_boxes():
    """Every delivered document, by hash: the transport controller's view of both directories."""
    listing = {}
    for box in ('inbox', 'outbox'):
        entries = {}
        root = os.path.join(_state(), box)
        for authorization in sorted(os.listdir(root)) if os.path.isdir(root) else []:
            directory = os.path.join(root, authorization)
            if not os.path.isdir(directory):
                continue
            entries[authorization] = {f: {'sha256': _sha256(os.path.join(directory, f)), 'bytes': os.path.getsize(os.path.join(directory, f)),
                                          'mode': oct(os.stat(os.path.join(directory, f)).st_mode & 0o777)}
                                      for f in sorted(os.listdir(directory)) if os.path.isfile(os.path.join(directory, f))}
            entries[authorization]['_mode'] = oct(os.stat(directory).st_mode & 0o777)
        listing[box] = entries
    return listing
def n_gc_runs():
    """The garbage collector's own run records, without the record bodies: one row per plan or apply run.
    Kept out of n_journal so that a refused request can be compared against the migration tables alone."""
    db = sqlite3.connect(f'file:{_state()}/state.sqlite?mode=ro', uri=True)
    try:
        rows = [dict(zip(('operation_id', 'mode', 'authorization_ref', 'collector_version', 'started_at', 'finished_at'), r))
                for r in db.execute('SELECT operation_id,mode,authorization_ref,collector_version,started_at,finished_at '
                                    'FROM garbage_collection_runs ORDER BY started_at, operation_id')]
    except sqlite3.OperationalError:
        rows = []
    db.close()
    return {'runs': rows, 'count': len(rows)}
def n_journal_row(table, key_column, key):
    db = sqlite3.connect(f'file:{_state()}/state.sqlite?mode=ro', uri=True)
    cursor = db.execute(f'SELECT * FROM {table} WHERE {key_column}=?', (key,))
    names = [d[0] for d in cursor.description]
    rows = [dict(zip(names, r)) for r in cursor.fetchall()]
    db.close()
    return {'rows': rows}
def n_restore_claim(authorization_id, timeout=5):
    db = sqlite3.connect(f'file:{_state()}/state.sqlite?mode=ro', uri=True, timeout=timeout)
    db.row_factory = sqlite3.Row
    row = db.execute('SELECT authorization_id,universe_uuid,container_id,state,created_at '
                     'FROM migration_restore_claims WHERE authorization_id=?', (authorization_id,)).fetchone()
    db.close()
    return {'claim': dict(row) if row else None}
def n_journal_write(table, key_column, key, values):
    """Deliberate, test-owned journal forgery, used only on rows this suite created, to exercise the
    collector's refusal of a malformed or wrongly bound record. The suite restores the original values and
    asserts that it did. This is not a product mechanism: root can always forge a journal, which is why the
    collector re-hashes and re-binds every document it relies on instead of trusting a state column."""
    db = sqlite3.connect(f'{_state()}/state.sqlite', timeout=30)
    with db:
        assignments = ', '.join(f'{c}=?' for c in values)
        db.execute(f'UPDATE {table} SET {assignments} WHERE {key_column}=?', [*values.values(), key])
    db.close()
    return n_journal_row(table, key_column, key)
def n_journal_delete(table, key_column, key):
    """Deliberate, test-owned removal of a row this suite's own operation wrote, to simulate a service that
    died between an effect and the record describing it. Same caveat as n_journal_write."""
    db = sqlite3.connect(f'{_state()}/state.sqlite', timeout=30)
    with db:
        db.execute(f'DELETE FROM {table} WHERE {key_column}=?', (key,))
    db.close()
    return n_journal_row(table, key_column, key)
def n_collector_crash_state(operation_id, universe_uuid):
    """Read-only durable state at the test-only post-commit collector barrier."""
    db = sqlite3.connect(f'file:{_state()}/state.sqlite?mode=ro', uri=True, timeout=5)
    db.row_factory = sqlite3.Row
    def rows(sql, values):
        return [dict(row) for row in db.execute(sql, values).fetchall()]
    effects = rows('SELECT operation_id,candidate_key,class,universe_uuid,applied_at,result '
                   'FROM garbage_collection_effects WHERE operation_id=?', (operation_id,))
    for effect in effects:
        effect['result'] = json.loads(effect['result'])
    operations = rows('SELECT id,status,result FROM operations WHERE id=?', (operation_id,))
    attempts = rows('SELECT id,started_at,finished_at,outcome FROM operation_attempts '
                    'WHERE operation_id=? ORDER BY id', (operation_id,))
    runs = rows('SELECT operation_id,mode,started_at,finished_at FROM garbage_collection_runs '
                'WHERE operation_id=?', (operation_id,))
    reservations = rows('SELECT universe_uuid,state,detail FROM migration_reservations WHERE universe_uuid=?', (universe_uuid,))
    for reservation in reservations:
        reservation['detail'] = json.loads(reservation['detail']) if reservation['detail'] else None
    history = rows('SELECT universe_uuid,class,class_number,container_id,container_absent_at_collection,'
                   'checkpoint_operation_id,collected_by_operation,collected_at FROM migration_collection_history '
                   'WHERE universe_uuid=? AND collected_by_operation=?', (universe_uuid, operation_id))
    tombstones = rows('SELECT universe_uuid,class,class_number,container_id,container_absent_at_collection,'
                      'checkpoint_operation_id,collected_by_operation,collected_at FROM migration_universe_tombstones '
                      'WHERE universe_uuid=?', (universe_uuid,))
    db.close()
    return {'operation': operations[0] if len(operations) == 1 else None, 'attempts': attempts,
            'effects': effects, 'runs': runs, 'reservation': reservations[0] if len(reservations) == 1 else None,
            'history': history, 'tombstone': tombstones[0] if len(tombstones) == 1 else None}
def n_snapshot():
    return {'podman': n_podman_state(), 'journal': n_journal(), 'boxes': n_boxes()}
def n_inspect(name, timeout=120):
    if _podman('container', 'exists', name, check=False, timeout=timeout).returncode:
        return {'container': None}
    return {'container': json.loads(_out('container', 'inspect', name, timeout=timeout))[0]}
def n_labelled(uuid_value):
    return {'containers': [{'id': c['Id'], 'names': c.get('Names'), 'state': c.get('State')}
                           for c in json.loads(_out('ps', '--all', '--format', 'json'))
                           if (c.get('Labels') or {}).get('io.podmesh.universe') == uuid_value]}
def n_image_id(reference):
    return {'image': next((i['Id'] for i in json.loads(_out('images', '--all', '--format', 'json')) if reference in (i.get('Names') or [])), None)}
def n_podman_run(args, check=True):
    p = _podman(*args, check=check)
    return {'exit': p.returncode, 'stdout': p.stdout.decode().strip(), 'stderr': p.stderr.decode().strip()}
def n_remove_owned(entry):
    """Remove only the exact container whose current facts still match a controller-side ledger entry."""
    observed = n_inspect(entry['name'])['container']
    if observed is None:
        return {'removed': False, 'absent': True, 'verified': True, 'entry': entry}
    verdict = owned_removal_verdict(entry, observed)
    if not verdict['verified']:
        return {'removed': False, 'absent': False, 'verified': False, 'entry': entry,
                'observed': {'Id': observed.get('Id'), 'Names': observed.get('Name') or observed.get('Names')},
                'reason': verdict['reason']}
    result = n_podman_run(['rm', '--force', '--time', '0', entry['container_id']], check=False)
    result.update({'removed': result['exit'] == 0, 'absent': n_inspect(entry['name'])['container'] is None,
                   'verified': True, 'entry': entry})
    return result
def n_events(since, until):
    text = _out('events', '--since', str(since), '--until', str(until), '--stream=false', '--format', 'json')
    return {'events': [json.loads(line) for line in text.splitlines() if line.strip()]}
def n_counter(uuid_value, seconds=3.0, interval=0.25):
    """Memory-continuity observation from outside the universe: the application's own /tmp/state,
    read through /proc/<pid>/root as root. No podman exec, no write to the universe."""
    name = 'podmesh-' + uuid_value
    if _podman('container', 'exists', name, check=False).returncode:
        return {'samples': [], 'pid': None, 'error': 'container absent'}
    pid = json.loads(_out('container', 'inspect', name))[0]['State']['Pid']
    samples, deadline = [], time.time() + float(seconds)
    while time.time() < deadline:
        try:
            samples.append([round(time.time(), 3), open(f'/proc/{pid}/root/tmp/state').read().strip()])
        except OSError as e:
            samples.append([round(time.time(), 3), f'unreadable: {e}'])
        time.sleep(float(interval))
    return {'samples': samples, 'pid': pid}
def n_memory(uuid_value):
    c = json.loads(_out('container', 'inspect', 'podmesh-' + uuid_value))[0]
    path = '/sys/fs/cgroup' + c['State']['CgroupPath'] + '/memory.current'
    return {'memory_current_bytes': int(open(path).read().strip()), 'pid': c['State']['Pid']}
def n_scope(unit):
    state = subprocess.run(['systemctl', 'show', '--property=ActiveState', '--value', unit], capture_output=True, text=True).stdout.strip()
    return {'unit': unit, 'active_state': state}
def n_conmon(name):
    """Where conmon of a running universe lives, and whether its scope exists."""
    if _podman('container', 'exists', name, check=False).returncode:
        return {'conmon_cgroup': None, 'conmon_scope_exists': None, 'container': None}
    c = json.loads(_out('container', 'inspect', name))[0]
    pid, cid = c['State'].get('ConmonPid'), c['Id']
    cgroup = None
    if pid:
        try:
            cgroup = open(f'/proc/{pid}/cgroup').read().strip()
        except OSError:
            cgroup = None
    scope = f'/sys/fs/cgroup/machine.slice/libpod-conmon-{cid}.scope'
    return {'conmon_cgroup': cgroup, 'conmon_scope_exists': os.path.exists(scope), 'conmon_scope': scope, 'container': cid}
def n_path(path):
    if not os.path.exists(path):
        return {'exists': False}
    return {'exists': True, 'is_dir': os.path.isdir(path), 'mode': oct(os.stat(path).st_mode & 0o777),
            'bytes': os.path.getsize(path) if os.path.isfile(path) else None,
            'sha256': _sha256(path) if os.path.isfile(path) else None,
            'entries': sorted(os.listdir(path)) if os.path.isdir(path) else None}
def n_space(path):
    usage = shutil.disk_usage(path)
    return {'path': path, 'total': usage.total, 'used': usage.used, 'free': usage.free}
def n_read(path, limit=200000):
    with open(path, 'rb') as f:
        data = f.read(limit)
    return {'text': data.decode(errors='replace'), 'sha256': _sha256(path), 'bytes': os.path.getsize(path)}
def n_write_document(box, authorization, name, text):
    """Transport-controller write into a delivery directory (used to deliver, tamper or forge)."""
    directory = os.path.join(_state(), box, authorization)
    os.makedirs(directory, mode=0o700, exist_ok=True)
    path = os.path.join(directory, name)
    with open(path, 'w') as f:
        f.write(text)
    os.chmod(path, 0o600)
    return {'path': path, 'sha256': _sha256(path), 'bytes': os.path.getsize(path)}
def n_corrupt(path, mode='append', data='tampered'):
    size = os.path.getsize(path)
    if mode == 'append':
        with open(path, 'ab') as f:
            f.write(data.encode())
    elif mode == 'truncate':
        os.truncate(path, size - len(data.encode()))
    else:
        raise ValueError(mode)
    return {'path': path, 'previous_bytes': size, 'bytes': os.path.getsize(path), 'sha256': _sha256(path)}
def n_corrupt_archive(source, target, member):
    """A self-consistent archive defect: one checkpoint image truncated, the archive rebuilt in order.
    Simulates a damaged archive that still lists the expected entries (documents are forged separately).

    `member` selects the damage: a name truncates that entry, None truncates the inventory (an immediate
    CRIU failure), and 'largest-pages' truncates the biggest memory image, which is the hard shape that
    makes CRIU spin and write until something stops it."""
    work = target + '.work'
    subprocess.run(['rm', '-rf', work], check=True)
    os.makedirs(work, mode=0o700)
    entries = subprocess.run(['tar', '-tf', source], check=True, capture_output=True, text=True).stdout.splitlines()
    subprocess.run(['tar', '-C', work, '-xf', source], check=True)
    if member == 'largest-pages':
        member = None
    elif member is None:
        # Truncating the inventory leaves every entry and every other file intact, so the archive still passes
        # every structural check and only CRIU discovers the damage, failing early. Truncating a memory image
        # instead was observed to make CRIU spin and write a multi-gigabyte restore log.
        member = 'checkpoint/inventory.img' if 'checkpoint/inventory.img' in entries else None
    if member is None:
        pages = [e for e in entries if e.startswith('checkpoint/pages-') and e.endswith('.img')]
        assert pages, entries
        member = max(pages, key=lambda e: os.path.getsize(os.path.join(work, e)))
    victim = os.path.join(work, member)
    before = os.path.getsize(victim)
    os.truncate(victim, before // 2)
    listing = os.path.join(work, '.entries')
    with open(listing, 'w') as f:
        f.write('\n'.join(e for e in entries if e != './') + '\n')
    subprocess.run(['tar', '-C', work, '--zstd', '--no-recursion', '-cf', target, '-T', listing], check=True)
    os.chmod(target, 0o600)
    subprocess.run(['rm', '-rf', work], check=True)
    return {'target': target, 'member': member, 'member_bytes_before': before, 'member_bytes_after': before // 2,
            'bytes': os.path.getsize(target), 'sha256': _sha256(target)}
def _libpod_cgroups(container_id):
    return [f'/sys/fs/cgroup/machine.slice/libpod-{container_id}.scope',
            f'/sys/fs/cgroup/machine.slice/libpod-conmon-{container_id}.scope']
def _cgroup_members(path):
    found = []
    for root, _dirs, files in os.walk(path):
        if 'cgroup.procs' in files:
            try:
                found += [(int(l), root) for l in open(os.path.join(root, 'cgroup.procs')) if l.strip()]
            except OSError:
                pass
    return sorted(set(found))
def _start_epoch(pid):
    """Start time of a process, from /proc/<pid>/stat, in seconds since the epoch."""
    try:
        boot = next(int(l.split()[1]) for l in open('/proc/stat') if l.startswith('btime '))
        stat = open(f'/proc/{pid}/stat').read()
        return boot + int(stat[stat.rindex(')') + 2:].split()[19]) // 100
    except (OSError, ValueError, StopIteration, IndexError):
        return None
def _live_cgroup(pid):
    try:
        return open(f'/proc/{pid}/cgroup').read().strip().rsplit('::', 1)[-1]
    except OSError:
        return None
def _pidfd_sigkill(pid, container_id, not_before=None):
    """Signal a pinned process only after its cgroup and start time are rechecked through /proc."""
    if not callable(getattr(os, 'pidfd_open', None)) or not callable(getattr(signal, 'pidfd_send_signal', None)):
        return {'pid': pid, 'decision': 'refused', 'reason': 'pidfd signalling is unavailable'}
    try:
        pidfd = os.pidfd_open(pid, 0)
    except OSError as e:
        return {'pid': pid, 'decision': 'refused', 'reason': f'pidfd_open failed: {e}'}
    try:
        cgroup = _live_cgroup(pid)
        started = _start_epoch(pid)
        prefixes = (f'/machine.slice/libpod-{container_id}.scope', f'/machine.slice/libpod-conmon-{container_id}.scope')
        in_scope = isinstance(cgroup, str) and cgroup.startswith(prefixes)
        recent = not_before is None or (started is not None and started >= int(not_before))
        if not in_scope or not recent:
            return {'pid': pid, 'decision': 'refused', 'reason': 'post-pidfd ownership recheck failed',
                    'cgroup': cgroup, 'start_epoch': started, 'member_of_expected_cgroup': in_scope,
                    'started_at_or_after_claim': recent}
        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
        return {'pid': pid, 'decision': 'sigkill', 'reason': 'signal sent through pidfd after ownership recheck',
                'cgroup': cgroup, 'start_epoch': started}
    except OSError as e:
        return {'pid': pid, 'decision': 'refused', 'reason': f'pidfd_send_signal failed: {e}'}
    finally:
        os.close(pidfd)
def n_cgroup_facts(container_id):
    """The suite's own reading of the facts a reclaim must be proven on: the two cgroups of a container,
    their members, and each member's start time. Independent of what PodMesh reports about them."""
    facts = {}
    for path in _libpod_cgroups(container_id):
        members = _cgroup_members(path) if os.path.isdir(path) else []
        facts[path] = {'exists': os.path.isdir(path),
                       'processes': [{'pid': pid, 'cgroup_procs_file': root, 'start_epoch': _start_epoch(pid),
                                      'comm': (open(f'/proc/{pid}/comm').read().strip() if os.path.exists(f'/proc/{pid}/comm') else None)}
                                     for pid, root in members]}
    facts['total_processes'] = sum(len(v['processes']) for v in facts.values() if isinstance(v, dict))
    return facts
def n_df(path):
    """Raw `df -B1` for the evidence, exactly as the tool prints it."""
    p = subprocess.run(['df', '-B1', path], capture_output=True, text=True, check=True)
    return {'path': path, 'df': p.stdout, 'free': shutil.disk_usage(path).free, 'at': time.time()}
def n_journal_text(unit=None, since=None, lines=200):
    """Raw journal text of a unit, for a resource incident's evidence. Distinct from n_journal, which
    reads the migration tables."""
    argv = ['journalctl', '--no-pager', '-n', str(lines), '-u', unit or _unit()]
    if since:
        argv += ['--since', f'@{int(since)}']
    p = subprocess.run(argv, capture_output=True, text=True)
    return {'unit': unit or _unit(), 'text': p.stdout, 'exit': p.returncode}
def n_restore_under_watchdog(request, floor_bytes, universe_uuid, authorization_id,
                             graph='/var/lib/containers/storage', timeout=900):
    """Sends one API request while a TEST-OWNED free-space watchdog runs beside it.

    The watchdog is this suite's own cleanup, not a product mechanism: if free space on the graph root
    drops below the floor, it ends every process in the cgroups of the exact named container bound to the
    restore claim and records why. The product's own bound is expected to act long before that; the watchdog
    exists so that a failure of the product's bound cannot fill a laboratory disk."""
    assert request.get('universe_uuid') == universe_uuid and request.get('authorization_id') == authorization_id
    box = []
    baseline = shutil.disk_usage(graph).free
    def send():
        box.append(n_api(request, timeout))
    thread = threading.Thread(target=send)
    thread.start()
    samples, fired, signals, binding = [], None, [], {'verified': False, 'errors': ['not observed yet'], 'container_id': None}
    containment_complete = False
    while thread.is_alive():
        free = shutil.disk_usage(graph).free
        samples.append([round(time.time(), 2), free])
        try:
            observed = n_inspect('podmesh-' + universe_uuid, timeout=2)['container']
            claim = n_restore_claim(authorization_id, timeout=.5)['claim']
            current_binding = watchdog_binding({'universe_uuid': universe_uuid, 'authorization_id': authorization_id}, observed, claim)
        except (OSError, sqlite3.Error, subprocess.SubprocessError, ValueError) as e:
            observed, claim = None, None
            current_binding = {'verified': False,
                               'errors': [f'bounded watchdog observation failed: {type(e).__name__}: {e}'],
                               'container_id': None}
        binding = current_binding
        if free < floor_bytes and not containment_complete:
            if fired is None:
                fired = {'at': time.time(), 'free': free, 'attempts': []}
            fired['attempts'].append({'at': time.time(), 'binding': binding})
            if binding['verified']:
                for path in _libpod_cgroups(binding['container_id']):
                    for pid, _root in _cgroup_members(path):
                        signals.append(_pidfd_sigkill(pid, binding['container_id'], claim.get('created_at') if claim else None))
                containment_complete = True
            else:
                signals.append({'decision': 'refused', 'reason': 'watchdog target was not bound to the exact restore claim'})
            fired['signals'] = signals
        time.sleep(.25)
    thread.join(60)
    try:
        observed = n_inspect('podmesh-' + universe_uuid, timeout=2)['container']
        claim = n_restore_claim(authorization_id, timeout=.5)['claim']
        final_binding = watchdog_binding({'universe_uuid': universe_uuid, 'authorization_id': authorization_id}, observed, claim)
    except (OSError, sqlite3.Error, subprocess.SubprocessError, ValueError) as e:
        final_binding = {'verified': False,
                         'errors': [f'bounded final watchdog observation failed: {type(e).__name__}: {e}'],
                         'container_id': None}
    binding = final_binding
    return {'response': box[0]['response'] if box else None, 'begin_ns': box[0]['begin_ns'] if box else None,
            'end_ns': box[0]['end_ns'] if box else None, 'watchdog_fired': fired, 'floor_bytes': floor_bytes,
            'baseline_free': baseline, 'minimum_free': min([s[1] for s in samples], default=baseline),
            'maximum_consumed': baseline - min([s[1] for s in samples], default=baseline),
            'attempt_container_id': binding.get('container_id'), 'target_binding': binding,
            'samples': samples[-60:], 'sample_count': len(samples)}
def n_kill_container_processes(container_id):
    """Test-owned cleanup: PodMesh reports the processes a failed restore leaves behind but never kills them,
    so the suite kills the ones that still name its own disposable container."""
    assert len(container_id) == 64 and all(c in '0123456789abcdef' for c in container_id), container_id
    signals = []
    for pid in n_processes(container_id):
        signals.append(_pidfd_sigkill(pid, container_id))
    return {'signals': signals, 'killed': [s['pid'] for s in signals if s['decision'] == 'sigkill']}
def n_kill_container_cgroups(container_id):
    """Test-owned cleanup by cgroup residency, for a disposable container of this suite: used only when the
    product deliberately left processes alone (an abort without reclaim_processes), so that the laboratory
    host does not keep them."""
    assert len(container_id) == 64 and all(c in '0123456789abcdef' for c in container_id), container_id
    signals = []
    for path in _libpod_cgroups(container_id):
        for pid, _root in _cgroup_members(path):
            signals.append(_pidfd_sigkill(pid, container_id))
    deadline = time.time() + 30
    while time.time() < deadline and any(os.path.isdir(p) for p in _libpod_cgroups(container_id)):
        time.sleep(.25)
    return {'signals': signals, 'killed': [s['pid'] for s in signals if s['decision'] == 'sigkill'],
            'cgroups_gone': not any(os.path.isdir(p) for p in _libpod_cgroups(container_id))}
def n_kill_service():
    subprocess.run(['systemctl', 'kill', '--signal=SIGKILL', _unit()], check=True)
    return {'killed': _unit()}
def n_restart_service():
    subprocess.run(['systemctl', 'restart', _unit()], check=True)
    return n_ready()
def n_interrupt_collector_after_effect(request, barrier_dir, expected_main_pid, expected_binary_sha256, timeout=90):
    """Arm the exact post-commit barrier, prove it, then SIGKILL only this qualification unit's MainPID."""
    operation_id = request.get('operation_id')
    universe_uuid = request.get('candidates', [{}])[0].get('universe_uuid')
    candidate_class = request.get('candidates', [{}])[0].get('class')
    assert request.get('operation') == 'garbage_collect_apply' and len(request.get('candidates', [])) == 1
    assert str(uuid.UUID(operation_id)) == operation_id and str(uuid.UUID(universe_uuid)) == universe_uuid
    assert candidate_class == 'terminal_reservation_container_absent'
    directory = os.path.abspath(barrier_dir)
    assert os.path.realpath(directory) == directory, 'collector barrier directory traverses a symlink'
    directory_stat = os.lstat(directory)
    assert stat.S_ISDIR(directory_stat.st_mode) and directory_stat.st_uid == 0 and directory_stat.st_mode & 0o777 == 0o700
    def main_pid():
        text = subprocess.run(['systemctl', 'show', _unit(), '--property=MainPID', '--value'],
                              capture_output=True, text=True, check=True).stdout.strip()
        return int(text) if text.isdigit() else 0
    initial_pid = main_pid()
    assert initial_pid == expected_main_pid and initial_pid > 0
    assert _sha256(f'/proc/{initial_pid}/exe') == expected_binary_sha256
    process_env = dict(item.partition(b'=')[::2] for item in open(f'/proc/{initial_pid}/environ', 'rb').read().split(b'\0') if b'=' in item)
    assert os.path.realpath(process_env[b'PODMESH_TEST_COLLECTOR_BARRIER_DIR'].decode()) == directory
    arm_path = os.path.join(directory, f'arm-{operation_id}.json')
    reached_path = os.path.join(directory, f'reached-{operation_id}.json')
    release_path = os.path.join(directory, f'release-{operation_id}.json')
    assert all(not os.path.lexists(path) for path in (arm_path, reached_path, release_path)), 'exact barrier control already exists'
    control = {'format': 'podmesh-test-collector-barrier/1', 'operation_id': operation_id, 'phase': 'arm'}
    descriptor = os.open(arm_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0), 0o600)
    with os.fdopen(descriptor, 'w') as arm:
        json.dump(control, arm, sort_keys=True)
        arm.write('\n')
        arm.flush()
        os.fsync(arm.fileno())
    directory_fd = os.open(directory, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0))
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)
    box, killed = [], False
    def send():
        box.append(n_api(request, timeout))
    thread = threading.Thread(target=send)
    thread.start()
    marker, state_at_barrier = None, None
    expected = {'operation_id': operation_id, 'candidate_key': universe_uuid,
                'universe_uuid': universe_uuid, 'class': candidate_class}
    try:
        deadline = time.monotonic() + 30
        while not os.path.lexists(reached_path):
            assert time.monotonic() < deadline and thread.is_alive(), ('collector never reached its post-commit barrier', box)
            time.sleep(.05)
        reached_stat = os.lstat(reached_path)
        assert (stat.S_ISREG(reached_stat.st_mode) and not stat.S_ISLNK(reached_stat.st_mode)
                and reached_stat.st_uid == 0 and reached_stat.st_mode & 0o777 == 0o600 and reached_stat.st_size <= 4096)
        with open(reached_path) as reached:
            marker = json.load(reached)
        marker_verdict = validate_collector_barrier_marker(marker, expected)
        assert marker_verdict['verified'], marker_verdict
        state_at_barrier = n_collector_crash_state(operation_id, universe_uuid)
        state_verdict = validate_interrupted_collection_state(state_at_barrier, expected)
        assert state_verdict['verified'], state_verdict
        assert main_pid() == initial_pid and _sha256(f'/proc/{initial_pid}/exe') == expected_binary_sha256
        subprocess.run(['systemctl', 'kill', '--kill-whom=main', '--signal=SIGKILL', _unit()], check=True)
        killed = True
        thread.join(30)
        n_ready(120)
        thread.join(5)
        assert not thread.is_alive() and len(box) == 1, 'the interrupted collector socket did not terminate exactly once'
        restarted_pid = main_pid()
        assert restarted_pid > 0 and restarted_pid != initial_pid
        assert _sha256(f'/proc/{restarted_pid}/exe') == expected_binary_sha256
        state_after_restart = n_collector_crash_state(operation_id, universe_uuid)
        restarted_verdict = validate_interrupted_collection_state(state_after_restart, expected)
        assert restarted_verdict['verified'], restarted_verdict
        return {'first_response': box[0] if box else None, 'marker': marker,
                'state_at_barrier': state_at_barrier, 'state_after_restart': state_after_restart,
                'main_pid_before': initial_pid, 'main_pid_after': restarted_pid,
                'unit': _unit(), 'binary_sha256': expected_binary_sha256}
    finally:
        if not killed and thread.is_alive() and not os.path.lexists(release_path):
            release = dict(control, phase='release')
            descriptor = os.open(release_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0), 0o600)
            with os.fdopen(descriptor, 'w') as output:
                json.dump(release, output, sort_keys=True)
                output.write('\n')
                output.flush()
                os.fsync(output.fileno())
            thread.join(65)
        for path in (release_path, reached_path, arm_path):
            try:
                os.unlink(path)
            except FileNotFoundError:
                pass
def n_processes(*needles):
    found = []
    for pid in filter(str.isdigit, os.listdir('/proc')):
        try:
            argv = open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')
        except OSError:
            continue
        if all(n.encode() in argv for n in needles):
            found.append(int(pid))
    return found
def n_interrupt_restore(request, import_path, unit):
    """Sends migration_restore and kills the service while the restore command runs in its own scope.

    The kill is timed on Podman's own process, not on the systemd-run wrapper that carries the same
    arguments: waiting for the wrapper alone was observed to lose the race on a fast restore, because the
    request spends its first seconds hashing and decompressing the archive before the command starts. CRIU
    is recorded when it becomes visible, but the kill never waits for it beyond the command's own life."""
    box = []
    def send():
        box.append(n_api(request))
    thread = threading.Thread(target=send)
    thread.start()
    def argv_of(pid):
        try:
            return open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')
        except OSError:
            return []
    def podman_pids():
        found = []
        for pid in filter(str.isdigit, os.listdir('/proc')):
            argv = argv_of(pid)
            if argv and argv[0].endswith(b'/podman') and b'restore' in argv and f'--import={import_path}'.encode() in argv:
                found.append(int(pid))
        return found
    def criu_pids():
        found = []
        for pid in filter(str.isdigit, os.listdir('/proc')):
            argv = argv_of(pid)
            argv0 = argv[0].decode(errors='replace') if argv else ''
            if argv0.endswith('/criu') or argv0 == 'criu':
                found.append({'pid': int(pid), 'executable': argv0})
        return found
    began = time.time()
    deadline = began + 300
    restore_pids = []
    while not restore_pids:
        assert time.time() < deadline and thread.is_alive(), ('the restore command was never observed', box)
        restore_pids = podman_pids()
        time.sleep(.002)
    observed_at = time.time()
    # A short look for CRIU, abandoned the moment the command itself is gone: the kill must land while the
    # command is still running, which is what this test is about.
    criu, criu_deadline = [], time.time() + 3
    while time.time() < criu_deadline and not criu and thread.is_alive() and podman_pids():
        criu = criu_pids()
        time.sleep(.002)
    at_kill = {'restore_command_pids': restore_pids, 'criu_processes': criu, 'scope_before_kill': n_scope(unit)['active_state'],
               'command_seen_after_seconds': round(observed_at - began, 2), 'killed_after_seconds': round(time.time() - began, 2),
               'command_still_running_at_kill': bool(podman_pids()), 'request_still_open_at_kill': thread.is_alive()}
    subprocess.run(['systemctl', 'kill', '--signal=SIGKILL', _unit()], check=True)
    at_kill['restore_command_alive_after_kill'] = bool(podman_pids())
    at_kill['criu_alive_after_kill'] = bool(criu_pids())
    at_kill['scope_after_kill'] = n_scope(unit)['active_state']
    thread.join(120)
    n_ready(120)
    deadline = time.time() + 300
    while n_scope(unit)['active_state'] not in ('inactive', 'failed'):
        assert time.time() < deadline, 'the restore scope did not finish'
        time.sleep(.2)
    return {'first_response': box[0] if box else None, 'at_kill': at_kill, 'scope_after_finish': n_scope(unit)['active_state']}


def _node(argv):
    function = globals()['n_' + argv[0]]
    arguments = json.loads(base64.b64decode(argv[1])) if len(argv) > 1 else {}
    if isinstance(arguments, list):
        return function(*arguments)
    return function(**arguments)


# ---------------------------------------------------------- controller side

class Host:
    """One lab host, reached through SSH, with its API windows and its checks."""

    def __init__(self, role, target, control, socket_path, state_dir, unit):
        self.role, self.target, self.control = role, target, control
        self.socket, self.state_dir, self.unit = socket_path, state_dir, unit
        self.windows, self.fixtures, self.removed_by_test = [], [], {}
        self.ledger, self.cleanup_report = {}, []
        self.source = open(os.path.abspath(__file__), 'rb').read()
        self.identity = self.call('api', request={'operation': 'identity'})['response']['data']['host_uuid']

    def ssh(self, command, input_bytes=None, check=True):
        argv = ['ssh', '-o', 'BatchMode=yes', '-o', 'ControlMaster=auto', '-o', f'ControlPath={self.control}/%C',
                '-o', 'ControlPersist=180', '-o', 'ConnectTimeout=15', self.target, command]
        p = subprocess.run(argv, input=input_bytes, capture_output=True)
        if check and p.returncode:
            raise RuntimeError(f'{self.role} ssh failed ({p.returncode}): {command}\n{p.stderr.decode()}')
        return p

    def call(self, function, **arguments):
        payload = base64.b64encode(json.dumps(arguments).encode()).decode()
        command = (f'sudo env PODMESH_SOCKET={self.socket} PODMESH_STATE_DIR={self.state_dir} PODMESH_UNIT={self.unit} '
                   f'python3 -B - {function} {payload}')
        p = self.ssh(command, input_bytes=self.source)
        lines = [l for l in p.stdout.decode().splitlines() if l.strip()]
        if not lines:
            raise RuntimeError(f'{self.role}.{function} returned nothing: {p.stderr.decode()}')
        return json.loads(lines[-1])

    # --- API
    def api(self, request):
        result = self.call('api', request=request)
        self.windows.append((result['begin_ns'], result['end_ns'], request.get('operation'), request.get('operation_id')))
        response = result['response']
        data = response.get('data') if response.get('ok') else None
        if isinstance(data, dict) and isinstance(data.get('container_id'), str) and isinstance(data.get('universe_uuid'), str):
            self.record_container('podmesh-' + data['universe_uuid'], data['container_id'], data['universe_uuid'],
                                  request.get('operation_id'), 'api')
        if request.get('operation') == 'migration_restore' and response.get('ok') is False:
            self._record_failed_restore_claim(request)
        return response
    def ok(self, request, check=None, checks=None):
        response = self.api(request)
        assert response.get('ok'), (self.role, request, response)
        if check and checks is not None:
            checks.append(f'[{self.role}] {check}')
        return response['data']
    def refused(self, request, check, expected, checks):
        """A refusal must change no Podman state, no migration journal row and no delivered document."""
        before = self.call('snapshot')
        response = self.api(request)
        assert response.get('ok') is False and expected in json.dumps(response), (self.role, check, expected, response)
        after = self.call('snapshot')
        assert after == before, (self.role, check, 'a refused request changed state', _difference(before, after))
        checks.append(f'[{self.role}] refused without effect: {check}')
        return response
    def status(self, uuid_value):
        return self.ok({'operation': 'migration_status', 'universe_uuid': uuid_value})

    def attest(self, expected_binary_sha256, expected_collector_barrier_dir=None):
        proof = self.call('service_identity', expected_binary_sha256=expected_binary_sha256,
                          expected_collector_barrier_dir=expected_collector_barrier_dir)
        assert proof['verified'], (self.role, proof)
        assert proof['api_host_uuid'] == self.identity, (self.role, proof['api_host_uuid'], self.identity)
        return proof

    def record_container(self, name, container_id, universe_uuid=None, operation_id=None, source='direct'):
        assert len(container_id) == 64 and all(c in '0123456789abcdef' for c in container_id), container_id
        entry = {'name': name, 'container_id': container_id, 'universe_uuid': universe_uuid,
                 'operation_id': operation_id, 'source': source}
        self.ledger[name] = entry
        if name not in self.fixtures:
            self.fixtures.append(name)
        return entry

    def _record_failed_restore_claim(self, request):
        """Register the immutable container ID from every failed restore claim, including ordinary restores."""
        authorization_id = request.get('authorization_id')
        universe_uuid = request.get('universe_uuid')
        if not isinstance(authorization_id, str) or not isinstance(universe_uuid, str):
            return None
        try:
            claim = self.call('restore_claim', authorization_id=authorization_id)['claim']
        except Exception:
            # Final baseline-delta reconciliation remains the fail-closed backstop when the claim cannot be read.
            return None
        if not isinstance(claim, dict):
            return None
        container_id = claim.get('container_id')
        if (claim.get('authorization_id') != authorization_id or claim.get('universe_uuid') != universe_uuid
                or claim.get('state') not in ('restoring', 'restore_failed')
                or not isinstance(container_id, str) or len(container_id) != 64
                or any(c not in '0123456789abcdef' for c in container_id)):
            return None
        return self.record_container('podmesh-' + universe_uuid, container_id, universe_uuid,
                                     claim.get('operation_id') or request.get('operation_id'), 'failed_restore_claim')

    # --- fixtures owned by the suite
    def fixture(self, name, *args):
        self.call('podman_run', args=['create', '--pull=never', '--network=none', '--name', name, *args])
        container = self.call('inspect', name=name)['container']
        self.record_container(name, container['Id'], source='direct')
        return name
    def remove_fixture(self, name, note=None):
        entry = self.ledger.get(name)
        if entry is None:
            result = {'removed': False, 'verified': False, 'name': name,
                      'reason': 'no exact cleanup-ledger entry; state retained'}
        else:
            result = self.call('remove_owned', entry=entry)
            if result.get('absent'):
                self.ledger.pop(name, None)
                if name in self.fixtures:
                    self.fixtures.remove(name)
        self.cleanup_report.append(result)
        if note:
            self.removed_by_test[name] = note
        return result
    def cleanup(self, baseline_podman=None, observed_before_cleanup=None):
        for addition in untracked_container_additions(baseline_podman, observed_before_cleanup, self.ledger):
            self.cleanup_report.append({
                'removed': False, 'absent': False, 'verified': False,
                'container_id': addition['container_id'], 'observed': addition['observed'],
                'reason': 'container was added after the baseline but has no exact cleanup-ledger entry; uncertain state retained',
            })
        for name in list(self.ledger):
            self.remove_fixture(name)
        return self.cleanup_report


def _difference(before, after):
    """A short description of what a refused request changed, for the assertion message."""
    changed = []
    for key in before:
        if before[key] != after.get(key):
            changed.append(key)
    return changed


def request(operation, universe, reference, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), universe_uuid=universe, authorization_ref=reference, **extra)


def transfer(source, destination, authorization, files=('handoff.json', 'manifest.json', 'checkpoint.tar.zst'), box='outbox', into='inbox'):
    """Transport controller: copies documents from one host's outbox to the other's inbox, through this
    workstation, and verifies that every byte arrived by comparing SHA-256 on both sides."""
    members = ' '.join(f"{authorization}/{f}" for f in files)
    out = source.ssh(f'sudo tar -C {source.state_dir}/{box} -cf - {members}')
    destination.ssh(f'sudo mkdir -p -m 0700 {destination.state_dir}/{into} && sudo tar -C {destination.state_dir}/{into} -xf -',
                    input_bytes=out.stdout)
    sent = source.call('boxes')[box].get(authorization, {})
    arrived = destination.call('boxes')[into].get(authorization, {})
    for f in files:
        assert sent[f]['sha256'] == arrived[f]['sha256'], (f, sent.get(f), arrived.get(f))
    return {'authorization_id': authorization, 'files': {f: arrived[f] for f in files}}


def counter_values(samples):
    """(token, n) pairs from /tmp/state samples, ignoring unreadable reads."""
    values = []
    for _, text in samples:
        parts = text.split()
        if len(parts) == 2 and parts[1].isdigit():
            values.append((parts[0], int(parts[1])))
    return values


def memory_continued(before, after):
    """Continuity as the protocol defines it: same memory-only token, the first value after the restore at
    least the last one observed before the checkpoint, and strictly increasing afterwards."""
    b, a = counter_values(before), counter_values(after)
    assert b and a, ('no counter samples', before, after)
    token, last = b[-1]
    tokens = {t for t, _ in a}
    assert tokens == {token}, ('the token changed: a fresh start, not restored memory', token, tokens)
    assert a[0][1] >= last, ('the counter restarted below its checkpointed value', last, a[0][1])
    progression = [n for _, n in a]
    assert progression[-1] > progression[0], ('the counter did not progress after the restore', progression)
    assert all(y >= x for x, y in zip(progression, progression[1:])), ('the counter went backwards', progression)
    return {'token': token, 'last_before_checkpoint': last, 'first_after_restore': a[0][1], 'last_after_restore': progression[-1],
            'distinct_values_after_restore': len(set(progression))}


def event_report(host, since, until, universes, fixture_ids, extra_windows=(), removed_by_test=()):
    """Every Podman container event on API-managed universes must fall inside an API request window of that
    host. Events of the suite's own direct fixtures are identified by container ID and reported separately, as
    is the documented direct removal of a container the API deliberately refuses to delete (a reserved source)."""
    events = host.call('events', since=since, until=until)['events']
    windows = [(b, e) for b, e, *_ in host.windows] + list(extra_windows)
    managed = {'podmesh-' + u for u in universes}
    statuses, outside, fixture_events, test_removals = {}, [], 0, 0
    for e in events:
        if e.get('Type') != 'container' or e.get('Name') not in managed or e.get('Status') == 'cleanup':
            continue
        if e.get('ID') in fixture_ids:
            fixture_events += 1
            continue
        if e.get('Status') == 'remove' and e.get('ID') in removed_by_test:
            test_removals += 1
            continue
        statuses[e['Status']] = statuses.get(e['Status'], 0) + 1
        if not any(b <= e['timeNano'] <= f for b, f in windows):
            outside.append(e)
    return {'statuses': statuses, 'outside_api_windows': outside, 'suite_fixture_events': fixture_events,
            'documented_test_removals': test_removals, 'events_examined': len(events)}


if __name__ == '__main__':
    print(json.dumps(_node(sys.argv[1:])))
