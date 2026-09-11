#!/usr/bin/env python3
"""Delete ownership and snapshot provenance. Forged, borrowed and replaced containers must be
refused; snapshot images whose provenance does not match the journal must be retained; genuine
recorded resources are removed. Run as root on a disposable lab host. Universes are mutated only
through the API; direct Podman writes create or remove named fixtures."""
import json, os, socket, subprocess, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
checks, universes, fixtures, references = [], [], [], []

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
def image_present(image_id): return any(i['Id'] == image_id for i in images())
def image_named(ref): return [i for i in images() if ref in (i.get('Names') or [])]
def state():
    containers = {c['Id']: (tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'), c.get('ExitCode'), c.get('ImageID'))
                  for c in json.loads(out('ps', '--all', '--format', 'json'))}
    return containers, {i['Id']: tuple(sorted(i.get('Names') or [])) for i in images()}, sorted(out('volume', 'ls', '--quiet').split())
def request(op, u, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), universe_uuid=u, authorization_ref='disposable-lab-delete-ownership-test', **extra)
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
def universe(command):
    u = str(uuid.uuid4()); universes.append(u)
    r = request('create', u, image='sha256:' + alpine, command=command); ok(r)
    return u, 'podmesh-' + u, r['operation_id']
def fixture(name, *args):
    fixtures.append(name)
    out('create', '--pull=never', '--network=none', '--name', name, *args)
    return inspect(name)['Id']
def snapshot(source, *labels):
    """Commit a fixture image under the PodMesh snapshot repository with chosen labels."""
    ref = 'localhost/podmesh-clone:' + str(uuid.uuid4()); references.append(ref)
    changes = [x for l in labels for x in ('--change', f'LABEL {l}')]
    out('commit', '--pause=false', *changes, source, ref)
    return ref, image_named(ref)[0]['Id']
def retained(result): return {x['image']: x['reason'] for x in result['snapshot_images_retained']}

alpine = next(i['Id'] for i in images() if any('alpine' in n for n in i.get('Names') or []))
baseline = state()
try:
    a, A, a_op = universe(['true'])
    anonymous = request('delete', a); del anonymous['authorization_ref']
    refused(anonymous, 'delete without authorization reference', 'Missing authorization_ref')
    refused(request('delete', 'not-a-uuid'), 'delete with invalid universe UUID', 'Invalid universe UUID')

    # Containers that carry PodMesh labels without a matching journal record.
    x = str(uuid.uuid4())
    fixture('podmesh-' + x, '--label', f'io.podmesh.universe={x}', '--label', 'io.podmesh.creation-operation=' + str(uuid.uuid4()), alpine, 'true')
    refused(request('delete', x), 'labelled container whose creation operation is unknown', 'not recorded')
    y = str(uuid.uuid4())
    fixture('podmesh-' + y, '--label', f'io.podmesh.universe={y}', '--label', f'io.podmesh.creation-operation={a_op}', alpine, 'true')
    refused(request('delete', y), 'labelled container borrowing another universe creation operation', 'does not match')
    e, E, e_op = universe(['true'])
    out('rm', E)
    forged = fixture(E, '--label', f'io.podmesh.universe={e}', '--label', f'io.podmesh.creation-operation={e_op}', alpine, 'true')
    refused(request('delete', e), 'universe container replaced out of band with identical name and labels', 'does not match')
    assert inspect(E)['Id'] == forged
    o = str(uuid.uuid4())
    fixture('podmesh-' + o, alpine, 'true')
    refused(request('delete', o), 'unmanaged container under a universe name', 'not managed')

    # A genuine universe: running deletion refused, explicit stop, then deletion.
    g, G, _ = universe(['sleep', '600'])
    ok(request('start', g))
    refused(request('delete', g), 'running recorded universe', 'stop it explicitly first')
    ok(request('stop', g, timeout_seconds=1, on_timeout='kill'))
    deleted_g = ok(request('delete', g))
    assert deleted_g['absent'] and not exists(G)
    checks.append('recorded universe deleted only after an explicit API stop')

    # Snapshot provenance for present and absent targets.
    base = 'pmfixture-snapshot-base-' + str(uuid.uuid4())
    fixture(base, alpine, 'true')
    a_id = inspect(A)['Id']
    b = str(uuid.uuid4()); universes.append(b)
    clone_b = request('clone', b, source_uuid=a)
    cloned = ok(clone_b); references.append(cloned['snapshot_reference'])
    z = str(uuid.uuid4())
    _, unknown = snapshot(base, f'io.podmesh.snapshot-for={z}', 'io.podmesh.snapshot-operation=' + str(uuid.uuid4()), f'io.podmesh.universe={a}')
    _, borrowed = snapshot(base, f'io.podmesh.snapshot-for={z}', 'io.podmesh.snapshot-operation=' + clone_b['operation_id'], f'io.podmesh.universe={a}')
    _, impostor = snapshot(base, f'io.podmesh.snapshot-for={b}', 'io.podmesh.snapshot-operation=' + clone_b['operation_id'], f'io.podmesh.universe={a}',
                           f'io.podmesh.snapshot-source-container={a_id}')
    deleted_z = ok(request('delete', z))
    kept = retained(deleted_z)
    assert deleted_z['absent'] and deleted_z['snapshot_images_removed'] == [] and set(kept) == {unknown, borrowed}, deleted_z
    assert all('provenance' in reason for reason in kept.values()) and image_present(unknown) and image_present(borrowed)
    checks.append('absent-target delete retains snapshots naming an unknown operation or another universe clone operation')
    deleted_b = ok(request('delete', b))
    kept = retained(deleted_b)
    assert deleted_b['snapshot_images_removed'] == [cloned['snapshot_image']] and set(kept) == {impostor} and 'provenance' in kept[impostor], deleted_b
    assert image_present(impostor) and not image_present(cloned['snapshot_image'])
    checks.append('clone deletion removes its recorded snapshot and retains an impostor labelled with the same operation, source and container')

    # A snapshot left by a recorded failed clone attempt is removable for the absent target.
    c = str(uuid.uuid4())
    occupant = 'podmesh-' + c
    fixture(occupant, alpine, 'true')
    clone_c = request('clone', c, source_uuid=a)
    refused(clone_c, 'clone onto a name held by an unmanaged container (records a failed attempt)', 'not managed')
    podman('rm', occupant); fixtures.remove(occupant)
    ref_c = 'localhost/podmesh-clone:' + clone_c['operation_id']; references.append(ref_c)
    out('commit', '--pause=false', '--change', f'LABEL io.podmesh.snapshot-for={c}', '--change', 'LABEL io.podmesh.snapshot-operation=' + clone_c['operation_id'],
        '--change', f'LABEL io.podmesh.snapshot-source-container={a_id}', A, ref_c)
    leftover = image_named(ref_c)[0]['Id']
    deleted_c = ok(request('delete', c))
    assert deleted_c['snapshot_images_removed'] == [leftover] and deleted_c['snapshot_images_retained'] == [] and not image_present(leftover), deleted_c
    checks.append('absent-target delete removes a leftover snapshot whose failed clone attempt is recorded for that universe')

    # Cleanup: fixtures directly, universes through the API.
    for ref in references:
        if image_named(ref): podman('image', 'rm', ref)
    for name in fixtures: podman('rm', '--force', '--time', '0', name)
    fixtures.clear()
    ok(request('delete', e))
    ok(request('delete', a))
    assert state() == baseline, 'Podman containers, images or volumes differ from the baseline'
    checks.append('all pre-existing containers, images and volumes unchanged; no leftovers')
finally:
    for u in universes:
        if exists('podmesh-' + u): podman('rm', '--force', '--time', '0', 'podmesh-' + u, check=False)
    for name in fixtures:
        if exists(name): podman('rm', '--force', '--time', '0', name, check=False)
    for ref in references:
        if image_named(ref): podman('image', 'rm', ref, check=False)

print(json.dumps({'status': 'PASS', 'version': api({'operation': 'capabilities'})['data']['version'], 'checks': checks,
                  'absent_target_delete': deleted_z, 'clone_delete': deleted_b, 'failed_attempt_leftover_delete': deleted_c}))
