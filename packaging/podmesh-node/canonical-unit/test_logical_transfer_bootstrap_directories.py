"""Focused build-CT filesystem/protocol tests; author-only, no native SQL/Podman.
The fixture has actual UID/GID1102 directories and an ordinary0644 regular witness.
SQL, provider and quiescence are replaced only to exercise transfer plumbing.
"""
import base64
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
NAME = 'checkpoint158-witness'
PAYLOAD = b'authored checkpoint fixture\n'  # Not the private158 witness contents.


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), HERE / name)
    module = importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module


@contextlib.contextmanager
def fixture(module, parent, witness):
    home = parent / 'node';home.mkdir(mode=0o700)
    for part in ('etc', 'sav', 'sav/app', 'sav/admin', 'lib'):
        (home / part).mkdir(mode=0o700)
    app = home / 'sav/app';os.chown(app, 1102, 1102)
    for name in module.BOOTSTRAP_DIRS:
        p = app / name;p.mkdir(mode=0o700);os.chown(p, 1102, 1102);p.chmod(0o700)
    if witness:
        p = app / NAME;p.write_bytes(PAYLOAD);os.chown(p, 1102, 1102);p.chmod(0o644)
    artifact = b'unit fixture, not a qualified image'
    (home / 'lib/entrypoint.py').write_bytes(artifact)
    digest = hashlib.sha256(artifact).hexdigest()
    birth = {'phase': 'running', 'configuration': {}, 'provenance': {'sha256': {'entrypoint.py': digest}}}
    (home / 'sav/admin/birth.json').write_text(json.dumps(birth));(home / 'sav/admin/birth.json').chmod(0o600)
    inventory = {'identity': {'machine_id_sha256': 'fixture-provider'}}
    with patch.multiple(module, HOME=home, ADMIN=home/'sav/admin', LIB=home/'lib',
                        ADOPTION_ENTRYPOINT_SHA256=digest), \
         patch.object(module, 'quiesced', side_effect=lambda _token: contextlib.nullcontext()), \
         patch.object(module, 'inventory', return_value=inventory), \
         patch.object(module, 'provider_machine_sha256', return_value='fixture-provider'), \
         patch.object(module, 'dump', return_value=b'fixture SQL, not executed'):
        yield home, inventory


class BootstrapDirectories(unittest.TestCase):
    def setUp(self):
        self.assertEqual(os.geteuid(), 0, 'run as root on the declared build CT')
        self.native = load('logical-transfer-native.py')
        self.host = load('logical-transfer.py')

    def test_export_readback_and_host_bundle_include_exact_empty_directory_metadata(self):
        with tempfile.TemporaryDirectory(prefix='bootstrap-export-', dir='/root') as directory:
            root = Path(directory)
            with fixture(self.native, root, True) as (home, inventory):
                exported = self.native.main({'mode': 'export', 'token': 'fixture', 'files': [NAME]})
                readback = self.native.main({'mode': 'readback', 'token': 'fixture', 'files': [NAME]})
                expected = {n: {'type': 'directory', 'uid': 1102, 'gid': 1102, 'mode': 0o700, 'empty': True}
                            for n in self.native.BOOTSTRAP_DIRS}
                self.assertEqual(exported['directories'], expected)
                self.assertEqual(readback['directories'], expected)
                self.assertEqual(exported['files'], readback['files'])
                self.assertEqual(exported['files'][NAME], {'data': base64.b64encode(PAYLOAD).decode(),
                                                         'uid': 1102, 'gid': 1102, 'mode': 0o644})
                node = types.SimpleNamespace(c={'etc_root': str(home/'etc'), 'sav_root': str(home/'sav'),
                    'image': '1'*64}, worker='fixture worker', native=Mock(return_value=exported),
                    resume=Mock(), validate=Mock(return_value={'Id': 'fixture', 'State': {'Running': True}}))
                output = root / 'bundle'
                result = self.host.export(node, output, [NAME])
                self.assertEqual(result['status'], 'COMPLETED_COHERENT_EXPORT')
                manifest, sql, saved = self.host.load_bundle(output)
                self.assertEqual(manifest['directories'], expected)
                self.assertEqual(sql, b'fixture SQL, not executed')
                self.assertEqual(saved, exported['files'])
                self.assertEqual(manifest['files'][NAME]['mode'], 0o644)
                self.assertEqual(manifest['files'][NAME]['uid'], 1102)
                self.assertEqual(sorted(p.name for p in (output/'files').iterdir()), [NAME])
                node.resume.assert_called_once_with()
                for changed in (None, {}, dict(expected, extra=expected['inbox']),
                                dict(expected, inbox=dict(expected['inbox'], empty=False)),
                                dict(expected, inbox=dict(expected['inbox'], type='symlink'))):
                    with self.assertRaises(RuntimeError): self.host.directories_manifest(changed)

    def test_restore_preserves_native_directory_inodes_and_exact_regular_witness(self):
        with tempfile.TemporaryDirectory(prefix='bootstrap-restore-', dir='/root') as directory:
            root = Path(directory)
            with fixture(self.native, root, False) as (home, inventory):
                directories = self.native.bootstrap_directories()
                before = {n: (home/'sav/app'/n).stat().st_ino for n in directories}
                saved = {NAME: {'data': base64.b64encode(PAYLOAD).decode(), 'uid': 1102, 'gid': 1102, 'mode': 0o644}}
                request = {'mode': 'restore', 'token': 'fixture', 'files': [NAME], 'saved': saved,
                           'directories': directories, 'fresh_identity': {'fixture': True},
                           'sql': base64.b64encode(b'fixture SQL').decode(), 'inventory': inventory}
                query = Mock(side_effect=lambda q: '0\n' if q.startswith('SELECT COUNT(*)') else '')
                with patch.object(self.native, 'identity', return_value=request['fresh_identity']), \
                     patch.object(self.native, 'sql', query), \
                     patch.object(self.native, 'client', side_effect=lambda: contextlib.nullcontext('fixture.cnf')), \
                     patch.object(self.native, 'run', return_value=b'') as importer:
                    result = self.native.main(request)
                self.assertTrue(result['compared']);importer.assert_called_once()
                self.assertEqual(query.call_args_list[0].args[0], 'DROP DATABASE `podmesh-node`;')
                self.assertEqual(self.native.bootstrap_directories(), directories)
                self.assertEqual(before, {n: (home/'sav/app'/n).stat().st_ino for n in directories})
                self.assertEqual(self.native.files([NAME]), saved)
                self.assertEqual((home/'sav/app'/NAME).read_bytes(), PAYLOAD)
                witness = (home/'sav/app'/NAME).stat()
                self.assertEqual((witness.st_uid, witness.st_gid, witness.st_mode & 0o777), (1102, 1102, 0o644))
                readback = self.native.main({'mode': 'readback', 'token': 'fixture', 'files': [NAME]})
                self.assertEqual(readback['files'], saved);self.assertEqual(readback['directories'], directories)

    def test_nonempty_extra_symlink_and_target_metadata_mismatch_refuse_before_sql(self):
        cases = ('nested-file', 'nested-dir', 'extra-dir', 'extra-file', 'directory-symlink', 'witness-symlink')
        for case in cases:
            with self.subTest(case=case), tempfile.TemporaryDirectory(prefix='bootstrap-refusal-', dir='/root') as directory:
                root = Path(directory)
                with fixture(self.native, root, True) as (home, _inventory):
                    app = home/'sav/app'
                    if case == 'nested-file': (app/'inbox/private-content').write_bytes(b'not to be transferred')
                    elif case == 'nested-dir': (app/'inbox/nested').mkdir()
                    elif case == 'extra-dir': (app/'undeclared').mkdir()
                    elif case == 'extra-file': (app/'undeclared').write_bytes(b'not declared')
                    elif case == 'directory-symlink':
                        (app/'inbox').rmdir();(root/'outside').mkdir();(app/'inbox').symlink_to(root/'outside', target_is_directory=True)
                    else:
                        (app/NAME).unlink();(root/'outside').write_bytes(b'not followed');(app/NAME).symlink_to(root/'outside')
                    with self.assertRaises(RuntimeError): self.native.files([NAME])
        with tempfile.TemporaryDirectory(prefix='bootstrap-target-mismatch-', dir='/root') as directory:
            with fixture(self.native, Path(directory), False) as (home, inventory):
                expected = self.native.bootstrap_directories();(home/'sav/app/inbox').chmod(0o750)
                request = {'mode': 'restore', 'token': 'fixture', 'files': [], 'saved': {},
                           'directories': expected, 'fresh_identity': {}, 'sql': '', 'inventory': inventory}
                with patch.object(self.native, 'sql') as sql:
                    with self.assertRaises(RuntimeError): self.native.main(request)
                    sql.assert_not_called()
                self.assertEqual((home/'sav/app/inbox').stat().st_mode & 0o777, 0o750)


if __name__ == '__main__':
    unittest.main()
