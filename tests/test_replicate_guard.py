#!/usr/bin/env python3
"""The guardian's decisions in tools/replicate-universe.py, without a laboratory: fake hosts answer as the API
would, timers and the independent SSH observation are stubbed, the ledger lives in a temporary directory.
Each case names the review finding (PCA design review, 2026-09-17) it holds the tool to.
Run: python3 -B tests/test_replicate_guard.py"""
import argparse, fcntl, importlib.util, os, pathlib, sys, tempfile, unittest, uuid

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tests'))
os.environ['PODMESH_HA_LEDGER'] = tempfile.mkdtemp(prefix='podmesh-guard-test-')
spec = importlib.util.spec_from_file_location('replicate', ROOT / 'tools' / 'replicate-universe.py')
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)

ACTIVE, S1, S2 = 'lab@c', 'lab@a', 'lab@b'
IDS = {ACTIVE: 'id-c', S1: 'id-a', S2: 'id-b'}
L, M = 30, 20


class FakeHost:
    def __init__(self, target, answers=None, container=None):
        self.role = self.target = target
        self.identity = IDS[target]
        self.answers, self.sent, self.container = answers or {}, [], container

    def api(self, req):
        self.sent.append(req)
        a = self.answers.get(req['operation'], {'ok': True, 'data': {}})
        return a(req) if callable(a) else a

    def call(self, function, **arguments):
        if function == 'inspect':
            return {'container': self.container}
        raise AssertionError(function)


def running():
    return {'State': {'Running': True, 'Status': 'running'}}


def sent(host, op=None):
    return [r['operation'] for r in host.sent if op is None or r['operation'] == op]


class Guard(unittest.TestCase):
    def setUp(self):
        self.u = str(uuid.uuid4())
        self.now = 10_000.0
        tool.time.time = lambda: self.now
        tool.time.sleep = lambda s: setattr(self, 'now', self.now + s)
        tool.timer_state = lambda u: {'unit': 'x', 'armed': False, 'next': None, 'last_trigger': None}
        tool.guard_timer_state = lambda u: {'unit': 'g', 'armed': True}
        tool.cmd_stop = lambda args: {'result': 'disarmed'}
        tool.cmd_start = lambda args: {'result': 'armed'}
        self.hosts, self.observed, self.fence = {}, {}, {}
        tool.host = lambda target: self.hosts[target] if target in self.hosts else (_ for _ in ()).throw(RuntimeError('unreachable'))
        tool.try_host = lambda target: self.hosts.get(target)
        tool.observe_container = lambda target, u, seconds=8: self.observed.get(target, 'unreachable')
        tool.fence_state = lambda h: self.fence.get(h, {'host': h, 'mandate_present': True, 'timer_active': True, 'stop_seconds': 2})
        tool.save(self.u, {'universe': self.u, 'cycles': [], 'rotations': [],
                           'replication': {'active': ACTIVE, 'hosts': [ACTIVE, S1, S2], 'standbys': [S1, S2], 'mode': 'all', 'capture': 'live', 'interval_seconds': 60,
                                           'lease_seconds': L, 'takeover_margin_seconds': M},
                           'guard': {'lease_seconds': L, 'takeover_margin_seconds': M, 'tick_seconds': 10, 'keep_stale': False, 'order': [S1, S2],
                                     'armed_at': 1_000, 'last_tick': None, 'last_renewed_at': 9_990, 'last_renewed_host': ACTIVE, 'state': 'guarding', 'failed_ticks': 0}})
        tool.write_renewal(self.u, ACTIVE, 9_990, confirmed=True)

    def args(self, **kw):
        return argparse.Namespace(**{'universe': self.u, 'reference': 'unit-test', 'wait': 1, **kw})

    def copy_on(self, target, point='p1'):
        ledger = tool.load(self.u)
        ledger['cycles'].append({'point': point, 'generation': 3, 'prepared_at': 9_950, 'capture': 'live', 'standby': IDS[target], 'staged_at': 9_951})
        tool.save(self.u, ledger)

    def tick(self, **kw):
        return tool.cmd_guard_tick(self.args(**kw))

    # --- renewal
    def test_a_tick_renews_and_records_the_attempt_before_the_answer(self):
        seen_attempt = []
        def renew(req):
            seen_attempt.append(tool.read_renewal(self.u).get('attempt'))
            return {'ok': True, 'data': {'expires_at': 10_030}}
        self.hosts[ACTIVE] = FakeHost(ACTIVE, {'activation_renew': renew}, container=running())
        r = self.tick()
        self.assertEqual(r['result'], 'renewed')
        self.assertEqual(seen_attempt, [self.now], 'the attempt is on disk before the host could commit it')
        self.assertEqual(tool.load(self.u)['guard']['last_renewed_at'], int(self.now))

    def test_a_renewal_whose_answer_is_lost_still_moves_the_base_of_the_wait(self):
        # Review blocker "M does not cover ... an unseen later renewal": the host committed, the reply never came.
        def cut(req):
            raise RuntimeError('ssh: connection reset')
        self.hosts[ACTIVE] = FakeHost(ACTIVE, {'activation_renew': cut})
        self.now = 10_010
        r = self.tick()
        self.assertEqual(r['result'], 'failed')
        del self.hosts[ACTIVE]
        self.now = 9_990 + L + M + 5          # past the deadline counted from the ACKNOWLEDGED renewal
        self.copy_on(S1)
        self.hosts[S1] = FakeHost(S1)
        r = self.tick()
        self.assertEqual(r['result'], 'failed', 'no failover: the lost-answer attempt at 10 010 is the base')
        self.assertEqual(r['since'], 10_010)

    def test_the_renewal_does_not_wait_for_a_run_holding_the_lock(self):
        # Review: a run holding the lock must not make the lease lapse on a healthy host.
        self.hosts[ACTIVE] = FakeHost(ACTIVE, container=running())
        f = open(tool.ledger_path(self.u).with_suffix('.lock'), 'a+')
        fcntl.flock(f, fcntl.LOCK_EX)
        try:
            r = self.tick()
        finally:
            fcntl.flock(f, fcntl.LOCK_UN)
            f.close()
        self.assertEqual(r['result'], 'skipped')
        self.assertEqual(sent(self.hosts[ACTIVE]), ['activation_renew'])

    def test_a_late_guardian_restarts_the_universe_its_lapse_got_fenced(self):
        # Review: a re-acquired lease over a fenced universe must not be reported as guarded and running.
        self.hosts[ACTIVE] = FakeHost(ACTIVE, {'activation_renew': {'ok': False, 'error': 'The activation lease has expired; acquire it again'},
                                              'activation_acquire': {'ok': True, 'data': {}}}, container={'State': {'Running': False, 'Status': 'exited'}})
        r = self.tick()
        self.assertEqual(r['result'], 'restarted_in_place')
        self.assertEqual(sent(self.hosts[ACTIVE]), ['activation_renew', 'activation_acquire', 'start'])
        self.assertEqual(tool.load(self.u)['incidents'][-1]['kind'], 'self_fenced_by_late_guardian')

    def test_a_universe_seen_stopped_during_a_capture_is_looked_at_again_before_any_start(self):
        # A live capture's dump stops the universe for about a second; the renewal runs outside the lock.
        states = iter([{'State': {'Running': False, 'Status': 'exited'}}, running()])
        class Flicker(FakeHost):
            def call(self, function, **arguments):
                return {'container': next(states)}
        self.hosts[ACTIVE] = Flicker(ACTIVE)
        r = self.tick()
        self.assertEqual(r['result'], 'renewed')
        self.assertEqual(r['observed_under_lock'], 'running')
        self.assertEqual(sent(self.hosts[ACTIVE], 'start'), [])

    # --- failover
    def test_no_failover_before_two_failed_ticks_and_the_deadline(self):
        self.copy_on(S1)
        self.hosts[S1] = FakeHost(S1)
        self.now = 9_990 + L + M + 1
        r = self.tick()
        self.assertEqual((r['result'], tool.load(self.u)['guard']['failed_ticks']), ('failed', 1))
        self.assertEqual(sent(self.hosts[S1]), [])

    def test_failover_to_the_first_standby_with_a_copy(self):
        self.copy_on(S2)
        self.hosts[S1] = FakeHost(S1)
        self.hosts[S2] = FakeHost(S2, {'recovery_point_promote': {'ok': True, 'data': {'started': True}}})
        self.now = 9_990 + L + M + 1
        self.tick()
        self.now += 10
        r = self.tick()
        self.assertEqual(r['result'], 'failed_over', r)
        self.assertEqual(sent(self.hosts[S2]), ['activation_require', 'activation_acquire', 'recovery_point_promote'])
        ledger = tool.load(self.u)
        self.assertEqual((ledger['replication']['active'], ledger['replication']['standbys']), (S2, [S1, ACTIVE]))
        self.assertIsNone(ledger.get('takeover_intent'))
        self.assertEqual(tool.read_renewal(self.u)['active'], S2, 'the new holder starts a new renewal record')
        self.assertEqual(ledger['guard']['order'], [S1, ACTIVE], 'the old active host joins the failover order, last')
        self.assertEqual(ledger['guard']['last_renewed_host'], S2)
        self.assertEqual([i['kind'] for i in ledger['incidents']], ['lost_host_failover'])

    def test_failover_refused_while_the_old_active_is_seen_running_or_unknown(self):
        # Review blocker: a host whose daemon is down but which still runs the universe.
        for seen in ('running', 'unknown'):
            self.setUp()
            self.copy_on(S1)
            self.hosts[S1] = FakeHost(S1)
            self.observed[ACTIVE] = seen
            self.now = 9_990 + L + M + 1
            self.tick()
            self.now += 10
            r = self.tick()
            self.assertEqual(r['result'], 'failover_refused', seen)
            self.assertEqual(sent(self.hosts[S1]), [])
            ledger = tool.load(self.u)
            self.assertEqual((ledger['guard']['state'], ledger['guard']['failed_ticks']), ('failing_over', 2), 'the refusal is persisted, not lost on reload')
            self.now += 10
            self.tick()
            incidents = tool.load(self.u)['incidents']
            self.assertEqual(([i['kind'] for i in incidents], incidents[-1].get('repeated')), (['failover_refused'], 2), 'one incident, counted')

    def test_no_standby_is_an_incident(self):
        self.now = 9_990 + L + M + 1
        self.tick()
        self.now += 10
        self.assertEqual(self.tick()['result'], 'no_standby')

    def test_a_takeover_interrupted_after_the_promotion_is_finished_not_repeated(self):
        # Review blocker: a crash between the promotion and the ledger swap.
        self.copy_on(S1)
        calls = []
        def promote(req):
            calls.append(req['operation_id'])
            if len(calls) == 1:
                raise RuntimeError('the workstation died here')
            return {'ok': True, 'data': {'started': True, 'replayed': True}}
        self.hosts[S1] = FakeHost(S1, {'recovery_point_promote': promote,
                                       'activation_status': {'ok': True, 'data': {'live': True, 'holder_host_uuid': 'id-a'}}}, container=running())
        self.now = 9_990 + L + M + 1
        self.tick()
        self.now += 10
        with self.assertRaises(RuntimeError):
            self.tick()
        ledger = tool.load(self.u)
        self.assertEqual(ledger['replication']['active'], ACTIVE, 'the swap did not happen')
        self.assertTrue(ledger.get('takeover_intent'))
        # The old active comes back meanwhile: the next tick must not renew its lease.
        self.hosts[ACTIVE] = FakeHost(ACTIVE, container=None)
        self.now += 10
        r = self.tick()
        self.assertEqual(r['result'], 'failed_over')
        self.assertEqual(len(set(calls)), 1, 'the same promotion operation ID, replayed by the standby')
        self.assertEqual(sent(self.hosts[ACTIVE], 'activation_renew'), [])
        self.assertEqual(tool.load(self.u)['replication']['active'], S1)

    # --- reintegration
    def test_reintegration_records_evidence_then_deletes_the_stale_copy(self):
        ledger = tool.load(self.u)
        ledger['replication']['active'], ledger['replication']['standbys'] = S1, [S2, ACTIVE]
        ledger['takeovers'] = [{'at': 9_900, 'from': ACTIVE, 'to': S1, 'planned': False, 'point': 'p', 'capture': 'live', 'reintegrated_at': None}]
        tool.save(self.u, ledger)
        tool.write_renewal(self.u, S1, 9_995, confirmed=True)
        self.hosts[S1] = FakeHost(S1, container=running())
        self.hosts[ACTIVE] = FakeHost(ACTIVE, {'delete': {'ok': True, 'data': {}}}, container={'State': {'Running': False, 'Status': 'exited', 'ExitCode': 137, 'FinishedAt': 'x'}})
        self.observed[ACTIVE] = 'stopped'
        self.tick()
        self.assertEqual(sent(self.hosts[ACTIVE]), ['delete'])
        inc = tool.load(self.u)['incidents'][-1]
        self.assertEqual((inc['kind'], inc['evidence']['ExitCode']), ('host_reintegrated', 137))

    def test_reintegration_stops_a_running_stale_copy_as_split_brain(self):
        ledger = tool.load(self.u)
        ledger['replication']['active'], ledger['replication']['standbys'] = S1, [S2, ACTIVE]
        ledger['takeovers'] = [{'at': 9_900, 'from': ACTIVE, 'to': S1, 'planned': False, 'point': 'p', 'capture': 'live', 'reintegrated_at': None}]
        ledger['guard']['keep_stale'] = True
        tool.save(self.u, ledger)
        tool.write_renewal(self.u, S1, 9_995, confirmed=True)
        self.hosts[S1] = FakeHost(S1, container=running())
        self.hosts[ACTIVE] = FakeHost(ACTIVE, {'stop': {'ok': True, 'data': {'forced': True}}}, container=running())
        self.observed[ACTIVE] = 'running'
        self.tick()
        self.assertEqual(sent(self.hosts[ACTIVE]), ['stop'])
        kinds = [i['kind'] for i in tool.load(self.u)['incidents']]
        self.assertEqual(kinds[-2:], ['split_brain_observed', 'host_reintegrated'])

    # --- arming
    def test_guard_refuses_a_margin_that_does_not_cover_the_fence(self):
        # Review blocker: the margin must cover the fence's period, accuracy, overhead, stop grace and skew.
        self.hosts[ACTIVE] = FakeHost(ACTIVE)
        self.fence[ACTIVE] = {'host': ACTIVE, 'mandate_present': True, 'timer_active': True, 'stop_seconds': 10}
        with self.assertRaises(tool.Refused) as e:
            tool.cmd_guard(self.args(lease=30, margin=20, tick=10, keep_stale=False))
        self.assertIn('23 s at least', str(e.exception))
        self.assertEqual(sent(self.hosts[ACTIVE]), [])


if __name__ == '__main__':
    unittest.main(verbosity=1)
