#!/usr/bin/env python3
"""Class 5 for recovery points: collection after a declared retention, never on age alone.

Needs Podman and PODMESH_STATE_DIR. Three generations of one universe are prepared; then the
collector is asked to plan and apply, and every condition of the contract's class 5 is shown
to refuse on its own: no retention declared, a generation the retention keeps, a point younger
than the minimum age, an evidence hold, an investigation hold, the bytes bound, an archive that
no longer hashes to its record. The one collection that goes through leaves a retained
manifest, marks the point collected, removes the archive and its directory, verifies that from
outside, replays without a second removal, and the point's own prepare operation still replays
from the retained manifest.
"""
import hashlib, json, os, socket, subprocess, time, uuid

endpoint = os.environ['PODMESH_SOCKET']
state = os.environ['PODMESH_STATE_DIR']
CLASS = 'recovery_point_archive_after_retention'

def api(request):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(300); s.connect(endpoint)
        s.sendall(json.dumps(request).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())

def op(operation, **extra):
    return api(dict({'operation': operation, 'operation_id': str(uuid.uuid4()),
                     'authorization_ref': 'disposable-lab'}, **extra))

def refused(answer, fragment, label):
    assert not answer['ok'], f'{label}: accepted, expected refusal — {answer}'
    assert fragment in json.dumps(answer), f'{label}: refused for another reason — {answer}'

def plan(scope):
    p = op('garbage_collect_plan', universe_uuids=scope)
    assert p['ok'], p
    return p

def candidate(p, point):
    return next(c for c in p['data']['candidates'] if c['key'] == point)

def apply(plan_id, targets, **extra):
    return op('garbage_collect_apply', plan_operation_id=plan_id, candidates=targets, **extra)

def target(point, universe):
    return {'class': CLASS, 'recovery_point_uuid': point, 'universe_uuid': universe}

images = json.loads(subprocess.check_output(['podman', 'images', '--format', 'json']))
image = next(i['Id'] for i in images if any('alpine' in (n or '') for n in (i.get('Names') or [])))
if not image.startswith('sha256:'):
    image = 'sha256:' + image

u = str(uuid.uuid4()); name = 'podmesh-' + u
try:
    assert op('create', universe_uuid=u, image=image, network_profile='isolated', command=['sh', '-c', "trap 'exit 0' TERM; sleep 600 & wait"])['ok']
    assert op('start', universe_uuid=u, observe_seconds=1)['ok']
    assert op('stop', universe_uuid=u, timeout_seconds=10, on_timeout='kill')['data']['forced'] is False
    prepares, points = [], []
    for _ in range(3):
        pid = str(uuid.uuid4())
        r = api({'operation': 'recovery_point_prepare', 'operation_id': pid, 'universe_uuid': u, 'authorization_ref': 'disposable-lab'})
        assert r['ok'], r
        prepares.append(pid); points.append(r['data']['recovery_point_uuid'])
    g1, g2, g3 = points
    outbox = lambda p: os.path.join(state, 'outbox', p)
    assert all(os.path.isfile(os.path.join(outbox(p), 'rootfs.tar')) for p in points)

    # --- No retention declared: the plan lists the points, each blocked for that reason alone.
    p0 = plan([u])
    for pt in points:
        c = candidate(p0, pt)
        assert c['class'] == CLASS and c['class_number'] == 5 and c['collectable'] is False, c
        assert any('no retention is declared' in b for b in c['blockers']), c['blockers']
        assert c['proofs']['manifest']['sha256_matches_record'] is True and c['proofs']['archive']['present'] is True, c['proofs']
    assert p0['data']['limits']['recovery_points_examined'] == 3, p0['data']['limits']
    refused(apply(p0['data']['collection_operation_id'], [target(g1, u)]), 'no retention is declared', 'apply with no retention')
    assert os.path.isfile(os.path.join(outbox(g1), 'rootfs.tar')), 'a refused apply removed something'

    # --- Retention bounds.
    refused(op('collection_retention_declare', universe_uuid=u, keep_latest=0, minimum_age_seconds=0), 'keep_latest must be', 'keep nothing')
    refused(op('collection_retention_declare', universe_uuid=u, keep_latest=1, minimum_age_seconds=10**10), 'minimum_age_seconds must be', 'age beyond the bound')

    # --- Keep the newest one, but require an age no point has yet: blocked on age alone.
    assert op('collection_retention_declare', universe_uuid=u, keep_latest=1, minimum_age_seconds=3600)['ok']
    p1 = plan([u])
    c = candidate(p1, g1)
    assert c['collectable'] is False and any('younger than 3600' in b for b in c['blockers']), c['blockers']
    assert not any('no retention' in b for b in c['blockers']), c['blockers']
    c3 = candidate(p1, g3)
    assert any('among the newest 1' in b for b in c3['blockers']), c3['blockers']

    # --- Age satisfied, keep the newest one: generations 1 and 2 are candidates, 3 is kept.
    declared = op('collection_retention_declare', universe_uuid=u, keep_latest=1, minimum_age_seconds=0)
    assert declared['ok'] and declared['data']['retention']['keep_latest'] == 1, declared
    p2 = plan([u])
    assert candidate(p2, g1)['collectable'] is True and candidate(p2, g2)['collectable'] is True, p2['data']['candidates']
    c3 = candidate(p2, g3)
    assert c3['collectable'] is False and any('among the newest 1' in b for b in c3['blockers']), c3
    assert p2['data']['counts']['collectable_by_class'][CLASS] == 2, p2['data']['counts']
    refused(apply(p2['data']['collection_operation_id'], [target(g3, u)]), 'among the newest 1', 'apply on the kept generation')

    # --- Holds. An evidence hold blocks the deletion; an investigation hold blocks everything.
    hold = op('collection_hold_declare', universe_uuid=u, scope='evidence_hold', reason='incident 42 under review')
    assert hold['ok'] and hold['data']['holds'][0]['scope'] == 'evidence_hold', hold
    hid = hold['data']['holds'][0]['hold_id']
    refused(op('collection_hold_declare', universe_uuid=u, scope='legal', reason='x'), 'scope must be', 'unknown hold scope')
    # The journal contract: the same declaration under its ID is history; a different one under that ID is refused.
    hq = {'operation': 'collection_hold_declare', 'operation_id': hid, 'universe_uuid': u, 'authorization_ref': 'disposable-lab',
          'scope': 'evidence_hold', 'reason': 'incident 42 under review'}
    replay = api(hq)
    assert replay['ok'] and replay['data']['replayed'] is True and len(replay['data']['holds']) == 1, replay
    refused(api(dict(hq, reason='something else')), 'already belongs to a different request', 'hold declaration reused under a different request')
    assert len(op('collection_status', universe_uuid=u)['data']['holds']) == 1
    p3 = plan([u])
    assert any('evidence hold' in b and 'artifact deletion' in b for b in candidate(p3, g1)['blockers']), candidate(p3, g1)['blockers']
    refused(apply(p2['data']['collection_operation_id'], [target(g1, u)]), 'evidence hold', 'apply under an evidence hold')
    assert os.path.isfile(os.path.join(outbox(g1), 'rootfs.tar'))
    released = op('collection_hold_release', universe_uuid=u, hold_id=hid)
    assert released['ok'] and released['data']['holds'] == [] and released['data']['released_holds'][0]['hold_id'] == hid, released
    refused(op('collection_hold_release', universe_uuid=u, hold_id=hid), 'already released', 'release twice')
    refused(op('collection_hold_release', universe_uuid=u, hold_id=str(uuid.uuid4())), 'No hold', 'release an unknown hold')
    inv = op('collection_hold_declare', universe_uuid=u, scope='investigation_hold', reason='forensics')
    assert inv['ok']
    refused(apply(p2['data']['collection_operation_id'], [target(g1, u)]), 'investigation hold', 'apply under an investigation hold')
    assert op('collection_hold_release', universe_uuid=u, hold_id=inv['data']['holds'][0]['hold_id'])['ok']

    # --- The bytes bound: an archive larger than the run's allowance is not removed.
    size = os.path.getsize(os.path.join(outbox(g1), 'rootfs.tar'))
    refused(apply(p2['data']['collection_operation_id'], [target(g1, u)], max_bytes=size - 1), 'max_bytes', 'apply under a bytes bound too small')
    assert os.path.isfile(os.path.join(outbox(g1), 'rootfs.tar'))

    # --- A tampered archive is never collected: the fresh proof re-hashes it. Generation 2 is tampered
    # inside its bytes; the plan (which sizes but does not hash) still lists it, the apply refuses it.
    with open(os.path.join(outbox(g2), 'rootfs.tar'), 'r+b') as f:
        f.seek(1024 + 256); b = f.read(1); f.seek(1024 + 256); f.write(bytes([b[0] ^ 0x01]))
    refused(apply(p2['data']['collection_operation_id'], [target(g2, u)]), 'no longer hashes', 'apply on a tampered archive')
    assert os.path.isfile(os.path.join(outbox(g2), 'rootfs.tar')), 'a refused apply removed the evidence'

    # --- The collection that goes through.
    aid = str(uuid.uuid4())
    done = api({'operation': 'garbage_collect_apply', 'operation_id': aid, 'authorization_ref': 'disposable-lab',
                'plan_operation_id': p2['data']['collection_operation_id'], 'candidates': [target(g1, u)]})
    assert done['ok'], done
    d = done['data']
    assert d['status'] == 'verified' and d['effects_applied'] == 1 and d['bytes_removed'] >= size, d
    r = d['results'][0]
    assert r['action'] == 'collected_recovery_point' and r['verified'] is True and r['generation'] == 1, r
    assert r['retained_manifest']['collecting_operation_id'] == aid and r['retained_manifest']['terminal_state'] == 'collected', r
    assert r['proofs_repeated_before_the_effect']['archive']['sha256_matches_record'] is True, r
    assert not os.path.exists(outbox(g1)), 'the outbox directory survived the collection'
    assert os.path.isfile(os.path.join(outbox(g3), 'rootfs.tar')) and os.path.isfile(os.path.join(outbox(g2), 'rootfs.tar')), 'a collection touched another point'

    # The journal: state collected, the retained manifest, the status, and the prepare's replay.
    st = op('recovery_point_status', universe_uuid=u)['data']
    assert [p['state'] for p in st['recovery_points']] == ['collected', 'prepared', 'prepared'], st
    cs = op('collection_status', universe_uuid=u)['data']
    assert cs['retained_manifests'][0]['recovery_point_uuid'] == g1 and cs['retained_manifests'][0]['collecting_operation_id'] == aid, cs
    replayed = api({'operation': 'recovery_point_prepare', 'operation_id': prepares[0], 'universe_uuid': u, 'authorization_ref': 'disposable-lab'})
    assert replayed['ok'] and replayed['data']['replayed'] is True and replayed['data']['collected'] is True, replayed
    assert replayed['data']['current_manifest']['pieces'][0]['plaintext_sha256'] == candidate(p2, g1)['proofs']['recorded_rootfs_sha256'], replayed
    # Once collected it is no longer a candidate, and an apply naming it is refused, not repeated.
    p4 = plan([u])
    assert all(c['key'] != g1 for c in p4['data']['candidates']), 'a collected point is still a candidate'
    refused(apply(p2['data']['collection_operation_id'], [target(g1, u)]), 'not prepared', 'apply on a collected point')

    # Replay of the apply: history, no second effect.
    again = api({'operation': 'garbage_collect_apply', 'operation_id': aid, 'authorization_ref': 'disposable-lab',
                 'plan_operation_id': p2['data']['collection_operation_id'], 'candidates': [target(g1, u)]})
    assert again['ok'] and again['data']['replayed'] is True and again['data']['current']['candidates'][0]['outbox_present'] is False, again

    # --- Recovery after a crash between the retained manifest and the removal: the journal already says
    # collected; a retry finishes the removal rather than deciding anything. Simulated by restoring the
    # archive directory of a fresh collection from a copy, then deleting the collector's progress row.
    # Generation 2 is repaired first (its tamper is undone) so that it is collectable.
    import shutil, sqlite3
    with open(os.path.join(outbox(g2), 'rootfs.tar'), 'r+b') as f:
        f.seek(1024 + 256); b = f.read(1); f.seek(1024 + 256); f.write(bytes([b[0] ^ 0x01]))
    backup = os.path.join(state, 'g2-copy'); shutil.copytree(outbox(g2), backup)
    aid2 = str(uuid.uuid4())
    done2 = api({'operation': 'garbage_collect_apply', 'operation_id': aid2, 'authorization_ref': 'disposable-lab',
                 'plan_operation_id': p2['data']['collection_operation_id'], 'candidates': [target(g2, u)]})
    assert done2['ok'] and done2['data']['results'][0]['verified'] is True, done2
    shutil.copytree(backup, outbox(g2))                      # the crash left the files behind
    j = sqlite3.connect(os.path.join(state, 'state.sqlite'))
    j.execute('DELETE FROM garbage_collection_effects WHERE operation_id=?', (aid2,))
    j.execute("UPDATE operations SET status='pending' WHERE id=?", (aid2,))
    j.commit(); j.close()
    resumed = api({'operation': 'garbage_collect_apply', 'operation_id': aid2, 'authorization_ref': 'disposable-lab',
                   'plan_operation_id': p2['data']['collection_operation_id'], 'candidates': [target(g2, u)]})
    assert resumed['ok'], resumed
    rr = resumed['data']['results'][0]
    assert rr['recovered'] is True and rr['verified'] is True and 'finished, not repeated' in rr['note'], rr
    assert not os.path.exists(outbox(g2)), 'the recovered effect did not finish the removal'
    assert resumed['data']['effects_recovered'] == 1 and resumed['data']['effects_applied'] == 1, resumed['data']

    print('PASS: recovery point retention — class 5 blocked without a retention, on age, on the kept generation, '
          'under an evidence hold and an investigation hold, on the bytes bound and on a tampered archive; one '
          'collection retained its manifest, removed the archive and verified it, left the others alone; the '
          'point replays from the retained manifest; the apply replays without a second effect; and a crash '
          'between the record and the removal is finished on retry.')
finally:
    subprocess.run(['podman', 'rm', '-f', name], capture_output=True)
