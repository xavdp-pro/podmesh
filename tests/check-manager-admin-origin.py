#!/usr/bin/env python3
"""The manager origin's administration surface, every gate exercised locally.

No lab, no container: the responder is run against a stub resident (a script printing the
ordered facts) and a stub control socket that records what it is asked to append. What is
verified: every path is 503 without the governor mark, including the administration ones; with
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

def ask(method, path, body=None, cookie=None):
    c = http.client.HTTPConnection('127.0.0.1', PORT, timeout=15)
    headers = {}
    if body is not None:
        headers['Content-Type'] = 'application/x-www-form-urlencoded'
    if cookie:
        headers['Cookie'] = cookie
    c.request(method, path, body, headers)
    r = c.getresponse()
    text = r.read().decode('utf-8', 'replace')
    out = (r.status, text, r.getheader('Set-Cookie') or '', r.getheader('Location') or '')
    c.close()
    return out

for _ in range(60):
    try:
        ask('GET', '/ready'); break
    except OSError:
        time.sleep(0.2)
else:
    raise AssertionError('the origin did not come up')

try:
    # fail-closed, administration included
    for path in ('/', '/ready', '/admin'):
        status, text, _, _ = ask('GET', path)
        assert status == 503 and 'not the governor' in text, (path, status, text[:200])
    status, _, _, _ = ask('POST', '/admin/login', 'login=x&password=y')
    assert status == 503
    checks.append('without the governor mark every path answers 503, administration and its login included')

    mark.write_text(json.dumps({'resource': LOGICAL, 'epoch': 7, 'marked_at': int(time.time())}))
    status, text, _, _ = ask('GET', '/ready')
    assert status == 200 and json.loads(text)['epoch'] == 7 and json.loads(text)['replica_id'] == REPLICA, text
    checks.append('with the mark, /ready answers the contract JSON at the marked epoch')

    # no administrator: the page refuses and names the host door
    status, text, _, _ = ask('GET', '/admin')
    assert status == 200 and 'No administrator exists yet' in text and 'not created from this page' in text, text[:300]
    status, text, _, _ = ask('POST', '/admin/users', 'login=intruder&password=correcthorsebattery')
    assert status == 401 and not appended, (status, appended)
    checks.append('with no administrator on record: the page refuses and names the host door; a creation without a session appends nothing')

    # one administrator exists, written from the host door
    import hashlib, secrets as _s
    salt = bytes.fromhex('00112233445566778899aabbccddeeff')
    digest = hashlib.scrypt(b'first-admin-password', salt=salt, n=16384, r=8, p=1, dklen=32)
    stored = f'scrypt.16384.8.1.{salt.hex()}.{digest.hex()}'
    set_facts([{'scope': SCOPE, 'subject': 'admin.user.xavier', 'value': stored, 'subject_revision': 1}])
    status, text, _, _ = ask('GET', '/admin')
    assert status == 200 and 'Sign in' in text and 'xavier' not in text, text[:300]
    checks.append('with an administrator on record the page asks to sign in, and names nobody before that')

    status, text, cookie, _ = ask('POST', '/admin/login', 'login=xavier&password=wrong-password')
    assert status == 401 and not cookie, (status, cookie)
    checks.append('a wrong password is refused and opens no session')

    status, _, cookie, where = ask('POST', '/admin/login', 'login=xavier&password=first-admin-password')
    assert status == 303 and where == '/admin' and 'HttpOnly' in cookie and 'Secure' in cookie and 'SameSite=Strict' in cookie, (status, cookie)
    session = cookie.split(';')[0]
    checks.append('the right password opens a session whose cookie is HttpOnly, Secure and SameSite=Strict')

    status, text, _, _ = ask('GET', '/admin', cookie=session)
    assert status == 200 and 'Administration' in text and 'xavier' in text, text[:300]
    checks.append('the session shows the administration page and the administrators on record')

    # the rules of creation
    for body, fragment in (('login=UPPER%20CASE&password=correcthorsebattery', 'A login is 1 to 64'),
                           ('login=x&password=short', 'at least 12'),
                           ('login=repeatrepeats&password=repeatrepeats', 'not a password'),
                           ('login=xavier&password=correcthorsebattery', 'already exists')):
        status, text, _, _ = ask('POST', '/admin/users', body, cookie=session)
        assert status == 400 and fragment in text, (body, status, text[:200])
    assert not appended, appended
    checks.append('a bad login, a short password, a password equal to its login and a login already taken: each refused, nothing appended')

    status, text, _, _ = ask('POST', '/admin/users', 'login=second&password=another-good-password', cookie=session)
    assert status == 200 and 'created' in text, text[:300]
    assert len(appended) == 1, appended
    wrote = appended[0]
    assert wrote['operation'] == 'append_observation' and wrote['scope'] == SCOPE, wrote
    assert wrote['subject'] == 'admin.user.second', wrote
    assert wrote['value'].startswith('scrypt.16384.8.1.') and len(wrote['value']) <= 128, wrote['value'][:40]
    assert 'another-good-password' not in json.dumps(wrote), 'the password reached the store'
    checks.append("the created administrator is one observation in this replica's own scope, the password hashed and absent from what is written")

    # the account a deployment creates: it carries a flag, and nothing opens until it is replaced
    set_facts([{'scope': SCOPE, 'subject': 'admin.user.xavier', 'value': stored, 'subject_revision': 1},
               {'scope': SCOPE, 'subject': 'admin.flag.xavier', 'value': 'must_change', 'subject_revision': 1}])
    status, text, _, _ = ask('GET', '/admin', cookie=session)
    assert status == 200 and 'Change the password' in text and 'Create an administrator' not in text, text[:300]
    status, text, _, _ = ask('POST', '/admin/users', 'login=third&password=another-good-password', cookie=session)
    assert status == 403 and 'Replace the deployment password' in text and len(appended) == 1, (status, appended)
    checks.append('an account still carrying its deployment password opens only the change page, and may name nobody')

    for body, fragment in (('current=wrong&next=a-good-new-password&again=a-good-new-password', 'current password is wrong'),
                           ('current=first-admin-password&next=a-good-new-password&again=different-one', 'two new entries differ'),
                           ('current=first-admin-password&next=short&again=short', 'at least 12'),
                           ('current=first-admin-password&next=first-admin-password&again=first-admin-password', 'the old one')):
        status, text, _, _ = ask('POST', '/admin/password', body, cookie=session)
        assert status == 400 and fragment in text, (body, status, text[:200])
    assert len(appended) == 1, appended
    checks.append('a wrong current password, two differing entries, a short one and the old one again: each refused, nothing appended')

    status, text, _, _ = ask('POST', '/admin/password', 'current=first-admin-password&next=a-good-new-password&again=a-good-new-password', cookie=session)
    assert status == 200 and 'password was changed' in text, text[:300]
    assert len(appended) == 3, appended
    assert appended[1]['subject'] == 'admin.user.xavier' and appended[1]['value'].startswith('scrypt.'), appended[1]
    assert appended[2]['subject'] == 'admin.flag.xavier' and appended[2]['value'] == 'changed', appended[2]
    assert 'a-good-new-password' not in json.dumps(appended), 'the new password reached the store'
    checks.append('the change writes the new hash and clears the flag, and the new password is absent from what is written')
    set_facts([{'scope': SCOPE, 'subject': 'admin.user.xavier', 'value': stored, 'subject_revision': 1}])

    # what it looks like once replicated, and a login in two scopes
    set_facts([{'scope': SCOPE, 'subject': 'admin.user.xavier', 'value': stored, 'subject_revision': 1},
               {'scope': 'm-u2/lab-b/observations', 'subject': 'admin.user.xavier', 'value': stored, 'subject_revision': 1}])
    status, text, _, _ = ask('GET', '/admin', cookie=session)
    assert 'more than one scope' in text, text[:400]
    checks.append('a login written in two scopes is shown as a conflict, not silently resolved')

    status, _, cookie, _ = ask('POST', '/admin/logout', '', cookie=session)
    status, text, _, _ = ask('GET', '/admin', cookie=session)
    assert 'Sign in' in text, text[:200]
    checks.append('signing out closes the session')

    # the mark removed mid-session: everything closes again
    mark.unlink()
    status, text, _, _ = ask('GET', '/admin', cookie=session)
    assert status == 503, (status, text[:200])
    checks.append('the mark removed, the administration closes with everything else')
    print(json.dumps({'result': 'PASS', 'checks': checks}, indent=2))
finally:
    server.terminate()
    try:
        server.wait(timeout=10)
    except subprocess.TimeoutExpired:
        server.kill()
