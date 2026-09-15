#!/usr/bin/env python3
"""Crash and storage-failure safety of secrets (Codex, P1). One lab host, PODMESH_SOURCE_SSH, the
transient service variables and PODMESH_DAEMON_BINARY. Verified from Podman's store and the
daemon's status: a declaration failing after the store was written is refused and the uncommitted
secret is removed; one crashing after the store was written is finished by the restart when the
store holds the intended content (state effective); one crashing before the record became
effective likewise; a removal crashing after the store was cleared is finished by the restart;
names are immutable (another content under the same name is refused, the same content is
idempotent); status never shows content.
"""
import hashlib, json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
BINARY = os.environ['PODMESH_DAEMON_BINARY']
control = tempfile.mkdtemp(prefix='podmesh-secrash-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-secrets-crash'
checks = []

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def daemon(fault=None):
    # The stop must be complete -- the unit gone and its runtime directory removed -- before the new
    # unit of the same name starts, or the old unit's cleanup removes the new daemon's socket.
    A.ssh(f'sudo -n systemctl stop {unit} 2>/dev/null; sudo -n systemctl reset-failed {unit} 2>/dev/null; for i in $(seq 1 100); do systemctl is-active --quiet {unit} || [ -d {os.path.dirname(socket_path)} ] || break; sleep 0.1; done', check=False)
    mark = A.ssh('date +%s').stdout.decode().strip()
    env_ = f'--setenv=PODMESH_FAULT={fault} ' if fault else ''
    A.ssh(f'sudo -n systemd-run --quiet --unit={unit} --property=RuntimeDirectory={os.path.basename(os.path.dirname(socket_path))} --property=RuntimeDirectoryMode=0700 '
          f'--property=StateDirectory={os.path.basename(state_dir)} --property=StateDirectoryMode=0700 --property=UMask=0077 '
          f'--setenv=PODMESH_STATE_DIR={state_dir} --setenv=PODMESH_SOCKET={socket_path} {env_}{BINARY}')
    for _ in range(50):
        if A.ssh(f'sudo -n test -S {socket_path}', check=False).returncode == 0 and A.call('ready', seconds=20).get('ready'):
            break
        time.sleep(0.2)
    else:
        raise AssertionError('the daemon did not come up')
    line = A.ssh(f'sudo -n journalctl -u {unit} --since @{mark} --no-pager -o cat | grep "network reconciliation at startup" | tail -1', check=False).stdout.decode().strip()
    return json.loads(line.split(': ', 1)[1]) if line else {}

def api_or_dead(req):
    try:
        r = A.api(req)
    except RuntimeError:
        return None
    return None if r.get('interrupted') else r

def in_store(name):
    return A.ssh(f'sudo -n podman secret exists {name}', check=False).returncode == 0

def store_digest(name):
    return hashlib.sha256(A.ssh(f'sudo -n podman secret inspect --showsecret --format "{{{{.SecretData}}}}" {name}').stdout.decode().strip().encode()).hexdigest()

def place(name, content):
    A.ssh(f'sudo -n mkdir -p -m 0700 {state_dir}/inbox/secrets && sudo -n install -m 0600 -o root -g root /dev/stdin {state_dir}/inbox/secrets/{name}', input_bytes=content)

def row(name):
    st = A.ok(hostwide('secret_status'))
    assert 'lab-secret-content' not in json.dumps(st), 'status shows content'
    return next((s for s in st['secrets'] if s['name'] == name), None)

def journal_state(name):
    """The record's state read from the journal itself while the daemon is dead (a crashed unit
    has no socket): what the restart will find."""
    script = f"import sqlite3; c = sqlite3.connect('file:{state_dir}/state.sqlite?mode=ro', uri=True); r = c.execute('SELECT state FROM secrets WHERE name = ? AND removed_at IS NULL', ('{name}',)).fetchone(); print(r[0] if r else 'absent')"
    return A.ssh(f'sudo -n python3 -c "{script}"').stdout.decode().strip()

names = []
try:
    daemon()
    # a failure after the store was written: refused, the uncommitted secret removed
    n = f'lab-secret-{uuid.uuid4().hex[:8]}'; names.append(n); content = f'lab-secret-content-{uuid.uuid4()}'.encode()
    daemon('secret-after-store')
    place(n, content)
    r = A.api(hostwide('secret_declare', name=n, source=n))
    assert not r.get('ok') and 'simulated storage failure' in r['error'], r
    daemon()
    assert not in_store(n) and row(n) is None, 'the uncommitted secret survived'
    checks.append('a declaration failing after the store was written: refused, the store cleared, no record')

    # a crash after the store was written: the restart finishes it (the store holds the intended content)
    n = f'lab-secret-{uuid.uuid4().hex[:8]}'; names.append(n); content = f'lab-secret-content-{uuid.uuid4()}'.encode()
    daemon('secret-after-store:crash')
    place(n, content)
    assert api_or_dead(hostwide('secret_declare', name=n, source=n)) is None
    assert in_store(n) and journal_state(n) == 'declaring', 'the crash did not leave a declaring secret'
    rec = daemon()
    assert row(n)['state'] == 'effective' and store_digest(n) == hashlib.sha256(content).hexdigest() and any(s['name'] == n and s['now'] == 'effective' for s in rec['secrets']), (rec, row(n))
    checks.append('a declaration crashing after the store was written: the restart verified the store\'s content against the intent and finished it (effective)')

    # a crash before the record became effective: same
    n = f'lab-secret-{uuid.uuid4().hex[:8]}'; names.append(n); content = f'lab-secret-content-{uuid.uuid4()}'.encode()
    daemon('secret-before-effective:crash')
    place(n, content)
    assert api_or_dead(hostwide('secret_declare', name=n, source=n)) is None
    rec = daemon()
    assert row(n)['state'] == 'effective' and store_digest(n) == hashlib.sha256(content).hexdigest(), rec
    checks.append('a declaration crashing before its record became effective: finished by the restart')

    # names are immutable: another content refused, the same content idempotent
    place(n, b'other-content')
    r = A.api(hostwide('secret_declare', name=n, source=n))
    assert not r.get('ok') and 'immutable' in r['error'], r
    assert store_digest(n) == hashlib.sha256(content).hexdigest()
    A.ssh(f'sudo -n rm -f {state_dir}/inbox/secrets/{n}')
    place(n, content)
    r = A.api(hostwide('secret_declare', name=n, source=n))
    assert r.get('ok') and r['data'].get('already_declared') is True, r
    checks.append('immutable names: another content under the same name refused (the store untouched), the same content idempotent')

    # a removal crashing after the store was cleared: finished by the restart
    daemon('secret-remove-after-store:crash')
    assert api_or_dead(hostwide('secret_remove', name=n)) is None
    assert not in_store(n) and journal_state(n) == 'removing'
    rec = daemon()
    assert row(n) is None and any(s['name'] == n and s['now'] == 'gone' for s in rec['secrets']), rec
    checks.append('a removal crashing after the store was cleared: the restart finished it, the record gone')
    print(json.dumps({'result': 'PASS', 'checks': checks}, indent=2))
finally:
    daemon()
    for n in names:
        A.api(hostwide('secret_remove', name=n))
        A.ssh(f'sudo -n podman secret rm {n} 2>/dev/null; sudo -n rm -f {state_dir}/inbox/secrets/{n}', check=False)
    print(f'leftover secrets: {[n for n in names if in_store(n)]}', file=sys.stderr)
