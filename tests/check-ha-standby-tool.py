#!/usr/bin/env python3
"""The agent's side as a tool: activate, two capture cycles, a takeover, on two lab hosts.

Same environment as check-recovery-point-two-hosts.py, plus PODMESH_FENCING_LAB (the directory
holding the fencing laboratory's fencing_lab.py). The gate and the ledger live in a temporary
directory of this run. The tool is driven as a subprocess, exactly as an operator or an agent
would drive it, and its JSON reports are what is asserted on -- plus what the hosts show from
outside: the marker written on the active host found in the promoted universe before its first
start, and the active host refused afterwards.
"""
import json, os, subprocess, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-ha-tool-')
A = Host('active', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
B = Host('standby', os.environ['PODMESH_DESTINATION_SSH'], control, socket_path, state_dir, unit)
work = tempfile.mkdtemp(prefix='podmesh-ha-gate-')
env = dict(os.environ, PODMESH_GATE=os.path.join(work, 'gate.sqlite'), PODMESH_HA_LEDGER=os.path.join(work, 'ledger'))
reference = 'disposable-lab-ha-tool'
checks = []

def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', reference, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)

def image_on(host):
    out = host.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}}', 'docker.io/library/alpine:3.22'])
    return next(l for l in out['stdout'].split() if l.startswith('sha256:'))

u = str(uuid.uuid4()); marker = uuid.uuid4().hex
ledger = None
try:
    gate = tool('gate', 'init')
    assert gate['created'] and gate['authority_id'], gate
    assert tool('gate', 'declare', '--universe', u)['epoch'] == 0
    checks.append('gate created and the universe declared as its resource at epoch 0')

    A.ok(request('create', u, reference, image=image_on(A), network_profile='isolated',
                 command=['sh', '-c', f"printf %s '{marker}' > /marker-{marker}; trap 'exit 0' TERM; sleep 600 & wait"]))
    activated = tool('activate', '--universe', u, '--host', os.environ['PODMESH_SOURCE_SSH'], '--lease', '20', '--margin', '5')
    assert activated['epoch'] == 1 and activated['lease']['live'] and activated['started'] is False, activated
    A.ok(request('start', u, reference, observe_seconds=1))
    checks.append('activated on the active host under epoch 1 and started through the API')

    # A cycle refuses when the host does not hold a live lease -- the standby here.
    refused = tool('cycle', '--universe', u, '--active', os.environ['PODMESH_DESTINATION_SSH'], '--standby', os.environ['PODMESH_SOURCE_SSH'], expect=2)
    assert 'does not hold a live lease' in refused['refused'], refused
    checks.append('a cycle from a host without the lease is refused')

    first = tool('cycle', '--universe', u, '--active', os.environ['PODMESH_SOURCE_SSH'], '--standby', os.environ['PODMESH_DESTINATION_SSH'], '--keep', '1')
    assert first['generation'] == 1 and first['manifest_signed'] is False and first['pruned_on_standbys'] == [], first
    q1 = first['quarantined']
    assert B.call('marker', name='podmesh-' + q1, marker=marker)['present'], 'the first quarantined copy lacks the marker'
    second = tool('cycle', '--universe', u, '--active', os.environ['PODMESH_SOURCE_SSH'], '--standby', os.environ['PODMESH_DESTINATION_SSH'], '--keep', '1')
    assert second['generation'] == 2 and second['points_on_active_outbox'] == 2, second
    assert second['retention_declared_on_active'] == {'keep_latest': 3, 'minimum_age_seconds': 3600}, second
    rt = A.ok(request('collection_status', u, reference))
    assert rt['retention']['keep_latest'] == 3 and rt['retention']['minimum_age_seconds'] == 3600, rt
    q2 = second['quarantined']
    assert q1 in second['pruned_on_standbys'] or any(k['quarantined_uuid'] == q1 for k in second['prune_refused']), second
    checks.append('two capture cycles: generations 1 and 2, the marker in the quarantined copy, the older copy pruned or its refusal recorded (%s)'
                  % ('pruned' if q1 in second['pruned_on_standbys'] else 'refused: ' + second['prune_refused'][0]['refused']))
    running = A.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u])['stdout'].strip()
    assert running == 'true', 'the active universe is not running again after the capture'

    # A takeover while the active host is reachable and entitled is refused: that is a handoff.
    refused = tool('takeover', '--universe', u, '--active', os.environ['PODMESH_SOURCE_SSH'], '--standby', os.environ['PODMESH_DESTINATION_SSH'], expect=2)
    assert 'planned handoff' in refused['refused'], refused
    checks.append('a takeover while the active host is reachable and entitled is refused as a handoff, not a takeover')

    # The failure: the active host stops renewing. The tool waits for the lapse plus the margin on the
    # active host's own clock, fences it, rotates the epoch, promotes -- and leaves the start to us so
    # that the marker can be checked before it.
    status = A.ok(request('activation_status', u, reference))
    while A.call('time')['time'] < status['expires_at'] + 1:
        time.sleep(.5)
    took = tool('takeover', '--universe', u, '--active', os.environ['PODMESH_SOURCE_SSH'], '--standby', os.environ['PODMESH_DESTINATION_SSH'], '--no-start')
    assert took['epoch'] == 2 and took['active_reachable'] and took['started'] is False, took
    assert took['waited']['fence'] and took['waited']['fence'].get('forced') is False, took['waited']
    assert took['promoted_from']['quarantined_uuid'] == q2 and took['active_superseded']['delivered'] and took['active_superseded']['highest_epoch_seen'] == 2, took
    assert took['data_lost_since_seconds'] >= 0 and took['not_proven'], took
    assert B.call('marker', name='podmesh-' + u, marker=marker)['present'], 'the promoted universe on the standby lacks the marker before its first start'
    B.ok(request('start', u, reference, observe_seconds=1))
    assert B.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u])['stdout'].strip() == 'true'
    assert A.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + u])['stdout'].strip() == 'false'
    r = A.api(request('start', u, reference, observe_seconds=0))
    assert not r['ok'] and 'activation' in json.dumps(r), r
    checks.append('takeover after the lapse: fenced, margin waited on the active clock, epoch 2 on the standby, promoted from the newest copy, '
                  'marker present before the first start, running on the standby, stopped and refused on the active host, superseded')

    # The gate itself: a second rotation from the same expected epoch is refused -- the laboratory's rule.
    assert tool('gate', 'inspect', '--universe', u)['resource']['epoch'] == 2
    checks.append('the gate stands at epoch 2')

    # The realistic failure: the active host cannot be reached at all. A second universe is captured once;
    # then the takeover is asked with an unreachable address for the active side. The tool waits lease +
    # margin on the standby's clock -- by which time any lease the active host held has lapsed -- and takes
    # over. The active host, still running its copy exactly as a partitioned host would, is then stopped by
    # its OWN fence when that is run there: the two windows did not overlap, which is the design's claim.
    v = str(uuid.uuid4()); marker2 = uuid.uuid4().hex
    tool('gate', 'declare', '--universe', v)
    A.ok(request('create', v, reference, image=image_on(A), network_profile='isolated',
                 command=['sh', '-c', f"printf %s '{marker2}' > /marker-{marker2}; trap 'exit 0' TERM; sleep 600 & wait"]))
    # Activated with a LONGER lease and margin than the tool's defaults: the takeover carries no
    # flags, and the wait must come from what was activated, not from a default. A first version
    # of the tool waited on its own defaults here, which the independent review caught.
    tool('activate', '--universe', v, '--host', os.environ['PODMESH_SOURCE_SSH'], '--lease', '30', '--margin', '10')
    A.ok(request('start', v, reference, observe_seconds=1))
    one = tool('cycle', '--universe', v, '--active', os.environ['PODMESH_SOURCE_SSH'], '--standby', os.environ['PODMESH_DESTINATION_SSH'])
    q3 = one['quarantined']
    renewed_at = A.call('time')['time']
    began = time.time()
    took2 = tool('takeover', '--universe', v, '--active', 'lab@203.0.113.1', '--standby', os.environ['PODMESH_DESTINATION_SSH'])
    waited = time.time() - began
    assert took2['active_reachable'] is False and took2['active_superseded'] is None and took2['started'] is True, took2
    assert took2['waited']['margin']['on'].startswith("the standby's clock"), took2['waited']
    assert took2['waited']['margin']['lease_seconds'] == 30 and took2['waited']['margin']['takeover_margin_seconds'] == 10, took2['waited']
    assert waited >= 41, f'the tool did not wait the ACTIVATED lease + margin on the standby: {waited:.1f}s'
    assert B.call('marker', name='podmesh-' + v, marker=marker2)['present'], 'the promoted second universe lacks its marker'
    lapsed = A.ok(request('activation_status', v, reference))
    assert lapsed['live'] is False and lapsed['expires_at'] <= renewed_at + 30, lapsed
    fenced = A.ok({'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference, 'timeout_seconds': 10})
    hit = {e['universe_uuid']: e for e in fenced['fenced']}
    assert v in hit and hit[v]['forced'] is False, f'the active host did not fence its copy after the lapse: {fenced}'
    assert A.call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + v])['stdout'].strip() == 'false'
    checks.append('takeover with the active host unreachable: waited %.0fs (the ACTIVATED lease 30 + margin 10, not the tool\'s defaults) on the standby\'s clock, promoted and started there; '
                  'the active host\'s lease had lapsed by then and its own fence stopped its copy without escalation' % waited)
    print(json.dumps({'result': 'PASS', 'checks': checks, 'universe': u, 'quarantined': [q1, q2, q3]}, indent=2))
finally:
    v_ = v if 'v' in dir() else ''
    for host, names in ((A, [u, v_]), (B, [q1 if 'q1' in dir() else '', q2 if 'q2' in dir() else '', q3 if 'q3' in dir() else '', u, v_])):
        for n in names:
            if n:
                host.api(request('stop', n, reference, timeout_seconds=10, on_timeout='kill'))
                host.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + n], check=False)
    tags = B.call('podman_run', args=['images', '--format', '{{.Repository}}:{{.Tag}}'], check=False).get('stdout', '').split()
    for tag in tags:
        if tag.startswith('localhost/podmesh-restore:'):
            B.call('podman_run', args=['rmi', '--force', tag], check=False)
