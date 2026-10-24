#!/usr/bin/python3
"""Private in-container operations for the bounded manual logical restore helper."""
import base64
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time

HOME = Path('/apps/podmesh-node')
ADMIN = HOME / 'sav/admin'
RUNTIME = Path('/run/podmesh-node/admin')
LIB = Path('/usr/lib/podmesh-node-canonical')
ADOPTION_ENTRYPOINT_SHA256 = '2490cd272725d53dc341c7056cbf0a1817449f83119e0a42119f3c9d5ca22f22'
LIMIT = 16 * 1024 * 1024
BOOTSTRAP_DIRS = ('inbox', 'outbox', 'migrations', 'podman-tmp')
FLAGS = ['--single-transaction', '--quick', '--routines', '--events', '--triggers',
         '--hex-blob', '--skip-comments', '--skip-dump-date', '--skip-lock-tables',
         '--order-by-primary', '--databases', 'podmesh-node']


_SOURCE = 'logical-transfer-native.py'
_COMPONENT = 'native'

# Public failure evidence contains only fixed stages, class names and own code locations.
_DIAGNOSTIC_PREFIX = 'logical-transfer-diagnostic '
_CODE_FILENAME = sys._getframe().f_code.co_filename
_STAGE = 'entrypoint'
_EXCEPTION_NAMES = frozenset(('Exception RuntimeError ValueError TypeError KeyError IndexError '
    'AssertionError OSError FileNotFoundError PermissionError FileExistsError IsADirectoryError '
    'NotADirectoryError BlockingIOError TimeoutError BrokenPipeError ProcessLookupError '
    'JSONDecodeError TimeoutExpired CalledProcessError Error OverflowError UnicodeDecodeError '
    'UnicodeEncodeError RecursionError MemoryError InterruptedError').split())


def _stage(name):
    global _STAGE
    _STAGE = name


def _failure_record(error, stage=None, secondary=False):
    frames = []
    trace = error.__traceback__
    while trace is not None:
        frame = trace.tb_frame
        if frame.f_globals is globals() and frame.f_code.co_filename == _CODE_FILENAME:
            name = frame.f_code.co_name
            if not (re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]{0,63}', name)
                    or name in ('<module>', '<listcomp>', '<dictcomp>', '<setcomp>', '<genexpr>', '<lambda>')):
                name = 'code'
            frames.append({'source': _SOURCE, 'function': name, 'line': trace.tb_lineno})
        trace = trace.tb_next
    name = type(error).__name__
    return {'component': _COMPONENT, 'stage': stage or _STAGE,
            'exception': name if name in _EXCEPTION_NAMES else 'Exception',
            'secondary': secondary, 'frames': frames[-8:]}


def _emit_failure(error, stage=None, secondary=False):
    try:
        print(_DIAGNOSTIC_PREFIX + json.dumps(_failure_record(error, stage, secondary),
              separators=(',', ':')), file=sys.stderr)
    except Exception:
        pass  # A diagnostic write must never replace the operational failure.


def _cleanup(operation, stage):
    primary = sys.exc_info()[1]
    failed_stage = _STAGE
    try:
        operation()
    except Exception as error:
        if primary is None:
            _stage(stage)
            raise
        _emit_failure(error, stage=stage, secondary=True)
    finally:
        if primary is not None:
            _stage(failed_stage)


def need(ok, reason):
    if not ok:
        raise RuntimeError(reason)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(data):
    return base64.b64encode(data).decode('ascii')


def decoded(text):
    need(isinstance(text, str) and len(text) <= 2 * LIMIT, 'payload exceeds bound')
    data = base64.b64decode(text, validate=True)
    need(len(data) <= LIMIT, 'payload exceeds bound')
    return data


def write_new(path, data, uid=0, gid=0, mode=0o600):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        os.fchown(stream.fileno(), uid, gid)
        os.fchmod(stream.fileno(), mode)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def regular(path, uid, mode, gid=None):
    gid = uid if gid is None else gid
    m = path.lstat()
    need(stat.S_ISREG(m.st_mode) and m.st_uid == uid and m.st_gid == gid
         and stat.S_IMODE(m.st_mode) == mode and m.st_nlink == 1, 'regular file contract differs')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as stream:
        s = os.fstat(stream.fileno())
        need((s.st_dev, s.st_ino) == (m.st_dev, m.st_ino) and s.st_size <= LIMIT,
             'file changed or exceeds bound')
        data = stream.read(LIMIT + 1)
    need(len(data) <= LIMIT, 'file exceeds bound')
    return data


def run(args, data=None):
    def bounds():
        resource.setrlimit(resource.RLIMIT_FSIZE, (LIMIT, LIMIT))
    # Raw SQL diagnostics stay root-private, never in the host JSON response.
    fd, name = tempfile.mkstemp(prefix='logical-transfer-', dir=ADMIN)
    with os.fdopen(fd, 'wb') as errors, tempfile.TemporaryFile(dir=RUNTIME) as out:
        result = subprocess.run(args, input=data, stdout=out, stderr=errors, timeout=45,
                                env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C.UTF-8'},
                                preexec_fn=bounds)
        need(result.returncode == 0, 'native command failed; private diagnostics retained')
        need(os.fstat(out.fileno()).st_size < LIMIT, 'native result exceeds fixture bound')
        out.seek(0)
        data = out.read()
        if os.fstat(errors.fileno()).st_size == 0:
            actual = Path(name).lstat()
            held = os.fstat(errors.fileno())
            need((actual.st_dev, actual.st_ino) == (held.st_dev, held.st_ino),
                 'new successful diagnostic identity changed')
            Path(name).unlink()  # Only this call's successful empty private file.
        return data


@contextmanager
def client(app=False):
    profile = json.loads((HOME / 'etc/store.json').read_text())
    need(profile == {'engine': 'mariadb', 'mariadb': {'user': 'podmesh-node',
         'database': 'podmesh-node', 'socket': '/run/podmesh-node/mysql/mysql.sock',
         'password_file': '/apps/podmesh-node/etc/mysql/localhost/passwd'}}, 'profile differs')
    password = regular(HOME / ('etc/mysql/localhost/passwd' if app else 'etc/admin/passwd'),
                       1102 if app else 0, 0o600)
    need(re.fullmatch(b'[0-9a-f]{64}', password), 'credential format differs')
    fd, name = tempfile.mkstemp(prefix='transfer-client-', dir=RUNTIME)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(b'[client]\nprotocol=socket\nuser=' + (b'podmesh-node' if app else b'root')
                         + b'\nsocket=/run/podmesh-node/mysql/mysql.sock\npassword=' + password + b'\n')
        yield name
    finally:
        _cleanup(lambda: os.unlink(name), 'cleanup.client')


def sql(query, app=False):
    with client(app) as cnf:
        return run(['/usr/bin/mariadb', '--defaults-file=' + cnf, '--batch', '--raw',
                    '--skip-column-names', '--default-character-set=utf8mb4'], query.encode()).decode()


def dump(schema=False):
    with client() as cnf:
        return run(['/usr/bin/mariadb-dump', '--defaults-file=' + cnf, *FLAGS]
                   + (['--no-data'] if schema else []))


def app_process():
    found = []
    for p in Path('/proc').iterdir():
        if p.name.isdigit():
            try:
                fields = dict(line.split(':', 1) for line in (p / 'status').read_text().splitlines() if ':' in line)
                if (fields.get('Name', '').strip() == 'podmeshd'
                        and fields.get('Uid', '').split() == ['1102'] * 4
                        and fields.get('PPid', '').strip() == '1'):
                    # starttime is field22 after the final ')' in /proc/PID/stat.
                    found.append({'pid': int(p.name), 'start': (p / 'stat').read_text().rsplit(')', 1)[1].split()[19]})
            except OSError:
                pass
    need(len(found) == 1, 'unique owned APP required')
    return found[0]


def resume(token):
    lock = RUNTIME / 'logical-transfer-pause.json'
    if not lock.exists():
        return
    held = json.loads(regular(lock, 0, 0o600))
    need(held['token'] == token and held['app'] == app_process(), 'pause ownership changed')
    os.kill(held['app']['pid'], signal.SIGCONT)
    lock.unlink()


@contextmanager
def quiesced(token):
    app = app_process()
    state = Path('/proc', str(app['pid']), 'status').read_text()
    need(not re.search(r'^State:\s+[Tt]', state, re.M), 'APP already stopped by another owner')
    write_new(RUNTIME / 'logical-transfer-pause.json', json.dumps({'token': token, 'app': app}).encode())
    try:
        os.kill(app['pid'], signal.SIGSTOP)
        for _ in range(40):
            if re.search(r'^State:\s+T', Path('/proc', str(app['pid']), 'status').read_text(), re.M):
                break
            time.sleep(.025)
        else:
            raise RuntimeError('APP quiescence not observed')
        yield
    finally:
        _cleanup(lambda: resume(token), 'cleanup.resume')


def file_names(names):
    need(isinstance(names, list) and len(names) <= 16 and len(set(names)) == len(names), 'file list differs')
    need(all(isinstance(n, str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,127}', n)
             and n not in ('.', '..') for n in names), 'only explicit flat regular saved files supported')
    return sorted(names)


def bootstrap_directories():
    result = {}
    for name in BOOTSTRAP_DIRS:
        path = HOME / 'sav/app' / name
        m = path.lstat()
        mode = stat.S_IMODE(m.st_mode)
        need(stat.S_ISDIR(m.st_mode) and mode <= 0o777,
             'plain native bootstrap directory required; no symlink')
        need(not any(path.iterdir()), 'nonempty bootstrap directory outside flat-file contract')
        result[name] = {'type': 'directory', 'uid': m.st_uid, 'gid': m.st_gid,
                        'mode': mode, 'empty': True}
    return result


def files(names):
    names = file_names(names)
    folder = HOME / 'sav/app'
    need(not set(names) & set(BOOTSTRAP_DIRS), 'file/directory names overlap')
    need(sorted(p.name for p in folder.iterdir()) == sorted(names + list(BOOTSTRAP_DIRS)),
         'undeclared saved application state')
    bootstrap_directories()  # Check all four actual types, ordinary modes and empty contents.
    result = {}
    total = 0
    for n in names:
        m = (folder / n).lstat()
        mode = stat.S_IMODE(m.st_mode)
        need(mode <= 0o777, 'special file mode bits outside plain-file contract')
        data = regular(folder / n, m.st_uid, mode, m.st_gid)
        total += len(data)
        need(total <= LIMIT, 'saved files exceed bound')
        result[n] = {'data': encoded(data), 'uid': m.st_uid, 'gid': m.st_gid, 'mode': mode}
    return result


def identity():
    values = dict(line.split('\t') for line in sql("SELECT `key`,HEX(value) FROM `podmesh-node`.metadata WHERE `key` IN ('machine_id','host_uuid');").splitlines())
    need(set(values) == {'machine_id', 'host_uuid'}, 'identity missing')
    return {'host_uuid': bytes.fromhex(values['host_uuid']).decode('ascii'),
            'machine_id_sha256': digest(bytes.fromhex(values['machine_id']))}


def inventory():
    rows = sql("SELECT TABLE_NAME,TABLE_TYPE,COALESCE(ENGINE,'') FROM information_schema.TABLES WHERE TABLE_SCHEMA='podmesh-node' ORDER BY TABLE_NAME;").splitlines()
    policy = json.loads((LIB / 'grants.json').read_text())['tables']
    need(len(rows) == 40 and {r.split('\t')[0] for r in rows} == set(policy), 'current 40-table schema required')
    objects = sql("SELECT TABLE_NAME FROM information_schema.VIEWS WHERE TABLE_SCHEMA='podmesh-node' UNION ALL SELECT ROUTINE_NAME FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA='podmesh-node' UNION ALL SELECT TRIGGER_NAME FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA='podmesh-node' UNION ALL SELECT EVENT_NAME FROM information_schema.EVENTS WHERE EVENT_SCHEMA='podmesh-node';")
    need(not objects.strip(), 'views/routines/triggers/events outside bounded fixture contract')
    tables = {}
    filtered = {}
    for row in rows:
        name, kind, engine = row.split('\t')
        need(kind == 'BASE TABLE' and engine == 'InnoDB', 'InnoDB base tables required')
        quoted = '`' + name.replace('`', '``') + '`'
        columns = sql("SELECT COLUMN_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA='podmesh-node' AND TABLE_NAME=CONVERT(0x" + name.encode().hex() + " USING utf8mb4) ORDER BY ORDINAL_POSITION;").splitlines()
        need(int(sql('SELECT COUNT(*) FROM `podmesh-node`.' + quoted)) <= 1000, 'row bound exceeded')
        expr = ["IF(`" + c.replace('`', '``') + "` IS NULL,'N',CONCAT('H',HEX(CAST(`" + c.replace('`', '``') + "` AS BINARY))))" for c in columns]
        query = 'SELECT ' + ','.join(expr) + ' FROM `podmesh-node`.' + quoted
        def contents(suffix=''):
            data = sql(query + suffix + ';').splitlines()
            return {'rows': len(data), 'sha256': digest(json.dumps(sorted(data), separators=(',', ':')).encode())}
        tables[name] = {'columns': columns, **contents()}
        if name in ('metadata', 'observations'):
            filtered[name] = contents(" WHERE `key`<>'machine_id'" if name == 'metadata' else " WHERE operation<>'host_identity_adopt'")
    schema = re.sub(rb'AUTO_INCREMENT=\d+ ', b'', dump(True))
    audits = sql("SELECT HEX(result) FROM `podmesh-node`.observations WHERE operation='host_identity_adopt' ORDER BY id;").splitlines()
    return {'tables': tables, 'schema_sha256': digest(schema), 'objects': [], 'filtered': filtered,
            'identity': identity(), 'audits': [json.loads(bytes.fromhex(v)) for v in audits]}


def provider_machine_sha256():
    probe = """import json,socket,struct
with socket.socket(socket.AF_UNIX) as s:
 s.settimeout(8);s.connect('/run/podmesh-host/capability.sock')
 assert struct.unpack('3i',s.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))[1]==0
 s.sendall(b'{"protocol":"podmesh-host-capability/1","action":{"kind":"host_identity"},"intent":null}\\n')
 r=json.loads(s.makefile('rb').readline(65537));assert r['ok'];print(r['data']['machine_id'])
"""
    machine = run(['/usr/bin/setpriv', '--reuid=1102', '--regid=1102', '--clear-groups',
                   '--bounding-set=-all', '--inh-caps=-all', '--ambient-caps=-all',
                   '--no-new-privs', '/usr/bin/python3', '-I', '-c', probe]).strip()
    need(re.fullmatch(b'[0-9a-fA-F]{32}', machine), 'real provider identity differs')
    return digest(machine)


def api():
    # The kernel gives this short-lived reader the actual APP identity, with no capabilities.
    probe = """import json,socket
out={}
for op in ('identity','capabilities','observations'):
 with socket.socket(socket.AF_UNIX) as s:
  s.settimeout(5);s.connect('/run/podmesh-node/api/api.sock')
  s.sendall(json.dumps({'operation':op}).encode()+b'\\n')
  r=json.loads(s.makefile('rb').readline(65537));assert r['ok'];out[op]=r['data']
print(json.dumps(out))
"""
    return json.loads(run(['/usr/bin/setpriv', '--reuid=1102', '--regid=1102', '--clear-groups',
                          '--bounding-set=-all', '--inh-caps=-all', '--ambient-caps=-all',
                          '--no-new-privs', '/usr/bin/python3', '-I', '-c', probe]))


def main(request):
    need(os.getuid() == 0 and os.getgid() == 0, 'root administration only')
    mode = request['mode']
    if mode == 'resume':
        _stage('native.resume')
        resume(request['token'])
        return {'resumed': True}
    _stage('native.birth')
    birth = json.loads(regular(ADMIN / 'birth.json', 0, 0o600))
    need(birth['phase'] == 'running', 'truthful running canonical birth required')
    need(birth['provenance']['sha256']['entrypoint.py'] == ADOPTION_ENTRYPOINT_SHA256,
         'image must embed the pinned adoption entrypoint')
    _stage('native.artifacts')
    for name, expected in birth['provenance']['sha256'].items():
        need(name in ('podmeshd', 'podmesh-store-admin', 'entrypoint.py', 'grants.json')
             and digest((LIB / name).read_bytes()) == expected, 'actual immutable artifact differs')
    if mode == 'binding':
        _stage('native.binding')
        observed = identity()
        actual = provider_machine_sha256()
        need(observed['machine_id_sha256'] == actual, 'NODE binding differs from real host provider')
        return {'birth': birth, 'identity': observed, 'provider_machine_sha256': actual, 'api': api()}
    _stage('native.quiesce')
    with quiesced(request['token']):
        names = file_names(request['files'])
        if mode == 'export':
            _stage('export.inventory.before')
            before = inventory()
            _stage('export.provider')
            need(before['identity']['machine_id_sha256'] == provider_machine_sha256(),
                 'source binding differs from real host provider')
            _stage('export.dump')
            payload = dump()
            need(payload.strip(), 'native SQL dump is empty')
            _stage('export.files')
            saved = files(names)
            _stage('export.inventory.after')
            need(inventory() == before, 'source changed during export')
            return {'inventory': before, 'sql': encoded(payload), 'files': saved,
                    'directories': bootstrap_directories(), 'birth': birth}
        if mode == 'readback':
            _stage('readback.inventory')
            return {'inventory': inventory(), 'files': files(names), 'directories': bootstrap_directories()}
        need(mode == 'restore', 'unknown native operation')
        _stage('restore.empty-target')
        need(files([]) == {} and bootstrap_directories() == request['directories'],
             'fresh target bootstrap directory metadata differs or saved files already exist')
        need(identity() == request['fresh_identity'], 'fresh target identity changed')
        payload = decoded(request['sql'])
        # The owning host command has created this target in this invocation only.
        _stage('restore.import')
        sql('DROP DATABASE `podmesh-node`;')
        need(sql("SELECT COUNT(*) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME='podmesh-node';").strip() == '0', 'application database must be absent before import')
        with client() as cnf:
            run(['/usr/bin/mariadb', '--defaults-file=' + cnf, '--default-character-set=utf8mb4'], payload)
        _stage('restore.files')
        need(set(request['saved']) == set(names), 'saved file inventory differs')
        for name in names:
            item = request['saved'][name]
            need(set(item) == {'data', 'uid', 'gid', 'mode'}
                 and all(type(item[k]) is int and 0 <= item[k] < 4294967295 for k in ('uid', 'gid'))
                 and type(item['mode']) is int and 0 <= item['mode'] <= 0o777,
                 'plain regular file ownership/mode required')
            write_new(HOME / 'sav/app' / name, decoded(item['data']), item['uid'], item['gid'], item['mode'])
        _stage('restore.inventory')
        observed = inventory()
        need(observed == request['inventory'] and files(names) == request['saved']
             and bootstrap_directories() == request['directories'], 'restored SQL/files/directories differ')
        write_new(ADMIN / 'restore.sql', payload)
        return {'compared': True, 'inventory': observed}


if __name__ == '__main__':
    def interrupted(signum, frame):
        raise RuntimeError('administration interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    signal.signal(signal.SIGALRM, interrupted)
    signal.alarm(360)
    try:
        _stage('entrypoint.limit')
        need(len(sys.argv) == 2 and sys.argv[1].isdigit(), 'explicit payload limit required')
        LIMIT = int(sys.argv[1])
        need(LIMIT > 0, 'positive explicit payload limit required')
        _stage('entrypoint.request')
        raw = sys.stdin.buffer.read(4 * LIMIT + 1)
        need(len(raw) <= 4 * LIMIT, 'request exceeds bound')
        print(json.dumps(main(json.loads(raw)), separators=(',', ':')))
    except Exception as error:
        print('logical transfer native step failed; retained private diagnostics', file=sys.stderr)
        _emit_failure(error)
        raise SystemExit(1)
