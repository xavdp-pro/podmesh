#!/usr/bin/env python3
"""The administration app driven by a real headless browser (tests/browser-manager-admin.mjs),
against the Express origin (packaging/podmesh-manager/universe/origin, built), a stub resident
holding the deployment account with its default password and the must-change flag, and a stub
control socket. What a server-side test cannot see is
verified: the script renders, the refusal appears in place with the fields kept, the eye shows and
hides what is typed, Enter signs in, the change view is forced, and no form post happens."""
import json, os, pathlib, socket, subprocess, sys, tempfile, threading, time, hashlib

HERE = pathlib.Path(__file__).resolve().parent
ORIGIN = HERE.parent / 'packaging' / 'podmesh-manager' / 'universe' / 'origin'
work = pathlib.Path(tempfile.mkdtemp(prefix='podmesh-admin-browser-'))
PORT = 18090
SCOPE = 'm-u2/lab-a/observations'
config = {'control_socket': str(work / 'control.sock'),
          'network': {'replica_id': 'replica-one', 'manager': {'logical_manager_id': 'logical',
                      'grants': [{'scope': SCOPE, 'owner_replica_id': 'replica-one'}]}}}
assert (ORIGIN / 'dist' / 'index.html').exists(), 'build the origin first: npm run build in ' + str(ORIGIN)
(work / 'config.json').write_text(json.dumps(config))
salt = bytes.fromhex('00112233445566778899aabbccddeeff')
stored = f"scrypt.16384.8.1.{salt.hex()}.{hashlib.scrypt(b'podmesh', salt=salt, n=16384, r=8, p=1, dklen=32).hex()}"
(work / 'facts.json').write_text(json.dumps({'ordered_facts': [
    {'scope': SCOPE, 'subject': 'admin.user.admin', 'value': stored, 'subject_revision': 1},
    {'scope': SCOPE, 'subject': 'admin.flag.admin', 'value': 'must_change', 'subject_revision': 1}]}))
stub = work / 'resident'
stub.write_text('#!/bin/sh\ncat "$(dirname "$0")/facts.json"\n')
stub.chmod(0o755)
(work / 'governor.json').write_text(json.dumps({'epoch': 3, 'marked_at': int(time.time())}))

def control():
    s = socket.socket(socket.AF_UNIX); s.bind(config['control_socket']); s.listen(4)
    while True:
        c, _ = s.accept()
        while c.recv(65536):
            pass
        c.sendall(b'{"result":"observed"}'); c.close()
threading.Thread(target=control, daemon=True).start()

env = dict(os.environ, PODMESH_MANAGER_CONFIG=str(work / 'config.json'), PODMESH_MANAGER_STATE=str(work),
           PODMESH_MANAGER_BINARY=str(stub), PODMESH_GOVERNOR_MARK=str(work / 'governor.json'), PODMESH_ORIGIN_PORT=str(PORT))
server = subprocess.Popen(['node', str(ORIGIN / 'server' / 'index.mjs')], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
try:
    for _ in range(50):
        try:
            socket.create_connection(('127.0.0.1', PORT), 1).close(); break
        except OSError:
            time.sleep(0.2)
    # The session cookie is Secure: a browser keeps it only over https or on localhost.
    p = subprocess.run(['node', str(HERE / 'browser-manager-admin.mjs'), f'http://localhost:{PORT}'],
                       capture_output=True, text=True, timeout=120)
    sys.stdout.write(p.stdout)
    if p.returncode:
        sys.stderr.write(p.stderr[-2000:])
        sys.exit(1)
finally:
    server.terminate()
