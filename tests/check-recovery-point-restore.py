#!/usr/bin/env python3
"""Restore a prepared recovery point as a quarantined, new-identity universe, and prove it.

Needs Podman and PODMESH_STATE_DIR. The "transport" between capture and restore is a copy of
the outbox directory into the inbox: that is the external transport controller's job in the
design and it is deliberately not PodMesh's, so the check does it by hand.
"""
import hashlib, json, os, shutil, socket, subprocess, tarfile, uuid

endpoint = os.environ['PODMESH_SOCKET']
state = os.environ['PODMESH_STATE_DIR']

def api(request):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(180); s.connect(endpoint)
        s.sendall(json.dumps(request).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())

def op(operation, **extra):
    return api(dict({'operation': operation, 'operation_id': str(uuid.uuid4()),
                     'authorization_ref': 'disposable-lab'}, **extra))

def refused(answer, fragment, label):
    assert not answer['ok'], f'{label}: accepted, expected refusal — {answer}'
    assert fragment in json.dumps(answer), f'{label}: refused for another reason — {answer}'

def marker_in(name, marker):
    tar = subprocess.run(['podman', 'export', name], capture_output=True, check=True).stdout
    with tarfile.open(fileobj=__import__('io').BytesIO(tar)) as t:
        hit = next((n for n in t.getnames() if n.strip('./') == f'marker-{marker}'), None)
        return hit and t.extractfile(hit).read() == marker.encode()

images = json.loads(subprocess.check_output(['podman', 'images', '--format', 'json']))
image = next(i['Id'] for i in images if any('alpine' in (n or '') for n in (i.get('Names') or [])))
if not image.startswith('sha256:'):
    image = 'sha256:' + image

marker = uuid.uuid4().hex
src = str(uuid.uuid4()); new = str(uuid.uuid4())
cleanup = ['podmesh-' + src, 'podmesh-' + new]
try:
    # Capture, exactly as the capture check does it.
    assert op('create', universe_uuid=src, image=image,
              command=['sh', '-c', f"printf %s '{marker}' > /marker-{marker}; trap 'exit 0' TERM; sleep 600 & wait"])['ok']
    assert op('start', universe_uuid=src, observe_seconds=1)['ok']
    assert op('stop', universe_uuid=src, timeout_seconds=10, on_timeout='kill')['data']['forced'] is False
    prepared = op('recovery_point_prepare', universe_uuid=src)
    assert prepared['ok'], prepared
    point = prepared['data']['recovery_point_uuid']

    # Transport: the controller's job, done by hand.
    shutil.copytree(os.path.join(state, 'outbox', point), os.path.join(state, 'inbox', point))
    inbox = os.path.join(state, 'inbox', point)

    # Refusals first, on copies, so the honest inbox stays honest.
    refused(op('recovery_point_restore', universe_uuid=new, recovery_point_uuid=str(uuid.uuid4())),
            'No recovery point with this identifier', 'restore from an absent inbox')
    # As on a second host: the manifest names a universe this host has never heard of, so the
    # local "already exists" rule cannot mask the new-identity rule.
    foreign = str(uuid.uuid4())
    elsewhere = os.path.join(state, 'inbox', 'elsewhere-' + point); shutil.copytree(inbox, elsewhere)
    m = json.load(open(os.path.join(elsewhere, 'recovery-point-manifest.json'))); m['universe_uuid'] = foreign
    with open(os.path.join(elsewhere, 'recovery-point-manifest.json'), 'w') as f:
        f.write(json.dumps(m, sort_keys=True, separators=(',', ':')))
    refused(op('recovery_point_restore', universe_uuid=foreign, recovery_point_uuid='elsewhere-' + point),
            'must not reuse the source universe', 'restore into a source identity unknown here')
    refused(op('recovery_point_restore', universe_uuid=src, recovery_point_uuid=point),
            'must not reuse the source universe', 'restore into the source identity')

    # The tamper lands INSIDE the marker file's bytes, not in a tar header: Podman imports such
    # an archive without complaint, so only the digest binding stands between it and a universe.
    tampered = os.path.join(state, 'inbox', 'tampered-' + point); shutil.copytree(inbox, tampered)
    with tarfile.open(os.path.join(inbox, 'rootfs.tar')) as t:
        member = next(m for m in t.getmembers() if m.name.strip('./') == f'marker-{marker}')
        at = member.offset_data
    with open(os.path.join(tampered, 'rootfs.tar'), 'r+b') as f:
        f.seek(at); b = f.read(1); f.seek(at); f.write(bytes([b[0] ^ 0x01]))
    refused(op('recovery_point_restore', universe_uuid=new, recovery_point_uuid='tampered-' + point),
            'does not hash to the digest the manifest binds', 'restore of a tampered archive')

    claims = os.path.join(state, 'inbox', 'claims-' + point); shutil.copytree(inbox, claims)
    m = json.load(open(os.path.join(claims, 'recovery-point-manifest.json')))
    m['signed'] = True
    with open(os.path.join(claims, 'recovery-point-manifest.json'), 'w') as f:
        f.write(json.dumps(m, sort_keys=True, separators=(',', ':')))
    refused(op('recovery_point_restore', universe_uuid=new, recovery_point_uuid='claims-' + point),
            'signature this build cannot verify', 'restore of a manifest claiming a signature')

    loose = os.path.join(state, 'inbox', 'loose-' + point); shutil.copytree(inbox, loose)
    with open(os.path.join(loose, 'recovery-point-manifest.json'), 'w') as f:
        json.dump(json.load(open(os.path.join(inbox, 'recovery-point-manifest.json'))), f, indent=2)
    refused(op('recovery_point_restore', universe_uuid=new, recovery_point_uuid='loose-' + point),
            'not in canonical form', 'restore of a non-canonical manifest')

    # The honest restore.
    rid = str(uuid.uuid4())
    restored = api({'operation': 'recovery_point_restore', 'operation_id': rid, 'universe_uuid': new,
                    'authorization_ref': 'disposable-lab', 'recovery_point_uuid': point})
    assert restored['ok'], restored
    d = restored['data']
    assert d['restored_universe_uuid'] == new and d['source_universe_uuid'] == src, d
    assert d['quarantined'] is True and d['started'] is False and d['manifest_signed'] is False, d
    assert d['rootfs_sha256'] == prepared['data']['rootfs_sha256'], d

    # Quarantined: created, not running, and no network.
    insp = json.loads(subprocess.check_output(['podman', 'inspect', 'podmesh-' + new]))[0]
    assert insp['State']['Running'] is False and insp['State']['Status'] == 'created', insp['State']
    assert insp['HostConfig']['NetworkMode'] == 'none', insp['HostConfig']['NetworkMode']
    assert insp['Config']['Labels'].get('io.podmesh.universe') == new, 'the restored container must carry the NEW identity'

    # THE assertion: the restored filesystem carries the marker the source wrote while running.
    assert marker_in('podmesh-' + new, marker), 'the restored universe does not carry the marker: the round trip moved nothing'

    # Owned like any created universe: start and stop go through with no special case.
    assert op('start', universe_uuid=new, observe_seconds=1)['ok'], 'the restored universe is not owned by this host'
    assert op('stop', universe_uuid=new, timeout_seconds=10, on_timeout='kill')['ok']

    # Idempotent by operation ID, and the same universe comes back.
    again = api({'operation': 'recovery_point_restore', 'operation_id': rid, 'universe_uuid': new,
                 'authorization_ref': 'disposable-lab', 'recovery_point_uuid': point})
    assert again['ok'] and again['data']['replayed'] is True and again['data']['container_id'] == d['container_id'], again

    # A crash after the derived create but before the restore record: the journal still holds
    # the verified create, and a replay must reuse the image it names rather than import again.
    import sqlite3
    j = sqlite3.connect(os.path.join(state, 'state.sqlite'))
    assert j.execute('DELETE FROM recovery_point_restores WHERE operation_id=?', (rid,)).rowcount == 1
    # A crash before the journal's own update leaves the operation pending, which is what a retry re-evaluates.
    assert j.execute("UPDATE operations SET status='pending', result=NULL WHERE id=?", (rid,)).rowcount == 1
    j.commit(); j.close()
    before = subprocess.check_output(['podman', 'images', '-q']).split()
    resumed = api({'operation': 'recovery_point_restore', 'operation_id': rid, 'universe_uuid': new,
                   'authorization_ref': 'disposable-lab', 'recovery_point_uuid': point})
    assert resumed['ok'] and resumed['data']['replayed'] is False, resumed
    assert resumed['data']['container_id'] == d['container_id'] and resumed['data']['imported_image_id'] == d['imported_image_id'], resumed
    assert subprocess.check_output(['podman', 'images', '-q']).split() == before, 'the resumed restore imported a second image'

    print('PASS: recovery point restore — new identity, quarantined with no network, marker carried across, '
          'owned by the ordinary create path, tampered archive refused, claimed signature refused, '
          'non-canonical manifest refused, source identity refused here and as on a second host, absent inbox refused, '
          'replay idempotent, and a crash before the record resumes without a second import.')
finally:
    for n in cleanup:
        subprocess.run(['podman', 'rm', '-f', n], capture_output=True)
    # Every tag derived from this point, including any a weakened daemon imported for a copy
    # it should have refused.
    tags = subprocess.run(['podman', 'images', '--format', '{{.Repository}}:{{.Tag}}'], capture_output=True, text=True).stdout.split()
    for tag in tags:
        if tag.startswith('localhost/podmesh-restore:') and 'point' in dir() and tag.endswith(point):
            subprocess.run(['podman', 'rmi', '-f', tag], capture_output=True)
