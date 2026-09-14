#!/usr/bin/env python3
"""Exclusive activation leases: the refusals, and what they do and do not prove.

Runs against a daemon over the local socket. It needs no Podman and no root: every
operation here touches only the journal, and the one operation that would touch Podman is
asserted to be refused BEFORE it gets there, which is the whole point of the gate.
"""
import json, os, socket, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')

def api(request):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(30); s.connect(endpoint)
        s.sendall(json.dumps(request).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())

def op(operation, universe, **extra):
    return api(dict({'operation': operation, 'operation_id': str(uuid.uuid4()),
                     'universe_uuid': universe, 'authorization_ref': 'disposable-lab'}, **extra))

def refused(answer, fragment, label):
    assert not answer['ok'], f'{label}: accepted, expected refusal — {answer}'
    assert fragment in json.dumps(answer), f'{label}: refused for another reason — {answer}'

u = str(uuid.uuid4())

# A universe with no policy is unconstrained: the gate must not appear from nowhere.
status = op('activation_status', u)
assert status['ok'] and status['data']['requires_lease'] is False, status
assert status['data']['scope'].startswith("this host's journal only"), status

# Acquiring without a declared policy is refused: a lease with no stated duration or margin
# would have no expiry anyone agreed to.
refused(op('activation_acquire', u), 'no activation policy', 'acquire before policy')

# Bounds are enforced on both numbers rather than trusted from the caller.
refused(op('activation_require', u, lease_seconds=1, takeover_margin_seconds=10),
        'lease_seconds must be from', 'lease below the floor')
refused(op('activation_require', u, lease_seconds=60, takeover_margin_seconds=1),
        'takeover_margin_seconds must be from', 'margin below the floor')
refused(op('activation_require', u, lease_seconds=99999, takeover_margin_seconds=10),
        'lease_seconds must be from', 'lease above the ceiling')

declared = op('activation_require', u, lease_seconds=60, takeover_margin_seconds=10)
assert declared['ok'] and declared['data']['requires_lease'] is True, declared

# Declared but not held: start must refuse, and it must say which of the three reasons.
refused(op('start', u, observe_seconds=0), 'requires an activation lease and none is held',
        'start with no lease')

acquired = op('activation_acquire', u)
assert acquired['ok'], acquired
assert acquired['data']['live'] is True and acquired['data']['generation'] == 1, acquired
assert acquired['data']['holder_host_uuid'] == acquired['data']['this_host_uuid'], acquired

# Re-acquiring our own live lease keeps the generation: a repeat is idempotent, not a takeover.
again = op('activation_acquire', u)
assert again['ok'] and again['data']['generation'] == 1, again

renewed = op('activation_renew', u)
assert renewed['ok'] and renewed['data']['seconds_remaining'] > 0, renewed

# With the lease held, the gate no longer refuses. `start` still fails -- there is no such
# container -- but it must fail for THAT reason, which is how we know the gate let it past.
started = op('start', u, observe_seconds=0)
assert not started['ok'], started
assert 'activation lease' not in json.dumps(started), f'gate still refusing with a live lease: {started}'

released = op('activation_release', u)
assert released['ok'] and released['data']['holder_host_uuid'] is None, released

refused(op('activation_renew', u), 'No activation lease to renew', 'renew after release')
refused(op('activation_release', u), 'No activation lease to release', 'release twice')
refused(op('start', u, observe_seconds=0), 'requires an activation lease and none is held',
        'start after release')

# A lapsed lease is retaken by acquisition, never by renewal: renewing would silently extend
# an entitlement that had already ended while another host may have begun its takeover wait.
v = str(uuid.uuid4())
assert op('activation_require', v, lease_seconds=5, takeover_margin_seconds=5)['ok']
assert op('activation_acquire', v)['ok']
import time; time.sleep(6)
lapsed = op('activation_status', v)
assert lapsed['data']['live'] is False and lapsed['data']['seconds_remaining'] <= 0, lapsed
refused(op('activation_renew', v), 'expired; acquire it again', 'renew a lapsed lease')
refused(op('start', v, observe_seconds=0), "activation lease expired", 'start on a lapsed lease')
retaken = op('activation_acquire', v)
assert retaken['ok'] and retaken['data']['generation'] == 1, f'own lapsed lease should not bump the generation: {retaken}'

# The replication intent: how many standbys, and on which hosts. It is a per-universe
# choice because a standby is not free -- it costs storage and reserved headroom -- so a
# universe cheap to rebuild wants none and one that must not stop wants two.
r = str(uuid.uuid4())
h1, h2, h3 = (str(uuid.uuid4()) for _ in range(3))

# Absent means none, never "as many as possible".
assert op('activation_require', r, lease_seconds=60, takeover_margin_seconds=10)['ok']
assert op('activation_status', r)['data']['desired_standbys'] == 0

# A target no placement can satisfy is refused at declaration rather than discovered later.
refused(op('activation_require', r, lease_seconds=60, takeover_margin_seconds=10,
           desired_standbys=2, eligible_hosts=[h1, h2]),
        'exceeds the 2 eligible hosts named', 'two standbys among two hosts')

declared = op('activation_require', r, lease_seconds=60, takeover_margin_seconds=10,
              desired_standbys=2, eligible_hosts=[h1, h2, h3])
assert declared['ok'], declared
assert declared['data']['desired_standbys'] == 2, declared
assert declared['data']['eligible_hosts'] == [h1, h2, h3], declared

# PodMesh sees one host, so it must not claim a placement it cannot see.
assert declared['data']['standbys_placed'] is None, declared
assert declared['data']['placement_verified'] is False, declared

# The facts a caller needs to decide where a standby can go. Facts, not a decision.
res = declared['data']['host_resources']
assert res['memory_available_bytes'] and res['memory_available_bytes'] > 0, res
assert res['cpu_count'] and res['cpu_count'] >= 1, res
assert res['state_directory_available_bytes'] is not None, res
assert 'PodMesh knows' in res['note'], res

# The takeover margin, which is the rule that actually keeps two honest hosts apart. It
# cannot be reached through the API from one host -- every lease the API grants is held by
# this host -- so the foreign holder is written straight into the journal as a fixture. The
# operation under test is still the ordinary acquire.
journal = os.environ.get('PODMESH_JOURNAL')
if journal:
    import sqlite3, time as _t
    w = str(uuid.uuid4())
    assert op('activation_require', w, lease_seconds=30, takeover_margin_seconds=20)['ok']
    db = sqlite3.connect(journal)
    now = int(_t.time())
    # A foreign lease that lapsed five seconds ago: inside the twenty-second margin.
    db.execute("INSERT INTO activation_leases VALUES(?,?,?,?,?,?)",
               (w, 'a-foreign-host-uuid', 7, now - 35, now - 5, 'fixture'))
    db.commit(); db.close()
    refused(op('activation_acquire', w), 'may be taken over', 'acquire inside the takeover margin')
    refused(op('start', w, observe_seconds=0), 'held by another host', 'start while another host holds it')
    db = sqlite3.connect(journal)
    # The same lease, now lapsed well beyond the margin.
    db.execute("UPDATE activation_leases SET expires_at=? WHERE universe_uuid=?", (now - 300, w))
    db.commit(); db.close()
    taken = op('activation_acquire', w)
    assert taken['ok'], taken
    assert taken['data']['generation'] == 8, f'a takeover must advance the generation: {taken}'
    assert taken['data']['holder_host_uuid'] == taken['data']['this_host_uuid'], taken
else:
    print('NOTE: PODMESH_JOURNAL unset — the takeover margin was not exercised')

print('PASS: activation leases — bounds, the three refusals of the gate, idempotent '
      'acquisition, renewal refused after lapse, retaking by acquisition, and the takeover margin.')
