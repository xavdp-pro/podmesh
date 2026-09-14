#!/usr/bin/env python3
"""Epoch-bound activation: the maker's screen, as the fencing laboratory defines it.

Runs against a daemon over the local socket; no Podman, no root. The permits are minted here,
because the gate is the laboratory's fixture and PodMesh never contacts it: what is under test
is PodMesh's side -- exact permit form, binding to this universe, this host and this boot, the
durable screen that refuses superseded epochs, the takeover that needs a newer epoch, and the
supersession that voids this host's entitlement in the gate, the renewal and the fence's view.
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

this_host = api({'operation': 'identity'})['data']['host_uuid']
boot = open('/proc/sys/kernel/random/boot_id').read().strip()
AUTHORITY = 'gate-' + uuid.uuid4().hex[:12]

def permit(resource, epoch, replica=this_host, instance=boot, authority=AUTHORITY, grant=None):
    return {'authority_id': authority, 'resource': resource, 'epoch': epoch,
            'replica_id': replica, 'instance_id': instance, 'grant_id': grant or 'grant-' + uuid.uuid4().hex[:12]}

u = str(uuid.uuid4())

# --- An ungated policy refuses a permit: it would be checked against nothing.
assert op('activation_require', u, lease_seconds=60, takeover_margin_seconds=10)['ok']
refused(op('activation_acquire', u, permit=permit(u, 1)), 'names no authority', 'permit under an ungated policy')
refused(op('activation_supersede', u, permit=permit(u, 1)), 'names no authority', 'supersede under an ungated policy')

# --- Declare the authority. The identifier is the laboratory's.
refused(op('activation_require', u, lease_seconds=60, takeover_margin_seconds=10, authority_id='-bad'),
        'starting alphanumeric', 'authority identifier starting with a hyphen')
refused(op('activation_require', u, lease_seconds=60, takeover_margin_seconds=10, authority_id='x' * 97),
        'ASCII characters', 'authority identifier too long')
gated = op('activation_require', u, lease_seconds=60, takeover_margin_seconds=10, authority_id=AUTHORITY)
assert gated['ok'] and gated['data']['authority_id'] == AUTHORITY, gated

# --- Under an authority, acquisition needs a permit, in exactly the six fields.
refused(op('activation_acquire', u), 'requires a permit', 'gated acquire without a permit')
refused(op('activation_acquire', u, permit='not-an-object'), 'must be an object', 'permit that is a string')
missing = permit(u, 1); del missing['grant_id']
refused(op('activation_acquire', u, permit=missing), 'exactly the fields', 'permit missing a field')
extra = permit(u, 1); extra['note'] = 'x'
refused(op('activation_acquire', u, permit=extra), 'exactly the fields', 'permit with an extra field')
refused(op('activation_acquire', u, permit=permit(u, 0)), 'epoch must be from 1', 'epoch zero')
refused(op('activation_acquire', u, permit=permit(u, 2**31)), 'epoch must be from 1', 'epoch beyond the bound')
refused(op('activation_acquire', u, permit=permit(u, 'one')), 'epoch must be an integer', 'epoch as text')
refused(op('activation_acquire', u, permit=permit(u, 1, grant='bad grant')), 'ASCII characters', 'grant with a space')
# The laboratory's 4096-byte bound on the serialized permit is unreachable through this API:
# a whole request is refused at the same size before any field is read, so the outer bound
# is what refuses here, and the permit's own bound is defence in depth that the code says is
# unreachable. Six identifiers of at most 96 characters could never reach it anyway.
huge = permit(u, 1); huge['grant_id'] = 'g' * 5000
refused(op('activation_acquire', u, permit=huge), 'Request too large', 'oversized permit, refused by the request bound')

# --- Binding: this universe, this host, this boot, this authority.
refused(op('activation_acquire', u, permit=permit(u, 1, authority='another-gate')), 'different authority', 'permit from another authority')
refused(op('activation_acquire', u, permit=permit(str(uuid.uuid4()), 1)), 'different resource', 'permit for another universe')
refused(op('activation_acquire', u, permit=permit(u, 1, replica=str(uuid.uuid4()))), 'bound to another replica', 'permit bound to another host')
refused(op('activation_acquire', u, permit=permit(u, 1, instance=str(uuid.uuid4()))), 'another incarnation', 'permit bound to a previous boot')

# --- A well-bound permit at epoch 1 activates; the screen records it.
first = op('activation_acquire', u, permit=permit(u, 1, grant='grant-one'))
assert first['ok'] and first['data']['epoch'] == 1 and first['data']['grant_id'] == 'grant-one', first
assert first['data']['highest_epoch_seen'] == 1 and first['data']['superseded'] is False, first
assert first['data']['generation'] == 1 and first['data']['live'] is True, first
assert 'never start a second one' in first['data']['permit_verification'], first

# Re-acquiring under the same grant is idempotent; under a newer grant of our own it moves
# the epoch without a takeover.
again = op('activation_acquire', u, permit=permit(u, 1, grant='grant-one'))
assert again['ok'] and again['data']['generation'] == 1 and again['data']['epoch'] == 1, again
# One grant per epoch: a different grant at the recorded epoch is not something the gate issues.
refused(op('activation_acquire', u, permit=permit(u, 1, grant='grant-one-again')), 'already granted here under another grant', 'second grant at the recorded epoch')
newer = op('activation_acquire', u, permit=permit(u, 2, grant='grant-two'))
assert newer['ok'] and newer['data']['generation'] == 1 and newer['data']['epoch'] == 2, newer

# --- The screen: an epoch already superseded is refused whatever else the permit says.
refused(op('activation_acquire', u, permit=permit(u, 1, grant='grant-one')), 'is superseded', 'acquire under a superseded epoch')
assert op('activation_renew', u)['ok']
started = op('start', u, observe_seconds=0)
assert not started['ok'] and 'activation' not in json.dumps(started), f'gate refusing with a live, current lease: {started}'

# --- Supersession: a newer grant bound to ANOTHER replica is delivered here.
refused(op('activation_supersede', u, permit=permit(u, 2, replica='other-host')), 'does not supersede', 'supersede with the current epoch')
refused(op('activation_supersede', u, permit=permit(u, 3, replica='other-host', authority='another-gate')), 'different authority', 'supersede from another authority')
over = op('activation_supersede', u, permit=permit(u, 3, replica='other-host', instance='other-boot', grant='grant-three'))
assert over['ok'] and over['data']['highest_epoch_seen'] == 3 and over['data']['superseded'] is True, over
# The lease row is still this host's and still live -- and no longer an entitlement.
assert over['data']['holder_host_uuid'] == this_host and over['data']['live'] is True and over['data']['epoch'] == 2, over
refused(op('start', u, observe_seconds=0), 'superseded by epoch 3', 'start after supersession')
refused(op('activation_renew', u), 'superseded by epoch 3', 'renew after supersession')
refused(op('activation_acquire', u, permit=permit(u, 2, grant='grant-two')), 'is superseded', 'acquire again under the overtaken epoch')
refused(op('activation_supersede', u, permit=permit(u, 3, replica='other-host')), 'does not supersede', 'supersede twice with the same epoch')

# The fence's view: this universe is no longer one it is entitled to run. There is no
# container, so the fence reports it as not running rather than fencing it -- what matters
# is that it is NOT reported as "lease is live".
fenced = api({'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': 'disposable-lab', 'timeout_seconds': 5})
assert fenced['ok'], fenced
mine = {e['universe_uuid']: e for e in fenced['data']['left_running_or_absent']}
assert u in mine and mine[u]['reason'] == 'not running', f'a superseded lease must not count as live: {mine.get(u)}'

# --- Rotation back to this host: epoch 4, bound here, takes over cleanly.
back = op('activation_acquire', u, permit=permit(u, 4, grant='grant-four'))
assert back['ok'] and back['data']['epoch'] == 4 and back['data']['superseded'] is False, back
assert op('activation_renew', u)['ok']

# --- The takeover from another holder needs a newer epoch than that holder's, on top of the
# margin; the foreign holder is a journal fixture, as in the lease check.
journal = os.environ.get('PODMESH_JOURNAL')
if journal:
    import sqlite3, time as _t
    w = str(uuid.uuid4())
    assert op('activation_require', w, lease_seconds=30, takeover_margin_seconds=5, authority_id=AUTHORITY)['ok']
    db = sqlite3.connect(journal); now = int(_t.time())
    # A foreign lease at epoch 5, lapsed well beyond the margin.
    db.execute("INSERT INTO activation_leases VALUES(?,?,?,?,?,?,?,?)",
               (w, 'a-foreign-host-uuid', 7, now - 335, now - 300, 'fixture', 5, 'grant-five'))
    db.commit(); db.close()
    refused(op('activation_acquire', w, permit=permit(w, 5)), 'requires a newer epoch than the previous holder', 'takeover under the same epoch')
    refused(op('activation_acquire', w, permit=permit(w, 4)), 'requires a newer epoch than the previous holder', 'takeover under an older epoch')
    taken = op('activation_acquire', w, permit=permit(w, 6))
    assert taken['ok'] and taken['data']['generation'] == 8 and taken['data']['epoch'] == 6, taken
    # And inside the margin, even a newer epoch does not shorten the wait: the margin guards
    # the ungated start, which no epoch can gate.
    x = str(uuid.uuid4())
    assert op('activation_require', x, lease_seconds=30, takeover_margin_seconds=20, authority_id=AUTHORITY)['ok']
    db = sqlite3.connect(journal); now = int(_t.time())
    db.execute("INSERT INTO activation_leases VALUES(?,?,?,?,?,?,?,?)",
               (x, 'a-foreign-host-uuid', 1, now - 35, now - 5, 'fixture', 1, 'grant-one'))
    db.commit(); db.close()
    refused(op('activation_acquire', x, permit=permit(x, 2)), 'may be taken over', 'newer epoch inside the takeover margin')
else:
    print('NOTE: PODMESH_JOURNAL unset — the takeover-needs-a-newer-epoch rule was not exercised')

print('PASS: epoch-bound activation — permit form and bounds, binding to universe, host, boot and '
      'authority, the screen refusing superseded epochs, supersession voiding the gate, the renewal '
      'and the fence\'s entitlement, rotation back, and a takeover needing a newer epoch on top of the margin.')
