#!/usr/bin/env python3
"""Source-side migration preparation through the PodMesh API, verified independently.

Development test for a disposable lab host running an isolated PodMesh service. Every product
mutation goes through the API. Direct Podman writes are limited to named fixtures and to removing
this test's reserved universes, which the API deliberately refuses to delete. No restore is run."""
import hashlib, json, os, socket, stat, subprocess, tempfile, time, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']
RUNTIME = {'/usr/lib/podmesh-vzcriu/criu': '4ecb663e7e3b019cdfa534c0d4cce4134938f87ae3b45cac35a7481c0a947cd4',
           '/usr/bin/podmesh-vzcriu': '97d0f0728c54dd340b6ec6d002e1946b3a41fdd8ceb3ded5d4d5cccfda44b28e',
           '/opt/podmesh-vzcriu-kit/bin/criu': 'fcbec55d0401080d020b1a56299d007792ca8215f68c533756080ff5e86948eb'}
checks, windows, universes, fixtures, removed_by_test = [], [], [], [], {}

def raw_api(r, timeout=400):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(timeout); s.connect(endpoint)
        s.sendall(json.dumps(r).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())
def api(r):
    begin = time.time_ns()
    try:
        return raw_api(r)
    finally:
        windows.append((begin, time.time_ns()))
def podman(*args, check=True):
    p = subprocess.run(['podman', *args], capture_output=True)
    if check and p.returncode: raise RuntimeError(f'podman {args}: {p.stderr.decode()}')
    return p
def out(*args): return podman(*args).stdout.decode().strip()
def inspect(name): return json.loads(out('container', 'inspect', name))[0]
def exists(name): return podman('container', 'exists', name, check=False).returncode == 0
def images(): return json.loads(out('images', '--all', '--format', 'json'))
def state():
    containers = {c['Id']: (tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'), c.get('ExitCode'), c.get('ImageID'))
                  for c in json.loads(out('ps', '--all', '--format', 'json'))}
    return containers, {i['Id']: tuple(sorted(i.get('Names') or [])) for i in images()}, sorted(out('volume', 'ls', '--quiet').split())
def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''): h.update(chunk)
    return h.hexdigest()
def request(op, u, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), universe_uuid=u, authorization_ref='disposable-lab-migration-source-test', **extra)
def ok(r, check=None):
    result = api(r); assert result['ok'], (r, result)
    if check: checks.append(check)
    return result['data']
def refused(r, check, expected):
    before = state()
    result = api(r)
    assert result['ok'] is False and expected in json.dumps(result), (check, expected, result)
    assert state() == before, (check, 'refused request changed Podman state')
    checks.append('refused without effect: ' + check)
    return result
def status(u):
    result = raw_api({'operation': 'migration_status', 'universe_uuid': u}); assert result['ok'], result
    return result['data']
def ready():
    for _ in range(300):
        try:
            if raw_api({'operation': 'capabilities'}, 5)['ok']: return
        except (OSError, ValueError): pass
        time.sleep(.1)
    raise RuntimeError('Service did not become ready')
def image_of(ref): return next(i['Id'] for i in images() if ref in (i.get('Names') or []))
def universe(image, command):
    u = str(uuid.uuid4()); universes.append(u)
    ok(request('create', u, image='sha256:' + image, command=command))
    return u, 'podmesh-' + u
def fixture(name, *args):
    fixtures.append(name)
    out('create', '--pull=never', '--network=none', '--name', name, *args)

alpine = image_of('docker.io/library/alpine:3.22')
debian = next((i['Id'] for i in images() if 'docker.io/library/debian:13-slim' in (i.get('Names') or [])), None)
host = raw_api({'operation': 'identity'})['data']['host_uuid']
destination = str(uuid.uuid4())
baseline = state()
since = int(time.time()) - 1
unrelated = 'pmfixture-unrelated-' + str(uuid.uuid4())
def migration(op, u, container_id, **override):
    r = request(op, u, container_id=container_id, image='sha256:' + alpine, source_host_uuid=host, destination_host_uuid=destination)
    r.update(override)
    return r
def no_reservation(u, operation_id=None):
    assert status(u)['reservation'] is None
    if operation_id: assert not os.path.exists(os.path.join(state_dir, 'migrations', operation_id))
try:
    out('run', '-d', '--pull=never', '--network=none', '--name', unrelated, alpine, 'sleep', '3600')
    unrelated_view = (inspect(unrelated)['Id'], inspect(unrelated)['State']['StartedAt'])
    c, C = universe(alpine, COUNTER)
    ok(request('start', c))
    cid, c_started = inspect(C)['Id'], inspect(C)['State']['StartedAt']

    # Refused before the journal records anything.
    anonymous = migration('migration_checkpoint', c, cid); del anonymous['authorization_ref']
    refused(anonymous, 'checkpoint without authorization reference', 'Missing authorization_ref')
    refused(migration('migration_checkpoint', c, cid[:12]), 'abbreviated container ID', 'container_id')
    refused(migration('migration_checkpoint', c, cid, image='docker.io/library/alpine:3.22'), 'mutable image reference', 'sha256')
    refused(migration('migration_checkpoint', c, cid, destination_host_uuid='not-a-uuid'), 'invalid destination UUID', 'destination_host_uuid')
    refused(migration('migration_checkpoint', c, cid, destination_host_uuid=host), 'destination equal to source', 'must differ')
    # Refused by fresh preconditions: no suspension, no reservation, no artifact directory.
    cases = [('source host UUID of another host', {'source_host_uuid': str(uuid.uuid4())}, 'is not this host'),
             ('destination set to this host', {'source_host_uuid': str(uuid.uuid4()), 'destination_host_uuid': host}, 'destination_host_uuid is this host'),
             ('container ID of another container', {'container_id': inspect(unrelated)['Id']}, 'container_id does not match')]
    if debian: cases.append(('image of another image', {'image': 'sha256:' + debian}, 'image does not match'))
    for label, override, expected in cases:
        override = dict(override)
        r = migration('migration_checkpoint', c, override.pop('container_id', cid), **override)
        refused(r, 'checkpoint with ' + label, expected)
        no_reservation(c, r['operation_id'])
    s, S = universe(alpine, COUNTER)
    r = migration('migration_checkpoint', s, inspect(S)['Id'])
    refused(r, 'checkpoint of a created, never started universe', 'not running'); no_reservation(s, r['operation_id'])
    if debian:
        g, G = universe(debian, ['sleep', '3600'])
        ok(request('start', g))
        gid = inspect(G)['Id']
        report = ok(migration('migration_preflight', g, gid, image='sha256:' + debian))
        assert report['compatible'] is False and any('glibc' in b for b in report['blockers']), report
        r = migration('migration_checkpoint', g, gid, image='sha256:' + debian)
        refused(r, 'checkpoint of a glibc workload (rseq risk for CRIU 3.15)', 'glibc'); no_reservation(g, r['operation_id'])
    x = str(uuid.uuid4())
    fixture('podmesh-' + x, '--label', f'io.podmesh.universe={x}', '--label', 'io.podmesh.creation-operation=' + str(uuid.uuid4()), alpine, 'sleep', '3600')
    out('start', 'podmesh-' + x)
    refused(migration('migration_checkpoint', x, inspect('podmesh-' + x)['Id']), 'checkpoint of a labelled container unknown to the journal', 'not recorded')
    o = str(uuid.uuid4())
    fixture('podmesh-' + o, alpine, 'sleep', '3600'); out('start', 'podmesh-' + o)
    refused(migration('migration_checkpoint', o, inspect('podmesh-' + o)['Id']), 'checkpoint of an unmanaged container', 'not managed')

    # Preflight has no effect.
    preflight = migration('migration_preflight', c, cid)
    report = ok(preflight)
    assert report['compatible'] is True and report['blockers'] == [], report
    assert report['facts']['runtime']['sha256'] == RUNTIME and all(p['libc'] == 'musl' for p in report['facts']['processes']['processes'])
    assert inspect(C)['State']['Status'] == 'running' and inspect(C)['State']['StartedAt'] == c_started
    no_reservation(c, preflight['operation_id'])
    assert ok(preflight)['historical']
    checks.append('preflight reports compatibility with pinned runtime hashes and musl processes, without reservation, suspension or artifacts')

    # Artifact directory without a reservation: files in it are refused, an empty one (left by a crash
    # before the reservation was persisted) is reused and made private.
    stray = migration('migration_checkpoint', c, cid)
    stray_dir = os.path.join(state_dir, 'migrations', stray['operation_id'])
    os.makedirs(stray_dir); open(os.path.join(stray_dir, 'foreign'), 'w').close()
    refused(stray, 'checkpoint whose artifact directory holds files without a reservation', 'not empty')
    assert status(c)['reservation'] is None and os.listdir(stray_dir) == ['foreign'] and inspect(C)['State']['StartedAt'] == c_started
    os.remove(os.path.join(stray_dir, 'foreign')); os.rmdir(stray_dir)

    # Checkpoint.
    checkpoint = migration('migration_checkpoint', c, cid)
    os.makedirs(os.path.join(state_dir, 'migrations', checkpoint['operation_id']), mode=0o755)
    result = ok(checkpoint)
    ci = inspect(C)
    assert result['status'] == 'verified' and result['finalized_after_interruption'] is False, result
    assert ci['State']['Checkpointed'] is True and ci['State']['Running'] is False and ci['State']['Status'] == 'exited', ci['State']
    assert ci['State']['StartedAt'] == c_started and ci['Id'] == cid
    directory = os.path.join(state_dir, 'migrations', checkpoint['operation_id'])
    archive, manifest_path, log_path = (os.path.join(directory, f) for f in ('checkpoint.tar.zst', 'manifest.json', 'dump.log'))
    assert result['artifact_directory'] == directory and stat.S_IMODE(os.stat(directory).st_mode) == 0o700
    assert all(stat.S_IMODE(os.stat(os.path.join(directory, f)).st_mode) == 0o600 for f in os.listdir(directory))
    assert sha256(archive) == result['archive']['sha256'] and os.path.getsize(archive) == result['archive']['bytes']
    assert sha256(manifest_path) == result['manifest']['sha256']
    manifest = json.load(open(manifest_path))
    assert (manifest['operation_id'], manifest['universe_uuid'], manifest['container_id'], manifest['image_id'], manifest['source_host_uuid'],
            manifest['destination_host_uuid'], manifest['container_started_at']) == (checkpoint['operation_id'], c, cid, alpine, host, destination, c_started), manifest
    assert manifest['archive']['sha256'] == result['archive']['sha256'] and manifest['runtime']['sha256'] == RUNTIME
    dump = open(log_path, 'rb').read()
    assert b'(gitid v3.15.5.3)' in dump and b'Dumping finished successfully' in dump
    assert open(ci['State']['CheckpointLog'], 'rb').read() == dump
    listing = subprocess.run(['tar', '-tf', archive], capture_output=True, text=True, check=True).stdout.splitlines()
    assert {'config.dump', 'spec.dump', 'checkpoint/inventory.img'} <= set(listing)
    checks.append('checkpoint stops the source with Podman Checkpointed=true, same container and start time; archive, manifest and dump log hashes verified independently')
    checks.append('dump log proves the private runtime v3.15.5.3 performed the dump; manifest binds operation, universe, container, image, source and destination')
    checks.append('an empty artifact directory left without a reservation is reused and made private (0700)')
    current = status(c)
    assert current['reservation']['state'] == 'checkpointed' and current['reservation']['operation_id'] == checkpoint['operation_id']
    assert current['artifacts']['archive_sha256_matches'] is True and current['artifacts']['manifest_sha256_matches'] is True
    release = current['release']
    assert release['permitted'] is False and release['operation_available'] is False
    assert release['preconditions_observed']['source_not_running'] is True and release['preconditions_observed']['source_checkpointed'] is True
    checks.append('status reports the durable reservation, verified artifacts and release preconditions, and states that no release exists')

    # Replay does not recapture.
    mtime, checkpointed_at = os.stat(archive).st_mtime_ns, ci['State']['CheckpointedAt']
    history = ok(checkpoint)
    assert history['replayed'] and history['historical'] and history['original_result'] == result, history
    assert history['current_artifacts']['archive_sha256_matches'] is True and history['current']['running'] is False
    assert inspect(C)['State']['CheckpointedAt'] == checkpointed_at and os.stat(archive).st_mtime_ns == mtime
    checks.append('retried checkpoint is historical: no recapture, same CheckpointedAt and archive, fresh re-hash')
    size = os.path.getsize(archive)
    with open(archive, 'ab') as f: f.write(b'tampered')
    assert ok(checkpoint)['current_artifacts']['archive_sha256_matches'] is False
    assert status(c)['artifacts']['archive_sha256_matches'] is False
    os.truncate(archive, size)
    assert sha256(archive) == result['archive']['sha256'] and ok(checkpoint)['current_artifacts']['archive_sha256_matches'] is True
    checks.append('replay and status detect a modified archive; the exact original bytes match again')

    # Reservation enforcement.
    refused(request('start', c), 'start of a reserved universe', 'reserved')
    refused(request('delete', c), 'delete of a reserved universe', 'reserved')
    refused(request('clone', str(uuid.uuid4()), source_uuid=c), 'clone from a reserved universe', 'reserved')
    refused(migration('migration_checkpoint', c, cid), 'second migration operation on a reserved universe', 'already reserved')
    refused(dict(checkpoint, destination_host_uuid=str(uuid.uuid4())), 'checkpoint operation ID reused with another destination', 'different request')
    noop = ok(request('stop', c, timeout_seconds=1, on_timeout='kill'))
    assert noop['action'] == 'none_already_stopped' and noop['stop_signal'] is None
    blocked = ok(migration('migration_preflight', c, cid))
    assert blocked['compatible'] is False and any('already reserved' in b for b in blocked['blockers'])
    checks.append('stop stays available on a reserved universe and has no effect on a checkpointed source')

    # Durability across a service restart.
    subprocess.run(['systemctl', 'restart', unit], check=True)
    ready()
    after = status(c)
    assert after['reservation']['state'] == 'checkpointed' and after['artifacts']['archive_sha256_matches'] is True
    assert ok(checkpoint)['current_artifacts']['archive_sha256_matches'] is True
    refused(request('start', c), 'start of a reserved universe after service restart', 'reserved')
    checks.append('reservation, artifacts and historical replay survive a service restart')

    # The API refuses to remove the reserved universe; this test removes its own fixture container.
    podman('rm', '--force', '--time', '0', C); removed_by_test[c] = 'reserved checkpointed fixture removed directly by the test'
    refused(request('create', c, image='sha256:' + alpine, command=['true']), 'create reusing a reserved universe UUID', 'reserved')
    refused(request('delete', c), 'delete of an absent reserved universe', 'reserved')
    gone = status(c)
    assert gone['reservation']['state'] == 'checkpointed' and gone['release']['preconditions_observed']['source_container_present'] is False
    assert gone['artifacts']['archive_sha256_matches'] is True
    checks.append('artifacts and reservation are preserved after the source container is removed out of band')

    # Cleanup of unreserved universes through the API, fixtures directly.
    if debian:
        ok(request('stop', g, timeout_seconds=1, on_timeout='kill')); ok(request('delete', g))
    ok(request('delete', s))
    for name in fixtures: podman('rm', '--force', '--time', '0', name)
    fixtures.clear()
    c_now = inspect(unrelated)
    assert (c_now['Id'], c_now['State']['StartedAt']) == unrelated_view and c_now['State']['Status'] == 'running'
    checks.append('unrelated running container untouched')
    until = int(time.time()) + 1
    events = [json.loads(e) for e in out('events', '--since', str(since), '--until', str(until), '--stream=false', '--format', 'json').splitlines() if e.strip()]
    managed = {'podmesh-' + u for u in universes}
    outside, statuses = [], {}
    for e in events:
        if e.get('Type') != 'container' or e.get('Name') not in managed or e.get('Status') == 'cleanup': continue
        statuses[e['Status']] = statuses.get(e['Status'], 0) + 1
        u = e['Name'][len('podmesh-'):]
        if any(b <= e['timeNano'] <= f for b, f in windows): continue
        if u in removed_by_test and e['Status'] == 'remove': continue
        outside.append(e)
    assert not outside, outside
    checks.append('every Podman event on API-created universes falls inside an API request window, except the documented fixture removal: ' + json.dumps(statuses))
    podman('rm', '--force', '--time', '0', unrelated)
    assert state() == baseline, 'Podman containers, images or volumes differ from the baseline'
    checks.append('all pre-existing containers, images and volumes unchanged; no container leftovers')
finally:
    for u in universes:
        if exists('podmesh-' + u): podman('rm', '--force', '--time', '0', 'podmesh-' + u, check=False)
    for name in fixtures + [unrelated]:
        if exists(name): podman('rm', '--force', '--time', '0', name, check=False)

print(json.dumps({'status': 'PASS', 'podman': out('--version'), 'host_uuid': host, 'destination_host_uuid': destination,
                  'checks': checks, 'api_windows': len(windows), 'checkpoint_request': checkpoint, 'checkpoint_result': result,
                  'artifact_directory': directory, 'artifact_files': sorted(os.listdir(directory)), 'preflight_facts': report['facts'],
                  'status_after_removal': gone, 'reserved_universes_left_in_journal': [c]}))
