#!/usr/bin/env python3
"""A rotation to another holder in tools/ha-standby.py, without a laboratory (review of V3-1, item 1): the previous
holder may still renew its own lease under a follow mandate, and since V3-1 resume its route and its connector when
its replica returns; a barrier of `lease + margin from now` let the new holder publish while it did. Fake hosts and a
fake gate answer as the API and the fencing laboratory would, and the tool's own functions are driven: the
supersession is delivered to the previous holder, when it is named and reached, before the new holder acquires and
before any proof is made; when it is not, the barrier is no earlier than the recorded follow mandate's not_after plus
the lease plus the margin; and what cannot be known is refused before the gate moves.
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
        path = pathlib.Path(self.dir.name) / f'{R}.json'
        path.write_text(json.dumps({'universe': R, 'cycles': [], 'rotations': [], 'policy': {'lease_seconds': LEASE, 'takeover_margin_seconds': MARGIN}, **fields}))

    def rotate(self, previous=None, previous_host=None, stated=None):
        tool.try_host = lambda role, target: previous
        args = types.SimpleNamespace(universe=R, host='lab@new', reference='unit-test', lease=None, margin=None, standbys=None,
                                     previous_host=previous_host, follow_mandate_not_after=stated)
        with self.assertRaises(Reported) as caught:
            tool.cmd_rotate(args)
        return caught.exception.args[0]

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


if __name__ == '__main__':
    unittest.main()
