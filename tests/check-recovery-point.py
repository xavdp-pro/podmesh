#!/usr/bin/env python3
"""Recovery points: a stopped universe becomes an immutable, digested, honestly-unsigned point.

Needs Podman. The decisive assertion is that the exported rootfs CONTAINS A MARKER written by
the running container: an export of a never-started container is byte-identical to its image,
so a check that only looked at digests would pass on a capture that captured nothing.
"""
import io, json, os, socket, subprocess, tarfile, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')

def api(request):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(120); s.connect(endpoint)
        s.sendall(json.dumps(request).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())

def op(operation, **extra):
    return api(dict({'operation': operation, 'operation_id': str(uuid.uuid4()),
                     'authorization_ref': 'disposable-lab'}, **extra))

def refused(answer, fragment, label):
    assert not answer['ok'], f'{label}: accepted, expected refusal — {answer}'
    assert fragment in json.dumps(answer), f'{label}: refused for another reason — {answer}'

images = json.loads(subprocess.check_output(['podman', 'images', '--format', 'json']))
image = next(i['Id'] for i in images if any('alpine' in (n or '') for n in (i.get('Names') or [])))
if not image.startswith('sha256:'):
    image = 'sha256:' + image

marker = uuid.uuid4().hex
u = str(uuid.uuid4()); name = 'podmesh-' + u
k = str(uuid.uuid4()); killed = 'podmesh-' + k
try:
    # Step 0 of the design, verbatim: the marker is a literal in the command, in both the
    # file name and the contents, and PID 1 handles its stop signal.
    created = op('create', universe_uuid=u, image=image, network_profile='isolated',
                 command=['sh', '-c', f"printf %s '{marker}' > /marker-{marker}; trap 'exit 0' TERM; sleep 600 & wait"])
    assert created['ok'], created

    # No point of a universe that has never been captured.
    assert op('recovery_point_status', universe_uuid=u)['data']['recovery_points'] == []
    refused(op('recovery_point_prepare', universe_uuid=str(uuid.uuid4())), 'No such universe', 'prepare of a nonexistent universe')

    assert op('start', universe_uuid=u, observe_seconds=1)['ok']
    refused(op('recovery_point_prepare', universe_uuid=u), 'universe is running', 'prepare of a running universe')

    stopped = op('stop', universe_uuid=u, timeout_seconds=10, on_timeout='kill')
    assert stopped['ok'] and stopped['data'].get('forced') is False, stopped

    first = op('recovery_point_prepare', universe_uuid=u)
    assert first['ok'], first
    d = first['data']
    assert d['state'] == 'prepared' and d['signed'] is False and d['generation'] == 1, d
    assert d['parent_recovery_point_uuid'] is None and d['consistency_class'] == 'quiescent', d
    assert d['rootfs_bytes'] > 0, d

    # The manifest on disk is the one that was digested, and it says what it is.
    outbox = d['outbox']
    with open(os.path.join(outbox, 'recovery-point-manifest.json'), 'rb') as f:
        raw = f.read()
    import hashlib
    assert hashlib.sha256(raw).hexdigest() == d['manifest_sha256'], 'manifest digest does not match the file'
    m = json.loads(raw)
    assert m['signed'] is False and m['signature'] is None and m['state'] == 'prepared', m
    assert m['format_version'].endswith('unsigned-unencrypted'), m['format_version']
    assert m['image_id'] == image or m['image_id'] == image.removeprefix('sha256:'), m['image_id']
    assert m['stop']['exit_code'] == 0 and m['stop']['escalated_to_kill_suspected'] is False, m['stop']
    assert m['data_lifecycle'] is None and m['data_lifecycle_declared'] is False, 'absent must be typed, not omitted'
    assert m['pieces'][0]['plaintext_sha256'] == d['rootfs_sha256'] and m['pieces'][0]['ciphertext_sha256'] is None, m['pieces']
    assert m['producer_identity'] == api({'operation': 'identity'})['data']['host_uuid'], 'producer must be this host'
    # Canonical: sorted keys, no whitespace, and re-serializing what we parsed reproduces the bytes.
    assert json.dumps(m, sort_keys=True, separators=(',', ':')).encode() == raw, 'manifest is not in canonical form'

    # THE assertion. The rootfs must carry bytes only a running container could have written.
    rootfs = os.path.join(outbox, 'rootfs.tar')
    with open(rootfs, 'rb') as f:
        assert hashlib.sha256(f.read()).hexdigest() == d['rootfs_sha256'], 'rootfs digest does not match the file'
    with tarfile.open(rootfs) as t:
        names = t.getnames()
        hit = next((n for n in names if n.strip('./') == f'marker-{marker}'), None)
        assert hit, f'the marker file is not in the export: an export equal to the image proves nothing ({len(names)} entries)'
        assert t.extractfile(hit).read() == marker.encode(), 'the marker file does not carry the marker bytes'

    # Idempotent by operation ID.
    rid = str(uuid.uuid4())
    again1 = api({'operation': 'recovery_point_prepare', 'operation_id': rid, 'universe_uuid': u, 'authorization_ref': 'disposable-lab'})
    again2 = api({'operation': 'recovery_point_prepare', 'operation_id': rid, 'universe_uuid': u, 'authorization_ref': 'disposable-lab'})
    assert again1['ok'] and again2['ok'] and again2['data']['replayed'] is True, (again1, again2)
    assert again2['data']['recovery_point_uuid'] == again1['data']['recovery_point_uuid'], 'a replay must return the same point'

    # A second point has generation 2 and names the first as its parent.
    assert again1['data']['generation'] == 2 and again1['data']['parent_recovery_point_uuid'] == d['recovery_point_uuid'], again1
    listed = op('recovery_point_status', universe_uuid=u)['data']
    assert [p['generation'] for p in listed['recovery_points']] == [1, 2], listed
    assert 'unsigned' in listed['note']

    # A stop that escalated to SIGKILL has no class, and the capture is refused rather than
    # downgraded. A bare `sleep` as PID 1 ignores SIGTERM, which is what forces the escalation.
    assert op('create', universe_uuid=k, image=image, network_profile='isolated', command=['sleep', '600'])['ok']
    assert op('start', universe_uuid=k, observe_seconds=1)['ok']
    forced = op('stop', universe_uuid=k, timeout_seconds=1, on_timeout='kill')
    assert forced['ok'] and forced['data'].get('forced') is True, f'expected the stop to escalate: {forced}'
    refused(op('recovery_point_prepare', universe_uuid=k), 'escalated to SIGKILL', 'prepare after an escalated stop')
    assert op('recovery_point_status', universe_uuid=k)['data']['recovery_points'] == [], 'a refused capture must record nothing'

    print('PASS: recovery points — marker found in the export, canonical unsigned manifest that says so, '
          'digests bound to the files, idempotent replay, generations chained, running refused, '
          'and an escalated stop refused with no weaker class.')
finally:
    for n in (name, killed):
        subprocess.run(['podman', 'rm', '-f', n], capture_output=True)
