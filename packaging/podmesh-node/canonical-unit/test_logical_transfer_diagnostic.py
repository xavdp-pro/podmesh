"""Focused error-boundary tests for the build CT; no Podman, database or service.
Source-only candidate: never execute/import this file on the operator workstation.
"""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import runpy
import signal
import sys
import tempfile
import types
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent


def load(filename):
    spec = importlib.util.spec_from_file_location(filename.replace('-', '_'), HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class FailureDiagnostics(unittest.TestCase):
    def test_native_failure_locations_are_safe_and_only_validated_evidence_is_forwarded(self):
        native = load('logical-transfer-native.py')
        host = load('logical-transfer.py')
        secret = 'DO_NOT_RENDER_SQL_ROW_PASSWORD_CONFIG_OR_ARGV'
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / secret
            native._stage('export.inventory.before')
            error_output = io.StringIO()
            try:
                native.regular(path, 0, 0o600)
            except FileNotFoundError as error:
                with contextlib.redirect_stderr(error_output): native._emit_failure(error)
            text = error_output.getvalue()
            self.assertNotIn(secret, text)
            self.assertNotIn(str(HERE), text)
            record = json.loads(text.removeprefix(native._DIAGNOSTIC_PREFIX))
            self.assertEqual(record['exception'], 'FileNotFoundError')
            self.assertEqual(record['stage'], 'export.inventory.before')
            self.assertTrue(any(v['function'] == 'regular' for v in record['frames']))
            self.assertTrue(all(v['source'] == 'logical-transfer-native.py' and v['line'] > 0
                                for v in record['frames']))
            forwarded = io.StringIO()
            with contextlib.redirect_stderr(forwarded):
                host._forward_native_failure((secret + '\n' + text).encode())
            self.assertEqual(forwarded.getvalue(), text)
            for change in ({'extra': secret}, {'stage': secret}, {'exception': secret},
                           {'frames': [{'source': secret, 'function': 'regular', 'line': 1}]},
                           {'frames': [{'source': 'logical-transfer-native.py', 'function': secret, 'line': 1}]}):
                invalid = dict(record, **change)
                rejected = io.StringIO()
                with contextlib.redirect_stderr(rejected):
                    host._forward_native_failure((host._DIAGNOSTIC_PREFIX + json.dumps(invalid)).encode())
                self.assertEqual(rejected.getvalue(), '')
            unknown = type(secret, (Exception,), {})(secret)
            self.assertEqual(native._failure_record(unknown)['exception'], 'Exception')

    def test_export_preserves_first_exception_partial_bundle_and_attempts_resume(self):
        host = load('logical-transfer.py')
        primary = RuntimeError('PRIVATE_SQL_ROW')
        secondary = PermissionError('PRIVATE_CREDENTIAL_PATH')
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder);sav = root / 'sav';(sav / 'admin').mkdir(parents=True)
            output = root / 'bundle'
            node = types.SimpleNamespace(c={'etc_root': str(root / 'etc'), 'sav_root': str(sav)},
                validate=Mock(return_value={'Id': 'own', 'State': {'Running': True}}),
                native=Mock(side_effect=primary), resume=Mock(side_effect=secondary))
            stderr = io.StringIO()
            with patch.object(host, 'absent', return_value=output), patch.object(host, 'protected'), \
                 contextlib.redirect_stderr(stderr):
                with self.assertRaises(RuntimeError) as got: host.export(node, output, [])
            self.assertIs(got.exception, primary)
            node.resume.assert_called_once_with()
            self.assertEqual(sorted(p.name for p in output.iterdir()), ['files'])
            text = stderr.getvalue()
            self.assertNotIn('PRIVATE_SQL_ROW', text);self.assertNotIn('PRIVATE_CREDENTIAL_PATH', text)
            record = json.loads(text.removeprefix(host._DIAGNOSTIC_PREFIX))
            self.assertTrue(record['secondary']);self.assertEqual(record['stage'], 'cleanup.resume')
            self.assertEqual(record['exception'], 'PermissionError')
            self.assertEqual(host._STAGE, 'export.lock')

    def test_native_quiesce_resume_failure_does_not_mask_primary(self):
        native = load('logical-transfer-native.py')
        primary = ValueError('PRIVATE_DUMP_BYTES')
        stderr = io.StringIO()
        with patch.object(native, 'app_process', return_value={'pid': 123, 'start': 'fixed'}), \
             patch.object(native, 'write_new'), patch.object(Path, 'read_text', side_effect=['State:\tS\n', 'State:\tT\n']), \
             patch.object(native.os, 'kill'), patch.object(native, 'resume', side_effect=OSError('PRIVATE_TOKEN')) as resume, \
             contextlib.redirect_stderr(stderr):
            with self.assertRaises(ValueError) as got:
                with native.quiesced('PRIVATE_TOKEN'):
                    native._stage('export.dump')
                    raise primary
        self.assertIs(got.exception, primary);resume.assert_called_once_with('PRIVATE_TOKEN')
        self.assertEqual(native._STAGE, 'export.dump')
        self.assertNotIn('PRIVATE_', stderr.getvalue())
        # A cleanup-only failure remains an error; successful cleanup stays quiet.
        for module in (native, load('logical-transfer.py')):
            cleanup_error = OSError('PRIVATE_ONLY_CLEANUP')
            with self.assertRaises(OSError) as got:
                module._cleanup(Mock(side_effect=cleanup_error), 'cleanup.resume')
            self.assertIs(got.exception, cleanup_error)
            output = io.StringIO();operation = Mock()
            with contextlib.redirect_stderr(output): module._cleanup(operation, 'cleanup.resume')
            operation.assert_called_once_with();self.assertEqual(output.getvalue(), '')

    def test_restore_receipt_write_cannot_replace_first_target_failure(self):
        host = load('logical-transfer.py')
        primary = RuntimeError('PRIVATE_TARGET_FAILURE')
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            node = types.SimpleNamespace(c={'image': 'a' * 64, 'container': 'own',
                'etc_root': str(root / 'etc'), 'sav_root': str(root / 'sav')},
                create=Mock(side_effect=primary), cid='own', lifecycle=None,
                stop=Mock(return_value={'status': 'STOPPED_WITH_INCOMPLETE_LIFECYCLE',
                    'container_stopped': True, 'clean_lifecycle': False, 'errors': []}))
            def podman(*args, **kwargs):
                if args[:2] == ('container', 'exists'): return types.SimpleNamespace(returncode=1)
                if args[:2] == ('image', 'inspect'):
                    return types.SimpleNamespace(stdout=json.dumps([{'Id': 'a' * 64}]).encode())
                return types.SimpleNamespace(returncode=0)
            node.podman = podman
            stderr = io.StringIO()
            with patch.object(host, 'load_bundle', return_value=({'sql': {}}, b'', {})), \
                 patch.object(host, 'restore_artifact'), patch.object(host, 'absent', side_effect=Path), \
                 patch.object(host, 'json_new', side_effect=OSError('PRIVATE_RECEIPT_FAILURE')), \
                 contextlib.redirect_stderr(stderr):
                with self.assertRaises(RuntimeError) as got:
                    host.restore(node, root / 'bundle', root / 'receipt', False)
            self.assertIs(got.exception, primary);node.stop.assert_called_once_with()
            self.assertEqual(host._STAGE, 'restore.new-target')
            self.assertNotIn('PRIVATE_', stderr.getvalue())
            record = json.loads(stderr.getvalue().removeprefix(host._DIAGNOSTIC_PREFIX))
            self.assertTrue(record['secondary']);self.assertEqual(record['stage'], 'cleanup.receipt')

    def test_cli_failure_stays_generic_exit_one_and_success_output_stays_unchanged(self):
        # Exercise the real entrypoint boundaries; replace only safe fixture inputs
        # and suppress signal installation. No engine/process/SQL is invoked.
        secret = 'PRIVATE_INPUT_MUST_NOT_APPEAR'
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(sys, 'argv', ['logical-transfer.py', '--config', '/missing/'+secret,
                 '--max-payload-bytes', '1024', 'export', '--output', '/missing/bundle']), \
             patch.object(signal, 'signal'), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            with self.assertRaises(SystemExit) as got:
                runpy.run_path(str(HERE / 'logical-transfer.py'), run_name='__main__')
        self.assertEqual(got.exception.code, 1);self.assertEqual(stdout.getvalue(), '')
        self.assertTrue(stderr.getvalue().startswith('logical transfer incomplete;'))
        self.assertIn('logical-transfer-diagnostic ', stderr.getvalue())
        self.assertNotIn(secret, stderr.getvalue());self.assertNotIn('Traceback', stderr.getvalue())
        host = load('logical-transfer.py')
        result = {'status': 'COMPLETED_COHERENT_EXPORT', 'output': '/own/bundle'}
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(sys, 'argv', ['logical-transfer.py', '--config', '/own/config',
                 '--max-payload-bytes', '1024', 'export', '--output', '/own/bundle']), \
             patch.object(host, 'configuration', return_value={}), patch.object(host, 'Node'), \
             patch.object(host, 'export', return_value=result), patch('os.getuid', return_value=0), \
             patch('os.getgid', return_value=0), patch('os.umask'), \
             contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            self.assertIsNone(host.main())
        self.assertEqual(json.loads(stdout.getvalue()), result);self.assertEqual(stderr.getvalue(), '')
        for raw, fails in ((b'{"PRIVATE_JSON":', True), (b'{"mode":"resume","token":"PRIVATE_TOKEN"}', False)):
            stdout, stderr = io.StringIO(), io.StringIO()
            with patch.object(sys, 'argv', ['logical-transfer-native.py', '1024']), \
                 patch.object(sys, 'stdin', types.SimpleNamespace(buffer=io.BytesIO(raw))), \
                 patch.object(signal, 'signal'), patch.object(signal, 'alarm'), \
                 patch('os.getuid', return_value=0), patch('os.getgid', return_value=0), \
                 patch.object(Path, 'exists', return_value=False), \
                 contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                if fails:
                    with self.assertRaises(SystemExit) as got:
                        runpy.run_path(str(HERE / 'logical-transfer-native.py'), run_name='__main__')
                    self.assertEqual(got.exception.code, 1);self.assertEqual(stdout.getvalue(), '')
                    self.assertTrue(stderr.getvalue().startswith('logical transfer native step failed;'))
                    self.assertNotIn('PRIVATE_', stderr.getvalue())
                else:
                    runpy.run_path(str(HERE / 'logical-transfer-native.py'), run_name='__main__')
                    self.assertEqual(json.loads(stdout.getvalue()), {'resumed': True})
                    self.assertEqual(stderr.getvalue(), '')


if __name__ == '__main__':
    unittest.main()
