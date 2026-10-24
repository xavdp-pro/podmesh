"""One MANAGER policy test for explicit recorded-running recovery; build CT only.
Real protected files, no DB/engine: the native DB spawn is a sentinel boundary.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import types
import unittest
from unittest.mock import patch


class AtDatabaseStart(Exception):
    pass


class AtSupervisorStart(Exception):
    pass


class ProcessRecovery(unittest.TestCase):
    def test_manager_explicit_known_running_preserves_identity_and_rejects_other_births(self):
        self.assertEqual(os.geteuid(), 0, 'run this protected-fixture test as root on the build CT')
        source = Path(__file__).resolve().parent
        repo = source.parents[2]
        with tempfile.TemporaryDirectory(prefix='manager-process-recovery-', dir='/root') as directory:
            root = Path(directory)
            image = root / 'image'; image.mkdir(mode=0o700)
            shutil.copyfile(source / 'entrypoint.py', image / 'entrypoint.py')
            shutil.copyfile(repo / 'packaging/podmesh-node/canonical-unit/entrypoint.py', image / 'lifecycle.py')
            spec = importlib.util.spec_from_file_location('manager_recovery_candidate', image / 'entrypoint.py')
            m = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(m)
            state = root / 'state'; state.mkdir(mode=0o700)
            saved = root / 'saved'; saved.mkdir(mode=0o700)
            data = saved / 'mariadb'; data.mkdir(mode=0o700)
            (data / 'mysql').mkdir(mode=0o700)
            admin = root / 'runtime-admin'; admin.mkdir(mode=0o700)
            password, root_password, profile, config = [root / name for name in ('app-password', 'root-password', 'store.json', 'config.json')]
            m.exclusive_file(password, 'a' * 64, m.APP_UID, m.APP_UID)
            m.exclusive_file(root_password, 'b' * 64)
            m.exclusive_file(profile, json.dumps({'engine': 'mariadb', 'mariadb': {
                'user': 'podmesh-manager', 'database': 'podmesh-manager',
                'socket': str(m.DBSOCK), 'password_file': str(password)}}), 0, m.APP_UID, 0o440)
            m.exclusive_file(config, json.dumps({'observation_writer_uid': m.APP_UID,
                'control_socket': str(m.API), 'network': {'database_path': str(saved / 'app/manager.sqlite'),
                'bind': '127.0.0.1:19543', 'replica_id': '10000000-0000-0000-0000-000000000001',
                'manager': {'logical_manager_id': '20000000-0000-0000-0000-000000000001',
                    'replicas': [{'replica_id': '10000000-0000-0000-0000-000000000001',
                                  'host_id': '30000000-0000-0000-0000-000000000001'}],
                    'grants': [{'scope': 'fixture', 'owner_replica_id': '10000000-0000-0000-0000-000000000001'}]},
                'peers': []}, 'interval_ms': 100, 'max_backoff_ms': 100, 'incoming_workers': 1}), 0, m.APP_UID, 0o440)
            fingerprint = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (password, root_password, profile)}
            provenance = {'exact': 'same-manager-candidate'}
            marker = state / 'birth.json'
            with patch.multiple(m, STATE=state, SAV=saved, DATA=data, ADMIN=admin,
                                PASS=password, ROOTPASS=root_password, PROFILE=profile, CONFIG=config), \
                 patch.object(m.u, 'STATE', state):
                # Real protected() and configuration() execute; no permission or
                # credential reader is mocked. Full daemon layout is not emulated.
                config_document, config_sha256 = m.configuration()
                recorded = {'phase': 'running', 'datadir': [data.stat().st_dev, data.stat().st_ino],
                            'configuration': fingerprint, 'provenance': provenance,
                            'manager_config_sha256': config_sha256, 'system_accounts': [['root', 'localhost']]}
                def reset(phase='running', **changed):
                    document = dict(recorded, phase=phase, **changed)
                    marker.write_text(json.dumps(document)); marker.chmod(0o600)
                    if (admin / 'client.cnf').exists():
                        (admin / 'client.cnf').unlink()
                    return document
                def supervisor(recover):
                    actor = m.Supervisor(recover_running=recover)
                    actor.provenance = provenance
                    actor.config, actor.config_sha256 = config_document, config_sha256
                    actor.mysql = types.SimpleNamespace(pw_uid=999, pw_gid=999)
                    return actor
                def database_boundary(name, command, **kwargs):
                    self.assertEqual(name, 'db')
                    self.assertIn('/usr/sbin/mariadbd', command)
                    self.assertIn('--datadir=' + str(data), command)
                    self.assertIn('--skip-networking', command)
                    self.assertNotIn('/usr/bin/mariadb-install-db', command)
                    raise AtDatabaseStart()
                # Every refusal occurs before a DB spawn or initialize command,
                # preserving the actual recorded marker and credential bytes.
                refusals = [(phase, True, {}) for phase in ('failed', 'partial', 'initializing', 'starting')]
                refusals += [('running', False, {}), ('running', True, {'provenance': {'exact': 'other'}}),
                             ('running', True, {'datadir': [0, 0]}), ('running', True, {'configuration': {}})]
                for phase, recover, changed in refusals:
                    with self.subTest(phase=phase, recover=recover, changed=changed):
                        before = reset(phase, **changed)
                        actor = supervisor(recover)
                        with patch.object(actor, 'spawn') as spawn, patch.object(actor, 'run') as initialize:
                            with self.assertRaises(m.Refusal): actor.birth()
                            spawn.assert_not_called(); initialize.assert_not_called()
                        self.assertEqual(json.loads(marker.read_text()), before)
                marker.unlink()
                actor = supervisor(True)
                with patch.object(actor, 'spawn') as spawn, patch.object(actor, 'run') as initialize:
                    with self.assertRaises(m.Refusal): actor.birth()
                    spawn.assert_not_called(); initialize.assert_not_called()
                self.assertFalse(marker.exists())
                for phase, recover in (('stopped-clean', False), ('stopped-clean', True), ('running', True)):
                    with self.subTest(accepted_phase=phase, recover=recover):
                        reset(phase)
                        actor = supervisor(recover)
                        with patch.object(actor, 'spawn', side_effect=database_boundary) as spawn, \
                             patch.object(actor, 'run') as initialize:
                            with self.assertRaises(AtDatabaseStart): actor.birth()
                            spawn.assert_called_once(); initialize.assert_not_called()
                        actual = json.loads(marker.read_text())
                        self.assertEqual(actual['phase'], 'starting')
                        if phase == 'running':
                            self.assertEqual(actual['process_recovery'], {'from_phase': 'running',
                                'mode': 'explicit-native-mariadb-redo'})
                        else:
                            self.assertNotIn('process_recovery', actual)
                        for key in ('datadir', 'configuration', 'provenance', 'manager_config_sha256'):
                            self.assertEqual(actual[key], recorded[key])
                self.assertEqual(fingerprint, {str(p): hashlib.sha256(p.read_bytes()).hexdigest()
                                               for p in (password, root_password, profile)})
                self.assertEqual(hashlib.sha256(config.read_bytes()).hexdigest(), config_sha256)
                # Exercise main's real argv decision before layout/any child.
                previous_umask = os.umask(0o077)
                try:
                    for argv, recover in (([], False), (['--recover-known-running'], True)):
                        with patch.object(m.sys, 'argv', ['entrypoint.py', *argv]), \
                             patch.object(m, 'Supervisor', side_effect=AtSupervisorStart) as constructor:
                            with self.assertRaises(AtSupervisorStart): m.main()
                            constructor.assert_called_once_with(recover_running=recover)
                    for argv in (['--unknown'], ['--recover-known-running', '--unknown'],
                                 ['--recover-known-running', '--recover-known-running']):
                        with patch.object(m.sys, 'argv', ['entrypoint.py', *argv]), patch.object(m, 'Supervisor') as constructor:
                            with self.assertRaises(m.Refusal): m.main()
                            constructor.assert_not_called()
                finally:
                    os.umask(previous_umask)


if __name__ == '__main__':
    unittest.main()
