"""Fault-boundary tests for the manual transfer helper; execute on an authorized CT."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import signal
import stat
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


host = module('logical_transfer', 'logical-transfer.py')
native = module('logical_transfer_native', 'logical-transfer-native.py')


@unittest.skipUnless(os.getuid() == 0 and os.environ.get('PODMESH_TRANSFER_TEST_ROOT'),
                     'explicit disposable root-owned CT fixture required')
class ManualLogicalTransfer(unittest.TestCase):
    def setUp(self):
        root = host.protected(Path(os.environ['PODMESH_TRANSFER_TEST_ROOT']), True)
        self.temp = tempfile.TemporaryDirectory(prefix='transfer-test-', dir=root)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_export_error_releases_only_its_owned_quiescence(self):
        provider = self.root / 'provider'
        provider.mkdir()
        config = {'container': 'explicit-node', 'image': 'a' * 64,
                  'etc_root': str(self.root / 'etc'), 'sav_root': str(self.root / 'sav'),
                  'provider_dir': str(provider)}
        config_path = self.root / 'config.json'
        host.json_new(config_path, config)
        self.assertEqual(host.configuration(config_path), config)
        self.assertEqual(set(config), {'container', 'image', 'etc_root', 'sav_root', 'provider_dir'})
        launch = host.Node(config)
        self.assertEqual(launch.birth(), {})
        self.assertFalse((Path(config['sav_root']) / 'admin/birth.json').exists())
        with mock.patch.object(launch, 'podman', return_value=mock.Mock(stdout=b'owned-target\n')) as engine, \
                mock.patch.object(launch, 'validate'), \
                mock.patch.object(launch, 'inspect', return_value={'Id': 'owned-target', 'State': {
                    'Running': True, 'Pid': 4321, 'StartedAt': '2026-10-24T01:00:00Z'}}), \
                mock.patch.object(launch, 'birth', return_value={'phase': 'running'}):
            launch.create()
            argv = engine.call_args_list[0].args
            for required in ('--pull=never', '--image-volume=ignore', '--network=none', '--stop-timeout=60'):
                self.assertIn(required, argv)
            self.assertFalse(any(a.startswith(('--read-only', '--cap-', '--memory', '--cpus', '--pids-limit'))
                                 for a in argv))
        started_lifecycle = copy.deepcopy(launch.lifecycle)
        old = {'Id': 'owned-target', 'Config': {'Labels': {host.LABEL: launch.token}},
               'State': {'Running': False, 'Pid': 0, 'ExitCode': 0, 'Status': 'exited',
                         'StartedAt': '2026-10-24T01:00:00Z', 'FinishedAt': '2026-10-24T01:00:10Z'}}
        with mock.patch.object(launch, 'inspect', return_value=old), \
                mock.patch.object(launch, 'birth', return_value={'phase': 'stopped-clean'}), \
                mock.patch.object(launch, 'podman', return_value=mock.Mock(stdout=b'never-started\n')), \
                mock.patch.object(launch, 'validate', side_effect=RuntimeError('new container validation failed')):
            self.assertTrue(launch.stop()['clean_lifecycle'])
            with self.assertRaisesRegex(RuntimeError, 'new container validation failed'):
                launch.recreate()
        created = {'Id': 'never-started', 'State': {'Running': False, 'Pid': 0, 'ExitCode': 0,
                   'Status': 'created', 'StartedAt': '0001-01-01T00:00:00Z',
                   'FinishedAt': '0001-01-01T00:00:00Z'}}
        with mock.patch.object(launch, 'inspect', return_value=created), \
                mock.patch.object(launch, 'birth', return_value={'phase': 'stopped-clean'}) as stale:
            cleanup = launch.stop()
            self.assertEqual(cleanup['status'], 'NEVER_STARTED')
            self.assertTrue(cleanup['container_stopped'])
            self.assertFalse(cleanup['clean_lifecycle'])
            stale.assert_not_called()
        changed = copy.deepcopy(old)
        launch.cid = 'owned-target'
        launch.lifecycle = started_lifecycle
        changed['State']['StartedAt'] = '2026-10-24T02:00:00Z'
        with mock.patch.object(launch, 'inspect', return_value=changed):
            self.assertFalse(launch.stop()['clean_lifecycle'])
        app = {'pid': 987654, 'start': '12345'}
        runtime = self.root / 'runtime'
        runtime.mkdir(mode=0o700)
        with mock.patch.object(native, 'RUNTIME', runtime), \
                mock.patch.object(native, 'app_process', return_value=app), \
                mock.patch.object(Path, 'read_text', side_effect=['State:\tS (sleeping)\n', 'State:\tT (stopped)\n']), \
                mock.patch.object(native.os, 'kill') as kill:
            with self.assertRaisesRegex(RuntimeError, 'injected dump error'):
                with native.quiesced('owned-export'):
                    raise RuntimeError('injected dump error')
            self.assertEqual(kill.call_args_list,
                             [mock.call(app['pid'], signal.SIGSTOP), mock.call(app['pid'], signal.SIGCONT)])
            self.assertFalse((runtime / 'logical-transfer-pause.json').exists())
        # The outer host path also resumes on a lost/failed exec response and cannot publish completion.
        etc, sav = self.root / 'etc', self.root / 'sav'
        etc.mkdir(); sav.mkdir(); (sav / 'admin').mkdir(mode=0o700)
        node = mock.Mock(c={'etc_root': str(etc), 'sav_root': str(sav)})
        node.validate.return_value = {'Id': 'owned-source', 'State': {'Running': True}}
        node.native.side_effect = RuntimeError('lost exec response')
        with self.assertRaisesRegex(RuntimeError, 'lost exec response'):
            host.export(node, self.root / 'partial-export', [])
        node.resume.assert_called_once()
        self.assertFalse((self.root / 'partial-export/manifest.json').exists())
        node.stop.assert_not_called()

    def test_tampered_completed_bundle_and_existing_target_refuse_before_mutation(self):
        bundle = self.root / 'bundle'
        bundle.mkdir(mode=0o700); (bundle / 'files').mkdir(mode=0o700)
        payload = b'-- trusted controlled SQL fixture\n'
        host.write_new(bundle / 'logical.sql', payload)
        app = self.root / 'functional/sav/app'
        app.mkdir(parents=True)
        native.write_new(app / 'marker', b'regular saved marker\n', 1102, 1102, 0o644)
        with mock.patch.object(native, 'HOME', self.root / 'functional'):
            saved = native.files(['marker'])
        self.assertEqual({k: saved['marker'][k] for k in ('uid', 'gid', 'mode')},
                         {'uid': 1102, 'gid': 1102, 'mode': 0o644})
        host.write_new(bundle / 'files/marker', b'regular saved marker\n')
        self.assertEqual(stat.S_IMODE((bundle / 'files/marker').stat().st_mode), 0o600)
        manifest = {'format': host.FORMAT, 'status': 'completed-coherent-export',
                    'sql': {'bytes': len(payload), 'sha256': host.sha(payload)},
                    'files': host.files_manifest(saved), 'image': 'a' * 64,
                    'provenance': {'sha256': {'entrypoint.py': host.ADOPTION_ENTRYPOINT_SHA256}}}
        host.json_new(bundle / 'manifest.json', manifest)
        self.assertEqual(host.load_bundle(bundle)[2], saved)
        node = mock.Mock(c={'image': 'a' * 64, 'etc_root': str(self.root / 'already-owned'),
                            'sav_root': str(self.root / 'absent-sav')})
        (self.root / 'already-owned').mkdir()
        with self.assertRaisesRegex(RuntimeError, 'already exists'):
            host.restore(node, bundle, self.root / 'receipt.json', False)
        node.podman.assert_not_called()
        (bundle / 'logical.sql').write_bytes(b'-- altered SQL\n')
        with self.assertRaisesRegex(RuntimeError, 'checksum differs'):
            host.restore(node, bundle, self.root / 'receipt.json', False)
        node.podman.assert_not_called()
        self.assertFalse((self.root / 'absent-sav').exists())
        self.assertFalse((self.root / 'receipt.json').exists())
        # A failed final owned-container stop never asserts retained SQL stability.
        failed = mock.Mock(c={'image': 'a' * 64, 'etc_root': str(self.root / 'new-etc'),
                              'sav_root': str(self.root / 'new-sav')}, cid='incomplete', lifecycle=None)
        def engine(*args, **kwargs):
            if args[:2] == ('container', 'exists'):
                return mock.Mock(returncode=1)
            return mock.Mock(returncode=0, stdout=json.dumps([{'Id': 'a' * 64}]).encode())
        failed.podman.side_effect = engine
        failed.create.side_effect = RuntimeError('injected create failure')
        failed.stop.return_value = {'status': 'UNSTABLE_OR_UNVERIFIED_TARGET',
                                    'container_stopped': False, 'clean_lifecycle': False, 'errors': ['stop-failed']}
        with mock.patch.object(host, 'load_bundle', return_value=(manifest, payload, saved)):
            with self.assertRaisesRegex(RuntimeError, 'injected create failure'):
                host.restore(failed, bundle, self.root / 'failed-receipt.json', False)
        receipt = json.loads((self.root / 'failed-receipt.json').read_text())
        self.assertEqual(receipt['status'], 'FAILED_OR_INCOMPLETE')
        self.assertFalse(receipt['owned_target_stopped_cleanly'])
        self.assertIn('do not assume retained SQL state is stable', receipt['retained_state_warning'])
        # Child-style direct FD writes are observed; only this call's empty success diagnostic is removed.
        admin, runtime = self.root / 'diagnostics', self.root / 'sql-runtime'
        admin.mkdir(); runtime.mkdir()
        native.write_new(admin / 'old-empty', b'')
        def child(args, **kwargs):
            os.write(kwargs['stdout'].fileno(), b'actual-fd-bytes')
            return mock.Mock(returncode=0)
        with mock.patch.object(native, 'ADMIN', admin), mock.patch.object(native, 'RUNTIME', runtime), \
                mock.patch.object(native.subprocess, 'run', side_effect=child):
            self.assertEqual(native.run(['/controlled/unit-boundary']), b'actual-fd-bytes')
        self.assertEqual([p.name for p in admin.iterdir()], ['old-empty'])
        def failed_child(args, **kwargs):
            os.write(kwargs['stderr'].fileno(), b'private failure diagnostic')
            return mock.Mock(returncode=1)
        with mock.patch.object(native, 'ADMIN', admin), mock.patch.object(native, 'RUNTIME', runtime), \
                mock.patch.object(native.subprocess, 'run', side_effect=failed_child):
            with self.assertRaisesRegex(RuntimeError, 'native command failed'):
                native.run(['/controlled/unit-boundary'])
        retained = [p for p in admin.iterdir() if p.name != 'old-empty']
        self.assertEqual(len(retained), 1)
        self.assertEqual(retained[0].read_bytes(), b'private failure diagnostic')
        with mock.patch.object(native, 'ADMIN', admin), mock.patch.object(native, 'RUNTIME', runtime), \
                mock.patch.object(native, 'LIMIT', 4), \
                mock.patch.object(native.subprocess, 'run', side_effect=child):
            with self.assertRaisesRegex(RuntimeError, 'native result exceeds fixture bound'):
                native.run(['/controlled/unit-boundary'])

    def test_only_machine_binding_and_one_audit_may_differ_after_adoption(self):
        artifact = {'image': 'a' * 64,
                    'provenance': {'sha256': {'entrypoint.py': host.ADOPTION_ENTRYPOINT_SHA256}}}
        host.restore_artifact(artifact, 'a' * 64)
        old_base = copy.deepcopy(artifact)
        old_base['provenance']['sha256']['entrypoint.py'] = '658df23e6552b26cad1ab50d353e4a55ab35987eb04204c67a31679eb55e1ea0'
        with self.assertRaisesRegex(RuntimeError, 'embed the pinned adoption entrypoint'):
            host.restore_artifact(old_base, 'a' * 64)
        before = {'identity': {'host_uuid': 'retained-uuid', 'machine_id_sha256': 'old'},
                  'schema_sha256': 'schema', 'objects': [], 'filtered': {'original': 'payload'},
                  'tables': {'metadata': {'rows': 3}, 'observations': {'rows': 3},
                             'store_schema': {'rows': 1, 'sha256': 'unchanged'}}, 'audits': []}
        request = {'status': 'restored-and-compared', 'export_sha256': 'bound-export'}
        after = copy.deepcopy(before)
        after['identity']['machine_id_sha256'] = 'actual-target'
        after['tables']['observations']['rows'] += 1
        after['audits'] = [dict(request, observed_at=123, operation='explicit-restored-host-identity-adoption')]
        host.adoption_delta(before, after, 'actual-target', request)
        after['tables']['store_schema']['sha256'] = 'lost-original-data'
        with self.assertRaisesRegex(RuntimeError, 'original table changed'):
            host.adoption_delta(before, after, 'actual-target', request)
        after = copy.deepcopy(before)
        with self.assertRaisesRegex(RuntimeError, 'restored identity differs'):
            host.adoption_delta(before, after, 'actual-target', request)


if __name__ == '__main__':
    unittest.main()
