#!/usr/bin/env python3
"""A rotation to another holder in tools/ha-standby.py, without a laboratory (review of V3-1, item 1): the previous
holder may still renew its own lease under a follow mandate, and since V3-1 resume its route and its connector when
its replica returns; a barrier of `lease + margin from now` let the new holder publish while it did. Fake hosts and a
fake gate answer as the API and the fencing laboratory would, and the tool's own functions are driven: the
supersession is delivered to the previous holder, when it is named and reached, before the new holder acquires and
before any proof is made; when it is not, the barrier is no earlier than the recorded follow mandate's not_after plus
the lease plus the margin; and what cannot be known is refused before the gate moves. The second review's probes are
kept as regressions: every rotation carries the current epoch's barrier forward (a same-holder rotation and a holder
told later shorten nothing), the barrier counts the previous holder's lease, the proof outlives its barrier, and what
fails after the gate moved leaves a record a rerun carries.
Run: python3 -B tests/test_ha_rotate_barrier.py"""
import importlib.util, json, os, pathlib, sys, tempfile, time, types, unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tests'))
os.environ['PODMESH_HA_UNSIGNED'] = '1'
spec = importlib.util.spec_from_file_location('ha_standby', ROOT / 'tools' / 'ha-standby.py')
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)

R = '91eeb6bf-5489-405b-b77a-53105b0aff7a'
PREV, NEW = 'host-previous', 'host-new'
LEASE, MARGIN = 3600, 30


class Reported(Exception):
    """What the tool printed as its report, raised instead of exiting."""


class FakeHost:
    def __init__(self, identity, events, answers=None):
        self.identity, self.events, self.answers, self.role = identity, events, answers or {}, identity

    def api(self, req):
        self.events.append((self.identity, req['operation']))
        answer = self.answers.get(req['operation'])
        if answer is not None:
            return answer
        if req['operation'] == 'activation_acquire':
            return {'ok': True, 'data': {'generation': 1, 'expires_at': int(time.time()) + LEASE, 'live': True}}
        return {'ok': True, 'data': {'highest_epoch_seen': 6, 'superseded': True}}


class FakeGate:
    authority_id = 'lab-gate'

    def __init__(self, events):
        self.events = events

    def inspect(self, universe):
        return {'epoch': 5, 'replica_id': PREV}


class Rotation(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory(prefix='podmesh-rotate-')
        os.environ['PODMESH_HA_LEDGER'] = self.dir.name
        self.events = []
        self.gate = FakeGate(self.events)
        self.new = FakeHost(NEW, self.events)
        tool.gate_or_refuse = lambda create=False: self.gate
        tool.hosts = lambda args, *roles: [self.new]

        def permit_for(gate, universe, host, expected):
            self.events.append(('gate', 'transfer'))
            return {'authority_id': 'lab-gate', 'resource': universe, 'epoch': expected + 1, 'replica_id': host.identity, 'instance_id': 'b', 'grant_id': 'g6'}
        tool.permit_for = permit_for

        def out(report, code=0):
            raise Reported(report)
        tool.out = out

    def tearDown(self):
        self.dir.cleanup()

    def ledger(self, **fields):
        """The ledger as a rotation to epoch 5 left it (its proof recorded, nothing carried), with `fields` over it."""
        path = pathlib.Path(self.dir.name) / f'{R}.json'
        base = {'universe': R, 'cycles': [], 'rotations': [], 'policy': {'lease_seconds': LEASE, 'takeover_margin_seconds': MARGIN},
                'proofs': {'5': {'method': 'same_holder', 'eligible_after': 0, 'new_epoch': 5}}}
        path.write_text(json.dumps({**base, **fields}))

    def recorded(self):
        return json.loads((pathlib.Path(self.dir.name) / f'{R}.json').read_text())

    def rotate(self, previous=None, previous_host=None, stated=None, lease=None, barrier=None):
        tool.try_host = lambda role, target: previous
        args = types.SimpleNamespace(universe=R, host='lab@new', reference='unit-test', lease=lease, margin=None, standbys=None,
                                     previous_host=previous_host, follow_mandate_not_after=stated, barrier_not_before=barrier)
        with self.assertRaises(Reported) as caught:
            tool.cmd_rotate(args)
        return caught.exception.args[0]

    def to(self, holder):
        """The next rotation goes to `holder`."""
        self.new = FakeHost(holder, self.events)
        tool.hosts = lambda args, *roles: [self.new]

    def at(self, holder, epoch):
        """The gate names `holder` at `epoch`, as it does after a rotation there."""
        self.gate.inspect = lambda universe: {'epoch': epoch, 'replica_id': holder}

    def test_the_supersession_is_delivered_before_the_new_holder_acquires_and_before_any_proof(self):
        not_after = int(time.time()) + 86400
        self.ledger(follow_mandates={PREV: {'not_after': not_after, 'renew': 1}})
        report = self.rotate(previous=FakeHost(PREV, self.events), previous_host='lab@previous')
        order = [e for e in self.events if e[1] in ('transfer', 'activation_supersede', 'activation_acquire')]
        self.assertEqual(order, [('gate', 'transfer'), (PREV, 'activation_supersede'), (NEW, 'activation_acquire')])
        self.assertTrue(report['supersession']['delivered'])
        # Told of the rotation, the previous holder can renew nothing: the barrier is the lease and the margin.
        self.assertIsNone(report['barrier_covers_follow_mandate'])
        self.assertLessEqual(report['takeover_proof']['eligible_after'], int(time.time()) + LEASE + MARGIN)
        self.assertEqual(report['takeover_proof']['method'], 'lease_barrier')
        rotation = json.loads((pathlib.Path(self.dir.name) / f'{R}.json').read_text())['rotations'][-1]
        self.assertTrue(rotation['supersession']['delivered'])

    def test_an_unreached_previous_holder_under_a_recorded_mandate_pushes_the_barrier(self):
        not_after = int(time.time()) + 86400
        self.ledger(follow_mandates={PREV: {'not_after': not_after, 'renew': 1, 'issued_at': 1}})
        for previous, named in [(None, None), (None, 'lab@previous')]:
            report = self.rotate(previous=previous, previous_host=named)
            self.assertFalse(report['supersession']['delivered'])
            self.assertEqual(report['takeover_proof']['eligible_after'], not_after + LEASE + MARGIN)
            self.assertEqual(report['barrier_covers_follow_mandate']['not_after'], not_after)
            self.assertIn('renew by itself', report['takeover_proof']['barrier_basis'])

    def test_a_refused_supersession_counts_as_not_delivered(self):
        not_after = int(time.time()) + 7200
        self.ledger(follow_mandates={PREV: {'not_after': not_after, 'renew': 1}})
        refusing = FakeHost(PREV, self.events, {'activation_supersede': {'ok': False, 'error': 'Epoch 6 does not supersede epoch 6'}})
        report = self.rotate(previous=refusing, previous_host='lab@previous')
        self.assertFalse(report['supersession']['delivered'])
        self.assertEqual(report['takeover_proof']['eligible_after'], not_after + LEASE + MARGIN)

    def test_no_mandate_standing_leaves_the_barrier_where_it_was(self):
        now = int(time.time())
        for mandates in [None, {}, {PREV: {'not_after': now - 10, 'renew': 1}}, {PREV: {'not_after': now + 9999, 'renew': 0}}, {'other': {'not_after': now + 9999, 'renew': 1}}]:
            self.ledger(**({'follow_mandates': mandates} if mandates is not None else {}))
            report = self.rotate()
            self.assertIsNone(report['barrier_covers_follow_mandate'], mandates)
            self.assertLessEqual(report['takeover_proof']['eligible_after'], int(time.time()) + LEASE + MARGIN)

    def test_the_operator_states_a_mandate_the_ledger_does_not_record(self):
        self.ledger()
        stated = int(time.time()) + 5000
        report = self.rotate(stated=stated)
        self.assertEqual(report['takeover_proof']['eligible_after'], stated + LEASE + MARGIN)
        report = self.rotate(stated=0)
        self.assertIsNone(report['barrier_covers_follow_mandate'])

    def test_what_cannot_be_known_is_refused_before_the_gate_moves(self):
        for mandates in [{PREV: {'renew': 1}}, {PREV: {'not_after': 'soon', 'renew': 1}}, {PREV: 'x'}, ['not', 'a', 'record']]:
            self.events.clear()
            self.ledger(follow_mandates=mandates)
            with self.assertRaises(tool.Refusal) as refused:
                self.rotate()
            self.assertIn('follow_mandate_unknown', str(refused.exception))
            self.assertEqual(self.events, [], mandates)

    def test_a_named_previous_host_that_is_not_the_previous_holder_is_refused_before_the_gate_moves(self):
        self.ledger()
        with self.assertRaises(tool.Refusal) as refused:
            self.rotate(previous=FakeHost('someone-else', self.events), previous_host='lab@wrong')
        self.assertIn('previous_host_mismatch', str(refused.exception))
        self.assertEqual(self.events, [])

    def test_a_rotation_to_the_same_holder_needs_no_supersession_and_no_barrier(self):
        self.gate.inspect = lambda universe: {'epoch': 5, 'replica_id': NEW}
        self.ledger(follow_mandates={NEW: {'not_after': int(time.time()) + 86400, 'renew': 1}})
        report = self.rotate()
        self.assertIsNone(report['supersession'])
        self.assertEqual(report['takeover_proof']['method'], 'same_holder')


    # ------------------------------------------------------------ the second review's probes, kept

    def test_the_proof_outlives_its_barrier(self):
        not_after = int(time.time()) + 86400
        for mandates in [{}, {PREV: {'not_after': not_after, 'renew': 1}}]:
            self.ledger(follow_mandates=mandates)
            p = self.rotate()['takeover_proof']
            self.assertGreaterEqual(p['expires_at'], p['eligible_after'] + 3600, mandates)

    def test_a_same_holder_rotation_carries_the_barrier_it_follows(self):
        not_after = int(time.time()) + 86400
        self.ledger(follow_mandates={PREV: {'not_after': not_after, 'renew': 1}})
        first = self.rotate()                                   # PREV not reached: the barrier is pushed
        pushed = first['takeover_proof']['eligible_after']
        self.assertEqual(pushed, not_after + LEASE + MARGIN)
        self.at(NEW, 6)                                         # the operator rotates to NEW again, a minute later
        second = self.rotate()
        p = second['takeover_proof']
        self.assertEqual((p['method'], p['eligible_after'], p['carried_eligible_after']), ('same_holder', pushed, pushed))
        self.assertGreaterEqual(p['expires_at'], pushed + 3600)

    def test_a_holder_never_told_is_carried_to_the_next_rotation(self):
        not_after = int(time.time()) + 86400
        self.ledger(follow_mandates={PREV: {'not_after': not_after, 'renew': 1}})
        first = self.rotate()                                   # PREV never superseded
        self.assertFalse(first['supersession']['delivered'])
        self.at(NEW, 6)
        self.to('host-c')
        told = FakeHost(NEW, self.events)
        second = self.rotate(previous=told, previous_host='lab@new')   # NEW told this time
        self.assertTrue(second['supersession']['delivered'])
        self.assertGreaterEqual(second['takeover_proof']['eligible_after'], not_after + LEASE + MARGIN)

    def test_the_barrier_counts_the_lease_the_previous_holder_renews_under(self):
        self.ledger()                                           # the previous holder renews a 3600 s lease
        before = int(time.time())
        p = self.rotate(lease=20)['takeover_proof']
        self.assertGreaterEqual(p['eligible_after'], before + LEASE + MARGIN)
        self.assertEqual(self.recorded()['policy']['lease_seconds'], 20)   # the new policy is still this call's

    def test_a_dropped_session_with_the_previous_holder_is_reported_not_raised(self):
        not_after = int(time.time()) + 86400
        self.ledger(follow_mandates={PREV: {'not_after': not_after, 'renew': 1}})

        class Dropping(FakeHost):
            def api(self, req):
                raise RuntimeError('previous ssh failed (255)')
        report = self.rotate(previous=Dropping(PREV, self.events), previous_host='lab@previous')
        self.assertFalse(report['supersession']['delivered'])
        self.assertIn('could not be asked', report['supersession']['why'])
        self.assertEqual(report['takeover_proof']['eligible_after'], not_after + LEASE + MARGIN)
        self.assertEqual(self.recorded()['rotations'][-1]['state'], 'complete')

    def test_a_failure_after_the_gate_moved_leaves_what_a_rerun_carries(self):
        not_after = int(time.time()) + 86400
        self.ledger(follow_mandates={PREV: {'not_after': not_after, 'renew': 1}})

        class Failing(FakeHost):
            def api(self, req):
                if req['operation'] == 'activation_acquire':
                    raise RuntimeError('new host ssh failed (255)')
                return super().api(req)
        self.new = Failing(NEW, self.events)
        tool.hosts = lambda args, *roles: [self.new]
        tool.try_host = lambda role, target: None
        args = types.SimpleNamespace(universe=R, host='lab@new', reference='unit-test', lease=None, margin=None, standbys=None,
                                     previous_host=None, follow_mandate_not_after=None, barrier_not_before=None)
        with self.assertRaises(RuntimeError):
            tool.cmd_rotate(args)
        ledger = self.recorded()
        self.assertIn(('gate', 'transfer'), self.events)
        self.assertEqual(ledger['rotations'][-1]['state'], 'gate_moved')
        self.assertEqual(ledger['proofs']['6']['eligible_after'], not_after + LEASE + MARGIN)
        # The rerun: the gate names NEW at 6; a rotation to NEW carries the barrier recorded before the failure.
        self.to(NEW)
        self.at(NEW, 6)
        p = self.rotate()['takeover_proof']
        self.assertEqual((p['method'], p['eligible_after']), ('same_holder', not_after + LEASE + MARGIN))

    def test_a_rotation_without_the_current_epochs_proof_is_refused_unless_the_barrier_is_stated(self):
        for holder in (NEW, PREV):
            self.events.clear()
            self.at(holder, 5)
            self.ledger(proofs={})
            with self.assertRaises(tool.Refusal) as refused:
                self.rotate()
            self.assertIn('no_proof_for_current_epoch', str(refused.exception))
            self.assertEqual(self.events, [])
        stated = int(time.time()) + 50_000
        self.at(NEW, 5)
        p = self.rotate(barrier=stated)['takeover_proof']
        self.assertEqual((p['method'], p['eligible_after']), ('same_holder', stated))
        # A universe activated there (tools/ha-standby.py activate) set no barrier to carry.
        self.ledger(proofs={}, rotations=[{'epoch': 5, 'to': NEW, 'by': 'activate'}])
        self.assertEqual(self.rotate()['takeover_proof']['method'], 'same_holder')

    def test_a_fence_receipt_carries_only_what_it_carried(self):
        self.assertEqual(tool.carried_barrier(None), 0)
        self.assertEqual(tool.carried_barrier({'method': 'lease_barrier', 'eligible_after': 900}), 900)
        self.assertEqual(tool.carried_barrier({'method': 'same_holder', 'eligible_after': 900, 'carried_eligible_after': 800}), 900)
        self.assertEqual(tool.carried_barrier({'method': 'fence_receipt', 'eligible_after': 900}), 0)
        self.assertEqual(tool.carried_barrier({'method': 'fence_receipt', 'eligible_after': 900, 'carried_eligible_after': 800}), 800)

    def test_an_attested_fence_is_eligible_at_the_barrier_it_carried(self):
        carried = int(time.time()) + 40_000
        self.at(NEW, 6)
        proof = {'method': 'lease_barrier', 'previous_holder': PREV, 'new_holder': NEW, 'eligible_after': carried + 10,
                 'carried_eligible_after': carried, 'expires_at': int(time.time()) + 100, 'new_epoch': 6}
        self.ledger(proofs={'6': proof})
        receipt = pathlib.Path(self.dir.name) / 'receipt.json'
        receipt.write_text(json.dumps({'host': PREV, 'operation_id': 'f', 'fence': {'unentitled': [R], 'publishers_withdrawn': [], 'routes_withdrawn': []}}))
        with self.assertRaises(Reported) as caught:
            tool.cmd_attest_fence(types.SimpleNamespace(universe=R, receipt=str(receipt)))
        p = caught.exception.args[0]['takeover_proof']
        self.assertEqual((p['method'], p['eligible_after']), ('fence_receipt', carried))
        self.assertGreaterEqual(p['expires_at'], carried + 3600)

    def test_the_supersession_visit_stops_the_previous_connector(self):
        self.ledger()
        previous = FakeHost(PREV, self.events, {'publisher_status': {'ok': True, 'data': {'unit': {'state': 'inactive'}}}})
        report = self.rotate(previous=previous, previous_host='lab@previous')
        self.assertIn((PREV, 'publisher_stop'), self.events)
        self.assertEqual(report['supersession']['previous_connector'], {'stop': 'done', 'unit_after': 'inactive', 'stopped': True})

    def test_the_mandate_record_never_says_less_than_the_host_may_hold(self):
        now = int(time.time())
        self.ledger()
        tool.record_follow_mandate(R, PREV, 'lab-a', now + 86400, 900, 'r', confirmed=True)
        # A shorter mandate recorded before its installation: the longer one stands until the host's copy is read back.
        pending = tool.record_follow_mandate(R, PREV, 'lab-a', now + 3600, 900, 'r', confirmed=False)
        self.assertEqual((pending['not_after'], pending['installing_not_after'], pending['installed']), (now + 86400, now + 3600, False))
        confirmed = tool.record_follow_mandate(R, PREV, 'lab-a', now + 3600, 900, 'r', confirmed=True)
        self.assertEqual((confirmed['not_after'], confirmed['installed']), (now + 3600, True))
        self.assertEqual(self.recorded()['follow_mandates'][PREV]['not_after'], now + 3600)

    def test_the_ledger_has_one_writer_at_a_time(self):
        self.ledger()
        with tool.locked(R):
            with self.assertRaises(tool.Refusal) as refused:
                with tool.locked(R, wait_seconds=0.6):
                    pass
        self.assertIn('ledger_locked', str(refused.exception))
        taken = []
        real = tool.locked

        def recording(universe, wait_seconds=30):
            taken.append(universe)
            return real(universe, wait_seconds)
        tool.locked = recording
        try:
            self.rotate()
            tool.record_follow_mandate(R, PREV, 'lab-a', int(time.time()) + 60, 900, 'r', confirmed=True)
        finally:
            tool.locked = real
        self.assertEqual(taken, [R, R])


if __name__ == '__main__':
    unittest.main()
