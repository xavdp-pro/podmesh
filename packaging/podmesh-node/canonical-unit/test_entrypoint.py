"""Targeted supervisor tests for the declared build host.
These tests use only temporary own files/children; no MariaDB, NODE, network or images.
"""
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import stat
import sys
import tempfile
import time
import types
import unittest
from unittest.mock import patch, Mock

spec = importlib.util.spec_from_file_location('canonical_node', Path(__file__).with_name('entrypoint.py'))
u = importlib.util.module_from_spec(spec)
spec.loader.exec_module(u)


class Contracts(unittest.TestCase):
    def test_ambient_credentials_and_loader_config_never_enter_app(self):
        with patch.dict(os.environ, {'PODMESH_MARIADB_DSN': 'mysql://wrong', 'MYSQL_PWD': 'do-not-copy',
                                     'LD_PRELOAD': '/bad.so', 'PYTHONPATH': '/untrusted'}):
            env = u.app_env()
        self.assertNotIn('MYSQL_PWD', env)
        self.assertNotIn('PODMESH_MARIADB_DSN', env)
        self.assertNotIn('LD_PRELOAD', env)
        self.assertNotIn('PYTHONPATH', env)
        self.assertEqual(env['PODMESH_STORE_PROFILE'], '/apps/podmesh-node/etc/store.json')
        self.assertEqual(env['PODMESH_HOST_ADAPTER_SOCKET'], '/run/podmesh-host/capability.sock')

    def test_functional_launch_drops_groups_all_capability_sets_and_privilege_gain(self):
        argv = u.drop_command(1102, 1102, ['/exact/podmeshd'])
        for arg in ['--reuid=1102', '--regid=1102', '--clear-groups', '--bounding-set=-all',
                    '--inh-caps=-all', '--ambient-caps=-all', '--no-new-privs']:
            self.assertIn(arg, argv)
        self.assertEqual(argv[-1], '/exact/podmeshd')

    def test_readiness_refusal_is_not_a_ready_marker_and_child_is_reaped(self):
        s = u.Supervisor()
        child = Mock()
        child.poll.return_value = 1
        child.returncode = 1
        child.capture_file = io.BytesIO(b'not-ready')
        def launch(name, *_args, **_kw):
            s.children[name] = child
            return child
        with patch.object(s, 'spawn', side_effect=launch), patch.object(s, 'peek', return_value=1), patch.object(s, 'group_members', return_value=[]):
            with self.assertRaises(u.Refusal):
                s.run('readiness', ['/exact/helper'], 1)
        self.assertNotIn('readiness', s.children)

    def test_app_ready_only_after_database_schema_and_real_probe_contract(self):
        s = u.Supervisor()
        s.record = {'phase': 'starting'}
        child = Mock(pid=100)
        child.poll.return_value = None
        s.children['db'] = child
        def launch(name, argv, **kwargs):
            self.assertEqual(name, 'app')
            self.assertEqual(argv[-1], str(u.LIB / 'podmeshd'))
            self.assertNotIn('MYSQL_PWD', kwargs['env'])
            s.children['app'] = child
        attempts = []
        def probe(*args):
            self.assertEqual(s.record['phase'], 'starting')
            attempts.append(args)
            if len(attempts) == 1:
                raise u.Refusal('socket not ready')
            return 'ready\n'
        with patch.object(s, 'spawn', side_effect=launch), patch.object(s, 'run', side_effect=probe), patch.object(s, 'peek', return_value=None), \
             patch.object(u, 'no_tcp_listeners'), patch.object(u, 'durable_state') as marker, \
             patch.object(u.time, 'sleep'):
            s.start_application()
        self.assertEqual(len(attempts), 2)
        self.assertEqual(s.record['phase'], 'running')
        marker.assert_called_once()

    def test_unexpected_database_death_is_failure_and_app_stop_precedes_database(self):
        s = u.Supervisor()
        s.children = {'app': Mock(), 'db': Mock()}
        s.children['app'].poll.return_value = None
        s.children['db'].poll.return_value = 0
        with patch.object(s, 'peek', side_effect=[None, 0]), patch.object(s, 'reap_owned_orphans'), patch.object(u, 'no_tcp_listeners'):
            with self.assertRaises(u.Refusal):
                s.supervise()
        order = []
        def stop(name, _timeout):
            order.append(name)
            s.children.pop(name, None)
        with patch.object(s, 'stop_child', side_effect=stop):
            s.stop()
        self.assertEqual(order, ['app', 'db'])

    def test_TCP_listener_is_refused_instead_of_assuming_skip_networking(self):
        listener = 'header\n 0: 00000000:0CEA 00000000:0000 0A 0 0 0\n'
        with patch.object(Path, 'exists', return_value=True), patch.object(Path, 'read_text', return_value=listener):
            with self.assertRaises(u.Refusal):
                u.no_tcp_listeners()

    def test_grant_manifest_preserves_schema_authority_and_no_automatic_porting(self):
        tables = json.loads(Path(__file__).with_name('grants.json').read_text())['tables']
        self.assertEqual(len(tables), 40)
        self.assertEqual(tables['store_schema'], ['SELECT'])
        self.assertEqual(tables['activation_epochs'], ['SELECT'])
        self.assertEqual(tables['observations'], ['SELECT', 'INSERT'])
        self.assertEqual(tables['metadata'], ['SELECT', 'INSERT'])
        self.assertEqual(tables['operations'], ['SELECT', 'INSERT', 'UPDATE'])
        for privileges in tables.values():
            self.assertTrue(set(privileges) <= {'SELECT', 'INSERT', 'UPDATE', 'DELETE'})

    def test_PUBLIC_escalation_refuses_before_functional_launch(self):
        s = u.Supervisor()
        s.record = {'system_accounts': [['root', 'localhost']]}
        responses = ['root\tlocalhost\npodmesh-node\tlocalhost\n', 'CREATE\tNO\n']
        with patch.object(s, 'sql', side_effect=responses), patch.object(s, 'start_application') as app:
            with self.assertRaises(u.Refusal):
                s.verify_grants()
            app.assert_not_called()

    def test_schema_ledger_write_grant_is_not_silently_repaired(self):
        s = u.Supervisor()
        s.record = {'system_accounts': [['root', 'localhost']]}
        s.grants = {'store_schema': ['SELECT']}
        def response(statement):
            if 'FROM mysql.global_priv' in statement:
                return 'root\tlocalhost\npodmesh-node\tlocalhost\n'
            if 'FROM information_schema.TABLE_PRIVILEGES' in statement and 'LIKE' not in statement:
                return 'podmesh-node\tstore_schema\tUPDATE\tNO\n'
            return ''
        with patch.object(s, 'sql', side_effect=response):
            with self.assertRaises(u.Refusal):
                s.verify_grants()

    def test_owned_zombie_is_reaped_after_group_signaling_not_before(self):
        s = u.Supervisor()
        child = Mock(pid=12345, returncode=0, capture_file=None)
        s.children['app'] = child
        sequence = []
        child.wait.side_effect = lambda **_: sequence.append('reap')
        # A dead leader still owns a live session descendant for the first check.
        groups = [[(12345, 1), (12346, 1)], [(12345, 1), (12346, 1)],
                  [(12345, 1)], [(12345, 1)], []]
        with patch.object(s, 'peek', return_value=0), \
             patch.object(s, 'group_members', side_effect=lambda *_: groups.pop(0) if groups else []), \
             patch.object(u.os, 'killpg', side_effect=lambda *_: sequence.append('signal')), \
             patch.object(u.os, 'waitpid'):
            s.stop_child('app', 0.1)
        self.assertEqual(sequence, ['signal', 'reap'])
        self.assertNotIn('app', s.children)


@unittest.skipUnless(os.geteuid() == 0, 'private ownership fixtures require CT root')
class PrivateFilesAndOwnedChildren(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='canonical-node-test-', dir='/root')
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def test_unsafe_ancestor_and_symlink_leaf_are_refused_without_chown(self):
        parent = self.root / 'replaceable'
        parent.mkdir(mode=0o777)
        parent.chmod(0o777)
        target = parent / 'data'
        target.mkdir()
        with patch.object(u.os, 'chown') as chown:
            with self.assertRaises(u.Refusal):
                u.owned_directory(target, 0, 0, 0o755)
            chown.assert_not_called()
        link = self.root / 'link'
        link.symlink_to(target)
        with self.assertRaises(u.Refusal):
            u.protected(link)

    def test_existing_wrong_ownership_or_mode_is_never_repaired(self):
        path = self.root / 'data'
        path.mkdir(mode=0o755)
        path.chmod(0o755)
        with patch.object(u.os, 'chown') as chown:
            with self.assertRaises(u.Refusal):
                u.owned_directory(path, 0, 0, 0o700)
            chown.assert_not_called()
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o755)

    def test_partial_or_failed_birth_never_resets_credentials_or_runs_SQL(self):
        state = self.root / 'admin'
        state.mkdir(mode=0o700)
        marker = state / 'birth.json'
        marker.write_text('{"phase":"failed"}')
        marker.chmod(0o600)
        s = u.Supervisor()
        with patch.object(u, 'STATE', state), patch.object(u, 'exclusive_file') as create, \
             patch.object(s, 'sql') as sql:
            with self.assertRaises(u.Refusal):
                s.birth()
            create.assert_not_called()
            sql.assert_not_called()
        self.assertEqual(json.loads(marker.read_text())['phase'], 'failed')

    def test_stale_runtime_refuses_before_adapter_or_database_launch(self):
        home = self.root / 'home'
        home.mkdir(mode=0o755)
        home.chmod(0o755)
        etc, sav, run = home / 'etc', home / 'sav', home / 'runtime'
        for path in (etc, sav, run):
            path.mkdir(mode=0o755)
            path.chmod(0o755)
        real_protected = u.protected
        def guard(path, *args, **kwargs):
            if Path(path) == Path('/apps'):
                return home.stat()
            return real_protected(path, *args, **kwargs)
        s = u.Supervisor()
        with patch.multiple(u, HOME=home, ETC=etc, SAV=sav, RUN=run, DATA=sav/'mariadb', STATE=sav/'admin'), \
             patch.object(u, 'protected', side_effect=guard), \
             patch.object(u.os, 'getpid', return_value=1), patch.object(u.os.path, 'ismount', return_value=True), \
             patch.object(u.pwd, 'getpwuid', return_value=types.SimpleNamespace(pw_name='podmesh-node')), \
             patch.object(u.grp, 'getgrgid', return_value=types.SimpleNamespace(gr_name='podmesh-node')), \
             patch.object(u.pwd, 'getpwnam', return_value=types.SimpleNamespace(pw_uid=999, pw_gid=999)), \
             patch.object(u, 'adapter_precondition') as adapter, patch.object(s, 'spawn') as spawn:
            with self.assertRaises(u.Refusal):
                s.layout()
            adapter.assert_not_called()
            spawn.assert_not_called()
        self.assertTrue(run.exists())

    def test_pending_state_marker_is_not_silently_replaced(self):
        state = self.root / 'admin'
        state.mkdir(mode=0o700)
        pending = state / 'birth.pending'
        pending.write_text('retained interrupted state')
        with patch.object(u, 'STATE', state):
            with self.assertRaises(FileExistsError):
                u.durable_state({'phase': 'running'})
        self.assertEqual(pending.read_text(), 'retained interrupted state')

    def test_owned_TERM_and_forced_stop_have_distinct_results(self):
        for ignore in (False, True):
            ready = self.root / ('ready-' + str(ignore))
            code = ('import signal,time,pathlib,sys;signal.signal(signal.SIGTERM,'
                    + ('signal.SIG_IGN' if ignore else 'lambda *_:sys.exit(0)')
                    + ');pathlib.Path(sys.argv[1]).write_text("ready");time.sleep(30)')
            s = u.Supervisor()
            with patch.object(u, 'STATE', self.root):
                s.spawn('app', [sys.executable, '-I', '-c', code, str(ready)], capture=True)
            end = time.monotonic() + 3
            while not ready.exists() and time.monotonic() < end:
                time.sleep(0.01)
            self.assertTrue(ready.exists())
            self.assertEqual(s.stop_child('app', 0.1), not ignore)
            self.assertEqual(s.failed, ignore)


if __name__ == '__main__':
    unittest.main()
