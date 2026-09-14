#!/usr/bin/env python3
"""Promotion: a quarantined restore becomes the universe's own identity, under the lease.

Needs Podman, PODMESH_STATE_DIR and PODMESH_JOURNAL. This is the standby's side of a level 2
takeover on one host: the point is restored into quarantine, the universe's activation policy
is declared here, its lease is acquired -- refused while a foreign lease is inside the
takeover margin, written straight into the journal as a fixture since no API grants a lease to
another host -- and only then may the copy be promoted. The proof is the marker the source
wrote while running, exported again from the PROMOTED universe once it has been started
through the ordinary gate.
"""
import io, json, os, shutil, socket, sqlite3, subprocess, tarfile, time, uuid

endpoint = os.environ['PODMESH_SOCKET']
state = os.environ['PODMESH_STATE_DIR']
journal = os.environ['PODMESH_JOURNAL']

def api(request):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(180); s.connect(endpoint)
        s.sendall(json.dumps(request).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())

def op(operation, **extra):
    return api(dict({'operation': operation, 'operation_id': str(uuid.uuid4()),
                     'authorization_ref': 'disposable-lab'}, **extra))

def refused(answer, fragment, label):
    assert not answer['ok'], f'{label}: accepted, expected refusal — {answer}'
    assert fragment in json.dumps(answer), f'{label}: refused for another reason — {answer}'

def marker_in(name, marker):
    tar = subprocess.run(['podman', 'export', name], capture_output=True, check=True).stdout
    with tarfile.open(fileobj=io.BytesIO(tar)) as t:
        hit = next((n for n in t.getnames() if n.strip('./') == f'marker-{marker}'), None)
        return hit and t.extractfile(hit).read() == marker.encode()

def capture_and_restore(marker, src, new, as_source=None):
    """Capture a marked universe and restore it into quarantine, as the sibling checks prove.

    With `as_source`, the manifest carried to the inbox names that UUID as the universe it
    was taken from -- which is how a second host sees it: a source it has never created.
    """
    assert op('create', universe_uuid=src, image=image,
              command=['sh', '-c', f"printf %s '{marker}' > /marker-{marker}; trap 'exit 0' TERM; sleep 600 & wait"])['ok']
    assert op('start', universe_uuid=src, observe_seconds=1)['ok']
    assert op('stop', universe_uuid=src, timeout_seconds=10, on_timeout='kill')['data']['forced'] is False
    prepared = op('recovery_point_prepare', universe_uuid=src)
    assert prepared['ok'], prepared
    point = prepared['data']['recovery_point_uuid']
    shutil.copytree(os.path.join(state, 'outbox', point), os.path.join(state, 'inbox', point))
    if as_source:
        path = os.path.join(state, 'inbox', point, 'recovery-point-manifest.json')
        m = json.load(open(path)); m['universe_uuid'] = as_source
        with open(path, 'w') as f:
            f.write(json.dumps(m, sort_keys=True, separators=(',', ':')))
    restored = op('recovery_point_restore', universe_uuid=new, recovery_point_uuid=point)
    assert restored['ok'], restored
    return point

images = json.loads(subprocess.check_output(['podman', 'images', '--format', 'json']))
image = next(i['Id'] for i in images if any('alpine' in (n or '') for n in (i.get('Names') or [])))
if not image.startswith('sha256:'):
    image = 'sha256:' + image

marker = uuid.uuid4().hex
src = str(uuid.uuid4()); quarantined = str(uuid.uuid4())
other_src = str(uuid.uuid4()); other_quarantined = str(uuid.uuid4())
# The identity the standby takes over is the source's own UUID, which a real standby has never
# created. Here the source lives on the same host, so the manifest carried to the inbox names
# a fresh UUID as its source, exactly as a second host would see it.
takeover = str(uuid.uuid4())
points = []
cleanup = ['podmesh-' + u for u in (src, quarantined, other_src, other_quarantined)]
try:
    point = capture_and_restore(marker, src, quarantined, as_source=takeover); points.append(point)
    other = capture_and_restore(uuid.uuid4().hex, other_src, other_quarantined); points.append(other)

    # Refusals, each placed where the rule named for it is the ONLY thing refusing, so that
    # removing that rule from the daemon turns this check red with "accepted", not with
    # another rule's message.
    refused(op('recovery_point_promote', universe_uuid=takeover, restored_universe_uuid=str(uuid.uuid4())),
            'No quarantined restore under this identifier', 'promote an unknown copy')
    refused(op('recovery_point_promote', universe_uuid=takeover, restored_universe_uuid=takeover),
            'cannot be the same universe', 'promote a copy into itself')
    refused(op('recovery_point_promote', universe_uuid=takeover, restored_universe_uuid=quarantined),
            'under no activation policy', 'promote with no policy declared')

    assert op('activation_require', universe_uuid=takeover, lease_seconds=30, takeover_margin_seconds=20)['ok']

    # A foreign lease that lapsed five seconds ago: inside the twenty-second margin. Neither
    # the acquisition nor the promotion may go through. No API grants a lease to another host,
    # so the foreign holder is written straight into the journal as a fixture.
    now = int(time.time())
    j = sqlite3.connect(journal)
    j.execute('INSERT INTO activation_leases VALUES(?,?,?,?,?,?)',
              (takeover, 'a-foreign-host-uuid', 3, now - 35, now - 5, 'fixture'))
    j.commit(); j.close()
    refused(op('activation_acquire', universe_uuid=takeover), 'may be taken over', 'acquire inside the takeover margin')
    refused(op('recovery_point_promote', universe_uuid=takeover, restored_universe_uuid=quarantined),
            'held by another host', 'promote while the previous holder is inside the margin')

    # No lease at all, under a policy.
    j = sqlite3.connect(journal)
    assert j.execute('DELETE FROM activation_leases WHERE universe_uuid=?', (takeover,)).rowcount == 1
    j.commit(); j.close()
    refused(op('recovery_point_promote', universe_uuid=takeover, restored_universe_uuid=quarantined),
            'none is held', 'promote with a policy but no lease')

    # The same foreign lease, lapsed well beyond the margin: the takeover is allowed, and the
    # generation advances.
    j = sqlite3.connect(journal)
    j.execute('INSERT INTO activation_leases VALUES(?,?,?,?,?,?)',
              (takeover, 'a-foreign-host-uuid', 3, now - 335, now - 300, 'fixture'))
    j.commit(); j.close()
    taken = op('activation_acquire', universe_uuid=takeover)
    assert taken['ok'] and taken['data']['generation'] == 4, taken

    # With the lease held, the copy of ANOTHER universe is still refused: only the source rule
    # stands between it and a promotion.
    refused(op('recovery_point_promote', universe_uuid=takeover, restored_universe_uuid=other_quarantined),
            'not from this one', 'promote a copy of a different universe')

    pid = str(uuid.uuid4())
    promoted = api({'operation': 'recovery_point_promote', 'operation_id': pid, 'universe_uuid': takeover,
                    'authorization_ref': 'disposable-lab', 'restored_universe_uuid': quarantined})
    assert promoted['ok'], promoted
    d = promoted['data']
    cleanup.append('podmesh-' + takeover)
    assert d['universe_uuid'] == takeover and d['restored_universe_uuid'] == quarantined and d['recovery_point_uuid'] == point, d
    assert d['lease_generation'] == 4 and d['started'] is False and 'not mutual exclusion' in d['scope'], d

    # Created under the universe's own identity, not started, no network.
    insp = json.loads(subprocess.check_output(['podman', 'inspect', 'podmesh-' + takeover]))[0]
    assert insp['State']['Status'] == 'created' and insp['HostConfig']['NetworkMode'] == 'none', insp['State']
    assert insp['Config']['Labels'].get('io.podmesh.universe') == takeover
    # The quarantined copy is left where it was.
    assert subprocess.run(['podman', 'container', 'exists', 'podmesh-' + quarantined]).returncode == 0

    # THE assertion, taken BEFORE the first start: the promoted universe already carries the
    # marker the source wrote while running. After a start it would prove nothing, since the
    # universe's own command writes that marker again -- a promotion from the pristine image
    # passed this check until it was moved here.
    assert marker_in('podmesh-' + takeover, marker), 'the promoted universe does not carry the marker'

    # The start goes through the ordinary gate.
    assert op('start', universe_uuid=takeover, observe_seconds=1)['ok']
    assert op('stop', universe_uuid=takeover, timeout_seconds=10, on_timeout='kill')['ok']

    # Idempotent by operation ID.
    again = api({'operation': 'recovery_point_promote', 'operation_id': pid, 'universe_uuid': takeover,
                 'authorization_ref': 'disposable-lab', 'restored_universe_uuid': quarantined})
    assert again['ok'] and again['data']['replayed'] is True and again['data']['container_id'] == d['container_id'], again

    # And once promoted, the fence applies to it like to any universe under a policy.
    assert op('activation_release', universe_uuid=takeover)['ok']
    refused(op('start', universe_uuid=takeover, observe_seconds=0), 'none is held', 'start after the lease was released')

    print('PASS: promotion — refused for an unknown copy, a copy of another universe, a copy into itself, '
          'no policy, no lease, and a previous holder inside the takeover margin; allowed after the margin '
          'with the generation advanced; created under the universe\'s own identity, quarantined copy left in '
          'place; started through the gate and carrying the marker; replay idempotent; gate holds afterwards.')
finally:
    for n in cleanup:
        subprocess.run(['podman', 'rm', '-f', n], capture_output=True)
    tags = subprocess.run(['podman', 'images', '--format', '{{.Repository}}:{{.Tag}}'], capture_output=True, text=True).stdout.split()
    for tag in tags:
        if tag.startswith('localhost/podmesh-restore:') and any(tag.endswith(p) for p in points):
            subprocess.run(['podman', 'rmi', '-f', tag], capture_output=True)
