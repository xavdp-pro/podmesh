#!/usr/bin/env python3
"""Kill the PodMesh service while a clone commit runs, then retry the same operation.
Run as root on a disposable lab host. PODMESH_UNIT must restart on failure."""
import hashlib, json, os, socket, subprocess, tempfile, threading, time, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
size = int(os.environ.get('PODMESH_BLOB_MIB', '384'))

def api(r, timeout=400):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(timeout); s.connect(endpoint)
        s.sendall(json.dumps(r).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())
def out(*args):
    return subprocess.run(['podman', *args], check=True, capture_output=True).stdout.decode().strip()
def exists(name): return subprocess.run(['podman', 'container', 'exists', name]).returncode == 0
def images(): return json.loads(out('images', '--all', '--format', 'json'))
def named(ref): return [i for i in images() if ref in (i.get('Names') or [])]
def state():
    containers = {c['Id']: (tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'))
                  for c in json.loads(out('ps', '--all', '--format', 'json'))}
    return containers, {i['Id'] for i in images()}, sorted(out('volume', 'ls', '--quiet').split())
scratch = os.path.join(os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh'), 'podman-tmp')
def temporary():
    """Podman temporary leftovers: default location and the PodMesh scratch directory."""
    default = sorted(n for n in os.listdir('/var/tmp') if n.startswith(('buildah', 'container_images_storage')))
    size = sum(os.path.getsize(os.path.join(r, f)) for r, _, fs in os.walk(scratch) for f in fs)
    return {'var_tmp_podman_entries': default, 'scratch_bytes': size}
def layers():
    with open('/var/lib/containers/storage/overlay-layers/layers.json') as f: return len(json.load(f))
def commits(ref):
    found = []
    for pid in filter(str.isdigit, os.listdir('/proc')):
        try: argv = open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')
        except OSError: continue
        if b'commit' in argv and ref.encode() in argv: found.append(int(pid))
    return found
def request(op, u, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), universe_uuid=u, authorization_ref='disposable-lab-clone-interrupt-test', **extra)
def ready():
    for _ in range(300):
        try:
            if api({'operation': 'capabilities'}, 5)['ok']: return
        except OSError: pass
        time.sleep(.1)
    raise RuntimeError('Service did not return after the kill')

alpine = next(i['Id'] for i in images() if any('alpine' in n for n in i.get('Names') or []))
baseline, layers_before, temporary_before = state(), layers(), temporary()
a, b = str(uuid.uuid4()), str(uuid.uuid4())
A, B = 'podmesh-' + a, 'podmesh-' + b
clone = request('clone', b, source_uuid=a)
ref = 'localhost/podmesh-clone:' + clone['operation_id']
try:
    assert api(request('create', a, image='sha256:' + alpine, network_profile='isolated', command=['sh', '-c', 'cat /clone-marker']))['ok']
    digest = hashlib.sha256()
    with tempfile.TemporaryDirectory(dir='/var/tmp') as d:
        blob = os.path.join(d, 'blob')
        with open(blob, 'wb') as f:
            for _ in range(size):
                chunk = os.urandom(1 << 20); digest.update(chunk); f.write(chunk)
        marker = os.path.join(d, 'marker'); open(marker, 'w').write(a)
        out('cp', blob, A + ':/clone-blob'); out('cp', marker, A + ':/clone-marker')
    first = []
    def send():
        try: first.append(api(clone))
        except Exception as e: first.append({'interrupted': type(e).__name__})
    t = threading.Thread(target=send); t.start()
    deadline = time.time() + 60
    while not commits(ref):
        assert time.time() < deadline and t.is_alive(), ('commit not observed', first)
        time.sleep(.02)
    time.sleep(float(os.environ.get('PODMESH_KILL_DELAY', '0.5')))
    at_kill = {'commit_running': bool(commits(ref)), 'snapshot_tagged': bool(named(ref)), 'target_exists': exists(B), 'temporary': temporary()}
    subprocess.run(['systemctl', 'kill', '--signal=SIGKILL', unit], check=True)
    t.join(30)
    ready()
    time.sleep(1)
    after_kill = {'first_response': first[0], 'commit_running': bool(commits(ref)), 'snapshot_tagged': bool(named(ref)), 'target_exists': exists(B)}
    retry = api(clone); assert retry['ok'], retry
    result = retry['data']; assert 'replayed' not in result, result
    assert [c for c in json.loads(out('ps', '--all', '--format', 'json')) if B in (c.get('Names') or [])].__len__() == 1
    snapshot = named(ref); assert len(snapshot) == 1 and snapshot[0]['Id'] == result['snapshot_image']
    assert out('container', 'inspect', '--format', '{{.Image}}', B) == result['snapshot_image']
    with tempfile.TemporaryDirectory(dir='/var/tmp') as d:
        copied = os.path.join(d, 'blob'); out('cp', B + ':/clone-blob', copied)
        h = hashlib.sha256()
        with open(copied, 'rb') as f:
            for chunk in iter(lambda: f.read(1 << 20), b''): h.update(chunk)
        assert h.digest() == digest.digest(), 'clone data differs from source'
    replay = api(clone); assert replay['ok'] and replay['data']['replayed']
    deleted = api(request('delete', b)); assert deleted['ok'] and deleted['data']['snapshot_images_removed'] == [result['snapshot_image']], deleted
    assert api(request('delete', a))['ok']
    assert state() == baseline, 'Podman containers, images or volumes differ from the baseline'
    temporary_after = temporary()
    assert temporary_after == {'var_tmp_podman_entries': temporary_before['var_tmp_podman_entries'], 'scratch_bytes': 0}, temporary_after
finally:
    for name in (B, A):
        if exists(name): subprocess.run(['podman', 'rm', '--force', name], capture_output=True)
    if named(ref): subprocess.run(['podman', 'image', 'rm', ref], capture_output=True)

print(json.dumps({'status': 'PASS', 'version': api({'operation': 'capabilities'})['data']['version'], 'unit': unit, 'blob_mib': size,
                  'at_kill': at_kill, 'after_kill': after_kill,
                  'retry_path': 'reused committed snapshot' if result['snapshot_reused'] else 'committed again',
                  'storage_layers_before': layers_before, 'storage_layers_after': layers(), 'temporary_before': temporary_before, 'temporary_after': temporary_after,
                  'checks': ['service killed during clone commit', 'service restarted', 'same operation retried', 'one clone container', 'one snapshot image',
                             'clone data hash matches', 'verified retry replays', 'cleanup restores containers, images and volumes',
                             'no Podman temporary leftovers in /var/tmp or the PodMesh scratch directory'],
                  'source_uuid': a, 'clone_uuid': b}))
