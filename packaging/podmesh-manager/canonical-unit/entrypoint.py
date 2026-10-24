#!/usr/bin/python3
"""One MANAGER replica + private socket-only MariaDB. New instances only."""
import hashlib
import importlib.util
import ipaddress
import json
import os
from pathlib import Path
import pwd
import grp
import secrets
import signal
import socket
import stat
import struct
import time

# Reuse the existing owned-process supervision, output draining and stop ordering.
# The image copies the exact NODE source as this immutable sibling. No NODE runs.
_spec = importlib.util.spec_from_file_location('manager_lifecycle', Path(__file__).with_name('lifecycle.py'))
u = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(u)
HOME = Path('/apps/podmesh-manager')
ETC, SAV = HOME / 'etc', HOME / 'sav'
DATA, STATE = SAV / 'mariadb', SAV / 'admin'
RUN = Path('/run/podmesh-manager')
ADMIN, DBSOCK = RUN / 'admin', RUN / 'mysql' / 'mysql.sock'
ALIAS, API = ADMIN / 'mysql.sock', RUN / 'api' / 'control.sock'
PASS, ROOTPASS = ETC / 'mysql' / 'localhost' / 'passwd', ETC / 'admin' / 'passwd'
PROFILE, CONFIG = ETC / 'store.json', ETC / 'config.json'
LIB = Path('/usr/lib/podmesh-manager-canonical')
APP_UID = 1103
# Each entrypoint is its own process/module instance. These fixed bindings only
# redirect shared helpers to this role's declared paths; no caller input is used.
for _name in ('HOME', 'ETC', 'SAV', 'DATA', 'STATE', 'RUN', 'ADMIN', 'DBSOCK',
              'ALIAS', 'API', 'PASS', 'ROOTPASS', 'PROFILE', 'LIB', 'APP_UID'):
    setattr(u, _name, globals()[_name])
# Preserve the existing MANAGER delivery's 30-second graceful shutdown window.
u.STOP_APP = 30
require, protected = u.require, u.protected
owned_directory, exclusive_file = u.owned_directory, u.exclusive_file
read_private, durable_state = u.read_private, u.durable_state
drop_command, clean_env = u.drop_command, u.clean_env
Refusal = u.Refusal


def app_env():
    return dict(clean_env(), PODMESH_STORE_PROFILE=str(PROFILE),
                PODMESH_MANAGER_NETWORK_MODE='authenticated-static-peers',
                TMPDIR=str(RUN / 'app-tmp'))


def configuration():
    m = protected(CONFIG, 0, 0o440, 'file')
    require(m.st_gid == APP_UID and m.st_size <= 1048576, 'manager config access/size differs')
    raw = CONFIG.read_bytes()
    config = json.loads(raw)
    network = config['network']
    require(config['observation_writer_uid'] == APP_UID and config.get('votes') is None,
            'manager functional UID differs or vote authority requested')
    require(config['control_socket'] == str(API)
            and network['database_path'] == str(SAV / 'app' / 'manager.sqlite'),
            'manager state/control paths differ')
    host, port = network['bind'].rsplit(':', 1)
    ipaddress.IPv4Address(host)
    require(port.isdecimal() and 1024 <= int(port) <= 65535 and int(port) != 3306,
            'manager peer listener requires its own unprivileged port')
    # All UUIDs, topology, pair keys, scopes and timing fields are the real
    # daemon's configuration, never generated/replaced by this entrypoint.
    return config, hashlib.sha256(raw).hexdigest()


class Supervisor(u.Supervisor):
    def layout(self):
        require(os.geteuid() == 0 and os.getpid() == 1, 'entrypoint requires root PID1')
        require(pwd.getpwuid(APP_UID).pw_name == 'podmesh-manager'
                and grp.getgrgid(APP_UID).gr_name == 'podmesh-manager', 'functional identity differs')
        self.mysql = pwd.getpwnam('mysql')
        require(self.mysql.pw_uid not in (0, APP_UID), 'mysql identity must be separate')
        for path in (Path('/apps'), HOME, ETC, SAV):
            protected(path, 0, 0o755)
        require(os.path.ismount(ETC) and os.path.ismount(SAV), 'declare persistent etc and sav mounts')
        require((ETC.stat().st_dev, ETC.stat().st_ino) != (SAV.stat().st_dev, SAV.stat().st_ino),
                'configuration and saved state cannot alias')
        for path, uid, gid, mode in (
                (SAV / 'app', APP_UID, APP_UID, 0o700),
                (DATA, self.mysql.pw_uid, self.mysql.pw_gid, 0o700),
                (STATE, 0, 0, 0o700), (ETC / 'admin', 0, 0, 0o700),
                (ETC / 'mysql', 0, 0, 0o755), (ETC / 'mysql' / 'localhost', 0, 0, 0o755)):
            owned_directory(path, uid, gid, mode)
        require(not RUN.exists() and not RUN.is_symlink(), 'fresh runtime required')
        owned_directory(RUN, 0, 0, 0o755)
        for path, uid, gid, mode in (
                (ADMIN, 0, 0, 0o700), (RUN / 'mysql', self.mysql.pw_uid, APP_UID, 0o750),
                (RUN / 'api', APP_UID, APP_UID, 0o700), (RUN / 'app-tmp', APP_UID, APP_UID, 0o700),
                (RUN / 'db-tmp', self.mysql.pw_uid, self.mysql.pw_gid, 0o700)):
            owned_directory(path, uid, gid, mode)
        self.config, self.config_sha256 = configuration()
        marker = STATE / 'birth.json'
        if marker.exists():
            protected(marker, 0, 0o600, 'file')
            require(json.loads(marker.read_text()).get('manager_config_sha256') == self.config_sha256,
                    'existing manager identity/config changed; explicit upgrade/recovery required')
        protected(LIB, 0, 0o755)
        protected(LIB / 'provenance.json', 0, 0o444, 'file')
        self.provenance = json.loads((LIB / 'provenance.json').read_text())
        members = {'podmesh-managerd', 'podmesh-store-admin', 'entrypoint.py', 'lifecycle.py', 'grants.json'}
        require(set(self.provenance['sha256']) == members, 'incomplete manager artifact provenance')
        for name, digest in self.provenance['sha256'].items():
            protected(LIB / name, 0, 0o444 if name == 'grants.json' else 0o555, 'file')
            require(hashlib.sha256((LIB / name).read_bytes()).hexdigest() == digest, 'artifact bytes differ')
        self.grants = json.loads((LIB / 'grants.json').read_text())['tables']
        self.diagnostic = open(STATE / ('diagnostic-' + secrets.token_hex(8)), 'xb', buffering=0)
        os.chmod(self.diagnostic.fileno(), 0o600)
        # Real daemon parser validates the complete unchanged topology, no DB open.
        self.run('validate-config', drop_command(APP_UID, APP_UID, self.application_argv(validate=True)),
                 5, env=app_env())

    def application_argv(self, validate=False):
        command = [str(LIB / 'podmesh-managerd'), '--config', str(CONFIG),
                   '--state-dir', str(SAV / 'app'), '--runtime-dir', str(RUN / 'api')]
        return command + (['--validate-config'] if validate else [])

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
                           'provenance': self.provenance, 'manager_config_sha256': self.config_sha256}
            durable_state(self.record)
            exclusive_file(PASS, secrets.token_hex(32), APP_UID, APP_UID)
            exclusive_file(ROOTPASS, secrets.token_hex(32))
            profile = {'engine': 'mariadb', 'mariadb': {'user': 'podmesh-manager', 'database': 'podmesh-manager',
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
            self.pump()
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
        u.no_tcp_listeners()
        if fresh:
            initial_accounts = {tuple(line.split('\t')) for line in self.sql('SELECT User,Host FROM mysql.global_priv;').splitlines()}
            require(('root', 'localhost') in initial_accounts and initial_accounts <= {('root', 'localhost'), ('mysql', 'localhost'), ('mariadb.sys', 'localhost')},
                    'unexpected initial MariaDB account set')
            self.record['system_accounts'] = sorted(initial_accounts)
            self.sql("ALTER USER 'root'@'localhost' IDENTIFIED VIA unix_socket OR mysql_native_password USING PASSWORD('"
                     + root_password + "');\nCREATE DATABASE `podmesh-manager` CHARACTER SET utf8mb4;\n"
                     + "CREATE USER 'podmesh-manager'@'localhost' IDENTIFIED BY '" + app_password + "';\n")
        result = json.loads(self.run('schema', [str(LIB / 'podmesh-store-admin'), 'prepare', '--role', 'manager',
                                      '--profile', str(PROFILE), '--admin-socket', str(ALIAS),
                                      '--admin-password-file', str(ROOTPASS)], 30))
        require(result.get('status') == ('prepared_empty_schema' if fresh else 'already_current_no_ddl'),
                'schema-admin result is not the selected fresh/restart contract')
        if fresh:
            statements = []
            for table, privileges in self.grants.items():
                require(table.replace('_', '').isalnum() and set(privileges) <= {'SELECT', 'INSERT', 'UPDATE', 'DELETE'},
                        'grant manifest is not table-scoped DML')
                statements.append('GRANT ' + ','.join(privileges) + ' ON `podmesh-manager`.`' + table
                                  + "` TO 'podmesh-manager'@'localhost';")
            self.sql('\n'.join(statements))
        self.verify_grants()
        protected(ADMIN / 'client.cnf', 0, 0o600, 'file')
        (ADMIN / 'client.cnf').unlink()  # Boot-only duplicate credential, not original root password.
        self.start_application()

    def verify_grants(self):
        accounts = {tuple(line.split('\t')) for line in self.sql('SELECT User,Host FROM mysql.global_priv;').splitlines()}
        expected_accounts = {tuple(pair) for pair in self.record['system_accounts']} | {('podmesh-manager', 'localhost')}
        require(accounts == expected_accounts, 'unexpected database account or missing identity')
        for view in ('USER_PRIVILEGES', 'SCHEMA_PRIVILEGES', 'TABLE_PRIVILEGES', 'COLUMN_PRIVILEGES'):
            public = self.sql('SELECT PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.' + view + " WHERE UPPER(GRANTEE) LIKE '%PUBLIC%';")
            require(not public.strip() or (view == 'USER_PRIVILEGES' and all(row == 'USAGE\tNO' for row in public.splitlines())),
                    'PUBLIC privileges widen functional authority')
        require(not self.sql("SELECT Role FROM mysql.roles_mapping WHERE User='PUBLIC';").strip(),
                'PUBLIC inherited role refused')
        who = "CONVERT(0x" + "'podmesh-manager'@'localhost'".encode().hex() + " USING utf8mb4)"
        global_rows = self.sql('SELECT PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.USER_PRIVILEGES WHERE GRANTEE=' + who + ';')
        require(all(line == 'USAGE\tNO' for line in global_rows.splitlines()), 'functional global privileges refused')
        require(not self.sql('SELECT PRIVILEGE_TYPE FROM information_schema.SCHEMA_PRIVILEGES WHERE GRANTEE=' + who + ';').strip(),
                'functional schema-wide privileges refused')
        rows = self.sql('SELECT TABLE_SCHEMA,TABLE_NAME,PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.TABLE_PRIVILEGES WHERE GRANTEE=' + who + ';')
        actual = {tuple(line.split('\t')) for line in rows.splitlines()}
        expected = {('podmesh-manager', t, privilege, 'NO') for t, privileges in self.grants.items() for privilege in privileges}
        require(actual == expected, 'functional table grants differ; no automatic repair on restart')
        for view in ('COLUMN_PRIVILEGES',):
            require(not self.sql('SELECT PRIVILEGE_TYPE FROM information_schema.' + view + ' WHERE GRANTEE=' + who + ';').strip(),
                    'functional column/routine privileges refused')
        require(not self.sql("SELECT Routine_name FROM mysql.procs_priv WHERE User='podmesh-manager';").strip(),
                'functional routine privileges refused')
        require(not self.sql("SELECT Role FROM mysql.roles_mapping WHERE User='podmesh-manager' AND Host='localhost';").strip(),
                'functional inherited roles refused')

    def control(self, operation):
        child = self.children['app']
        with socket.socket(socket.AF_UNIX) as peer:
            peer.settimeout(1)
            peer.connect(str(API))
            observed = struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            require(observed == (child.pid, APP_UID, APP_UID), 'foreign manager control peer')
            peer.sendall(json.dumps({'operation': operation}).encode())
            peer.shutdown(socket.SHUT_WR)  # The resident frames requests by EOF.
            data = bytearray()
            while True:
                block = peer.recv(min(4096, 65537 - len(data)))
                if not block:
                    break
                require(len(data) + len(block) <= 65536, 'manager control reply oversized')
                data.extend(block)
            require(data, 'manager control reply absent')
            return json.loads(bytes(data))

    def start_application(self):
        self.spawn('app', drop_command(APP_UID, APP_UID, self.application_argv()), env=app_env())
        deadline = time.monotonic() + 30
        while True:
            self.pump()
            require(not self.stop_requested and all(self.peek(self.children[x]) is None for x in ('app', 'db')),
                    'owned manager/database died before control readiness')
            try:
                answer = self.control('status')
                require(answer.get('kind') == 'resident_observation'
                        and answer.get('replica_id') == self.config['network']['replica_id']
                        and answer.get('activation_authority') is False and answer.get('store_closed') is False,
                        'manager identity/store/authority status differs')
                break
            except (OSError, ValueError, Refusal):
                require(time.monotonic() < deadline, 'manager control readiness timeout')
                time.sleep(0.2)
        # A running local replica is useful before its peer catch-up completes.
        # Keep those two facts distinct; never infer quorum or active authority.
        self.record['phase'] = 'running'
        self.record['manager_replica_id'] = answer['replica_id']
        self.record['catch_up_at_start'] = answer['catch_up']
        durable_state(self.record)

    def stop_child(self, name, seconds):
        child = self.children.get(name)
        if name == 'app' and child is not None and self.peek(child) is None:
            try:
                require(self.control('shutdown') == {'shutdown_requested': True},
                        'typed manager shutdown not acknowledged')
                deadline = time.monotonic() + seconds
                while self.peek(child) is None and time.monotonic() < deadline:
                    self.pump(check=False)
                    time.sleep(0.02)
                require(self.peek(child) == 0 and not API.exists(), 'manager did not stop cleanly')
            except Exception:
                # Shared cleanup still drains/reaps owned children. A TERM/KILL
                # fallback is a failed birth receipt, never a clean restart.
                self.failed = True
            seconds = 0  # Grace period already spent; do not double the stop budget.
        return super().stop_child(name, seconds)

    def supervise(self):
        while not self.stop_requested:
            self.pump()
            self.reap_owned_orphans()
            require(all(self.peek(self.children[x]) is None for x in ('app', 'db')),
                    'unexpected owned manager/database death')
            time.sleep(0.1)


def main():
    os.umask(0o077)
    supervisor = Supervisor()
    def stop(_number, _frame):
        supervisor.stop_requested = True
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        supervisor.layout()
        supervisor.birth()
        supervisor.supervise()
    except Exception:
        supervisor.failed = True
        print('canonical MANAGER refused; preserve private state and diagnostics', flush=True)
    finally:
        try:
            supervisor.stop()
        except Exception:
            supervisor.failed = True
        finally:
            if supervisor.diagnostic is not None:
                supervisor.diagnostic.close()
    return 1 if supervisor.failed else 0


if __name__ == '__main__':
    raise SystemExit(main())
