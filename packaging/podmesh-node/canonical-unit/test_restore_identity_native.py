"""Opt-in native MariaDB/provider test; source-only candidate, not yet executed.

Run only on a separately leased disposable native test host, with real APP1102
and the real root provider mounted read-only at /run/podmesh-host. Supply a root0600
PODMESH_NATIVE_ADOPTION_ADMIN_CNF for a dedicated socket-only MariaDB server; its
podmesh-node database MUST be absent. This test creates/drops only that new DB.
It neither starts/stops that server nor claims a full container restore proof.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('canonical_node', Path(__file__).with_name('entrypoint.py'))
u = importlib.util.module_from_spec(spec)
spec.loader.exec_module(u)


@unittest.skipUnless(os.geteuid() == 0 and os.environ.get('PODMESH_NATIVE_ADOPTION_TEST') == '1',
                     'requires an explicitly leased native MariaDB + real provider fixture')
class NativeRestoredIdentity(unittest.TestCase):
    def test_adoption_cas_uuid_and_audit_are_one_native_transaction(self):
        cnf = Path(os.environ['PODMESH_NATIVE_ADOPTION_ADMIN_CNF'])
        u.protected(cnf, 0, 0o600, 'file')
        u.adapter_precondition()  # A real mounted root provider, never a mocked reply.
        created = False
        with tempfile.TemporaryDirectory(prefix='restore-identity-test-', dir='/root') as root:
            root = Path(root)
            etc, state, admin = root/'etc', root/'state', root/'runtime-admin'
            for directory in (etc, state, admin):
                directory.mkdir(mode=0o700)
            (etc/'admin').mkdir(mode=0o700)
            shutil.copyfile(cnf, admin/'client.cnf')
            (admin/'client.cnf').chmod(0o600)
            # Redirect only private file locations. All SQL, transactions and host
            # facts run through the real executables and the real provider.
            with patch.multiple(u, ETC=etc, STATE=state, ADMIN=admin):
                s = u.Supervisor(adopt_restored_identity=True)
                s.diagnostic = open(state/'diagnostic', 'xb', buffering=0)
                os.chmod(s.diagnostic.fileno(), 0o600)
                s.record = {'phase': 'starting', 'provenance': {'test': 'unchanged'}}
                try:
                    self.assertFalse(u.Supervisor().adopt_restored_identity)
                    self.assertTrue(u.Supervisor(recover_running=True).recover_running)
                    with self.assertRaises(u.Refusal):
                        u.Supervisor(recover_running=True, adopt_restored_identity=True)
                    self.assertEqual(s.sql("SELECT COUNT(*) FROM information_schema.SCHEMATA "
                                           "WHERE SCHEMA_NAME='podmesh-node';").strip(), '0')
                    s.sql('CREATE DATABASE `podmesh-node` CHARACTER SET utf8mb4;')
                    created = True
                    s.sql('CREATE TABLE `podmesh-node`.metadata '
                          '(`key` VARCHAR(191) PRIMARY KEY,value LONGTEXT NOT NULL) ENGINE=InnoDB;'
                          'CREATE TABLE `podmesh-node`.observations '
                          '(id BIGINT PRIMARY KEY AUTO_INCREMENT,observed_at BIGINT NOT NULL,'
                          'operation VARCHAR(64) NOT NULL,result LONGTEXT NOT NULL) ENGINE=InnoDB;')
                    machine = s.restored_target_machine()
                    previous = '0'*32 if machine != '0'*32 else '1'*32  # Source DATA fixture, not a fake provider.
                    node_uuid = '11111111-2222-4333-8444-555555555555'
                    s.sql("INSERT INTO `podmesh-node`.metadata VALUES ('machine_id','" + previous
                          + "'),('host_uuid','" + node_uuid + "'),('payload','keep-me');")
                    export = state/'restore.sql'
                    fd = os.open(export, os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o600)
                    with os.fdopen(fd, 'wb') as stream:
                        dumped = subprocess.run(['mariadb-dump', '--defaults-file='+str(admin/'client.cnf'),
                            '--protocol=socket', '--single-transaction', '--skip-comments', '--databases',
                            'podmesh-node'], stdout=stream, stderr=subprocess.PIPE, timeout=20)
                    self.assertEqual(dumped.returncode, 0, 'native fixture dump failed')
                    request = {'status': 'restored-and-compared',
                               'source_machine_sha256': hashlib.sha256(previous.encode()).hexdigest(),
                               'target_machine_sha256': hashlib.sha256(machine.encode()).hexdigest(),
                               'host_uuid': node_uuid,
                               'export_sha256': hashlib.sha256(export.read_bytes()).hexdigest()}
                    request_path = etc/'admin/restore-complete.json'
                    def publish(value):
                        request_path.write_text(json.dumps(value)); request_path.chmod(0o600)
                    def rows():
                        return dict(line.split('\t', 1) for line in s.sql(
                            'SELECT `key`,value FROM `podmesh-node`.metadata ORDER BY `key`;').splitlines())
                    def unchanged():
                        self.assertEqual(rows(), {'machine_id': previous, 'host_uuid': node_uuid,
                                                  'payload': 'keep-me'})
                        self.assertEqual(s.sql('SELECT COUNT(*) FROM `podmesh-node`.observations;').strip(), '0')
                        self.assertNotIn('host_identity_adoption', s.record)
                    with self.subTest('wrong expected old identity'):
                        publish(dict(request, source_machine_sha256='f'*64))
                        with self.assertRaisesRegex(u.Refusal, 'restored source identity differs'):
                            s.adopt_restored_host_identity()
                        unchanged()
                    with self.subTest('wrong NODE UUID'):
                        publish(dict(request, host_uuid='different-node'))
                        with self.assertRaisesRegex(u.Refusal, 'restored NODE UUID differs'):
                            s.adopt_restored_host_identity()
                        unchanged()
                    with self.subTest('SQL compare-and-swap loss writes no audit'):
                        audit = dict(request, observed_at=int(time.time()), operation='test-cas')
                        self.assertEqual(s.sql(u.restored_identity_sql('2'*32, machine, node_uuid, audit)).strip(), '0')
                        self.assertEqual(s.sql(u.restored_identity_sql(previous, machine, 'different-node', audit)).strip(), '0')
                        unchanged()
                    with self.subTest('native audit failure rolls back identity UPDATE'):
                        publish(request)
                        s.sql("CREATE TRIGGER `podmesh-node`.reject_adoption BEFORE INSERT ON "
                              "`podmesh-node`.observations FOR EACH ROW SIGNAL SQLSTATE '45000' "
                              "SET MESSAGE_TEXT='owned native audit failure';")
                        try:
                            with self.assertRaises(u.Refusal):
                                s.adopt_restored_host_identity()
                            unchanged()
                        finally:
                            s.sql('DROP TRIGGER `podmesh-node`.reject_adoption;')
                    with self.subTest('correct adoption keeps NODE UUID/data and appends one audit'):
                        publish(request)
                        s.adopt_restored_host_identity()
                        self.assertEqual(rows(), {'machine_id': machine, 'host_uuid': node_uuid,
                                                  'payload': 'keep-me'})
                        events = s.sql("SELECT result FROM `podmesh-node`.observations "
                                       "WHERE operation='host_identity_adopt';").splitlines()
                        self.assertEqual(len(events), 1)
                        audit = json.loads(events[0])
                        for key, value in request.items():
                            self.assertEqual(audit[key], value)
                        self.assertEqual(audit, s.record['host_identity_adoption'])
                        self.assertEqual(s.record['provenance'], {'test': 'unchanged'})
                        self.assertEqual(json.loads((state/'birth.json').read_text())['host_identity_adoption'], audit)
                finally:
                    if created:
                        s.sql('DROP DATABASE `podmesh-node`;')
                    s.record = None
                    s.stop()
                    s.diagnostic.close()


if __name__ == '__main__':
    unittest.main()
