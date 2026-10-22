#!/usr/bin/python3
"""Finite PID1 for a NEW canonical NODE; source candidate, not a migration tool."""
import grp
import hashlib
import json
import os
from pathlib import Path
import pwd
import secrets
import signal
import socket
import stat
import struct
import subprocess
import time
import tempfile

HOME = Path('/apps/podmesh-node')
ETC = HOME / 'etc'
SAV = HOME / 'sav'
DATA = SAV / 'mariadb'
STATE = SAV / 'admin'
RUN = Path('/run/podmesh-node')
ADMIN = RUN / 'admin'
DBSOCK = RUN / 'mysql' / 'mysql.sock'
ALIAS = ADMIN / 'mysql.sock'
API = RUN / 'api' / 'api.sock'
PASS = ETC / 'mysql' / 'localhost' / 'passwd'
ROOTPASS = ETC / 'admin' / 'passwd'
PROFILE = ETC / 'store.json'
LIB = Path('/usr/lib/podmesh-node-canonical')
APP_UID = 1102
STOP_APP = 10
STOP_DB = 20


class Refusal(Exception):
    pass


def require(ok, reason):
    if not ok:
        raise Refusal(reason)


def protected(path, uid=0, mode=None, kind='dir'):
    """No symlink leaf or replaceable ancestor; never repair an existing object."""
    path = Path(path)
    for parent in reversed(path.parents):
        m = parent.lstat()
        require(stat.S_ISDIR(m.st_mode) and m.st_uid == 0 and not m.st_mode & 0o022,
                'replaceable path ancestor')
    m = path.lstat()
    valid = stat.S_ISDIR(m.st_mode) if kind == 'dir' else stat.S_ISREG(m.st_mode)
    require(valid and m.st_uid == uid, 'unexpected path type or owner')
    require(mode is None or stat.S_IMODE(m.st_mode) == mode, 'unexpected path mode')
    require(not m.st_mode & 0o022, 'writable protected object')
    if kind == 'file':
        require(m.st_nlink == 1, 'credential/config hardlink refused')
    return m


def owned_directory(path, uid, gid, mode):
    path = Path(path)
    if not path.exists() and not path.is_symlink():
        path.mkdir(mode=mode)
        os.chown(path, uid, gid)
        path.chmod(mode)
    # Only its ancestors (not app-owned leaf) must be root nonreplaceable.
    m = protected(path, uid, mode)
    require(m.st_gid == gid, 'directory group differs')


def read_private(path, uid=0):
    before = protected(path, uid, 0o600, 'file')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        after = os.fstat(fd)
        require((before.st_dev, before.st_ino) == (after.st_dev, after.st_ino),
                'credential replaced')
        data = os.read(fd, 4097)
    finally:
        os.close(fd)
    require(0 < len(data) <= 4096 and data == data.strip() and b'\n' not in data,
            'credential must be a private single line')
    # Locally generated hex only: no SQL/option-file escaping ambiguity.
    require(len(data) == 64 and all(c in b'0123456789abcdef' for c in data),
            'credential format is not this birth recipe')
    return data.decode('ascii')


def exclusive_file(path, value, uid=0, gid=0, mode=0o600):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    try:
        os.fchown(fd, uid, gid)
        os.fchmod(fd, mode)
        os.write(fd, value.encode())
        os.fsync(fd)
    finally:
        os.close(fd)


def durable_state(document):
    path = STATE / 'birth.json'
    if path.exists():
        protected(path, 0, 0o600, 'file')
    temporary = STATE / 'birth.pending'
    exclusive_file(temporary, json.dumps(document, sort_keys=True) + '\n')
    os.replace(temporary, path)
    fd = os.open(STATE, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def drop_command(uid, gid, command):
    return ['/usr/bin/setpriv', '--reuid=' + str(uid), '--regid=' + str(gid),
            '--clear-groups', '--bounding-set=-all', '--inh-caps=-all',
            '--ambient-caps=-all', '--no-new-privs', *command]


def clean_env():
    # No caller DSN, MYSQL_PWD, loader hooks, Python paths or fallback config.
    return {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C.UTF-8', 'TZ': 'UTC',
            'HOME': str(HOME), 'PYTHONDONTWRITEBYTECODE': '1'}


def app_env():
    return dict(clean_env(), PODMESH_STORE_PROFILE=str(PROFILE),
                PODMESH_STATE_DIR=str(SAV / 'app'), PODMESH_SOCKET=str(API),
                PODMESH_HOST_ADAPTER_SOCKET='/run/podmesh-host/capability.sock',
                TMPDIR=str(RUN / 'app-tmp'))


def no_tcp_listeners():
    for name in ('tcp', 'tcp6'):
        path = Path('/proc/net') / name
        if path.exists():
            require(all(row.split()[3] != '0A' for row in path.read_text().splitlines()[1:]),
                    'TCP listener in private unit namespace')


def adapter_precondition():
    root = Path('/run/podmesh-host')
    m = protected(root, 0, 0o750)
    require(m.st_gid == APP_UID and os.statvfs(root).f_flag & os.ST_RDONLY,
            'host capability directory must be a dedicated read-only mount')
    endpoint = root / 'capability.sock'
    m = endpoint.lstat()
    require(stat.S_ISSOCK(m.st_mode) and m.st_uid == 0 and m.st_gid == APP_UID
            and stat.S_IMODE(m.st_mode) == 0o660, 'host capability endpoint differs')
    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(2)
        peer.connect(str(endpoint))
        require(struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))[1] == 0,
                'host adapter is not root')


class Supervisor:
    def __init__(self):
        self.children = {}
        self.stop_requested = False
        self.failed = False
        self.record = None
        self.diagnostic = None

    def spawn(self, name, args, env=None, stdin=None, capture=False):
        require(not self.stop_requested, 'startup interrupted before child launch')
        require(name not in self.children, 'child name already owned')
        capture_file = tempfile.TemporaryFile(dir=STATE) if capture else None
        child = subprocess.Popen(args, env=env or clean_env(), start_new_session=True,
                                 stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                                 stdout=capture_file if capture else self.diagnostic,
                                 stderr=subprocess.DEVNULL if capture else self.diagnostic)
        child.capture_file = capture_file
        self.children[name] = child
        if stdin is not None:
            try:
                child.stdin.write(stdin.encode())
                child.stdin.close()
            except BrokenPipeError:
                pass
        return child

    def peek(self, child):
        # WNOWAIT pins the leader PID/session against reuse until group cleanup.
        event = os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        if event is None:
            return None
        return event.si_status if event.si_code == os.CLD_EXITED else -event.si_status

    def run(self, name, args, timeout, stdin=None, env=None):
        child = self.spawn(name, args, stdin=stdin, env=env, capture=True)
        deadline = time.monotonic() + timeout
        while self.peek(child) is None:
            if self.stop_requested or time.monotonic() >= deadline:
                self.stop_child(name, 2)
                raise Refusal('administrative child interrupted or timed out')
            # Do not buffer arbitrarily large output: commands here emit bounded metadata.
            time.sleep(0.05)
        if any(pid != child.pid for pid, _ in self.group_members(child.pid)):
            self.failed = True
            self.stop_child(name, 2)
            raise Refusal('administrative child left descendants')
        # No further group signals after this reap: the session has drained.
        child.wait(timeout=2)
        child.capture_file.seek(0)
        output = child.capture_file.read(65537)
        child.capture_file.close()
        del self.children[name]
        require(len(output) <= 65536 and child.returncode == 0,
                'administrative child refused; private diagnostic retained')
        return output.decode()

    def group_members(self, group):
        members = []
        for item in Path('/proc').iterdir():
            if not item.name.isdecimal():
                continue
            try:
                fields = (item / 'stat').read_text().rpartition(') ')[2].split()
                # /proc stat suffix: state, ppid, pgrp, session, ...
                if int(fields[2]) == group and int(fields[3]) == group:
                    members.append((int(item.name), int(fields[1])))
            except (OSError, ValueError, IndexError):
                continue
        return members

    def stop_child(self, name, seconds):
        child = self.children.get(name)
        if child is None:
            return True
        # Never reap the session leader until all group signaling has ended.
        # WNOWAIT retains its PID even when it already died unexpectedly.
        def descendants():
            members = self.group_members(child.pid)
            tracked = {p.pid for p in self.children.values()}
            for pid, parent in members:
                if parent == os.getpid() and pid not in tracked:
                    try:
                        os.waitpid(pid, os.WNOHANG)
                    except ChildProcessError:
                        pass
            return [(pid, parent) for pid, parent in self.group_members(child.pid) if pid != child.pid]
        def signal_group(sig):
            try:
                os.killpg(child.pid, sig)
            except ProcessLookupError:
                pass
        running = self.peek(child) is None
        if running or descendants():
            signal_group(signal.SIGTERM)
        deadline = time.monotonic() + seconds
        forced = False
        while self.peek(child) is None or descendants():
            if time.monotonic() >= deadline:
                forced = True
                self.failed = True
                signal_group(signal.SIGKILL)
                kill_deadline = time.monotonic() + 2
                while (self.peek(child) is None or descendants()) and time.monotonic() < kill_deadline:
                    time.sleep(0.02)
                break
            time.sleep(0.02)
        drained = self.peek(child) is not None and not descendants()
        if drained:
            child.wait(timeout=2)  # No killpg is permitted after this point.
        if getattr(child, 'capture_file', None) is not None:
            child.capture_file.close()
        good = drained and not forced and child.returncode == 0
        self.failed |= not good
        if drained:
            del self.children[name]
        return good

    def stop(self):
        # Application always stops before its private database; helpers cannot outlive DB.
        self.stop_child('app', STOP_APP)
        for name in list(self.children):
            if name != 'db':
                self.stop_child(name, 2)
        self.stop_child('db', STOP_DB)
        if self.record is not None:
            self.record['phase'] = 'failed' if self.failed else 'stopped-clean'
            self.record['coherent_backup'] = False  # A stop is never a backup receipt.
            durable_state(self.record)

    def sql(self, statement):
        return self.run('sql', ['/usr/bin/mariadb', '--defaults-file=' + str(ADMIN / 'client.cnf'),
                               '--batch', '--raw', '--skip-column-names'], 5, statement)

    def layout(self):
        require(os.geteuid() == 0 and os.getpid() == 1, 'entrypoint requires root PID1')
        require(pwd.getpwuid(APP_UID).pw_name == 'podmesh-node'
                and grp.getgrgid(APP_UID).gr_name == 'podmesh-node', 'functional identity differs')
        self.mysql = pwd.getpwnam('mysql')
        require(self.mysql.pw_uid not in (0, APP_UID), 'mysql identity must be separate')
        for path in (Path('/apps'), HOME, ETC, SAV):
            protected(path, 0, 0o755)
        require(os.path.ismount(ETC) and os.path.ismount(SAV), 'etc and sav must be distinct declared mounts')
        require((ETC.stat().st_dev, ETC.stat().st_ino) != (SAV.stat().st_dev, SAV.stat().st_ino),
                'configuration and saved state cannot alias')
        for name, uid, gid, mode in (
                (SAV / 'app', APP_UID, APP_UID, 0o700),
                (DATA, self.mysql.pw_uid, self.mysql.pw_gid, 0o700),
                (STATE, 0, 0, 0o700), (ETC / 'admin', 0, 0, 0o700),
                (ETC / 'mysql', 0, 0, 0o755), (ETC / 'mysql' / 'localhost', 0, 0, 0o755)):
            owned_directory(name, uid, gid, mode)
        # Runtime is fresh per container lifecycle; stale sockets/markers refuse, never unlink.
        require(not RUN.exists() and not RUN.is_symlink(), 'stale or pre-existing runtime directory')
        owned_directory(RUN, 0, 0, 0o755)
        for name, uid, gid, mode in (
                (ADMIN, 0, 0, 0o700), (RUN / 'mysql', self.mysql.pw_uid, APP_UID, 0o750),
                (RUN / 'api', APP_UID, APP_UID, 0o700), (RUN / 'app-tmp', APP_UID, APP_UID, 0o700),
                (RUN / 'db-tmp', self.mysql.pw_uid, self.mysql.pw_gid, 0o700)):
            owned_directory(name, uid, gid, mode)
        adapter_precondition()
        for forbidden in ('/run/podman/podman.sock', '/var/run/docker.sock'):
            require(not Path(forbidden).exists(), 'container engine socket exposure refused')
        protected(LIB, 0, 0o755)
        protected(LIB / 'provenance.json', 0, 0o444, 'file')
        provenance = json.loads((LIB / 'provenance.json').read_text())
        for name, digest in provenance['sha256'].items():
            require(name in ('podmeshd', 'podmesh-store-admin', 'entrypoint.py', 'grants.json'),
                    'unknown provenance member')
            protected(LIB / name, 0, 0o555 if name != 'grants.json' else 0o444, 'file')
            require(hashlib.sha256((LIB / name).read_bytes()).hexdigest() == digest,
                    'immutable artifact hash differs')
        require(set(provenance['sha256']) == {'podmeshd', 'podmesh-store-admin', 'entrypoint.py', 'grants.json'},
                'incomplete immutable artifact provenance')
        policy = json.loads((LIB / 'grants.json').read_text())
        self.grants = policy['tables']
        self.provenance = provenance
        self.diagnostic = open(STATE / ('diagnostic-' + secrets.token_hex(8)), 'xb', buffering=0)
        os.chmod(self.diagnostic.fileno(), 0o600)

    def birth(self):
        require(not self.stop_requested, 'startup interrupted')
        marker = STATE / 'birth.json'
        fresh = not marker.exists()
        if fresh:
            require(not marker.is_symlink() and not any(DATA.iterdir()) and not any((SAV / 'app').iterdir()),
                    'unrecorded existing database/application state; no automatic adoption')
            require(not PASS.exists() and not ROOTPASS.exists() and not PROFILE.exists(),
                    'unrecorded credentials/profile; no reset')
            self.record = {'phase': 'initializing', 'datadir': [DATA.stat().st_dev, DATA.stat().st_ino],
                           'provenance': self.provenance}
            durable_state(self.record)
            exclusive_file(PASS, secrets.token_hex(32), APP_UID, APP_UID)
            exclusive_file(ROOTPASS, secrets.token_hex(32))
            profile = {'engine': 'mariadb', 'mariadb': {'user': 'podmesh-node', 'database': 'podmesh-node',
                        'socket': str(DBSOCK), 'password_file': str(PASS)}}
            exclusive_file(PROFILE, json.dumps(profile), 0, APP_UID, 0o440)
        else:
            protected(marker, 0, 0o600, 'file')
            self.record = json.loads(marker.read_text())
            require(self.record.get('phase') == 'stopped-clean', 'partial/unclean birth requires separate recovery authority')
            require(self.record.get('provenance') == self.provenance, 'new artifact needs explicit recovery/upgrade procedure')
            require(self.record.get('datadir') == [DATA.stat().st_dev, DATA.stat().st_ino]
                    and (DATA / 'mysql').is_dir(), 'database identity or initialization differs')
        app_password = read_private(PASS, APP_UID)
        root_password = read_private(ROOTPASS)
        require(app_password != root_password, 'application and administrator credentials must differ')
        protected(PROFILE, 0, 0o440, 'file')
        fingerprint = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (PASS, ROOTPASS, PROFILE)}
        if not fresh:
            require(self.record.get('configuration') == fingerprint, 'existing credential/profile changed')
        self.record['configuration'] = fingerprint
        self.record['phase'] = 'starting'
        durable_state(self.record)
        exclusive_file(ADMIN / 'client.cnf', '[client]\nprotocol=socket\nuser=root\nsocket=' + str(DBSOCK)
                       + '\npassword=' + root_password + '\nconnect-timeout=2\n')
        if fresh:
            self.run('initialize', drop_command(self.mysql.pw_uid, self.mysql.pw_gid,
                     ['/usr/bin/mariadb-install-db', '--no-defaults', '--datadir=' + str(DATA),
                      '--auth-root-authentication-method=socket', '--auth-root-socket-user=root', '--skip-test-db']), 60,
                     env=dict(clean_env(), TMPDIR=str(RUN / 'db-tmp')))
        self.spawn('db', drop_command(self.mysql.pw_uid, self.mysql.pw_gid,
                   ['/usr/sbin/mariadbd', '--no-defaults', '--datadir=' + str(DATA), '--socket=' + str(DBSOCK),
                    '--pid-file=' + str(RUN / 'mysql' / 'mysqld.pid'), '--tmpdir=' + str(RUN / 'db-tmp'),
                    '--skip-networking', '--skip-name-resolve',
                    '--general-log=0', '--slow-query-log=0', '--skip-log-bin']),
                   env=dict(clean_env(), TMPDIR=str(RUN / 'db-tmp')))
        deadline = time.monotonic() + 30
        while True:
            require(not self.stop_requested and self.peek(self.children['db']) is None, 'database died or startup interrupted')
            try:
                require(self.sql('SELECT 1;\n').strip() == '1', 'database readiness response differs')
                break
            except (Refusal, FileNotFoundError):
                require(not self.failed, 'unclean database readiness helper')
                require(time.monotonic() < deadline, 'database readiness timeout')
                time.sleep(0.2)
        endpoint = DBSOCK.lstat()
        require(stat.S_ISSOCK(endpoint.st_mode) and endpoint.st_uid == self.mysql.pw_uid,
                'database socket owner/type differs')
        with socket.socket(socket.AF_UNIX) as peer:
            peer.connect(str(DBSOCK))
            observed = struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            require(observed[:2] == (self.children['db'].pid, self.mysql.pw_uid), 'database socket is a foreign server')
        os.chown(DBSOCK, self.mysql.pw_uid, APP_UID)
        os.chmod(DBSOCK, 0o660)
        # Same inode, private root0700 alias, no symlink followed by schema-admin.
        os.link(DBSOCK, ALIAS, follow_symlinks=False)
        require(ALIAS.stat().st_ino == DBSOCK.stat().st_ino, 'private admin alias differs')
        no_tcp_listeners()
        if fresh:
            initial_accounts = {tuple(line.split('\t')) for line in self.sql('SELECT User,Host FROM mysql.global_priv;').splitlines()}
            require(('root', 'localhost') in initial_accounts and initial_accounts <= {('root', 'localhost'), ('mysql', 'localhost'), ('mariadb.sys', 'localhost')},
                    'unexpected initial MariaDB account set')
            self.record['system_accounts'] = sorted(initial_accounts)
            self.sql("ALTER USER 'root'@'localhost' IDENTIFIED VIA unix_socket OR mysql_native_password USING PASSWORD('"
                     + root_password + "');\nCREATE DATABASE `podmesh-node` CHARACTER SET utf8mb4;\n"
                     + "CREATE USER 'podmesh-node'@'localhost' IDENTIFIED BY '" + app_password + "';\n")
        result = json.loads(self.run('schema', [str(LIB / 'podmesh-store-admin'), 'prepare', '--role', 'node',
                                      '--profile', str(PROFILE), '--admin-socket', str(ALIAS),
                                      '--admin-password-file', str(ROOTPASS)], 30))
        require(result.get('status') == ('prepared_empty_schema' if fresh else 'already_current_no_ddl'),
                'schema-admin result is not the selected fresh/restart contract')
        if fresh:
            statements = []
            for table, privileges in self.grants.items():
                require(table.replace('_', '').isalnum() and set(privileges) <= {'SELECT', 'INSERT', 'UPDATE', 'DELETE'},
                        'grant manifest is not table-scoped DML')
                statements.append('GRANT ' + ','.join(privileges) + ' ON `podmesh-node`.`' + table
                                  + "` TO 'podmesh-node'@'localhost';")
            self.sql('\n'.join(statements))
        self.verify_grants()
        protected(ADMIN / 'client.cnf', 0, 0o600, 'file')
        (ADMIN / 'client.cnf').unlink()  # Boot-only duplicate credential, not original root password.
        self.start_application()

    def verify_grants(self):
        accounts = {tuple(line.split('\t')) for line in self.sql('SELECT User,Host FROM mysql.global_priv;').splitlines()}
        expected_accounts = {tuple(pair) for pair in self.record['system_accounts']} | {('podmesh-node', 'localhost')}
        require(accounts == expected_accounts, 'unexpected database account or missing identity')
        for view in ('USER_PRIVILEGES', 'SCHEMA_PRIVILEGES', 'TABLE_PRIVILEGES', 'COLUMN_PRIVILEGES', 'ROUTINE_PRIVILEGES'):
            public = self.sql('SELECT PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.' + view + " WHERE UPPER(GRANTEE) LIKE '%PUBLIC%';")
            require(not public.strip() or (view == 'USER_PRIVILEGES' and all(row == 'USAGE\tNO' for row in public.splitlines())),
                    'PUBLIC privileges widen functional authority')
        require(not self.sql("SELECT Role FROM mysql.roles_mapping WHERE User='PUBLIC';").strip(),
                'PUBLIC inherited role refused')
        who = "CONVERT(0x" + "'podmesh-node'@'localhost'".encode().hex() + " USING utf8mb4)"
        global_rows = self.sql('SELECT PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.USER_PRIVILEGES WHERE GRANTEE=' + who + ';')
        require(all(line == 'USAGE\tNO' for line in global_rows.splitlines()), 'functional global privileges refused')
        require(not self.sql('SELECT PRIVILEGE_TYPE FROM information_schema.SCHEMA_PRIVILEGES WHERE GRANTEE=' + who + ';').strip(),
                'functional schema-wide privileges refused')
        rows = self.sql('SELECT TABLE_SCHEMA,TABLE_NAME,PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.TABLE_PRIVILEGES WHERE GRANTEE=' + who + ';')
        actual = {tuple(line.split('\t')) for line in rows.splitlines()}
        expected = {('podmesh-node', t, privilege, 'NO') for t, privileges in self.grants.items() for privilege in privileges}
        require(actual == expected, 'functional table grants differ; no automatic repair on restart')
        for view in ('COLUMN_PRIVILEGES', 'ROUTINE_PRIVILEGES'):
            require(not self.sql('SELECT PRIVILEGE_TYPE FROM information_schema.' + view + ' WHERE GRANTEE=' + who + ';').strip(),
                    'functional column/routine privileges refused')
        require(not self.sql("SELECT Role FROM mysql.roles_mapping WHERE User='podmesh-node' AND Host='localhost';").strip(),
                'functional inherited roles refused')

    def start_application(self):
        self.spawn('app', drop_command(APP_UID, APP_UID, [str(LIB / 'podmeshd')]), env=app_env())
        deadline = time.monotonic() + 30
        probe = """import os,socket,struct,json,sys
assert os.getuid()==os.getgid()==1102 and not os.getgroups()
for row in open('/proc/self/status'):
 if row.split(':')[0] in ('CapInh','CapPrm','CapEff','CapBnd','CapAmb'): assert int(row.split()[1],16)==0
s=socket.socket(socket.AF_UNIX);s.settimeout(2);s.connect(sys.argv[1])
p=struct.unpack('3i',s.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12));assert p[:2]==(int(sys.argv[2]),1102)
s.sendall(b'{"operation":"capabilities"}\\n');r=json.loads(s.makefile('rb').readline(65537));assert r['ok'] and r['data']['store_engine']=='mariadb'
print('ready')
"""
        while True:
            require(not self.stop_requested and all(self.peek(self.children[x]) is None for x in ('app', 'db')),
                    'owned app/database died before readiness')
            try:
                answer = self.run('readiness', drop_command(APP_UID, APP_UID,
                                  ['/usr/bin/python3', '-I', '-c', probe, str(API), str(self.children['app'].pid)]), 3)
                require(answer.strip() == 'ready', 'functional readiness response differs')
                break
            except Refusal:
                require(not self.failed, 'unclean functional readiness helper')
                require(time.monotonic() < deadline, 'functional app readiness timeout')
                time.sleep(0.2)
        no_tcp_listeners()
        self.record['phase'] = 'running'
        durable_state(self.record)

    def reap_owned_orphans(self):
        tracked = {p.pid for p in self.children.values()}
        for child in self.children.values():
            for pid, parent in self.group_members(child.pid):
                if parent == os.getpid() and pid not in tracked:
                    try:
                        os.waitpid(pid, os.WNOHANG)
                    except ChildProcessError:
                        pass

    def supervise(self):
        while not self.stop_requested:
            self.reap_owned_orphans()
            no_tcp_listeners()
            require(all(self.peek(self.children[x]) is None for x in ('app', 'db')),
                    'unexpected owned child death')
            time.sleep(0.1)


def main():
    os.umask(0o077)
    supervisor = Supervisor()
    def signal_stop(_number, _frame):
        supervisor.stop_requested = True
    signal.signal(signal.SIGTERM, signal_stop)
    signal.signal(signal.SIGINT, signal_stop)
    try:
        supervisor.layout()
        supervisor.birth()
        supervisor.supervise()
    except Exception:
        supervisor.failed = True
        # Never render administrative SQL, credentials, environment or child stderr.
        print('canonical NODE refused; preserve private state and diagnostics', flush=True)
    finally:
        try:
            supervisor.stop()
        except Exception:
            supervisor.failed = True
    return 1 if supervisor.failed else 0


if __name__ == '__main__':
    raise SystemExit(main())
