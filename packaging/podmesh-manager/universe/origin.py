#!/usr/bin/env python3
"""The manager universe's origin: what a publishing connector proxies to.

Fail-closed on the governor mark. PodMesh writes that root-only file at the exclusive
publication, under the epoch gate, and removes it at the withdrawal or the fence. Without it
every path answers 503: a connector that reaches a replica which is not the governor gets
nothing. This process decides nothing about the role.

Surfaces:
  GET  /                    the human page (governor only)
  GET  /ready               the machine JSON the publisher contract requires
  GET  /admin               the administration page: one document, driven by script
  GET  /admin/api/state     what the page shows: which view, and the administrators once signed in
  POST /admin/api/login     {login, password}
  POST /admin/api/logout
  POST /admin/api/password  {current, next, again}
  POST /admin/api/users     {login, password}

NO FORM POSTS (operator rule, 2026-09-16, INTENT.md "Web surfaces"). Nothing here is a native
form submission: the page calls the API from script, stays in place, keeps what was typed and
shows the precise refusal where the person is looking. It is enforced twice -- the API refuses
any body that is not `application/json` (which a native form cannot send, so this also closes
cross-site form forgery), and the page's policy sets `form-action 'none'`, so the browser itself
refuses to submit a form.

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
         'h2{font-size:1.1rem;margin-top:2rem}p,dd,li{line-height:1.45;color:#4a433b}'
         'dl{display:grid;grid-template-columns:8rem 1fr;gap:.35rem 1rem}dt{color:#7a7268}a{color:#215547}'
         'label{display:block;margin:.9rem 0 .2rem;color:#7a7268;font-size:.9rem}'
         'input{box-sizing:border-box;width:100%;padding:.55rem .7rem;border:1px solid #d8cfbe;border-radius:.4rem;background:#fffdf8;font:inherit}'
         '.act{margin-top:1.1rem;padding:.55rem 1.1rem;border:0;border-radius:.4rem;background:#215547;color:#f4efe4;font:inherit;cursor:pointer}'
         '.act[disabled]{opacity:.6;cursor:wait}.quiet{margin-left:.5rem;padding:.2rem .6rem;border:1px solid #d8cfbe;border-radius:.3rem;background:transparent;color:#4a433b;font:inherit;cursor:pointer}'
         'table{border-collapse:collapse;width:100%;margin-top:1rem}td,th{text-align:left;padding:.4rem .6rem;border-bottom:1px solid #e4dccb}'
         '.note{background:#efe7d6;border-left:3px solid #215547;padding:.8rem 1rem;border-radius:.2rem}'
         '.bad{border-left-color:#8c3b2e}code{font-size:.92em}[hidden]{display:none!important}'
         '.secret{position:relative}.secret input{padding-right:3.6rem}'
         '.secret button{position:absolute;right:.35rem;top:50%;transform:translateY(-50%);margin:0;padding:.3rem .45rem;line-height:0;background:transparent;'
         'color:#7a7268;border:1px solid #d8cfbe;border-radius:.3rem;font-size:.8rem;cursor:pointer}'
         '.secret button:hover{color:#215547;border-color:#215547}')

# The page is one document; what it shows comes from /admin/api/state, every action is a JSON call.
# Everything the server sends is put in place with textContent, never parsed as markup. The eye on a
# password field shows what is really in it -- a capital a keyboard added, a space a password manager
# left -- which is most of what makes a sign-in fail.
APP = r"""
(function(){
var root=document.getElementById('app');
var SVG='http://www.w3.org/2000/svg';
function el(tag,attrs){var e=document.createElement(tag);attrs=attrs||{};
 Object.keys(attrs).forEach(function(k){if(k==='text')e.textContent=attrs[k];else if(k==='cls')e.className=attrs[k];else e.setAttribute(k,attrs[k]);});
 for(var i=2;i<arguments.length;i++){var c=arguments[i];if(c)e.appendChild(typeof c==='string'?document.createTextNode(c):c);}return e;}
// The eye: an open eye while the password is hidden, a struck-through one while it shows.
function icon(open){var s=document.createElementNS(SVG,'svg');['viewBox','0 0 24 24','width','18','height','18','fill','none','stroke','currentColor','stroke-width','2','stroke-linecap','round','stroke-linejoin','round','aria-hidden','true']
 .reduce(function(a,v,i,l){if(i%2===0)s.setAttribute(v,l[i+1]);return a;},0);
 function add(tag,attrs){var n=document.createElementNS(SVG,tag);Object.keys(attrs).forEach(function(k){n.setAttribute(k,attrs[k]);});s.appendChild(n);}
 if(open){add('path',{d:'M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z'});add('circle',{cx:'12',cy:'12',r:'3'});}
 else{add('path',{d:'M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24'});add('line',{x1:'1',y1:'1',x2:'23',y2:'23'});}
 return s;}
function field(id,label,type,auto){var input=el('input',{id:id,name:id,type:type,autocomplete:auto,spellcheck:'false',autocapitalize:'off'});
 var box=el('div',{cls:type==='password'?'secret':''},input);
 if(type==='password'){var b=el('button',{type:'button','aria-label':'Show the password',title:'Show the password'});b.appendChild(icon(true));
  b.addEventListener('click',function(){var shown=input.type==='text';input.type=shown?'password':'text';
   b.textContent='';b.appendChild(icon(shown));var label=shown?'Show the password':'Hide the password';
   b.setAttribute('aria-label',label);b.setAttribute('title',label);input.focus();});
  box.appendChild(b);}
 return {node:el('div',{},el('label',{for:id,text:label}),box),input:input};}
function note(){return el('p',{cls:'note bad',hidden:'hidden',role:'alert'});}
function show(n,text,good){n.textContent=text;n.className='note'+(good?'':' bad');n.hidden=!text;}
// Every action is a JSON call; the page never submits a form and never navigates.
function call(path,body){return fetch(path,{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json'},body:JSON.stringify(body||{})})
 .then(function(r){return r.json().catch(function(){return {error:'The manager answered '+r.status+' without a readable reason.'};}).then(function(j){j._status=r.status;return j;});})
 .catch(function(){return {error:'The manager could not be reached.',_status:0};});}
// A session that ended -- expired, or the replica restarted -- sends the page back to sign-in,
// saying why, instead of showing a refusal the person cannot act on.
function expired(j,path){if(j._status===401&&path!=='/admin/api/login'){load('Your session ended. Sign in again.');return true;}return false;}
// A <form> kept as the container, so a password manager pairs the login with its password and
// Enter submits; the submission is intercepted and becomes one JSON call. It has no method and no
// action, and the page's policy (form-action 'none') would refuse a native submission anyway.
function form(label,fields,run){var f=el('form',{novalidate:'novalidate'});fields.forEach(function(x){f.appendChild(x.node);});
 var go=el('button',{cls:'act',type:'submit',text:label});f.appendChild(go);
 f.addEventListener('submit',function(ev){ev.preventDefault();if(go.disabled)return;go.disabled=true;
  Promise.resolve(run()).then(function(){go.disabled=false;},function(){go.disabled=false;});});
 return f;}
function page(title){root.textContent='';root.appendChild(el('p',{text:'PODMESH / MANAGER'}));root.appendChild(el('h1',{text:title}));}
function load(flash){return fetch('/admin/api/state',{credentials:'same-origin'}).then(function(r){return r.json();})
 .then(function(s){try{render(s,flash);}catch(e){page('Unavailable');root.appendChild(el('p',{cls:'note bad',text:'The page could not be drawn: '+e.message}));}})
 .catch(function(){page('Unavailable');root.appendChild(el('p',{cls:'note bad',text:'The manager could not be reached.'}));});}
function render(s,flash){
 if(s.view==='closed'){page('Not the governor');root.appendChild(el('p',{cls:'note bad',text:'This replica does not hold the role; it administers nothing.'}));return;}
 if(s.view==='none'){page('No administrator exists yet.');
  root.appendChild(el('p',{cls:'note',text:'The first administrator is written from the host that carries the governor, as root, through PodMesh’s control door; a deployment does it (tools/manager-admin.py bootstrap).'}));return;}
 if(s.view==='login'){page('Sign in');var n=note();root.appendChild(n);if(flash)show(n,flash);
  var l=field('login','Administrator','text','username'),p=field('password','Password','password','current-password');
  root.appendChild(form('Sign in',[l,p],function(){show(n,'');return call('/admin/api/login',{login:l.input.value,password:p.input.value}).then(function(j){
   if(j.ok)return load();show(n,j.error||'Refused.');p.input.focus();});}));
  (l.input.value?p.input:l.input).focus();return;}
 if(s.view==='change'){page('Change the password.');
  root.appendChild(el('p',{cls:'note',text:'This account still carries the password it was given when the manager was deployed. Nothing else opens until it is replaced.'}));
  var n2=note();root.appendChild(n2);
  var u=el('input',{type:'text',name:'username',autocomplete:'username',value:s.login,hidden:'hidden','aria-hidden':'true'});
  var c=field('current','Current password','password','current-password'),x=field('next','New password','password','new-password'),a=field('again','New password again','password','new-password');
  var fc=form('Change it',[c,x,a],function(){show(n2,'');return call('/admin/api/password',{current:c.input.value,next:x.input.value,again:a.input.value}).then(function(j){
   if(expired(j,'/admin/api/password'))return;if(j.ok)return load();show(n2,j.error||'Refused.');});});
  fc.insertBefore(u,fc.firstChild);root.appendChild(fc);
  root.appendChild(el('p',{},'Signed in as ',el('code',{text:s.login}),'.'));c.input.focus();return;}
 page('Administration');
 var who=el('p',{},'Signed in as ',el('code',{text:s.login}),' on the governor at epoch ',el('code',{text:String(s.epoch)}),'.');
 var out=el('button',{cls:'quiet',type:'button',text:'Sign out'});who.appendChild(out);root.appendChild(who);
 out.addEventListener('click',function(){out.disabled=true;call('/admin/api/logout').then(function(){load();});});
 var msg=note();root.appendChild(msg);if(flash)show(msg,flash,true);
 root.appendChild(el('h2',{text:'Administrators'}));
 var table=el('table',{},el('tr',{},el('th',{text:'login'}),el('th',{text:'scope'})));
 (s.administrators||[]).forEach(function(r){table.appendChild(el('tr',{},el('td',{},el('code',{text:r.login})),
  el('td',{text:r.scopes.join(', ')+(r.conflict?' — written in more than one scope':'')})));});
 root.appendChild(table);
 root.appendChild(el('h2',{text:'Create an administrator'}));
 root.appendChild(el('p',{},'Written as a replicated fact in this replica’s own scope ',el('code',{text:s.scope}),'. The password is hashed on the replica and never stored, logged or replicated.'));
 var nl=field('new-login','Login','text','off'),np=field('new-password','Password','password','new-password');
 root.appendChild(form('Create',[nl,np],function(){show(msg,'');return call('/admin/api/users',{login:nl.input.value,password:np.input.value}).then(function(j){
  if(expired(j,'/admin/api/users'))return;if(j.ok)return load(j.message);show(msg,j.error||'Refused.');});}));
}
load();
})();
"""


def page_home(mark):
    lid, rid = html.escape(identity['logical_manager_id']), html.escape(identity['replica_id'])
    return (f'<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">'
            f'<title>PodMesh manager</title><style>{STYLE}</style><main>'
            f'<p>PODMESH / MANAGER ORIGIN</p><h1>This replica is the governor.</h1>'
            f'<p>The public hostname reaches the replica that currently holds the exclusive role. '
            f'Machine JSON stays at <a href="/ready"><code>/ready</code></a>, administration at <a href="/admin">/admin</a>.</p>'
            f'<dl><dt>epoch</dt><dd><code>{html.escape(str(mark.get("epoch")))}</code></dd>'
            f'<dt>replica</dt><dd><code>{rid}</code></dd><dt>logical</dt><dd><code>{lid}</code></dd></dl></main></html>').encode()


def page_app(nonce):
    # What shows before the script draws the page is itself the failure message: a script that is
    # blocked, fails to parse or never arrives leaves words that say so, never a blank page.
    return (f'<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">'
            f'<title>Administration — PodMesh manager</title><style>{STYLE}</style>'
            f'<main id="app"><p>PODMESH / MANAGER</p><h1>Administration</h1>'
            f'<noscript><p class="note bad">This page needs JavaScript: every action is a call to the manager\'s API, and nothing is submitted as a form.</p></noscript>'
            f'<p class="note" id="starting">Loading… If this message stays, the page\'s script could not run in this browser; '
            f'reload it, or check that nothing blocks scripts on this site.</p></main>'
            f'<script nonce="{nonce}">{APP}</script></html>').encode()


def admin_rows(admins):
    return [{'login': name, 'scopes': entry['scopes'], 'conflict': len(entry['scopes']) > 1}
            for name, entry in sorted(admins.items())]


class Origin(http.server.BaseHTTPRequestHandler):
    server_version = 'podmesh-manager-origin'

    def log_message(self, *a):
        pass

    def send(self, code, body, kind, nonce=None, cookie=None):
        self.send_response(code)
        self.send_header('Content-Type', kind)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Referrer-Policy', 'no-referrer')
        script = f"script-src 'nonce-{nonce}'; " if nonce else ''
        # form-action 'none': the browser itself refuses to submit a form from this origin.
        self.send_header('Content-Security-Policy',
                         f"default-src 'none'; style-src 'unsafe-inline'; {script}connect-src 'self'; form-action 'none'; frame-ancestors 'none'")
        if cookie:
            self.send_header('Set-Cookie', cookie)
        self.end_headers()
        self.wfile.write(body)

    def api(self, code, payload, cookie=None):
        self.send(code, json.dumps(payload).encode(), 'application/json', cookie=cookie)

    def closed(self, path):
        reason = 'not the governor' if path in ('/', '/index.html', '/ready') or path.startswith('/admin') else 'no such path'
        self.send(503, json.dumps({'ready': False, 'reason': reason, **identity}).encode(), 'application/json')

    def body(self):
        """The JSON body of an API call, or None. Anything else -- a native form's urlencoded or
        multipart body included -- is refused before it is read."""
        kind = (self.headers.get('Content-Type') or '').split(';')[0].strip().lower()
        if kind != 'application/json':
            return None
        length = int(self.headers.get('Content-Length') or 0)
        if length < 0 or length > 4096:
            return None
        try:
            data = json.loads(self.rfile.read(length) or b'{}')
        except ValueError:
            return None
        return data if isinstance(data, dict) else None

    def do_GET(self):
        path = self.path.split('?', 1)[0]
        mark = governor()
        if not mark:
            return self.closed(path)
        if path in ('/', '/index.html'):
            return self.send(200, page_home(mark), 'text/html; charset=utf-8')
        if path == '/ready':
            return self.api(200, {'ready': True, **identity, 'epoch': mark.get('epoch'), 'marked_at': mark.get('marked_at')})
        if path == '/admin':
            nonce = secrets.token_urlsafe(16)
            return self.send(200, page_app(nonce), 'text/html; charset=utf-8', nonce=nonce)
        if path == '/admin/api/state':
            try:
                admins = administrators()
            except Exception:
                return self.api(503, {'error': 'The store could not be read.'})
            if not admins:
                return self.api(200, {'view': 'none'})
            _, entry = session_of(self.headers)
            if not entry:
                return self.api(200, {'view': 'login'})
            if flags().get(entry['login']) == MUST_CHANGE:
                return self.api(200, {'view': 'change', 'login': entry['login']})
            return self.api(200, {'view': 'admin', 'login': entry['login'], 'epoch': mark.get('epoch'),
                                  'scope': SCOPE, 'administrators': admin_rows(admins)})
        return self.closed(path)

    def do_POST(self):
        path = self.path.split('?', 1)[0]
        mark = governor()
        if not mark:
            return self.closed(path)
        if not path.startswith('/admin/api/'):
            # No form posts: there is no endpoint a native form could reach.
            return self.api(405, {'error': 'No form posts here; the page calls /admin/api/ with JSON.'})
        fields = self.body()
        if fields is None:
            return self.api(415, {'error': 'The API takes a JSON body (Content-Type: application/json), at most 4096 bytes.'})
        if path == '/admin/api/logout':
            token, _ = session_of(self.headers)
            sessions.pop(token, None)
            return self.api(200, {'ok': True}, 'podmesh_admin=; Path=/admin; Max-Age=0; HttpOnly; SameSite=Strict; Secure')
        if path == '/admin/api/login':
            login = str(fields.get('login') or '').strip().lower()
            password = str(fields.get('password') or '')
            if failures.get(login, {}).get('until', 0) > time.time():
                return self.api(429, {'error': 'Too many attempts for that administrator. Wait a minute.'})
            try:
                admins = administrators()
            except Exception:
                return self.api(503, {'error': 'The store could not be read.'})
            entry = admins.get(login)
            if not entry or not verify_password(password, entry['value']):
                count = failures.get(login, {}).get('count', 0) + 1
                failures[login] = {'count': count, 'until': time.time() + 60 if count >= 5 else 0}
                return self.api(401, {'error': 'Wrong administrator or password.'})
            failures.pop(login, None)
            token = secrets.token_urlsafe(32)
            sessions[token] = {'login': login, 'until': time.time() + SESSION_SECONDS}
            return self.api(200, {'ok': True},
                            f'podmesh_admin={token}; Path=/admin; Max-Age={SESSION_SECONDS}; HttpOnly; SameSite=Strict; Secure')
        _, entry = session_of(self.headers)
        if not entry:
            return self.api(401, {'error': 'Sign in first. An administrator is named by an administrator.'})
        login = entry['login']
        try:
            admins = administrators()
        except Exception:
            return self.api(503, {'error': 'The store could not be read.'})
        if path == '/admin/api/password':
            current, nxt, again = (str(fields.get(k) or '') for k in ('current', 'next', 'again'))
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
                return self.api(400, {'error': problem})
            try:
                append_observation(SUBJECT_PREFIX + login, hash_password(nxt))
                append_observation(FLAG_PREFIX + login, 'changed')
            except Exception as e:
                return self.api(503, {'error': f'The resident refused: {e}'})
            return self.api(200, {'ok': True, 'message': 'The password was changed.'})
        if path == '/admin/api/users':
            if flags().get(login) == MUST_CHANGE:
                return self.api(403, {'error': 'Replace the deployment password before naming anyone.'})
            new = str(fields.get('login') or '').strip().lower()
            password = str(fields.get('password') or '')
            problem = None
            if not new or any(c not in LOGIN_ALPHABET for c in new) or len(new) > 64:
                problem = 'A login is 1 to 64 characters from a-z, 0-9, dot, dash and underscore.'
            elif new in admins:
                problem = 'That administrator already exists.'
            elif len(password) < MIN_PASSWORD:
                problem = f'A password is at least {MIN_PASSWORD} characters.'
            elif password.lower() == new:
                problem = 'A password that is the login is not a password.'
            if problem:
                return self.api(400, {'error': problem})
            try:
                append_observation(SUBJECT_PREFIX + new, hash_password(password))
            except Exception as e:
                return self.api(503, {'error': f'The resident refused: {e}'})
            return self.api(200, {'ok': True, 'message': f'Administrator {new} created in {SCOPE}; it replicates to the other replicas.'})
        return self.api(404, {'error': 'No such call.'})


if __name__ == '__main__':
    http.server.ThreadingHTTPServer(('0.0.0.0', PORT), Origin).serve_forever()
