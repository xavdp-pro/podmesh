#!/usr/bin/env python3
"""Check clone correctness, retries, source protection and preservation of unrelated
Podman resources against independent Podman reads. Run as root on a disposable lab host.
Every container and image this script creates is named from UUIDs generated here."""
import hashlib, json, os, socket, subprocess, tempfile, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
LABELS = ('io.podmesh.universe', 'io.podmesh.creation-operation')
checks, universes, references, forged = [], [], [], []

def api(r):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(400); s.connect(endpoint)
        s.sendall(json.dumps(r).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())
def podman(*args, check=True):
    p = subprocess.run(['podman', *args], capture_output=True)
    if check and p.returncode: raise RuntimeError(f'podman {args}: {p.stderr.decode()}')
    return p
def out(*args): return podman(*args).stdout.decode().strip()
def inspect(name): return json.loads(out('container', 'inspect', name))[0]
def exists(name): return podman('container', 'exists', name, check=False).returncode == 0
def images(): return json.loads(out('images', '--all', '--format', 'json'))
def image_named(ref): return [i for i in images() if ref in (i.get('Names') or [])]
def state():
    """Independent view of every container, image and volume on the host."""
    containers = {c['Id']: (tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'), c.get('ExitCode'), c.get('ImageID'))
                  for c in json.loads(out('ps', '--all', '--format', 'json'))}
    return containers, {i['Id']: tuple(sorted(i.get('Names') or [])) for i in images()}, sorted(out('volume', 'ls', '--quiet').split())
def source_view(name):
    c = inspect(name)
    return (c['Id'], c['State']['Status'], c['State']['StartedAt'], c['State']['FinishedAt'], c['Image'], c['Config']['Labels'], c['Config']['Cmd'], out('diff', name))
def request(op, u, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), universe_uuid=u, authorization_ref='disposable-lab-clone-test', **extra)
def ok(r, check=None):
    result = api(r); assert result['ok'], (r, result)
    if check: checks.append(check)
    return result['data']
def refused(r, check, expected):
    before = state()
    result = api(r)
    assert result['ok'] is False and expected in result['error'], (check, result)
    assert state() == before, (check, 'refused request changed Podman state')
    checks.append('refused without effect: ' + check)
def put(name, path, data):
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, 'f'); open(f, 'wb').write(data); out('cp', f, f'{name}:{path}')
def get(name, path):
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, 'f'); out('cp', f'{name}:{path}', f); return open(f, 'rb').read()
def new_universe():
    u = str(uuid.uuid4()); universes.append(u); return u, 'podmesh-' + u
def unmanaged(name, *labels):
    forged.append(name)
    args = [x for l in labels for x in ('--label', l)]
    out('create', '--pull=never', '--network=none', '--name', name, *args, alpine, 'true')
    return inspect(name)['Id']

alpine = next(i['Id'] for i in images() if any('alpine' in n for n in i.get('Names') or []))
baseline = state()
try:
    a, A = new_universe()
    create_a = request('create', a, image='sha256:' + alpine, command=['sh', '-c', 'cat /clone-marker'])
    ok(create_a, 'create clone source')
    blob = os.urandom(1 << 20)
    put(A, '/clone-marker', b'source-a'); put(A, '/clone-blob', blob)

    # Refusals before any effect.
    b, B = new_universe()
    refused(request('clone', b, source_uuid=b), 'same source and target UUID', 'new universe UUID')
    refused(request('clone', b, source_uuid='not-a-uuid'), 'invalid source UUID', 'Invalid source UUID')
    refused(request('clone', b, source_uuid=str(uuid.uuid4())), 'missing source', 'not found')
    x = str(uuid.uuid4())
    unmanaged('podmesh-' + x, f'io.podmesh.universe={x}', 'io.podmesh.creation-operation=' + str(uuid.uuid4()))
    refused(request('clone', b, source_uuid=x), 'labelled source unknown to the journal', 'not recorded')
    y = str(uuid.uuid4())
    unmanaged('podmesh-' + y, f'io.podmesh.universe={y}', 'io.podmesh.creation-operation=' + create_a['operation_id'])
    refused(request('clone', b, source_uuid=y), 'source label borrowing another universe operation', 'does not match')
    e, E = new_universe()
    create_e = request('create', e, image='sha256:' + alpine, command=['true'])
    ok(create_e)
    out('rm', E); forged.append(E)
    unmanaged(E, f'io.podmesh.universe={e}', 'io.podmesh.creation-operation=' + create_e['operation_id'])
    refused(request('clone', b, source_uuid=e), 'source container replaced out of band', 'does not match')
    o = str(uuid.uuid4())
    occupant = unmanaged('podmesh-' + o)
    refused(request('clone', o, source_uuid=a), 'target name held by an unmanaged container', 'not managed')
    assert inspect('podmesh-' + o)['Id'] == occupant
    r, R = new_universe()
    ok(request('create', r, image='sha256:' + alpine, command=['sleep', '120']))
    out('start', R)
    refused(request('clone', b, source_uuid=r), 'running source', 'must be stopped')
    out('pause', R)
    refused(request('clone', b, source_uuid=r), 'paused source', 'must be stopped')
    out('unpause', R); out('stop', '--time', '1', R)
    ok(request('delete', r))

    # Successful clone.
    before_clone, source_before = state(), source_view(A)
    clone_b = request('clone', b, source_uuid=a)
    references.append('localhost/podmesh-clone:' + clone_b['operation_id'])
    result = ok(clone_b, 'clone stopped source')
    assert result['snapshot_reused'] is False
    replay = ok(clone_b)
    assert replay['replayed'] and replay['original_result'] == result
    checks.append('retry replays the verified result without a new effect')
    refused(dict(clone_b, universe_uuid=str(uuid.uuid4())), 'operation ID reused for a different clone', 'different request')
    src, cl = inspect(A), inspect(B)
    assert cl['Id'] != src['Id'] and cl['Id'] == result['container_id'] and result['source_container_id'] == src['Id']
    assert cl['Config']['Labels']['io.podmesh.universe'] == b
    assert cl['Config']['Labels']['io.podmesh.creation-operation'] == clone_b['operation_id']
    assert cl['HostConfig']['NetworkMode'] == 'none' and cl['Mounts'] == [] and cl['State']['Status'] == 'created'
    assert cl['Config']['Cmd'] == src['Config']['Cmd']
    snapshot = image_named(references[0]); assert len(snapshot) == 1 and snapshot[0]['Id'] == cl['Image'] == result['snapshot_image']
    assert snapshot[0]['Labels']['io.podmesh.snapshot-for'] == b and snapshot[0]['Labels']['io.podmesh.snapshot-source-container'] == src['Id']
    after_clone = state()
    assert set(after_clone[0]) - set(before_clone[0]) == {cl['Id']} and set(before_clone[0]) <= set(after_clone[0])
    assert set(after_clone[1]) - set(before_clone[1]) == {cl['Image']} and set(before_clone[1]) <= set(after_clone[1])
    checks.append('exactly one new container and one snapshot image; new identity, labels, no network, no mounts, same command')
    assert source_view(A) == source_before
    checks.append('source identity, state, labels, command and filesystem diff unchanged')
    assert get(B, '/clone-marker') == b'source-a' and hashlib.sha256(get(B, '/clone-blob')).digest() == hashlib.sha256(blob).digest()
    checks.append('clone carries the source writable data byte for byte')
    put(B, '/clone-marker', b'clone-b'); assert get(A, '/clone-marker') == b'source-a'
    put(A, '/clone-marker', b'source-a-2'); assert get(B, '/clone-marker') == b'clone-b'
    checks.append('writes to either side do not reach the other')

    # Resume: an interrupted attempt committed the snapshot but did not create the clone.
    c, C = new_universe()
    clone_c = request('clone', c, source_uuid=a)
    ref_c = 'localhost/podmesh-clone:' + clone_c['operation_id']; references.append(ref_c)
    out('commit', '--pause=false', '--change', f'LABEL io.podmesh.snapshot-for={c}', '--change', 'LABEL io.podmesh.snapshot-operation=' + clone_c['operation_id'],
        '--change', 'LABEL io.podmesh.snapshot-source-container=' + src['Id'], A, ref_c)
    prepared, images_before = image_named(ref_c)[0]['Id'], set(state()[1])
    result_c = ok(clone_c)
    assert result_c['snapshot_reused'] is True and result_c['snapshot_image'] == prepared and set(state()[1]) == images_before
    assert get(C, '/clone-marker') == b'source-a-2'
    checks.append('retry after a simulated interruption reuses the committed snapshot without another image')
    f = str(uuid.uuid4())
    clone_f = request('clone', f, source_uuid=a)
    ref_f = 'localhost/podmesh-clone:' + clone_f['operation_id']; references.append(ref_f)
    out('commit', '--pause=false', '--change', 'LABEL io.podmesh.snapshot-for=' + str(uuid.uuid4()), A, ref_f)
    refused(clone_f, 'snapshot reference with foreign provenance', 'different provenance')
    out('image', 'rm', ref_f)

    # Clone of a clone, then deletions in dependency-hostile order.
    d, D = new_universe()
    clone_d = request('clone', d, source_uuid=b)
    references.append('localhost/podmesh-clone:' + clone_d['operation_id'])
    ok(clone_d, 'clone of a clone')
    assert get(D, '/clone-marker') == b'clone-b'
    deleted_a = ok(request('delete', a))
    assert not exists(A) and deleted_a['snapshot_images_removed'] == [] and deleted_a['snapshot_images_retained'] == []
    assert out('start', '--attach', B) == 'clone-b'
    checks.append('clone runs its inherited command on its own data after source deletion')
    deleted_b = ok(request('delete', b))
    accounted = set(deleted_b['snapshot_images_removed']) | {x['image'] for x in deleted_b['snapshot_images_retained']}
    assert accounted == {result['snapshot_image']} and not image_named(references[0]), deleted_b
    assert out('start', '--attach', D) == 'clone-b'
    checks.append('deleting a clone reports its snapshot and leaves a dependent clone runnable')
    delete_d = request('delete', d)
    deleted_d = ok(delete_d)
    assert deleted_d['snapshot_images_retained'] == [] and len(deleted_d['snapshot_images_removed']) == 1
    assert ok(delete_d)['replayed'] and ok(request('delete', d))['absent']
    deleted_c = ok(request('delete', c))
    assert deleted_c['snapshot_images_removed'] == [prepared]
    refused(request('delete', e), 'delete of a universe container replaced out of band', 'does not match')
    out('rm', E)
    ok(request('delete', e))
    checks.append('delete removes only this universe snapshots; repeated deletion is consistent')
    for name in forged: podman('rm', name, check=False)
    forged.clear()
    assert state() == baseline, 'Podman containers, images or volumes differ from the baseline'
    checks.append('all pre-existing containers, images and volumes unchanged; no leftovers')
finally:
    # Best effort removal of this run's own resources only, so a failure leaves no clutter.
    for u in universes:
        if exists('podmesh-' + u): podman('rm', '--force', 'podmesh-' + u, check=False)
    for name in forged: podman('rm', '--force', name, check=False)
    for ref in references:
        if image_named(ref): podman('image', 'rm', ref, check=False)
    for i in images():
        if (i.get('Labels') or {}).get('io.podmesh.snapshot-for') in universes: podman('image', 'rm', i['Id'], check=False)

capabilities = api({'operation': 'capabilities'})['data']
print(json.dumps({'status': 'PASS', 'version': capabilities['version'], 'podman': out('--version'), 'checks': checks,
                  'source_uuid': a, 'clone_uuid': b, 'clone_result': result}))
