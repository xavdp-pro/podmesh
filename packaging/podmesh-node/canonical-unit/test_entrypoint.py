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
        child.capture_data = bytearray(b'not-ready')
        child.stdin = None
        child.pending_input = b''
        def launch(name, *_args, **_kw):
            s.children[name] = child
            return child
        with patch.object(s, 'spawn', side_effect=launch), patch.object(s, 'peek', return_value=1), patch.object(s, 'group_members', return_value=[]), patch.object(s, 'pump'):
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
        with patch.object(s, 'spawn', side_effect=launch), patch.object(s, 'run', side_effect=probe), patch.object(s, 'peek', return_value=None), patch.object(s, 'pump'), \
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
        with patch.object(s, 'peek', side_effect=[None, 0]), patch.object(s, 'reap_owned_orphans'), patch.object(u, 'no_tcp_listeners'), patch.object(s, 'pump'):
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

    def test_routine_privilege_is_refused_via_procs_priv(self):
        s = u.Supervisor()
        s.record = {'system_accounts': [['root', 'localhost']]}
        s.grants = {}
        def response(statement):
            if 'FROM mysql.global_priv' in statement:
                return 'root\tlocalhost\npodmesh-node\tlocalhost\n'
            if 'FROM mysql.procs_priv' in statement:
                return 'malicious_procedure\n'
            return ''
        with patch.object(s, 'sql', side_effect=response):
            with self.assertRaises(u.Refusal):
                s.verify_grants()

    def test_owned_zombie_is_reaped_after_group_signaling_not_before(self):
        s = u.Supervisor()
        child = Mock(pid=12345, returncode=0, stdin=None, pending_input=b'')
        s.children['app'] = child
        sequence = []
        child.wait.side_effect = lambda **_: sequence.append('reap')
        # A dead leader still owns a live session descendant for the first check.
        groups = [[(12345, 1), (12346, 1)], [(12345, 1), (12346, 1)],
                  [(12345, 1)], [(12345, 1)], []]
        with patch.object(s, 'peek', return_value=0), patch.object(s, 'pump'), \
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


class BoundedNativeOutput(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='bounded-output-')
        self.root = Path(self.temp.name)
        self.supervisor = u.Supervisor()
        path = self.root / 'diagnostic'
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        self.supervisor.diagnostic = os.fdopen(fd, 'wb', buffering=0)
        self.path = path

    def tearDown(self):
        self.supervisor.stop()
        self.supervisor.diagnostic.close()
        self.assertFalse(self.supervisor.children)
        self.assertFalse(self.supervisor.output_pipes)
        self.temp.cleanup()

    def fixture(self, fd, count, name='app', capture=False):
        ready = self.root / ('ready-' + name)
        code = ('import os,signal,sys,time,pathlib;'
                'signal.signal(signal.SIGTERM,lambda *_:sys.exit(0));'
                'data=b"x"*int(sys.argv[2]);'
                'fd=int(sys.argv[1]);'
                r'exec("while data:\n n=os.write(fd,data);data=data[n:]");'
                'pathlib.Path(sys.argv[3]).touch();time.sleep(30)')
        child = self.supervisor.spawn(name, [sys.executable, '-I', '-c', code, str(fd), str(count), str(ready)], capture=capture)
        deadline = time.monotonic() + 5
        while not ready.exists() and time.monotonic() < deadline:
            self.supervisor.pump(check=False)
            time.sleep(0.005)
        self.assertTrue(ready.exists(), 'fixture must finish writing while its process stays alive')
        for _ in range(20):
            self.supervisor.pump(check=False)
            time.sleep(0.002)
        self.assertIsNone(self.supervisor.peek(child))
        return child

    def test_native_stdout_and_stderr_under_exact_over_limit_while_alive(self):
        for fd in (1, 2):
            for count in (u.OUTPUT_LIMIT-1, u.OUTPUT_LIMIT, u.OUTPUT_LIMIT+1):
                with self.subTest(fd=fd, count=count):
                    self.supervisor.diagnostic.seek(0)
                    self.supervisor.diagnostic.truncate()
                    self.supervisor.diagnostic_bytes = 0
                    self.supervisor.diagnostic_prefix.clear()
                    self.supervisor.failed = False
                    self.supervisor.output_failure = None
                    self.supervisor.failure_prefix = b''
                    self.supervisor.failure_prefix_saved = False
                    self.fixture(fd, count)
                    self.assertEqual(self.path.stat().st_size, min(count, u.OUTPUT_LIMIT))
                    self.assertEqual(self.path.read_bytes(), b'x'*min(count, u.OUTPUT_LIMIT))
                    self.assertEqual(stat.S_IMODE(self.path.stat().st_mode), 0o600)
                    self.assertEqual(self.supervisor.failed, count > u.OUTPUT_LIMIT)
                    if count > u.OUTPUT_LIMIT:
                        with self.assertRaises(u.Refusal):self.supervisor.pump()
                    self.supervisor.stop_child('app', 1)

    def test_admin_combined_stream_limit_and_bounded_memory_while_alive(self):
        for fd in (1, 2):
            for count in (u.OUTPUT_LIMIT-1, u.OUTPUT_LIMIT, u.OUTPUT_LIMIT+1):
                with self.subTest(fd=fd,count=count):
                    self.supervisor.output_failure = None
                    self.supervisor.failed = False
                    self.supervisor.diagnostic_bytes = 0
                    self.supervisor.diagnostic_prefix.clear()
                    self.supervisor.diagnostic.seek(0);self.supervisor.diagnostic.truncate()
                    child=self.fixture(fd,count,'sql',True)
                    self.assertLessEqual(len(child.capture_data),u.OUTPUT_LIMIT)
                    self.assertEqual(child.output_bytes,min(count,u.OUTPUT_LIMIT))
                    self.assertEqual(self.supervisor.failed,count>u.OUTPUT_LIMIT)
                    self.supervisor.stop_child('sql',1)

    def test_combined_admin_stdout_stderr_overflow_shares_one_budget(self):
        code='import os,time;os.write(1,b"o"*32768);os.write(2,b"e"*32769);time.sleep(30)'
        with self.assertRaises(u.Refusal):
            self.supervisor.run('sql',[sys.executable,'-I','-c',code],5)
        self.assertTrue(self.supervisor.failed)
        self.assertEqual(self.supervisor.children['sql'].output_bytes,u.OUTPUT_LIMIT)
        self.assertLessEqual(self.path.stat().st_size,32769)
        self.assertTrue(self.path.read_bytes().startswith(b'e'))
        self.assertLessEqual(len(self.supervisor.failure_prefix),u.OUTPUT_LIMIT)

    def test_run_drains_full_pipes_and_child_exit_before_reap(self):
        # More than a small kernel pipe can hold, but within the metadata cap.
        code=r'import os;data=b"z"*65536;exec("while data:\n n=os.write(1,data);data=data[n:]")'
        self.assertEqual(self.supervisor.run('schema',[sys.executable,'-I','-c',code],5),'z'*65536)
        self.assertFalse(self.supervisor.output_pipes)
        self.assertFalse(self.supervisor.children)

    def test_helper_stderr_overflow_after_full_stdout_retains_private_prefix(self):
        code = (r'import os,time;data=b"o"*65536;'
                r'exec("while data:\n n=os.write(1,data);data=data[n:]");'
                r'os.write(2,b"e");time.sleep(30)')
        with self.assertRaises(u.Refusal):
            self.supervisor.run('sql', [sys.executable, '-I', '-c', code], 5)
        self.assertEqual(self.supervisor.output_failure['reason'], 'helper-output-overflow')
        self.assertEqual(self.supervisor.output_failure['stream'], 'stderr')
        self.assertEqual(self.supervisor.failure_prefix, b'e')
        self.assertEqual(self.supervisor.children['sql'].output_bytes, u.OUTPUT_LIMIT)
        self.assertEqual(self.path.stat().st_size, 0)

    def test_overflow_never_waits_for_child_exit_and_owned_cleanup_keeps_foreign_alive(self):
        foreign=u.subprocess.Popen([sys.executable,'-I','-c','import time;time.sleep(30)'],start_new_session=True)
        try:
            code='import os,time;os.write(1,b"q"*262144);time.sleep(30)'
            with self.assertRaises(u.Refusal):
                self.supervisor.run('schema',[sys.executable,'-I','-c',code],5)
            self.assertTrue(self.supervisor.failed)
            self.assertEqual(len(self.supervisor.failure_prefix),u.OUTPUT_LIMIT)
            self.supervisor.stop()
            self.assertIsNone(foreign.poll())
            self.assertFalse(self.supervisor.output_pipes)
        finally:
            foreign.terminate();foreign.wait(timeout=3)

    def test_short_reads_are_not_eof_and_pipe_read_error_fails_closed(self):
        child=self.fixture(1,2,'sql',True)
        self.assertEqual(bytes(child.capture_data),b'xx')
        original=u.os.read
        def broken(fd,size):
            if fd in self.supervisor.output_pipes:raise OSError('owned read fixture')
            return original(fd,size)
        with patch.object(u.os,'read',side_effect=broken):
            with self.assertRaises(u.Refusal):self.supervisor.pump()
        self.assertEqual(self.supervisor.output_failure['reason'],'pipe-read')

    def test_diagnostic_write_error_is_failure_and_cannot_escape_private_prefix_cap(self):
        child=self.supervisor.spawn('app',[sys.executable,'-I','-c','import os,time;os.write(2,b"failure");time.sleep(30)'])
        sink=self.supervisor.diagnostic
        self.supervisor.diagnostic=Mock()
        self.supervisor.diagnostic.write.side_effect=OSError('private sink failed')
        try:
            deadline=time.monotonic()+5
            while not self.supervisor.failed and time.monotonic()<deadline:
                self.supervisor.pump(check=False);time.sleep(.005)
            self.assertEqual(self.supervisor.output_failure['reason'],'diagnostic-write')
            self.assertEqual(self.path.stat().st_size,0)
            self.assertEqual(self.supervisor.diagnostic_bytes,0)
        finally:self.supervisor.diagnostic=sink

    @unittest.skipUnless(os.geteuid() == 0, 'private output receipt requires CT root')
    def test_overflow_prefix_and_metadata_saved_once_with_failed_marker(self):
        with tempfile.TemporaryDirectory(prefix='bounded-private-',dir='/root') as root:
            state=Path(root)
            self.supervisor.record={'phase':'starting'}
            self.supervisor.failure_prefix=b'p'*u.OUTPUT_LIMIT
            self.supervisor.output_error('schema','stdout','helper-output-overflow')
            with patch.object(u,'STATE',state):
                self.supervisor.stop();self.supervisor.stop()
            prefixes=list(state.glob('output-prefix-*'))
            self.assertEqual(len(prefixes),1)
            self.assertEqual(prefixes[0].read_bytes(),b'p'*u.OUTPUT_LIMIT)
            self.assertEqual(stat.S_IMODE(prefixes[0].stat().st_mode),0o600)
            marker=json.loads((state/'birth.json').read_text())
            self.assertEqual(marker['phase'],'failed')
            self.assertEqual(marker['output_failure']['prefix_bytes'],u.OUTPUT_LIMIT)
            self.supervisor.record=None

    @unittest.skipUnless(os.geteuid() == 0, 'private failure marker requires CT root')
    def test_prefix_write_failure_still_persists_failed_metadata(self):
        with tempfile.TemporaryDirectory(prefix='bounded-prefix-failure-',dir='/root') as root:
            state=Path(root);self.supervisor.record={'phase':'starting'}
            self.supervisor.failure_prefix=b'private prefix'
            self.supervisor.output_error('sql','stdout','helper-output-overflow')
            original=u.os.write
            def fail_prefix(fd,data):
                # Only this test's new prefix fd is unlinked from Popen's pipe set.
                if data==b'private prefix':raise OSError('owned prefix fixture')
                return original(fd,data)
            with patch.object(u,'STATE',state),patch.object(u.os,'write',side_effect=fail_prefix):
                self.supervisor.stop()
            marker=json.loads((state/'birth.json').read_text())
            self.assertEqual(marker['phase'],'failed')
            self.assertEqual(marker['output_failure']['prefix_error'],'prefix-write')
            self.supervisor.record=None

    def test_repeated_readiness_stderr_counts_toward_lifetime_budget(self):
        code='import os,sys;os.write(2,b"n"*8192);sys.exit(1)'
        for _ in range(8):
            with self.assertRaises(u.Refusal):
                self.supervisor.run('readiness',[sys.executable,'-I','-c',code],5)
            self.assertIsNone(self.supervisor.output_failure)
        self.assertEqual(self.path.stat().st_size,u.OUTPUT_LIMIT)
        with self.assertRaises(u.Refusal):
            self.supervisor.run('readiness',[sys.executable,'-I','-c',code],5)
        self.assertEqual(self.supervisor.output_failure['reason'],'diagnostic-overflow')
        self.assertTrue(self.supervisor.failed)
        self.assertEqual(self.path.stat().st_size,u.OUTPUT_LIMIT)

    def test_failed_spawn_and_pipe_setup_leave_no_readers_or_child(self):
        with self.assertRaises(OSError):self.supervisor.spawn('sql',['/missing-owned-output-fixture'])
        self.assertFalse(self.supervisor.children);self.assertFalse(self.supervisor.output_pipes)
        self.assertTrue(self.supervisor.failed)
        self.assertEqual(self.supervisor.output_failure['reason'],'spawn')
        self.supervisor.output_failure=None
        with patch.object(u.os,'set_blocking',side_effect=OSError('owned setup fixture')):
            with self.assertRaises(OSError):
                self.supervisor.spawn('sql',[sys.executable,'-I','-c','import time;time.sleep(30)'],capture=True)
        self.assertFalse(self.supervisor.children);self.assertFalse(self.supervisor.output_pipes)
        self.assertTrue(self.supervisor.failed)
        self.assertEqual(self.supervisor.output_failure['reason'],'pipe-setup')

    def test_missing_output_pipe_refuses_and_reaps_real_owned_child(self):
        popen=u.subprocess.Popen
        def missing(*args,**kwargs):
            kwargs['stdout']=u.subprocess.DEVNULL
            kwargs['stderr']=u.subprocess.DEVNULL
            return popen(*args,**kwargs)
        with patch.object(u.subprocess,'Popen',side_effect=missing):
            with self.assertRaises(u.Refusal):
                self.supervisor.spawn('sql',[sys.executable,'-I','-c','import time;time.sleep(30)'],capture=True)
        self.assertTrue(self.supervisor.failed)
        self.assertFalse(self.supervisor.children)
        self.assertFalse(self.supervisor.output_pipes)

    def test_SIGTERM_during_SQL_stops_owned_groups_app_before_database(self):
        for name in ('db','app'):
            self.supervisor.spawn(name,[sys.executable,'-I','-c','import time;time.sleep(30)'])
        original=self.supervisor.pump
        def interrupt(check=True):
            original(check=check)
            self.supervisor.stop_requested=True
        with patch.object(self.supervisor,'pump',side_effect=interrupt):
            with self.assertRaises(u.Refusal):
                self.supervisor.run('sql',[sys.executable,'-I','-c','import time;time.sleep(30)'],5,stdin='SELECT 1;')
        order=[];stop=self.supervisor.stop_child
        def observed(name,seconds):order.append(name);return stop(name,seconds)
        with patch.object(self.supervisor,'stop_child',side_effect=observed):self.supervisor.stop()
        self.assertEqual(order[0],'app');self.assertEqual(order[-1],'db')


if __name__ == '__main__':
    unittest.main()
