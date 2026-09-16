#!/usr/bin/env python3
"""The manager origin's administration surface, every gate exercised locally.

No lab, no container: the responder is run against a stub resident (a script printing the
ordered facts) and a stub control socket that records what it is asked to append. What is
verified: every path is 503 without the governor mark, including the administration ones; no
native form post anywhere (the operator's rule of 2026-09-16) -- the page carries no form, its
policy forbids submitting one, and the server refuses a urlencoded body; with
no administrator on record the page refuses and names the host door instead; a login is refused
without a session; creation is refused without a session; a wrong password is refused and a
right one opens a session; a created administrator is written as an observation in this
replica's own scope, with the password absent from what is written; the login rules and the
password floor are enforced; a login already taken is refused; the session cookie is HttpOnly,
Secure and SameSite=Strict.
"""
import http.client, json, os, pathlib, socket, subprocess, sys, tempfile, threading, time

HERE = pathlib.Path(__file__).resolve().parent
ORIGIN = HERE.parent / 'packaging' / 'podmesh-manager' / 'universe' / 'origin.py'
work = pathlib.Path(tempfile.mkdtemp(prefix='podmesh-admin-origin-'))
checks = []
REPLICA, LOGICAL = 'replica-one', 'logical-manager'
SCOPE = 'm-u2/lab-a/observations'
PORT = 18089
appended = []
facts = []

config = {'control_socket': str(work / 'control.sock'),
          'network': {'replica_id': REPLICA, 'database_path': str(work / 'store.sqlite'),
                      'manager': {'logical_manager_id': LOGICAL,
                                  'grants': [{'scope': SCOPE, 'owner_replica_id': REPLICA},
                                             {'scope': 'm-u2/lab-b/observations', 'owner_replica_id': 'replica-two'}]}}}
(work / 'config.json').write_text(json.dumps(config))
(work / 'facts.json').write_text(json.dumps(facts))
stub = work / 'resident-stub'
stub.write_text('#!/bin/sh\ncat "$(dirname "$0")/facts.json" | sed "s/^/{\\"ordered_facts\\": /;s/$/}/"\n')
stub.chmod(0o755)

def set_facts(rows):
    (work / 'facts.json').write_text(json.dumps(rows))

def control_server():
    s = socket.socket(socket.AF_UNIX)
    s.bind(config['control_socket'])
    s.listen(8)
    while True:
        conn, _ = s.accept()
        data = b''
        while True:
            chunk = conn.recv(65536)
            if not chunk:
                break
            data += chunk
        try:
            appended.append(json.loads(data))
        except ValueError:
            pass
        conn.sendall(b'{"result":"observed"}')
        conn.close()

threading.Thread(target=control_server, daemon=True).start()
mark = work / 'governor.json'
env = dict(os.environ, PODMESH_MANAGER_CONFIG=str(work / 'config.json'), PODMESH_MANAGER_STATE=str(work),
           PODMESH_MANAGER_BINARY=str(stub), PODMESH_GOVERNOR_MARK=str(mark), PODMESH_ORIGIN_PORT=str(PORT))
server = subprocess.Popen([sys.executable, '-B', str(ORIGIN)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

def ask(method, path, body=None, cookie=None, kind='application/json'):
    c = http.client.HTTPConnection('127.0.0.1', PORT, timeout=15)
    headers = {}
    data = None
    if body is not None:
        headers['Content-Type'] = kind
        data = json.dumps(body) if kind == 'application/json' and not isinstance(body, str) else body
    if cookie:
        headers['Cookie'] = cookie
    c.request(method, path, data, headers)
    r = c.getresponse()
    text = r.read().decode('utf-8', 'replace')
    out = (r.status, text, r.getheader('Set-Cookie') or '', r.getheader('Content-Security-Policy') or '')
    c.close()
    return out


def js(text):
    try:
        return json.loads(text)
    except ValueError:
        return {}


for _ in range(60):
    try:
        ask('GET', '/ready'); break
    except OSError:
        time.sleep(0.2)
else:
    raise AssertionError('the origin did not come up')

try:
    # fail-closed, administration and its API included
    for path in ('/', '/ready', '/admin', '/admin/api/state'):
        status, text, _, _ = ask('GET', path)
        assert status == 503 and 'not the governor' in text, (path, status, text[:200])
    status, _, _, _ = ask('POST', '/admin/api/login', {'login': 'x', 'password': 'y'})
    assert status == 503
    checks.append('without the governor mark every path answers 503, the administration API and its sign-in included')

    mark.write_text(json.dumps({'resource': LOGICAL, 'epoch': 7, 'marked_at': int(time.time())}))
    status, text, _, _ = ask('GET', '/ready')
    assert status == 200 and js(text)['epoch'] == 7 and js(text)['replica_id'] == REPLICA, text
    checks.append('with the mark, /ready answers the contract JSON at the marked epoch')

    # the operator's rule: no form posts, enforced by the server and by the browser's policy
    status, text, _, csp = ask('GET', '/admin')
    import re
    assert status == 200 and not re.search(r'method\s*=\s*["\']?post', text, re.I) and not re.search(r'<form[^>]*\saction\s*=', text, re.I), text[:300]
    assert "form-action 'none'" in csp and "script-src 'nonce-" in csp, csp
    assert 'Show the password' in text and 'preventDefault' in text, 'the eye or the interception is missing'
    assert '<noscript>' in text and 'could not run' in text, 'no words for a page whose script does not run'
    checks.append("the page can post no form -- no method, no action, submissions intercepted, form-action 'none' -- carries the eye, and says so when its script cannot run")
    status, text, _, _ = ask('POST', '/admin/api/login', 'login=admin&password=podmesh', kind='application/x-www-form-urlencoded')
    assert status == 415, (status, text[:200])
    status, text, _, _ = ask('POST', '/admin/login', 'login=admin&password=podmesh', kind='application/x-www-form-urlencoded')
    assert status == 405, (status, text[:200])
    checks.append('a native form post is refused: 415 on the API for a urlencoded body, 405 on any path outside the API')

    # no administrator: the page refuses and names the host door
    status, text, _, _ = ask('GET', '/admin/api/state')
    assert status == 200 and js(text) == {'view': 'none'}, text
    status, text, _, _ = ask('POST', '/admin/api/users', {'login': 'intruder', 'password': 'correcthorsebattery'})
    assert status == 401 and not appended, (status, appended)
    checks.append('with no administrator on record: the state says so and names nobody; a creation without a session appends nothing')

    # one administrator exists, written from the host door with the default password
    import hashlib
    salt = bytes.fromhex('00112233445566778899aabbccddeeff')
    digest = hashlib.scrypt(b'podmesh', salt=salt, n=16384, r=8, p=1, dklen=32)
    stored = f'scrypt.16384.8.1.{salt.hex()}.{digest.hex()}'
    set_facts([{'scope': SCOPE, 'subject': 'admin.user.admin', 'value': stored, 'subject_revision': 1}])
    status, text, _, _ = ask('GET', '/admin/api/state')
    assert js(text) == {'view': 'login'}, text
    checks.append('with an administrator on record the state asks to sign in, and names nobody before that')

    status, text, cookie, _ = ask('POST', '/admin/api/login', {'login': 'admin', 'password': 'wrong-password'})
    assert status == 401 and not cookie and js(text)['error'] == 'Wrong administrator or password.', (status, cookie, text)
    checks.append('a wrong password is refused with its reason in JSON, and opens no session')

    status, text, cookie, _ = ask('POST', '/admin/api/login', {'login': ' Admin ', 'password': 'podmesh'})
    assert status == 200 and js(text)['ok'] and 'HttpOnly' in cookie and 'Secure' in cookie and 'SameSite=Strict' in cookie, (status, cookie)
    session = cookie.split(';')[0]
    checks.append('the right password opens a session (the login trimmed and lower-cased), the cookie HttpOnly, Secure and SameSite=Strict')

    status, text, _, _ = ask('GET', '/admin/api/state', cookie=session)
    assert js(text)['view'] == 'admin' and js(text)['administrators'][0]['login'] == 'admin', text
    checks.append('the session shows the administration view and the administrators on record')

    for body, fragment in (({'login': 'UPPER CASE', 'password': 'correcthorsebattery'}, 'A login is 1 to 64'),
                           ({'login': 'x', 'password': 'short'}, 'at least 12'),
                           ({'login': 'repeatrepeats', 'password': 'repeatrepeats'}, 'not a password'),
                           ({'login': 'admin', 'password': 'correcthorsebattery'}, 'already exists')):
        status, text, _, _ = ask('POST', '/admin/api/users', body, cookie=session)
        assert status == 400 and fragment in js(text).get('error', ''), (body, status, text[:200])
    assert not appended, appended
    checks.append('a bad login, a short password, a password equal to its login and a login already taken: each refused in JSON, nothing appended')

    status, text, _, _ = ask('POST', '/admin/api/users', {'login': 'second', 'password': 'another-good-password'}, cookie=session)
    assert status == 200 and js(text)['ok'], text[:300]
    assert len(appended) == 1, appended
    wrote = appended[0]
    assert wrote['operation'] == 'append_observation' and wrote['scope'] == SCOPE and wrote['subject'] == 'admin.user.second', wrote
    assert wrote['value'].startswith('scrypt.16384.8.1.') and len(wrote['value']) <= 128, wrote['value'][:40]
    assert 'another-good-password' not in json.dumps(wrote), 'the password reached the store'
    checks.append("the created administrator is one observation in this replica's own scope, the password hashed and absent from what is written")

    # the deployment account: nothing opens until its default password is replaced
    set_facts([{'scope': SCOPE, 'subject': 'admin.user.admin', 'value': stored, 'subject_revision': 1},
               {'scope': SCOPE, 'subject': 'admin.flag.admin', 'value': 'must_change', 'subject_revision': 1}])
    status, text, _, _ = ask('GET', '/admin/api/state', cookie=session)
    assert js(text) == {'view': 'change', 'login': 'admin'}, text
    status, text, _, _ = ask('POST', '/admin/api/users', {'login': 'third', 'password': 'another-good-password'}, cookie=session)
    assert status == 403 and 'deployment password' in js(text)['error'] and len(appended) == 1, (status, appended)
    checks.append('an account still carrying its deployment password sees only the change view, and may name nobody')

    for body, fragment in (({'current': 'wrong', 'next': 'a-good-new-password', 'again': 'a-good-new-password'}, 'current password is wrong'),
                           ({'current': 'podmesh', 'next': 'a-good-new-password', 'again': 'different-one'}, 'two new entries differ'),
                           ({'current': 'podmesh', 'next': 'short', 'again': 'short'}, 'at least 12'),
                           ({'current': 'podmesh', 'next': 'podmesh', 'again': 'podmesh'}, 'at least 12')):
        status, text, _, _ = ask('POST', '/admin/api/password', body, cookie=session)
        assert status == 400 and fragment in js(text).get('error', ''), (body, status, text[:200])
    assert len(appended) == 1, appended
    checks.append('a wrong current password, two differing entries and a short one: each refused in JSON, nothing appended')

    status, text, _, _ = ask('POST', '/admin/api/password', {'current': 'podmesh', 'next': 'a-good-new-password', 'again': 'a-good-new-password'}, cookie=session)
    assert status == 200 and js(text)['ok'], text[:300]
    assert len(appended) == 3 and appended[1]['subject'] == 'admin.user.admin' and appended[2] ['subject'] == 'admin.flag.admin' and appended[2]['value'] == 'changed', appended
    assert 'a-good-new-password' not in json.dumps(appended), 'the new password reached the store'
    checks.append('the change writes the new hash and clears the flag, and the new password is absent from what is written')

    set_facts([{'scope': SCOPE, 'subject': 'admin.user.admin', 'value': stored, 'subject_revision': 1},
               {'scope': 'm-u2/lab-b/observations', 'subject': 'admin.user.admin', 'value': stored, 'subject_revision': 1}])
    status, text, _, _ = ask('GET', '/admin/api/state', cookie=session)
    assert js(text)['administrators'][0]['conflict'] is True, text
    checks.append('a login written in two scopes is shown as a conflict, not silently resolved')

    ask('POST', '/admin/api/logout', {}, cookie=session)
    status, text, _, _ = ask('GET', '/admin/api/state', cookie=session)
    assert js(text) == {'view': 'login'}, text
    checks.append('signing out closes the session')

    mark.unlink()
    status, text, _, _ = ask('GET', '/admin/api/state', cookie=session)
    assert status == 503, (status, text[:200])
    checks.append('the mark removed, the administration closes with everything else')
    print(json.dumps({'result': 'PASS', 'checks': checks}, indent=2))
finally:
    server.terminate()
    try:
        server.wait(timeout=10)
    except subprocess.TimeoutExpired:
        server.kill()
