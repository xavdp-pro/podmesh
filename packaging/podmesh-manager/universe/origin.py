#!/usr/bin/env python3
"""The manager universe's origin: what a publishing connector proxies to.

Fail-closed on the governor mark. PodMesh writes that root-only file at the exclusive
publication, under the epoch gate, and removes it at the withdrawal or the fence. Without it
every path answers 503: a connector that reaches a replica which is not the governor gets
nothing. This process decides nothing about the role.

Surfaces:
  GET  /            the human page (governor only)
  GET  /ready       the machine JSON the publisher contract requires
  GET  /admin       sign in, or the administration page with a session
  POST /admin/login
  POST /admin/logout
  POST /admin/users create another administrator, with a session

WHERE ADMINISTRATORS COME FROM. An administrator is a replicated fact, written through the
resident's control socket in this replica's own granted scope, subject `admin.user.<login>`,
value `scrypt.<n>.<r>.<p>.<salt hex>.<hash hex>`. The password itself is never stored, never
logged and never replicated. **The first administrator cannot be created here.** With no
administrator on record this page says so and refuses: the first one is written from the host,
as root, through PodMesh's control door (`tools/manager-admin.py`). What has power has no front
door; the page is a surface over a decision made elsewhere, and only an administrator already
on record may name the next one.

Ownership, stated rather than assumed: each replica may write only in its own granted scope, so
an administrator created here lives in this replica's scope and replicates read-only to the
others. If the same login is written in two scopes, the page shows the conflict rather than
choosing a winner.
"""
import hashlib, hmac, html, http.server, json, os, secrets, socket, subprocess, time, urllib.parse

CONFIG = os.environ.get('PODMESH_MANAGER_CONFIG', '/etc/podmesh-manager/config.json')
STATE = os.environ.get('PODMESH_MANAGER_STATE', '/var/lib/podmesh-manager')
RESIDENT = os.environ.get('PODMESH_MANAGER_BINARY', '/usr/lib/podmesh-manager/podmesh-managerd')
MARK = os.environ.get('PODMESH_GOVERNOR_MARK', '/run/podmesh-manager/governor.json')
PORT = int(os.environ.get('PODMESH_ORIGIN_PORT', '8080'))

cfg = json.load(open(CONFIG))
identity = {'logical_manager_id': cfg['network']['manager']['logical_manager_id'],
            'replica_id': cfg['network']['replica_id']}
CONTROL = cfg['control_socket']
SCOPE = next(g['scope'] for g in cfg['network']['manager']['grants']
             if g['owner_replica_id'] == identity['replica_id'])

SUBJECT_PREFIX = 'admin.user.'
FLAG_PREFIX = 'admin.flag.'
MUST_CHANGE = 'must_change'
LOGIN_ALPHABET = 'abcdefghijklmnopqrstuvwxyz0123456789-_.'
MIN_PASSWORD = 12
SESSION_SECONDS = 3600
SCRYPT_N, SCRYPT_R, SCRYPT_P = 16384, 8, 1
sessions = {}
failures = {}


def governor():
    try:
        mark = json.load(open(MARK))
    except Exception:
        return None
    return mark or None


def facts():
    """The ordered facts, read through the resident's own read-only inspection."""
    p = subprocess.run([RESIDENT, '--inspect-store', '--config', CONFIG, '--state-dir', STATE],
                       capture_output=True, text=True, timeout=20)
    if p.returncode:
        raise RuntimeError('the store could not be inspected')
    return json.loads(p.stdout).get('ordered_facts') or []


def flags():
    """`admin.flag.<login>`: `must_change` while the account still carries the password it was
    given at deployment. A deployed manager has an administrator from the first minute, and that
    administrator cannot do anything until the password that came with it is replaced."""
    found = {}
    for fact in facts():
        subject = fact.get('subject') or ''
        if subject.startswith(FLAG_PREFIX):
            login = subject[len(FLAG_PREFIX):]
            revision = fact.get('subject_revision') or 0
            if revision >= found.get(login, (-1, None))[0]:
                found[login] = (revision, fact.get('value'))
    return {login: value for login, (_, value) in found.items()}


def administrators():
    """login -> {value, scopes}. Within one scope the resident keeps the latest revision of a
    subject; across scopes a login written in two places is a conflict and is reported as one,
    never silently resolved -- each replica owns its scope, and ownership is the model."""
    found = {}
    for fact in facts():
        subject = fact.get('subject') or ''
        if not subject.startswith(SUBJECT_PREFIX):
            continue
        login = subject[len(SUBJECT_PREFIX):]
        scope = fact.get('scope') or ''
        revision = fact.get('subject_revision') or 0
        entry = found.setdefault(login, {'value': None, 'scopes': [], 'revision': -1})
        if revision >= entry['revision']:
            entry['value'], entry['revision'] = fact.get('value'), revision
        if scope and scope not in entry['scopes']:
            entry['scopes'].append(scope)
    return {login: e for login, e in found.items() if e['value'] and e['value'] != 'revoked'}


def hash_password(password, salt=None):
    salt = salt or secrets.token_bytes(16)
    digest = hashlib.scrypt(password.encode(), salt=salt, n=SCRYPT_N, r=SCRYPT_R, p=SCRYPT_P, dklen=32)
    return f'scrypt.{SCRYPT_N}.{SCRYPT_R}.{SCRYPT_P}.{salt.hex()}.{digest.hex()}'


def verify_password(password, stored):
    try:
        kind, n, r, p, salt, digest = stored.split('.')
        if kind != 'scrypt':
            return False
        want = hashlib.scrypt(password.encode(), salt=bytes.fromhex(salt), n=int(n), r=int(r), p=int(p), dklen=len(digest) // 2)
    except Exception:
        return False
    return hmac.compare_digest(want.hex(), digest)


def append_observation(subject, value):
    """One typed observation through the resident's control socket, in this replica's scope.

    The resident answers `observed`, or `append_observation_uncertain` / `_busy` when it cannot
    say whether the fact landed. The same operation ID is retried in that case, exactly as the
    entrypoint does for the boot fact: the resident refuses to append an ID it already has, so
    the retry is safe and the administrator is recorded once."""
    operation_id = secrets.token_hex(16)
    request = json.dumps({'operation': 'append_observation', 'operation_id': operation_id,
                          'scope': SCOPE, 'subject': subject, 'value': value}).encode()
    last = ''
    for _ in range(5):
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(20)
            s.connect(CONTROL)
            s.sendall(request)
            s.shutdown(socket.SHUT_WR)
            chunks = []
            while True:
                chunk = s.recv(65536)
                if not chunk:
                    break
                chunks.append(chunk)
        last = b''.join(chunks).decode('utf-8', 'replace').strip()
        if '"result":"observed"' in last.replace(' ', ''):
            return last
        if 'append_observation_uncertain' in last or 'append_observation_busy' in last:
            time.sleep(1)
            continue
        break
    raise RuntimeError(f'the resident did not observe it: {last[:200]}')


def session_of(headers):
    raw = headers.get('Cookie') or ''
    for part in raw.split(';'):
        name, _, value = part.strip().partition('=')
        if name == 'podmesh_admin':
            entry = sessions.get(value)
            if entry and entry['until'] > time.time():
                return value, entry
            sessions.pop(value, None)
    return None, None


STYLE = ('body{margin:0;background:#f4efe4;color:#1c1916;font-family:ui-sans-serif,system-ui,sans-serif}'
         'main{max-width:40rem;margin:10vh auto;padding:0 1.5rem}h1{font-size:1.6rem;font-weight:600}'
         'p,dd,li{line-height:1.45;color:#4a433b}dl{display:grid;grid-template-columns:8rem 1fr;gap:.35rem 1rem}'
         'dt{color:#7a7268}a{color:#215547}label{display:block;margin:.9rem 0 .2rem;color:#7a7268;font-size:.9rem}'
         'input{width:100%;padding:.55rem .7rem;border:1px solid #d8cfbe;border-radius:.4rem;background:#fffdf8;font:inherit}'
         'button{margin-top:1.1rem;padding:.55rem 1.1rem;border:0;border-radius:.4rem;background:#215547;color:#f4efe4;font:inherit;cursor:pointer}'
         'table{border-collapse:collapse;width:100%;margin-top:1rem}td,th{text-align:left;padding:.4rem .6rem;border-bottom:1px solid #e4dccb}'
         '.note{background:#efe7d6;border-left:3px solid #215547;padding:.8rem 1rem;border-radius:.2rem}'
         '.bad{border-left-color:#8c3b2e}code{font-size:.92em}'
         '.secret{position:relative}.secret input{padding-right:3.4rem}'
         '.secret button{position:absolute;right:.35rem;top:.3rem;margin:0;padding:.3rem .55rem;background:transparent;'
         'color:#7a7268;border:1px solid #d8cfbe;border-radius:.3rem;font-size:.8rem;cursor:pointer}'
         '.secret button:hover{color:#215547;border-color:#215547}')

# A password typed into a field nobody can read is a password nobody can check. The eye shows what
# is actually in the field -- a capital the keyboard added, a space a password manager left --
# which is most of what makes a sign-in fail. It is the only script on the page, and the policy
# admits it by nonce and nothing else.
EYE = ("document.querySelectorAll('.secret').forEach(function(box){"
       "var i=box.querySelector('input'),b=box.querySelector('button');"
       "b.addEventListener('click',function(){"
       "var shown=i.type==='text';i.type=shown?'password':'text';"
       "b.textContent=shown?'\\u25cf\\u25cf\\u25cf':'\\u25c9';"
       "b.setAttribute('aria-label',shown?'Show the password':'Hide the password');"
       "i.focus();});});")


def secret_field(field_id, name, label, autocomplete):
    return (f'<label for="{field_id}">{label}</label><div class="secret">'
            f'<input id="{field_id}" name="{name}" type="password" autocomplete="{autocomplete}">'
            f'<button type="button" aria-label="Show the password" title="Show the password">&#128065;</button></div>')


def shell(title, body, nonce=''):
    script = f'<script nonce="{nonce}">{EYE}</script>' if nonce else ''
    return (f'<!doctype html><html lang="en"><meta charset="utf-8">'
            f'<meta name="viewport" content="width=device-width, initial-scale=1">'
            f'<title>{html.escape(title)}</title><style>{STYLE}</style><main>{body}</main>{script}</html>').encode()


def page_home(mark):
    lid, rid = html.escape(identity['logical_manager_id']), html.escape(identity['replica_id'])
    return shell('PodMesh manager',
                 f'<p>PODMESH / MANAGER ORIGIN</p><h1>This replica is the governor.</h1>'
                 f'<p>The public hostname reaches the replica that currently holds the exclusive role. '
                 f'Machine JSON stays at <a href="/ready"><code>/ready</code></a>, administration at '
                 f'<a href="/admin">/admin</a>.</p>'
                 f'<dl><dt>epoch</dt><dd><code>{html.escape(str(mark.get("epoch")))}</code></dd>'
                 f'<dt>replica</dt><dd><code>{rid}</code></dd>'
                 f'<dt>logical</dt><dd><code>{lid}</code></dd></dl>')


def page_login(message='', nonce=''):
    warning = f'<p class="note bad">{html.escape(message)}</p>' if message else ''
    return shell('Sign in — PodMesh manager',
                 f'<p>PODMESH / MANAGER</p><h1>Sign in</h1>{warning}'
                 f'<form method="post" action="/admin/login">'
                 f'<label for="login">Administrator</label><input id="login" name="login" autocomplete="username" autofocus>'
                 + secret_field('password', 'password', 'Password', 'current-password') +
                 f'<button type="submit">Sign in</button></form>', nonce)


def page_no_admin():
    return shell('No administrator — PodMesh manager',
                 '<p>PODMESH / MANAGER</p><h1>No administrator exists yet.</h1>'
                 '<p class="note">The first administrator is not created from this page. It is written on the host '
                 'that carries the governor, as root, through PodMesh\'s control door:</p>'
                 '<p><code>podmesh manager_admin_create</code> — see <code>tools/manager-admin.py</code>.</p>'
                 '<p>What has power has no front door. Once one administrator is on record, that administrator '
                 'names the next ones here.</p>')


def page_change(login, message='', bad=False, nonce=''):
    note = f'<p class="note{" bad" if bad else ""}">{html.escape(message)}</p>' if message else ''
    return shell('Change the password — PodMesh manager',
                 f'<p>PODMESH / MANAGER</p><h1>Change the password.</h1>'
                 f'<p class="note">This account still carries the password it was given when the manager was '
                 f'deployed. Nothing else opens until it is replaced.</p>{note}'
                 f'<form method="post" action="/admin/password">'
                 + secret_field('current', 'current', 'Current password', 'current-password')
                 + secret_field('next', 'next', 'New password', 'new-password')
                 + secret_field('again', 'again', 'New password again', 'new-password') +
                 f'<button type="submit">Change it</button></form>'
                 f'<p>Signed in as <code>{html.escape(login)}</code>.</p>', nonce)


def page_admin(mark, login, admins, message='', bad=False, nonce=''):
    rows = ''
    for name, entry in sorted(admins.items()):
        conflict = ' <em>(written in more than one scope)</em>' if len(entry['scopes']) > 1 else ''
        rows += f'<tr><td><code>{html.escape(name)}</code></td><td>{html.escape(", ".join(entry["scopes"]))}{conflict}</td></tr>'
    note = f'<p class="note{" bad" if bad else ""}">{html.escape(message)}</p>' if message else ''
    return shell('Administration — PodMesh manager',
                 f'<p>PODMESH / MANAGER</p><h1>Administration</h1>'
                 f'<p>Signed in as <code>{html.escape(login)}</code> on the governor at epoch '
                 f'<code>{html.escape(str(mark.get("epoch")))}</code>. '
                 f'<form method="post" action="/admin/logout" style="display:inline">'
                 f'<button type="submit" style="margin:0;padding:.2rem .6rem">Sign out</button></form></p>{note}'
                 f'<h2 style="font-size:1.1rem">Administrators</h2>'
                 f'<table><tr><th>login</th><th>scope</th></tr>{rows}</table>'
                 f'<h2 style="font-size:1.1rem;margin-top:2rem">Create an administrator</h2>'
                 f'<p>Written as a replicated fact in this replica\'s own scope <code>{html.escape(SCOPE)}</code>. '
                 f'The password is hashed here and never stored, logged or replicated.</p>'
                 f'<form method="post" action="/admin/users">'
                 f'<label for="new-login">Login</label><input id="new-login" name="login" autocomplete="off">'
                 + secret_field('new-password', 'password', 'Password', 'new-password') +
                 f'<button type="submit">Create</button></form>', nonce)


class Origin(http.server.BaseHTTPRequestHandler):
    server_version = 'podmesh-manager-origin'

    def log_message(self, *a):
        pass

    def reply(self, code, body, kind='text/html; charset=utf-8', cookie=None, nonce=None):
        self.send_response(code)
        self.send_header('Content-Type', kind)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Referrer-Policy', 'no-referrer')
        csp = "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'"
        if nonce:
            csp += f"; script-src 'nonce-{nonce}'"
        self.send_header('Content-Security-Policy', csp)
        if cookie:
            self.send_header('Set-Cookie', cookie)
        self.end_headers()
        self.wfile.write(body)

    def page(self, code, maker, cookie=None):
        nonce = secrets.token_urlsafe(16)
        return self.reply(code, maker(nonce), cookie=cookie, nonce=nonce)

    def closed(self, path):
        reason = 'not the governor' if path in ('/', '/index.html', '/ready') or path.startswith('/admin') else 'no such path'
        self.reply(503, json.dumps({'ready': False, 'reason': reason, **identity}).encode(), 'application/json')

    def form(self):
        length = int(self.headers.get('Content-Length') or 0)
        if length <= 0 or length > 4096:
            return {}
        raw = self.rfile.read(length).decode('utf-8', 'replace')
        return {k: v[0] for k, v in urllib.parse.parse_qs(raw).items()}

    def see(self, where, cookie=None):
        self.send_response(303)
        self.send_header('Location', where)
        self.send_header('Content-Length', '0')
        if cookie:
            self.send_header('Set-Cookie', cookie)
        self.end_headers()

    def do_GET(self):
        path = self.path.split('?', 1)[0]
        mark = governor()
        if not mark:
            return self.closed(path)
        if path in ('/', '/index.html'):
            return self.reply(200, page_home(mark))
        if path == '/ready':
            return self.reply(200, json.dumps({'ready': True, **identity, 'epoch': mark.get('epoch'),
                                               'marked_at': mark.get('marked_at')}).encode(), 'application/json')
        if path == '/admin':
            try:
                admins = administrators()
            except Exception:
                return self.reply(503, shell('Unavailable', '<h1>The store could not be read.</h1>'))
            if not admins:
                return self.reply(200, page_no_admin())
            _, entry = session_of(self.headers)
            if not entry:
                return self.page(200, lambda n: page_login(nonce=n))
            if flags().get(entry['login']) == MUST_CHANGE:
                return self.page(200, lambda n: page_change(entry['login'], nonce=n))
            return self.page(200, lambda n: page_admin(mark, entry['login'], admins, nonce=n))
        return self.closed(path)

    def do_POST(self):
        path = self.path.split('?', 1)[0]
        mark = governor()
        if not mark:
            return self.closed(path)
        if path == '/admin/logout':
            token, _ = session_of(self.headers)
            sessions.pop(token, None)
            return self.see('/admin', 'podmesh_admin=; Path=/admin; Max-Age=0; HttpOnly; SameSite=Strict; Secure')
        if path == '/admin/login':
            fields = self.form()
            login = (fields.get('login') or '').strip().lower()
            password = fields.get('password') or ''
            until = failures.get(login, {}).get('until', 0)
            if until > time.time():
                return self.page(429, lambda n: page_login('Too many attempts for that administrator. Wait a minute.', nonce=n))
            try:
                admins = administrators()
            except Exception:
                return self.reply(503, shell('Unavailable', '<h1>The store could not be read.</h1>'))
            entry = admins.get(login)
            if not entry or not verify_password(password, entry['value']):
                count = failures.get(login, {}).get('count', 0) + 1
                failures[login] = {'count': count, 'until': time.time() + 60 if count >= 5 else 0}
                return self.page(401, lambda n: page_login('Wrong administrator or password.', nonce=n))
            failures.pop(login, None)
            token = secrets.token_urlsafe(32)
            sessions[token] = {'login': login, 'until': time.time() + SESSION_SECONDS}
            return self.see('/admin', f'podmesh_admin={token}; Path=/admin; Max-Age={SESSION_SECONDS}; HttpOnly; SameSite=Strict; Secure')
        if path == '/admin/password':
            _, entry = session_of(self.headers)
            if not entry:
                return self.page(401, lambda n: page_login('Sign in first.', nonce=n))
            fields = self.form()
            login = entry['login']
            try:
                admins = administrators()
            except Exception:
                return self.reply(503, shell('Unavailable', '<h1>The store could not be read.</h1>'))
            current, nxt, again = fields.get('current') or '', fields.get('next') or '', fields.get('again') or ''
            record = admins.get(login)
            problem = None
            if not record or not verify_password(current, record['value']):
                problem = 'The current password is wrong.'
            elif nxt != again:
                problem = 'The two new entries differ.'
            elif len(nxt) < MIN_PASSWORD:
                problem = f'A password is at least {MIN_PASSWORD} characters.'
            elif nxt == current:
                problem = 'The new password is the old one.'
            elif nxt.lower() == login:
                problem = 'A password that is the login is not a password.'
            if problem:
                return self.page(400, lambda n: page_change(login, problem, bad=True, nonce=n))
            try:
                append_observation(SUBJECT_PREFIX + login, hash_password(nxt))
                append_observation(FLAG_PREFIX + login, 'changed')
            except Exception as e:
                return self.page(503, lambda n: page_change(login, f'The resident refused: {e}', bad=True, nonce=n))
            try:
                admins = administrators()
            except Exception:
                pass
            return self.page(200, lambda n: page_admin(mark, login, admins, 'The password was changed.', nonce=n))
        if path == '/admin/users':
            _, entry = session_of(self.headers)
            if not entry:
                return self.page(401, lambda n: page_login('Sign in first. An administrator is named by an administrator.', nonce=n))
            if flags().get(entry['login']) == MUST_CHANGE:
                return self.page(403, lambda n: page_change(entry['login'], 'Replace the deployment password before naming anyone.', bad=True, nonce=n))
            fields = self.form()
            login = (fields.get('login') or '').strip().lower()
            password = fields.get('password') or ''
            try:
                admins = administrators()
            except Exception:
                return self.reply(503, shell('Unavailable', '<h1>The store could not be read.</h1>'))
            problem = None
            if not login or any(c not in LOGIN_ALPHABET for c in login) or len(login) > 64:
                problem = 'A login is 1 to 64 characters from a-z, 0-9, dot, dash and underscore.'
            elif login in admins:
                problem = 'That administrator already exists.'
            elif len(password) < MIN_PASSWORD:
                problem = f'A password is at least {MIN_PASSWORD} characters.'
            elif password.lower() == login:
                problem = 'A password that is the login is not a password.'
            if problem:
                return self.page(400, lambda n: page_admin(mark, entry['login'], admins, problem, bad=True, nonce=n))
            try:
                append_observation(SUBJECT_PREFIX + login, hash_password(password))
            except Exception as e:
                return self.page(503, lambda n: page_admin(mark, entry['login'], admins, f'The resident refused: {e}', bad=True, nonce=n))
            try:
                admins = administrators()
            except Exception:
                pass
            return self.page(200, lambda n: page_admin(mark, entry['login'], admins,
                                              f'Administrator {login} created in {SCOPE}; it replicates to the other replicas.', nonce=n))
        return self.closed(path)


if __name__ == '__main__':
    http.server.ThreadingHTTPServer(('0.0.0.0', PORT), Origin).serve_forever()
