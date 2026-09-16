#!/usr/bin/env python3
"""The manager's administrators, through the root door -- which is the only way the first one
exists.

The manager universe's `/admin` page lets an administrator already on record name the next one.
It refuses to make the first: a page reachable from the Internet that can mint authority is a
front door to power, and what has power has no front door. The first administrator is written
here, from the host that carries a replica, as root, through PodMesh's typed control door
(`manager_observe`), which relays one observation to the resident inside the universe.

An administrator is a replicated fact: scope = the replica's own granted scope, subject
`admin.user.<login>`, value `scrypt.<n>.<r>.<p>.<salt hex>.<hash hex>`. The password is read
from a prompt or standard input, hashed here, and never written to the command line, the
journal, the store or a peer.

    tools/manager-admin.py bootstrap --host lab@… --universe <replica universe uuid>
    tools/manager-admin.py create --host lab@… --universe <replica universe uuid> --login xavier
    tools/manager-admin.py list   --host lab@… --universe <replica universe uuid>
    tools/manager-admin.py revoke --host lab@… --universe <replica universe uuid> --login someone

`bootstrap` is what a deployment runs: a manager that is up has an administrator from the first
minute, and nobody has to remember to make one. It is NOT a default account -- a default password
is a published password. It draws a fresh random one, writes it nowhere but the store's hash,
prints it once on this root-only channel, and marks the account `must_change`, so the page it
opens is the one that replaces that password and nothing else works until it is. Running it again
on a manager that already has an administrator does nothing.

Environment: the usual laboratory variables (PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT),
and PODMESH_REPLICA_SET when `--scope` is not given, to find the replica's scope.
"""
import argparse, getpass, hashlib, json, os, secrets, sys, tempfile, time, uuid

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '..', 'tests'))
from podmesh_two_hosts import Host  # noqa: E402

SUBJECT_PREFIX = 'admin.user.'
FLAG_PREFIX = 'admin.flag.'
DEPLOY_LOGIN = 'admin'
LOGIN_ALPHABET = 'abcdefghijklmnopqrstuvwxyz0123456789-_.'
MIN_PASSWORD = 12
SCRYPT_N, SCRYPT_R, SCRYPT_P = 16384, 8, 1


def hash_password(password):
    salt = secrets.token_bytes(16)
    digest = hashlib.scrypt(password.encode(), salt=salt, n=SCRYPT_N, r=SCRYPT_R, p=SCRYPT_P, dklen=32)
    return f'scrypt.{SCRYPT_N}.{SCRYPT_R}.{SCRYPT_P}.{salt.hex()}.{digest.hex()}'


def read_password(login):
    """Never from the command line: a password in argv is readable by every process on the host."""
    if not sys.stdin.isatty():
        password = sys.stdin.readline().rstrip('\n')
    else:
        password = getpass.getpass(f'password for {login}: ')
        again = getpass.getpass('again: ')
        if password != again:
            raise SystemExit('the two entries differ')
    if len(password) < MIN_PASSWORD:
        raise SystemExit(f'a password is at least {MIN_PASSWORD} characters')
    if password.lower() == login:
        raise SystemExit('a password that is the login is not a password')
    return password


def host_of(args):
    control = tempfile.mkdtemp(prefix='podmesh-manager-admin-')
    return Host('host', args.host, control,
                os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock'),
                os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh'),
                os.environ.get('PODMESH_UNIT', 'podmesh.service'))


def scope_of(args, h):
    if args.scope:
        return args.scope
    path = os.environ.get('PODMESH_REPLICA_SET')
    if not path:
        raise SystemExit('name --scope, or set PODMESH_REPLICA_SET so the replica\'s scope can be read')
    replica_set = json.load(open(path))
    alias = args.alias or os.environ.get('PODMESH_HOST_ALIAS')
    if not alias or alias not in (replica_set.get('scopes') or {}):
        raise SystemExit('name --alias (or PODMESH_HOST_ALIAS) matching a scope of the replica set')
    return replica_set['scopes'][alias]


def observe(h, args, scope, subject, value):
    """One observation through PodMesh's control door.

    The resident answers `append_observation_uncertain` when it cannot say whether the fact
    landed -- a replica restarted under the request, for instance. Uncertain is not failed: the
    store is read back, and the attempt is repeated only while the subject is genuinely absent,
    so an administrator is never written twice and never silently missing."""
    last = ''
    for attempt in range(5):
        answer = h.api({'operation': 'manager_observe', 'operation_id': str(uuid.uuid4()),
                        'universe_uuid': args.universe, 'authorization_ref': args.reference,
                        'scope': scope, 'subject': subject, 'value': value})
        if answer.get('ok'):
            return answer['data']
        last = answer.get('error') or ''
        if 'uncertain' not in last and 'busy' not in last:
            raise SystemExit(f'the control door refused: {last}')
        time.sleep(2)
        try:
            if subject[len(SUBJECT_PREFIX):] in administrators(h, args) and subject.startswith(SUBJECT_PREFIX):
                return {'note': f'observed after an uncertain answer on attempt {attempt + 1}'}
        except SystemExit:
            pass
    raise SystemExit(f'the control door stayed uncertain: {last}')


def administrators(h, args):
    """Read through the universe's own door: the status carries no facts, so the store is read
    with the resident's read-only inspection inside the universe."""
    answer = h.api({'operation': 'manager_status', 'operation_id': str(uuid.uuid4()),
                    'universe_uuid': args.universe, 'authorization_ref': args.reference})
    if not answer.get('ok'):
        raise SystemExit(f'the control door refused: {answer.get("error")}')
    out = h.call('podman_run', args=['exec', 'podmesh-' + args.universe,
                                     '/usr/lib/podmesh-manager/podmesh-managerd', '--inspect-store',
                                     '--config', '/etc/podmesh-manager/config.json',
                                     '--state-dir', '/var/lib/podmesh-manager'])
    facts = json.loads(out['stdout']).get('ordered_facts') or []
    # The store keeps every revision of a subject, not only the last: the current state of a
    # login is its highest revision inside each scope, and a revocation is just the revision that
    # says `revoked`.
    latest = {}
    for fact in facts:
        subject = fact.get('subject') or ''
        if not subject.startswith(SUBJECT_PREFIX):
            continue
        key = (fact.get('scope'), subject)
        revision = fact.get('subject_revision') or 0
        if revision >= latest.get(key, (-1, None))[0]:
            latest[key] = (revision, fact.get('value'))
    found = {}
    for (scope, subject), (revision, value) in latest.items():
        found.setdefault(subject[len(SUBJECT_PREFIX):], []).append(
            {'scope': scope, 'revision': revision, 'revoked': value == 'revoked'})
    return found


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('command', choices=['bootstrap', 'create', 'list', 'revoke'])
    p.add_argument('--host', required=True, help='ssh target of the host carrying the replica')
    p.add_argument('--universe', required=True, help='the replica universe uuid on that host')
    p.add_argument('--login')
    p.add_argument('--scope', help='the replica\'s granted scope; else read from the replica set')
    p.add_argument('--alias', help='lab-a, lab-b, lab-c: which scope of the replica set')
    p.add_argument('--reference', default='manager-admin-tool')
    args = p.parse_args()
    h = host_of(args)
    if args.command == 'bootstrap':
        existing = administrators(h, args)
        live = {n for n, entries in existing.items() if not all(e['revoked'] for e in entries)}
        if live:
            print(json.dumps({'bootstrap': 'not needed', 'administrators': sorted(live)}, indent=2))
            return
        scope = scope_of(args, h)
        # 24 characters of base32 from the OS: not a default password, and not one anybody typed.
        password = secrets.token_urlsafe(18)
        observe(h, args, scope, SUBJECT_PREFIX + DEPLOY_LOGIN, hash_password(password))
        observe(h, args, scope, FLAG_PREFIX + DEPLOY_LOGIN, 'must_change')
        print(json.dumps({'bootstrap': 'created', 'login': DEPLOY_LOGIN, 'password': password, 'scope': scope,
                          'note': 'shown once, on this root-only channel; the manager refuses everything else '
                                  'until this password is replaced at the first sign-in'}, indent=2))
        return
    if args.command == 'list':
        print(json.dumps({'administrators': administrators(h, args)}, indent=2, sort_keys=True))
        return
    login = (args.login or '').strip().lower()
    if not login or any(c not in LOGIN_ALPHABET for c in login) or len(login) > 64:
        raise SystemExit('a login is 1 to 64 characters from a-z, 0-9, dot, dash and underscore')
    scope = scope_of(args, h)
    if args.command == 'revoke':
        observe(h, args, scope, SUBJECT_PREFIX + login, 'revoked')
        print(json.dumps({'revoked': login, 'scope': scope,
                          'note': 'a revocation is an observation like any other; it replicates'}, indent=2))
        return
    existing = administrators(h, args)
    if login in existing and not all(e['revoked'] for e in existing[login]):
        raise SystemExit(f'{login} is already an administrator')
    observe(h, args, scope, SUBJECT_PREFIX + login, hash_password(read_password(login)))
    print(json.dumps({'created': login, 'scope': scope, 'first': not existing,
                      'note': 'the password was hashed here; it is not in the store, the journal or any peer'},
                     indent=2))


if __name__ == '__main__':
    main()
