#!/usr/bin/env python3
"""Offline tests for qualification identity, watchdog binding and cleanup ownership gates."""
import copy
import signal
import unittest
from unittest import mock

from podmesh_two_hosts import (Host, _pidfd_sigkill, owned_removal_verdict, untracked_container_additions,
                               signal_metric_counts, validate_collector_barrier_marker, validate_interrupted_collection_state,
                               validate_predelegation_refusal, validate_service_identity, watchdog_binding)


class ServiceIdentityTests(unittest.TestCase):
    def setUp(self):
        self.host_uuid = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
        self.expected = {'binary_sha256': 'a' * 64, 'socket': '/run/podmesh-collector-qual/api.sock',
                         'state_dir': '/var/lib/podmesh-collector-qual'}
        self.proof = {
            'load_state': 'loaded', 'active_state': 'active', 'main_pid': 101, 'peer_pid': 101,
            'peer_uid': 0, 'binary_sha256': 'a' * 64, 'socket': self.expected['socket'],
            'socket_is_unix': True, 'state_dir': self.expected['state_dir'], 'state_db_regular': True,
            'process_socket': self.expected['socket'], 'process_state_dir': self.expected['state_dir'],
            'api_host_uuid': self.host_uuid, 'db_host_uuid': self.host_uuid, 'machine_id': 'machine-a',
            'db_machine_id': 'machine-a',
        }

    def test_matching_identity_is_accepted(self):
        self.assertTrue(validate_service_identity(self.proof, self.expected)['verified'])

    def test_wrong_peer_pid_hash_and_state_are_all_rejected(self):
        proof = copy.deepcopy(self.proof)
        proof.update(peer_pid=202, binary_sha256='b' * 64,
                     db_host_uuid='bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb')
        verdict = validate_service_identity(proof, self.expected)
        self.assertFalse(verdict['verified'])
        self.assertEqual(len(verdict['errors']), 3)

    def test_missing_api_and_database_identity_is_rejected(self):
        proof = copy.deepcopy(self.proof)
        proof.update(api_host_uuid=None, db_host_uuid=None, machine_id='', db_machine_id='')
        verdict = validate_service_identity(proof, self.expected)
        self.assertFalse(verdict['verified'])
        self.assertIn('API host UUID differs from the configured SQLite state', verdict['errors'])
        self.assertIn('SQLite state belongs to another machine identity', verdict['errors'])


class WatchdogBindingTests(unittest.TestCase):
    def test_unrelated_sole_new_container_is_rejected(self):
        expected = {'universe_uuid': 'u-owned', 'authorization_id': 'auth-owned'}
        unrelated = {'Id': '1' * 64, 'Names': ['podmesh-u-other'],
                     'Config': {'Labels': {'io.podmesh.universe': 'u-other'}}}
        claim = {'authorization_id': 'auth-owned', 'universe_uuid': 'u-owned',
                 'container_id': '1' * 64, 'state': 'restore_failed'}
        self.assertFalse(watchdog_binding(expected, unrelated, claim)['verified'])

    def test_exact_name_label_claim_and_id_are_required(self):
        expected = {'universe_uuid': 'u-owned', 'authorization_id': 'auth-owned'}
        observed = {'Id': '2' * 64, 'Names': ['podmesh-u-owned'],
                    'Config': {'Labels': {'io.podmesh.universe': 'u-owned'}}}
        claim = {'authorization_id': 'auth-owned', 'universe_uuid': 'u-owned',
                 'container_id': '2' * 64, 'state': 'restore_failed'}
        verdict = watchdog_binding(expected, observed, claim)
        self.assertTrue(verdict['verified'])
        self.assertEqual(verdict['container_id'], '2' * 64)

    def test_pidfd_unavailable_refuses_without_numeric_pid_signal(self):
        with mock.patch.object(signal, 'pidfd_send_signal', None):
            result = _pidfd_sigkill(1234, '2' * 64)
        self.assertEqual(result['decision'], 'refused')
        self.assertIn('pidfd', result['reason'])


class CleanupLedgerTests(unittest.TestCase):
    def test_wrong_id_or_label_refuses_cleanup(self):
        entry = {'name': 'podmesh-u-owned', 'container_id': '3' * 64, 'universe_uuid': 'u-owned'}
        wrong_id = {'Id': '4' * 64, 'Names': ['podmesh-u-owned'],
                    'Config': {'Labels': {'io.podmesh.universe': 'u-owned'}}}
        wrong_label = {'Id': '3' * 64, 'Names': ['podmesh-u-owned'],
                       'Config': {'Labels': {'io.podmesh.universe': 'u-other'}}}
        self.assertFalse(owned_removal_verdict(entry, wrong_id)['verified'])
        self.assertFalse(owned_removal_verdict(entry, wrong_label)['verified'])

    def test_exact_ledger_entry_is_accepted(self):
        entry = {'name': 'podmesh-u-owned', 'container_id': '3' * 64, 'universe_uuid': 'u-owned'}
        observed = {'Id': '3' * 64, 'Names': ['podmesh-u-owned'],
                    'Config': {'Labels': {'io.podmesh.universe': 'u-owned'}}}
        self.assertTrue(owned_removal_verdict(entry, observed)['verified'])

    def test_ordinary_failed_restore_claim_is_registered(self):
        host = Host.__new__(Host)
        host.ledger, host.fixtures = {}, []
        universe = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
        authorization = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
        container_id = '5' * 64
        host.call = mock.Mock(return_value={'claim': {
            'authorization_id': authorization, 'universe_uuid': universe,
            'container_id': container_id, 'state': 'restore_failed', 'operation_id': 'claim-operation',
        }})
        entry = host._record_failed_restore_claim({
            'operation': 'migration_restore', 'operation_id': 'restore-operation',
            'authorization_id': authorization, 'universe_uuid': universe,
        })
        self.assertEqual(entry['container_id'], container_id)
        self.assertEqual(host.ledger['podmesh-' + universe]['source'], 'failed_restore_claim')
        self.assertEqual(entry['operation_id'], 'claim-operation')

    def test_failed_transport_does_not_register_a_restored_claim_for_cleanup(self):
        host = Host.__new__(Host)
        host.ledger, host.fixtures = {}, []
        universe = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
        authorization = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
        host.call = mock.Mock(return_value={'claim': {
            'authorization_id': authorization, 'universe_uuid': universe,
            'container_id': '5' * 64, 'state': 'restored', 'operation_id': 'claim-operation',
        }})
        entry = host._record_failed_restore_claim({
            'operation': 'migration_restore', 'operation_id': 'retry-operation',
            'authorization_id': authorization, 'universe_uuid': universe,
        })
        self.assertIsNone(entry)
        self.assertEqual(host.ledger, {})

    def test_untracked_post_baseline_addition_is_retained_as_uncertain(self):
        baseline = {'containers': {'1' * 64: [['pre-existing'], 'running']}}
        observed = {'containers': {
            '1' * 64: [['pre-existing'], 'running'],
            '6' * 64: [['podmesh-untracked'], 'exited'],
            '7' * 64: [['podmesh-tracked'], 'exited'],
        }}
        ledger = {'podmesh-tracked': {'container_id': '7' * 64}}
        additions = untracked_container_additions(baseline, observed, ledger)
        self.assertEqual(additions, [{'container_id': '6' * 64,
                                      'observed': [['podmesh-untracked'], 'exited']}])
        host = Host.__new__(Host)
        host.ledger, host.fixtures, host.cleanup_report = ledger, [], []
        host.remove_fixture = mock.Mock(return_value={'verified': True, 'absent': True})
        report = host.cleanup(baseline, observed)
        retained = [result for result in report if not result.get('verified')]
        self.assertEqual(len(retained), 1)
        self.assertEqual(retained[0]['container_id'], '6' * 64)
        self.assertIn('uncertain state retained', retained[0]['reason'])


class PredelegationRefusalTests(unittest.TestCase):
    def setUp(self):
        self.container_id = '8' * 64
        self.expected = {'class': 'failed_restore_claim',
                         'universe_uuid': 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
                         'authorization_id': 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'}
        self.response = {
            'error': 'Collection refused: survivors. They are reported, not ended; nothing was collected and migration_restore_abort was not entered',
            'details': {'applied': [], 'candidate': dict(self.expected), 'detail': {
                'candidate': dict(self.expected), 'delegated': False, 'effects_applied': 0,
                'runtime_processes': {
                    'known': True, 'source': 'cgroup_residency', 'authorizes_reclaim': True,
                    'observation_errors': [], 'container_id': self.container_id, 'claim_created_at': 100,
                    'count': 1, 'processes': [{
                        'pid': 42, 'cgroup': f'/machine.slice/libpod-{self.container_id}.scope/container',
                        'start_epoch': 101, 'started_at_or_after_claim': True,
                    }],
                },
            }},
        }

    def test_complete_exact_predelegation_proof_is_accepted(self):
        self.response['ok'] = False
        verdict = validate_predelegation_refusal(self.response, self.expected)
        self.assertTrue(verdict['verified'], verdict)

    def test_missing_or_unknown_runtime_proof_is_rejected(self):
        missing = copy.deepcopy(self.response)
        missing['ok'] = False
        del missing['details']['detail']['runtime_processes']
        self.assertFalse(validate_predelegation_refusal(missing, self.expected)['verified'])
        unknown = copy.deepcopy(self.response)
        unknown['ok'] = False
        unknown['details']['detail']['runtime_processes']['known'] = False
        self.assertFalse(validate_predelegation_refusal(unknown, self.expected)['verified'])
        wrong_transport = copy.deepcopy(self.response)
        wrong_transport['ok'] = True
        self.assertFalse(validate_predelegation_refusal(wrong_transport, self.expected)['verified'])


class CollectorCrashBarrierTests(unittest.TestCase):
    def setUp(self):
        self.operation = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
        self.universe = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
        self.collection_class = 'terminal_reservation_container_absent'
        self.expected = {'operation_id': self.operation, 'candidate_key': self.universe,
                         'universe_uuid': self.universe, 'class': self.collection_class}
        self.marker = dict(self.expected, format='podmesh-test-collector-barrier/1',
                           phase='effect_committed_verification_pending')
        pending = {'action': 'collected_reservation', 'universe_uuid': self.universe,
                   'class': self.collection_class, 'effect_state': 'committed', 'verification': 'pending'}
        collected = {'collected_by_operation': self.operation}
        self.state = {
            'operation': {'id': self.operation, 'status': 'pending', 'result': None},
            'attempts': [{'id': 1, 'started_at': 100, 'finished_at': None, 'outcome': None}],
            'effects': [{'operation_id': self.operation, 'candidate_key': self.universe,
                         'class': self.collection_class, 'universe_uuid': self.universe, 'result': pending}],
            'runs': [],
            'reservation': {'universe_uuid': self.universe, 'state': 'collected', 'detail': collected},
            'history': [{'universe_uuid': self.universe, 'class': self.collection_class,
                         'container_absent_at_collection': 1, 'collected_by_operation': self.operation}],
            'tombstone': {'universe_uuid': self.universe, 'class': self.collection_class,
                          'container_absent_at_collection': 1, 'collected_by_operation': self.operation},
        }

    def test_exact_marker_and_pending_committed_state_are_accepted(self):
        self.assertTrue(validate_collector_barrier_marker(self.marker, self.expected)['verified'])
        self.assertTrue(validate_interrupted_collection_state(self.state, self.expected)['verified'])

    def test_malformed_marker_or_missing_pending_proof_fails_closed(self):
        malformed = dict(self.marker, extra='not allowed')
        self.assertFalse(validate_collector_barrier_marker(malformed, self.expected)['verified'])
        missing = copy.deepcopy(self.state)
        missing['effects'] = []
        missing['operation']['status'] = 'verified'
        missing['runs'] = [{'operation_id': self.operation}]
        verdict = validate_interrupted_collection_state(missing, self.expected)
        self.assertFalse(verdict['verified'])
        self.assertGreaterEqual(len(verdict['errors']), 3)


class SignalMetricTests(unittest.TestCase):
    def test_candidates_attempts_and_deliveries_remain_distinct(self):
        entries = [
            {'signal_attempted': True, 'signal_outcome': 'delivered'},
            {'signal_attempted': True, 'signal_outcome': 'already_gone'},
            {'signal_attempted': False, 'signal_outcome': 'already_gone'},
            {'signal_attempted': False, 'signal_outcome': 'refused'},
        ]
        self.assertEqual(signal_metric_counts(entries), {
            'signal_candidates': 4, 'signal_attempts': 2, 'signals_delivered': 1,
            'processes_already_gone': 2, 'signals_refused': 1,
        })


if __name__ == '__main__':
    unittest.main()
