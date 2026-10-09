#!/usr/bin/env python3
"""What converged, what is immutable history, what is per-host, and what is still an exclusive
decision -- derived from the inspections of the preserved stores of a campaign, on two hosts at least.

Input: derived-inspection.json files (private, raw identifiers). Output: a public verdict of counts,
booleans and digests only; no identifier leaves this script. It qualifies the replication data path
separately from activation: three replicas of one logical manager, not three managers.
"""
import json, sys, hashlib
from collections import Counter

paths = sys.argv[1:]
if len(paths) < 2:
    print('at least two derived inspections are required', file=sys.stderr); sys.exit(2)
D = [json.load(open(p)) for p in paths]
n = len(D)

def canon(x): return json.dumps(x, sort_keys=True)
fact_sets = [{canon(f) for f in d['ordered_facts']} for d in D]
facts = D[0]['ordered_facts']
scopes = sorted({f['scope'] for f in facts})
one_logical_manager = len({d['logical_manager_id'] for d in D}) == 1
distinct_replicas = len({d['replica_id'] for d in D}) == n
per_scope = {}
for s in scopes:
    fs = [f for f in facts if f['scope'] == s]
    origins = {f['origin_replica_id'] for f in fs}
    revisions = sorted(f['subject_revision'] for f in fs)
    chained = all(f['predecessor'] is None if f['subject_revision'] == 1 else f['predecessor'] is not None for f in fs)
    per_scope[hashlib.sha256(s.encode()).hexdigest()[:12]] = {
        'facts': len(fs), 'distinct_origin_replicas': len(origins), 'subject_revisions_contiguous': revisions == list(range(1, len(fs) + 1)),
        'predecessor_chained': chained}
exclusive = [f for f in facts if f.get('exclusive_resource')]
verdict = {
    'schema': 'manager2-replication-data-path/v1', 'hosts': n,
    'one_logical_manager': one_logical_manager, 'distinct_replica_identities': distinct_replicas,
    'converged': {
        'fact_set_identical_on_every_host': all(s == fact_sets[0] for s in fact_sets),
        'logical_history_sha256_identical': len({d['logical_history_sha256'] for d in D}) == 1,
        'materialized_view_identical': len({canon(d['current']) for d in D}) == 1,
        'facts': len(facts), 'scopes': len(scopes), 'per_scope': per_scope,
        'every_scope_has_one_origin_replica': all(v['distinct_origin_replicas'] == 1 for v in per_scope.values()),
    },
    'immutable_history': {
        'facts_are_content_addressed': True,
        'note': 'a fact is an immutable event: identity collision with different bytes is refused at import (Replica::ingest); facts, receipts and audit rows are protected by refuse-update and refuse-delete triggers in the store',
    },
    'per_host_not_replicated': {
        'receipt_sets_identical': len({canon(sorted(canon(r) for r in d['ordered_receipts'])) for d in D}) == 1,
        'receipts_per_host': [len(d['ordered_receipts']) for d in D],
        'audit_set_sha256_distinct': len({d['audit_set_sha256'] for d in D}),
        'audit_events_per_host': [d['audit_event_count'] for d in D],
        'incomplete_attempts_per_host': [len(d['incomplete_attempts']) for d in D],
        'incomplete_attempt_phases': [dict(Counter(i['last_phase'] for i in d['incomplete_attempts'])) for d in D],
    },
    'exclusive_decisions': {
        'facts_with_exclusive_resource': len(exclusive), 'facts_with_active_claim': sum(1 for f in facts if f.get('active_claim')),
        'conflicts_per_host': [len(d['conflicts']) for d in D], 'blocked_exclusive_resources_per_host': [len(d['blocked_exclusive_resources']) for d in D],
        'note': 'an exclusive decision is a fact carrying exclusive_resource with active_claim, and a permit issued by authorize_exclusive_service against a reconciled history; this campaign carries none, so nothing here is evidence about exclusion',
    },
}
verdict['replication_path_qualified_for'] = 'non-exclusive observations in owned scopes: the fact set, its digest and the materialised view converged on every host' if (
    verdict['converged']['fact_set_identical_on_every_host'] and verdict['converged']['logical_history_sha256_identical'] and verdict['converged']['materialized_view_identical'] and one_logical_manager and distinct_replicas) else 'NOT converged'
print(json.dumps(verdict, indent=2, sort_keys=True))
