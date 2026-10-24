"""Run on the declared build CT. No database or engine; real owned IPC/children.
These tests cover the MANAGER-specific configuration and typed shutdown adapter.
They do not qualify MariaDB, native birth, persistent identity or replication.
"""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parent
ROOT = SOURCE.parents[2]
# The build copies this unchanged existing source into the manager image. Mirror
# only that source layout in an owned temporary directory for this test import.
_fixture = tempfile.TemporaryDirectory(prefix='manager-canonical-source-')
_image = Path(_fixture.name)
shutil.copyfile(SOURCE / 'entrypoint.py', _image / 'entrypoint.py')
shutil.copyfile(ROOT / 'packaging/podmesh-node/canonical-unit/entrypoint.py', _image / 'lifecycle.py')
_spec = importlib.util.spec_from_file_location('canonical_manager', _image / 'entrypoint.py')
m = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(m)


class ManagerLifecycle(unittest.TestCase):
    def test_configuration_preserves_operator_replica_topology_and_rejects_node_path(self):
        config = {'observation_writer_uid': 1103, 'control_socket': str(m.API),
                  'network': {'database_path': str(m.SAV / 'app/manager.sqlite'),
                              'bind': '127.0.0.1:19543', 'replica_id': 'operator-replica',
                              'manager': {'logical_manager_id': 'operator-logical', 'replicas': []},
                              'peers': []}}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'config.json'
            path.write_text(json.dumps(config))
            # Ownership is independently exercised by the native recipe; this
            # test isolates the path/identity adapter and preserves supplied bytes.
            with patch.object(m, 'CONFIG', path), patch.object(m, 'protected') as check:
                check.return_value.st_gid = 1103
                check.return_value.st_size = path.stat().st_size
                found, _ = m.configuration()
                self.assertEqual(found, config)
                config['network']['database_path'] = '/apps/podmesh-node/sav/app/state.sqlite'
                path.write_text(json.dumps(config))
                with self.assertRaises(m.Refusal):
                    m.configuration()

    def test_app_environment_keeps_manager_profile_and_explicit_peer_mode(self):
        with patch.dict(os.environ, {'PODMESH_MARIADB_DSN': 'mysql://wrong', 'MYSQL_PWD': 'wrong',
                                     'LD_PRELOAD': '/wrong', 'PODMESH_MANAGER_VOTE_DIR': '/wrong'}):
            env = m.app_env()
        self.assertEqual(env['PODMESH_STORE_PROFILE'], '/apps/podmesh-manager/etc/store.json')
        self.assertEqual(env['PODMESH_MANAGER_NETWORK_MODE'], 'authenticated-static-peers')
        for key in ('PODMESH_MARIADB_DSN', 'MYSQL_PWD', 'LD_PRELOAD', 'PODMESH_MANAGER_VOTE_DIR'):
            self.assertNotIn(key, env)

    def exercise_shutdown(self, acknowledged):
        # An actual child implements ONLY the local control protocol, without a
        # database. The real manager's semantic integration needs the native run.
        server = '''import socket,json,sys,os
p=sys.argv[1];ack=sys.argv[2]=='yes'
s=socket.socket(socket.AF_UNIX);s.bind(p);s.listen(1)
while True:
 c,_=s.accept()
 with c:
  b=bytearray()
  while True:
   chunk=c.recv(4096)
   if not chunk: break
   b.extend(chunk)
  r=json.loads(b)
  assert r=={'operation':'shutdown'}
  c.sendall(json.dumps({'shutdown_requested':ack}).encode())
 if ack: break
s.close();os.unlink(p)
'''
        with tempfile.TemporaryDirectory() as directory:
            api = Path(directory) / 'control.sock'
            supervisor = m.Supervisor()
            with patch.object(m, 'API', api), patch.object(m, 'APP_UID', os.getuid()):
                child = supervisor.spawn('app', [sys.executable, '-I', '-c', server, str(api),
                                                 'yes' if acknowledged else 'no'])
                try:
                    deadline = time.monotonic() + 5
                    while not api.exists():
                        self.assertIsNone(supervisor.peek(child))
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(0.01)
                    # Production stop has already received SIGTERM at PID1.
                    supervisor.stop_requested = True
                    good = supervisor.stop_child('app', 2)
                    self.assertEqual(good, acknowledged)
                    self.assertEqual(supervisor.failed, not acknowledged)
                    self.assertNotIn('app', supervisor.children)
                    self.assertIsNotNone(child.returncode)
                    if acknowledged:
                        self.assertEqual(child.returncode, 0)
                        self.assertFalse(api.exists())
                finally:
                    supervisor.stop()

    def test_stop_uses_typed_shutdown_and_reaps_real_child_after_pid1_stop(self):
        self.exercise_shutdown(True)

    def test_missing_shutdown_ack_is_failed_cleanup_not_clean_restart(self):
        self.exercise_shutdown(False)


if __name__ == '__main__':
    try:
        unittest.main()
    finally:
        _fixture.cleanup()
