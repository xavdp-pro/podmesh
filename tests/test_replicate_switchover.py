#!/usr/bin/env python3
"""The planned switchover's control flow in tools/replicate-universe.py, without a laboratory: fake hosts answer as
the API would, and each failure before the promotion must bring the universe back on the active host -- resumed
from its final capture, with the lease taken back when it had been released -- and never while the standby holds a
container for the universe. Run: python3 -B tests/test_replicate_switchover.py"""
import importlib.util, pathlib, sys, types, unittest, uuid

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tests'))
spec = importlib.util.spec_from_file_location('replicate', ROOT / 'tools' / 'replicate-universe.py')
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)

U = str(uuid.uuid4())
POINT = str(uuid.uuid4())


class FakeHost:
    def __init__(self, role, answers, container=None):
        self.role, self.identity, self.answers, self.container, self.sent = role, str(uuid.uuid4()), answers, container, []

    def api(self, req):
        self.sent.append(req)
        answer = self.answers.get(req['operation'], {'ok': True, 'data': {}})
        return answer(req) if callable(answer) else answer

    def call(self, function, **arguments):
        assert function == 'inspect'
        return {'container': self.container}


def final_capture(_req):
    return {'ok': True, 'data': {'recovery_point_uuid': POINT, 'generation': 3, 'final': True, 'capture': {'dump_seconds': 0.5}}}


def switch(A, B, transfer=lambda *a, **k: {'files': {}}):
    tool.transfer = transfer
    args = types.SimpleNamespace(reference='unit-test', planned=True, standby='lab@b')
    rep = {'active': 'lab@a', 'standbys': ['lab@b'], 'capture': 'live', 'interval_seconds': 300}
    return tool.planned_switchover(args, U, rep, A, B, 180, 30)


def operations(host):
    return [r['operation'] for r in host.sent]


class PlannedSwitchover(unittest.TestCase):
    def active(self, **extra):
        answers = {'activation_status': {'ok': True, 'data': {'live': True}}, 'recovery_point_prepare': final_capture}
        answers.update(extra)
        return FakeHost('active', answers)

    def test_success_promotes_and_retires_the_old_copy(self):
        A, B = self.active(), FakeHost('standby', {'recovery_point_promote': {'ok': True, 'data': {'started': True}}})
        report = switch(A, B)
        self.assertEqual(report['data_lost'], 'nothing: the capture was final')
        prepare = next(r for r in A.sent if r['operation'] == 'recovery_point_prepare')
        self.assertEqual((prepare['capture'], prepare['resume']), ('live', False))
        self.assertEqual(operations(A), ['activation_status', 'recovery_point_prepare', 'activation_release', 'delete'])
        self.assertEqual(operations(B), ['recovery_point_stage', 'activation_require', 'activation_acquire', 'recovery_point_promote'])

    def test_a_refused_staging_resumes_the_universe_where_it_was(self):
        A, B = self.active(), FakeHost('standby', {'recovery_point_stage': {'ok': False, 'error': 'image absent'}})
        with self.assertRaises(tool.RolledBack) as e:
            switch(A, B)
        self.assertIn('staging', str(e.exception))
        self.assertEqual(operations(A), ['activation_status', 'recovery_point_prepare', 'recovery_point_resume'])
        self.assertEqual(A.sent[-1]['recovery_point_uuid'], POINT)
        self.assertNotIn('recovery_point_promote', operations(B))

    def test_a_failed_transfer_resumes_the_universe(self):
        def broken(*_a, **_k):
            raise RuntimeError('ssh failed')
        A, B = self.active(), FakeHost('standby', {})
        with self.assertRaises(tool.RolledBack):
            switch(A, B, transfer=broken)
        self.assertEqual(operations(A)[-1], 'recovery_point_resume')
        self.assertEqual(operations(B), [])

    def test_a_refused_lease_on_the_standby_takes_the_lease_back_before_resuming(self):
        A = self.active()
        B = FakeHost('standby', {'activation_acquire': {'ok': False, 'error': 'superseded'}})
        with self.assertRaises(tool.RolledBack) as e:
            switch(A, B)
        self.assertIn('lease move', str(e.exception))
        self.assertEqual(operations(A), ['activation_status', 'recovery_point_prepare', 'activation_release', 'activation_acquire', 'recovery_point_resume'])

    def test_a_refused_promotion_resumes_only_while_the_standby_holds_nothing(self):
        A = self.active()
        B = FakeHost('standby', {'recovery_point_promote': {'ok': False, 'error': 'verification failed'}})
        with self.assertRaises(tool.RolledBack):
            switch(A, B)
        self.assertEqual(operations(A)[-1], 'recovery_point_resume')

        A = self.active()
        B = FakeHost('standby', {'recovery_point_promote': {'ok': False, 'error': 'verification failed'}}, container={'Id': 'x'})
        with self.assertRaises(tool.RolledBack) as e:
            switch(A, B)
        self.assertIn('nothing was undone', str(e.exception))
        self.assertNotIn('recovery_point_resume', operations(A))
        self.assertNotIn('activation_acquire', operations(A))

    def test_a_refused_final_capture_changes_nothing_more(self):
        A = self.active(recovery_point_prepare={'ok': False, 'error': 'preconditions not met'})
        B = FakeHost('standby', {})
        with self.assertRaises(tool.Refused) as e:
            switch(A, B)
        self.assertNotIsInstance(e.exception, tool.RolledBack)
        self.assertEqual(operations(A), ['activation_status', 'recovery_point_prepare'])
        self.assertEqual(operations(B), [])


class StoppedSwitchover(unittest.TestCase):
    def switch_stopped(self, A, B):
        tool.transfer = lambda *a, **k: {'files': {}}
        args = types.SimpleNamespace(reference='unit-test', planned=True, standby='lab@b')
        rep = {'active': 'lab@a', 'standbys': ['lab@b'], 'capture': 'stopped', 'interval_seconds': 300}
        return tool.planned_switchover(args, U, rep, A, B, 180, 30)

    def active(self, **extra):
        answers = {'activation_status': {'ok': True, 'data': {'live': True}}, 'stop': {'ok': True, 'data': {'forced': False}},
                   'recovery_point_prepare': {'ok': True, 'data': {'recovery_point_uuid': POINT, 'generation': 2}}}
        answers.update(extra)
        return FakeHost('active', answers)

    def test_success_stops_captures_restores_promotes_and_starts(self):
        A = self.active()
        B = FakeHost('standby', {'recovery_point_promote': {'ok': True, 'data': {}}})
        report = self.switch_stopped(A, B)
        self.assertFalse(report['with_memory'])
        self.assertEqual(operations(A), ['activation_status', 'stop', 'recovery_point_prepare', 'activation_release', 'delete'])
        self.assertEqual(operations(B), ['recovery_point_restore', 'activation_require', 'activation_acquire', 'recovery_point_promote', 'start'])

    def test_a_refused_restore_starts_the_universe_again_and_removes_the_quarantined_copy(self):
        A = self.active()
        B = FakeHost('standby', {'recovery_point_restore': {'ok': False, 'error': 'archive damaged'}})
        with self.assertRaises(tool.RolledBack):
            self.switch_stopped(A, B)
        self.assertEqual(operations(A)[-1], 'start')
        self.assertEqual(operations(B), ['recovery_point_restore', 'delete'])

    def test_an_escalated_stop_starts_it_again_and_captures_nothing(self):
        A = self.active(stop={'ok': True, 'data': {'forced': True}})
        B = FakeHost('standby', {})
        with self.assertRaises(tool.Refused):
            self.switch_stopped(A, B)
        self.assertEqual(operations(A), ['activation_status', 'stop', 'start'])
        self.assertEqual(operations(B), [])


if __name__ == '__main__':
    unittest.main(verbosity=1)
