#!/usr/bin/env python3
"""Package lifecycle rehearsal on one disposable lab host: remove, reinstall, roll back to the
previous package from the local APT cache, then re-upgrade from the signed repository.
Identity, journal, ownership and running workloads must survive; pre-existing containers,
images and volumes must be unchanged. Run as root with the service installed at the new version."""
import hashlib, json, os, socket, sqlite3, subprocess, time, uuid

new = os.environ['PODMESH_NEW_VERSION']
old = os.environ['PODMESH_ROLLBACK_VERSION']
old_deb = os.environ['PODMESH_ROLLBACK_DEB']
old_sha256 = os.environ['PODMESH_ROLLBACK_SHA256']
endpoint = '/run/podmesh/api.sock'
TRAP = ['sh', '-c', 'echo start >> /data.log; trap "echo term >> /data.log; exit 0" TERM; while true; do sleep 1; done']
steps, checks, observed = [], [], {}

def run(*cmd, check=True):
    p = subprocess.run(cmd, capture_output=True, text=True, env=dict(os.environ, DEBIAN_FRONTEND='noninteractive'))
    steps.append({'command': ' '.join(cmd), 'exit': p.returncode, 'stdout_tail': p.stdout[-600:], 'stderr_tail': p.stderr[-600:]})
    if check and p.returncode: raise RuntimeError(steps[-1])
    return p
def api(r, timeout=400):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(timeout); s.connect(endpoint)
        s.sendall(json.dumps(r).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())
def ready():
    for _ in range(300):
        try:
            if api({'operation': 'capabilities'}, 5)['ok']: return
        except (OSError, ValueError): pass
        time.sleep(.1)
    raise RuntimeError('Service did not become ready')
def package():
    p = subprocess.run(['dpkg-query', '-W', '-f=${Version} ${db:Status-Status}', 'podmesh'], capture_output=True, text=True)
    return p.stdout.strip() if p.returncode == 0 else 'not known to dpkg'
def active(): return subprocess.run(['systemctl', 'is-active', '--quiet', 'podmesh']).returncode == 0
def journal():
    db = sqlite3.connect('file:/var/lib/podmesh/state.sqlite?mode=ro', uri=True)
    try:
        meta = dict(db.execute('SELECT key,value FROM metadata'))
        operations = dict(db.execute('SELECT status,count(*) FROM operations GROUP BY status'))
        tables = sorted(r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'"))
        return {'host_uuid': meta.get('host_uuid'), 'operations': operations, 'rows': sum(operations.values()), 'tables': tables}
    finally:
        db.close()
def podman(*args, check=True):
    p = subprocess.run(['podman', *args], capture_output=True)
    if check and p.returncode: raise RuntimeError(f'podman {args}: {p.stderr.decode()}')
    return p
def out(*args): return podman(*args).stdout.decode().strip()
def inspect(name): return json.loads(out('container', 'inspect', name))[0]
def exists(name): return podman('container', 'exists', name, check=False).returncode == 0
def state():
    containers = {c['Id']: (tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'), c.get('ExitCode'), c.get('ImageID'))
                  for c in json.loads(out('ps', '--all', '--format', 'json'))}
    images = {i['Id']: tuple(sorted(i.get('Names') or [])) for i in json.loads(out('images', '--all', '--format', 'json'))}
    return containers, images, sorted(out('volume', 'ls', '--quiet').split())
def request(op, u, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), universe_uuid=u, authorization_ref='disposable-lab-package-rehearsal', **extra)
def ok(r):
    result = api(r); assert result['ok'], (r, result); return result['data']
def still_running(name, started_at):
    c = inspect(name)
    assert c['State']['Status'] == 'running' and c['State']['StartedAt'] == started_at, (name, c['State'])

alpine = next(i['Id'] for i in json.loads(out('images', '--format', 'json')) if any('alpine' in n for n in i.get('Names') or []))
assert package() == f'{new} installed', package()
baseline = state()
before = journal()
identity = api({'operation': 'identity'})['data']['host_uuid']
assert identity == before['host_uuid']
u, v, w = str(uuid.uuid4()), str(uuid.uuid4()), str(uuid.uuid4())
U, V, W = 'podmesh-' + u, 'podmesh-' + v, 'podmesh-' + w
try:
    ok(request('create', u, image='sha256:' + alpine, command=TRAP))
    start_u = request('start', u)
    assert ok(start_u)['running'] is True
    create_v = request('create', v, image='sha256:' + alpine, command=['true'])
    ok(create_v)
    u_started = inspect(U)['State']['StartedAt']

    # 1. Remove the package.
    run('apt-get', 'remove', '-y', 'podmesh')
    observed['after_remove_package'] = package()
    assert not observed['after_remove_package'].endswith(' installed')
    assert not os.path.exists('/usr/bin/podmesh') and not os.path.exists('/usr/bin/podmeshd') and not active()
    assert not os.path.exists('/etc/systemd/system/multi-user.target.wants/podmesh.service')
    after_remove = journal()
    assert after_remove['host_uuid'] == identity and after_remove['rows'] >= before['rows'] + 3, after_remove
    still_running(U, u_started)
    checks.append('remove: binaries, service and enablement gone; identity and journal retained; running universe unaffected')

    # 2. Reinstall the same version from the signed repository.
    run('apt-get', 'install', '-y', f'podmesh={new}')
    ready()
    assert package() == f'{new} installed' and api({'operation': 'identity'})['data']['host_uuid'] == identity
    history = ok(start_u)
    assert history['historical'] and history['current']['running'] is True and history['current_matches_recorded_container'] is True, history
    assert ok(create_v)['current']['present'] is True
    still_running(U, u_started)
    checks.append('reinstall: same identity; verified operations replay as historical results with fresh observations')

    # 3. Roll back to the previous version from the local APT cache.
    with open(old_deb, 'rb') as f:
        assert hashlib.sha256(f.read()).hexdigest() == old_sha256
    run('apt-get', 'install', '-y', '--allow-downgrades', old_deb)
    ready()
    assert package() == f'{old} installed'
    assert api({'operation': 'capabilities'})['data']['version'] == old
    assert api({'operation': 'identity'})['data']['host_uuid'] == identity
    create_w = request('create', w, image='sha256:' + alpine, command=['true'])
    observed['rollback_create'] = api(create_w)
    observed['rollback_delete'] = api(request('delete', w))
    assert observed['rollback_create']['ok'] and observed['rollback_delete']['ok'] and not exists(W), observed
    observed['rollback_start'] = api(request('start', u))
    assert observed['rollback_start']['ok'] is False
    observed['rollback_replay'] = api(create_v)
    assert observed['rollback_replay']['ok'] and observed['rollback_replay']['data']['replayed']
    still_running(U, u_started)
    checks.append('rollback: previous version runs on the newer journal; its create/delete contract works; start honestly unsupported; identity kept')

    # 4. Re-upgrade from the signed repository.
    run('apt-get', 'install', '-y', f'podmesh={new}')
    ready()
    assert package() == f'{new} installed' and api({'operation': 'identity'})['data']['host_uuid'] == identity
    still_running(U, u_started)
    stopped = ok(request('stop', u, timeout_seconds=10, on_timeout='kill'))
    assert stopped['action'] == 'stopped' and stopped['forced'] is False and inspect(U)['State']['ExitCode'] == 0, stopped
    ok(request('delete', u))
    ok(request('delete', v))
    final_history = ok(start_u)
    assert final_history['historical'] and final_history['current']['present'] is False
    final = journal()
    assert final['host_uuid'] == identity and 'operation_attempts' in final['tables'] and final['rows'] > after_remove['rows']
    checks.append('re-upgrade: ownership recorded before removal and rollback authorizes stop and delete')
    assert state() == baseline, 'Podman containers, images or volumes differ from the baseline'
    assert active()
    checks.append('pre-existing containers, images and volumes unchanged; service active')
finally:
    if package() != f'{new} installed':
        run('apt-get', 'install', '-y', f'podmesh={new}', check=False)
    for name in (U, V, W):
        if exists(name): podman('rm', '--force', '--time', '0', name, check=False)

print(json.dumps({'status': 'PASS', 'new_version': new, 'rollback_version': old, 'rollback_deb_sha256': old_sha256, 'host_uuid': identity,
                  'journal_before': before, 'journal_after_remove': after_remove, 'journal_final': final,
                  'checks': checks, 'observed': observed, 'steps': steps}))
