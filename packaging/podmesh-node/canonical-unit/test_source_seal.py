"""NEW affected cases, authored only. Explicit root build-CT fixture; no SQL/Podman.
Never execute on an operator workstation. Lifecycle/native engine calls are doubles.
"""
import base64
import copy
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent

def module(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), HERE / name)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


@unittest.skipUnless(os.getuid() == 0 and os.environ.get('PODMESH_SOURCE_SEAL_TEST_ROOT'),
                     'explicit disposable root build-CT only')
class SourceSeal(unittest.TestCase):
    def setUp(self):
        self.host = module('logical-transfer.py')
        self.entry = module('entrypoint.py')
        self.temp = tempfile.TemporaryDirectory(prefix='source-seal-', dir='/root')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ('etc', 'sav', 'provider'):
            (self.root / name).mkdir(mode=0o755)
        for name in ('sav/admin', 'sav/mariadb', 'etc/admin', 'etc/mysql', 'etc/mysql/localhost'):
            (self.root / name).mkdir(mode=0o700)
        self.sav = self.root / 'sav'
        self.admin = self.sav / 'admin'
        for name, owner, mode, data in [('admin/passwd', 0, 0o600, b'a'*64),
                                      ('mysql/localhost/passwd', 1102, 0o600, b'b'*64),
                                      ('store.json', 0, 0o440, b'{"engine":"mariadb"}')]:
            p = self.root / 'etc' / name
            p.write_bytes(data);os.chown(p, owner, owner);p.chmod(mode)
        self.config = {'container': 'owned-source', 'image': '1'*64,
                       'etc_root': str(self.root/'etc'), 'sav_root': str(self.sav),
                       'provider_dir': str(self.root/'provider')}
        datadir = self.sav / 'mariadb'
        self.birth = {'phase': 'stopped-clean', 'lifecycle_id': '2'*64,
                      'datadir': [datadir.stat().st_dev, datadir.stat().st_ino],
                      'configuration': self.host.source_configuration(self.config),
                      'provenance': {'sha256': {'entrypoint.py': self.host.FENCED_ENTRYPOINT_SHA256}}}
        self.birth_bytes = json.dumps(self.birth).encode()
        self.host.write_new(self.admin/'birth.json', self.birth_bytes)
        self.host.write_new(self.admin/'logical-transfer.lock', b'')
        self.inspection = {'Id': '3'*64, 'Name': '/owned-source',
                           'State': {'Running': False, 'Pid': 0, 'ExitCode': 0, 'Status': 'exited',
                                     'StartedAt': '2026-01-01T00:00:00Z', 'FinishedAt': '2026-01-01T00:00:10Z'}}
        self.node = types.SimpleNamespace(c=self.config, worker='explicit fixture worker',
                                         validate=Mock(return_value=self.inspection), cid=None)
        self.bundle = self.root / 'bundle';self.bundle.mkdir(mode=0o700)
        (self.bundle/'files').mkdir(mode=0o700)
        sql = b'fixture logical SQL, never executed\n'
        self.host.write_new(self.bundle/'logical.sql', sql)
        self.manifest = {'format': self.host.FORMAT, 'status': 'completed-coherent-export',
            'sql': {'bytes': len(sql), 'sha256': self.host.sha(sql)}, 'files': {},
            'directories': {n: {'type': 'directory', 'uid': 1102, 'gid': 1102, 'mode': 0o700, 'empty': True}
                            for n in self.host.BOOTSTRAP_DIRS},
            'image': self.config['image'], 'provenance': self.birth['provenance'],
            'configuration_sha256': self.birth['configuration'],
            'native_worker_sha256': self.host.sha(self.node.worker.encode()),
            'inventory': {'identity': {'host_uuid': '11111111-2222-3333-4444-555555555555',
                                       'machine_id_sha256': '4'*64}},
            'source': {'config': dict(self.config), 'container_id': '3'*64,
                       'started_at': self.inspection['State']['StartedAt'],
                       'datadir': self.birth['datadir'], 'lifecycle_id': '2'*64,
                       'running_birth_sha256': '5'*64}}
        self.host.json_new(self.bundle/'manifest.json', self.manifest)
        self.receipt = self.root/'seal-receipt.json'

    def rewrite_manifest(self, value):
        p = self.bundle/'manifest.json';p.write_text(json.dumps(value));p.chmod(0o600)

    def guard(self):
        return patch.multiple(self.entry, SAV=self.sav, STATE=self.admin)

    def test_both_helpers_pin_the_actual_new_entrypoint_and_retain_old_pins(self):
        native = module('logical-transfer-native.py')
        actual = self.host.sha((HERE/'entrypoint.py').read_bytes())
        self.assertEqual(self.host.FENCED_ENTRYPOINT_SHA256, actual)
        self.assertEqual(native.FENCED_ENTRYPOINT_SHA256, actual)
        self.assertNotIn(actual, (self.host.ADOPTION_ENTRYPOINT_SHA256, self.host.MONTHLY_ENTRYPOINT_SHA256))
        self.assertEqual(native.ADOPTION_ENTRYPOINT_SHA256, self.host.ADOPTION_ENTRYPOINT_SHA256)
        self.assertEqual(native.MONTHLY_ENTRYPOINT_SHA256, self.host.MONTHLY_ENTRYPOINT_SHA256)

    def test_new_export_records_current_source_incarnation_without_transferring_admin_state(self):
        running = copy.deepcopy(self.birth);running['phase'] = 'running'
        engine = copy.deepcopy(self.inspection);engine['State'].update(Running=True, Pid=123, Status='running')
        sql = b'fixture logical SQL, never executed\n'
        self.node.validate = Mock(return_value=engine)
        self.node.resume = Mock()
        self.node.native = Mock(return_value={'birth': running, 'inventory': self.manifest['inventory'],
            'sql': base64.b64encode(sql).decode(), 'files': {}, 'directories': self.manifest['directories']})
        output = self.root/'new-export'
        self.host.export(self.node, output, [])
        manifest, actual_sql, saved = self.host.load_bundle(output)
        self.assertEqual(manifest['source']['lifecycle_id'], running['lifecycle_id'])
        self.assertEqual(manifest['source']['container_id'], engine['Id'])
        self.assertEqual(manifest['source']['started_at'], engine['State']['StartedAt'])
        self.assertEqual(manifest['source']['running_birth_sha256'], self.host.document_sha256(running))
        self.assertEqual(manifest['source']['config'], self.config)
        self.assertEqual(actual_sql, sql);self.assertEqual(saved, {})
        self.node.resume.assert_called_once_with()
        self.assertEqual((self.admin/'birth.json').read_bytes(), self.birth_bytes)

    def test_seal_commits_exact_binding_exclusively_without_changing_birth(self):
        result = self.host.seal_source(self.node, self.bundle, self.receipt)
        self.assertEqual(result['status'], 'SOURCE_SEALED_LOCAL_COLD_HANDOFF')
        seal = self.admin/'source-seal.json'
        record = json.loads(seal.read_text())
        self.assertEqual(record['source'], self.manifest['source'])
        self.assertEqual(record['source_identity'], self.manifest['inventory']['identity'])
        self.assertEqual(record['bundle_manifest_sha256'], self.host.sha((self.bundle/'manifest.json').read_bytes()))
        self.assertEqual(record['stopped']['birth_sha256'], self.host.sha(self.birth_bytes))
        self.assertEqual(seal.stat().st_mode & 0o777, 0o600)
        self.assertEqual((self.admin/'birth.json').read_bytes(), self.birth_bytes)
        original = seal.read_bytes()
        with self.assertRaises(RuntimeError):
            self.host.seal_source(self.node, self.bundle, self.root/'second-receipt.json')
        self.assertEqual(seal.read_bytes(), original)

    def test_sealed_ordinary_recovery_adoption_and_monthly_modes_refuse_before_layout(self):
        self.host.write_new(self.admin/'source-seal.json', b'')  # Ambiguous/partial commits also block.
        for args in ([], ['--recover-known-running'], ['--adopt-restored-host-identity'],
                     ['--prepare-observations-months']):
            with self.guard(), patch.object(self.entry.sys, 'argv', ['entrypoint.py', *args]), \
                 patch.object(self.entry.Supervisor, 'layout') as layout, \
                 patch.object(self.entry.Supervisor, 'birth') as birth, \
                 patch.object(self.entry.Supervisor, 'supervise') as supervise:
                self.assertEqual(self.entry.main(), 1)
                layout.assert_not_called();birth.assert_not_called();supervise.assert_not_called()
            self.assertEqual((self.admin/'birth.json').read_bytes(), self.birth_bytes)

    def test_unsealed_old_birth_and_fresh_target_guard_do_not_change_state(self):
        with self.guard():self.entry.refuse_source_seal()
        self.assertEqual((self.admin/'birth.json').read_bytes(), self.birth_bytes)
        fresh = self.root/'fresh-target';fresh.mkdir(mode=0o755)
        with patch.multiple(self.entry, SAV=fresh, STATE=fresh/'admin'):
            self.entry.refuse_source_seal()
        self.assertEqual(list(fresh.iterdir()), [])
        # No parser fallback: malformed files, directories and dangling symlinks all block.
        seal = self.admin/'source-seal.json'
        for kind in ('malformed', 'directory', 'symlink'):
            if kind == 'directory':seal.mkdir()
            elif kind == 'symlink':seal.symlink_to('absent')
            else:seal.write_bytes(b'{partial')
            with self.guard(), self.assertRaises(self.entry.Refusal):self.entry.refuse_source_seal()
            if kind == 'directory':seal.rmdir()
            else:seal.unlink()

    def test_wrong_source_old_bundle_old_artifact_and_changed_dump_refuse_without_marker(self):
        variants = []
        old = copy.deepcopy(self.manifest);old.pop('source');variants.append(old)
        wrong = copy.deepcopy(self.manifest);wrong['source']['config']['container'] = 'other';variants.append(wrong)
        old_pin = copy.deepcopy(self.manifest);old_pin['provenance']['sha256']['entrypoint.py'] = self.host.MONTHLY_ENTRYPOINT_SHA256;variants.append(old_pin)
        wrong_life = copy.deepcopy(self.manifest);wrong_life['source']['lifecycle_id'] = '6'*64;variants.append(wrong_life)
        wrong_sql = copy.deepcopy(self.manifest);wrong_sql['sql']['sha256'] = '0'*64;variants.append(wrong_sql)
        for m in variants:
            self.rewrite_manifest(m)
            with self.assertRaises(RuntimeError):self.host.seal_source(self.node, self.bundle, self.receipt)
            self.assertFalse((self.admin/'source-seal.json').exists())
            self.assertEqual((self.admin/'birth.json').read_bytes(), self.birth_bytes)

    def test_running_source_changed_configuration_and_wrong_incarnation_refuse(self):
        for field, value in [('Running', True), ('Pid', 999), ('ExitCode', 1),
                             ('StartedAt', 'another-incarnation'), ('FinishedAt', '')]:
            i = copy.deepcopy(self.inspection);i['State'][field] = value
            with patch.object(self.node, 'validate', return_value=i), self.assertRaises(RuntimeError):
                self.host.seal_source(self.node, self.bundle, self.receipt)
            self.assertFalse((self.admin/'source-seal.json').exists())
        p = self.root/'etc/admin/passwd';p.write_bytes(b'c'*64)
        with self.assertRaises(RuntimeError):self.host.seal_source(self.node, self.bundle, self.receipt)
        self.assertFalse((self.admin/'source-seal.json').exists())

    def test_persistence_failure_or_lost_receipt_never_removes_refusal_marker(self):
        # Actual exclusive file survives an injected fsync failure; no automatic undo.
        with patch.object(self.host.os, 'fsync', side_effect=OSError('injected sync failure')):
            with self.assertRaises(OSError):self.host.seal_source(self.node, self.bundle, self.receipt)
        self.assertTrue((self.admin/'source-seal.json').exists())
        with self.guard(), self.assertRaises(self.entry.Refusal):self.entry.refuse_source_seal()
        self.assertEqual((self.admin/'birth.json').read_bytes(), self.birth_bytes)

    def test_lost_external_receipt_leaves_committed_seal_and_startup_blocked(self):
        original = self.host.json_new
        def publish(path, value):
            if path == self.receipt:
                raise OSError('injected receipt failure')
            return original(path, value)
        with patch.object(self.host, 'json_new', side_effect=publish):
            with self.assertRaises(OSError):self.host.seal_source(self.node, self.bundle, self.receipt)
        self.assertEqual(json.loads((self.admin/'source-seal.json').read_text())['status'], 'source-sealed')
        with self.guard(), self.assertRaises(self.entry.Refusal):self.entry.refuse_source_seal()
        self.assertFalse(self.receipt.exists())

    def test_accepted_legacy_restore_pins_remain_and_target_payload_excludes_admin_seal(self):
        for pin in (self.host.ADOPTION_ENTRYPOINT_SHA256, self.host.MONTHLY_ENTRYPOINT_SHA256,
                    self.host.FENCED_ENTRYPOINT_SHA256):
            self.host.restore_artifact({'image': self.config['image'],
                'provenance': {'sha256': {'entrypoint.py': pin}}}, self.config['image'])
        self.host.seal_source(self.node, self.bundle, self.receipt)
        manifest, sql, saved = self.host.load_bundle(self.bundle)
        self.assertEqual(saved, {})
        self.assertEqual(set(p.name for p in self.bundle.iterdir()), {'manifest.json', 'logical.sql', 'files'})
        self.assertIn('source', manifest)  # Binding is metadata; never a saved admin tree.
        target = self.root/'target';target.mkdir(mode=0o755)
        (target/'admin').mkdir(mode=0o700)
        with patch.multiple(self.entry, SAV=target, STATE=target/'admin'):
            self.entry.refuse_source_seal()
        self.assertEqual(list((target/'admin').iterdir()), [])


if __name__ == '__main__':
    unittest.main()
