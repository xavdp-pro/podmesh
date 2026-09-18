#!/usr/bin/env python3
"""The follow tick refuses without a mandate and does not invent a proof (docs/PUBLISHER-FOLLOW-LAB.md); and,
against a stubbed CLI that answers as a node would and records every call, it takes the branches of V3-1:
the route resumed when only the service address is missing, a running connector stopped at once on a
positive mismatch and on a reading that could not be made only three ticks in a row, a start without the
installed proof when that proof is for another epoch or cannot be read, a failed start backed off, and
nothing renewed, resumed or started after the mandate's not_after. Purely local:
no daemon, no host. Run: python3 -B tests/check-publisher-follow-script.py"""
import json, os, pathlib, subprocess, sys, tempfile, time

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'packaging', 'podmesh-publisher-follow')
checks = []

def run(env, expect):
    p = subprocess.run(['python3', '-B', SCRIPT], env={**os.environ, **env}, capture_output=True, text=True)
    assert p.returncode == expect, (p.returncode, p.stdout, p.stderr)
    return p

p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': '/no/such/mandate'}, 3)
assert 'no mandate' in p.stderr
checks.append('refuses without a mandate')

td = tempfile.mkdtemp(prefix='podmesh-follow-')
bad = pathlib.Path(td) / 'mandate'
bad.write_text('authorization_ref=ok\nresource=not-a-uuid\nproof=/tmp/x\n')
p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': str(bad)}, 3)
assert 'resource UUID' in p.stderr
checks.append('refuses a mandate that names no resource UUID')

UUID = '91eeb6bf-5489-405b-b77a-53105b0aff7a'
bad.write_text(f'authorization_ref=ok\nresource={UUID}\nproof=/tmp/x\nrenew=yes\n')
p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': str(bad)}, 3)
assert "renew must be 0 or 1" in p.stderr
checks.append('refuses a mandate whose renew is not 0 or 1')

# The bound on renewal: a mandate that renews for ever would make the holder's lease immortal,
# and lease expiry is what withdraws an active manager nobody can reach.
bad.write_text(f'authorization_ref=ok\nresource={UUID}\nproof=/tmp/x\nrenew=1\nrenew_below=900\n')
p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': str(bad)}, 3)
assert 'not_after' in p.stderr
checks.append('refuses a mandate that carries no not_after')

bad.write_text(f'authorization_ref=ok\nresource={UUID}\nproof=/tmp/x\nrenew=1\nnot_after=4102444800\n')
p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': str(bad)}, 3)
assert 'renew_below' in p.stderr
checks.append('refuses a renewing mandate that names no renewal window')

bad.write_text(f'authorization_ref=ok\nresource={UUID}\nproof=/tmp/x\nrenew=1\nnot_after=4102444800\nrenew_below=0\n')
p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': str(bad)}, 3)
assert 'renew_below must be' in p.stderr
checks.append('refuses a renewal window of zero')

# ---------------------------------------------------------------- the tick's branches, against a stubbed CLI
STUB = f"""#!{sys.executable}
import json, os, sys
op, path = sys.argv[1], sys.argv[2]
req = json.load(open(path))
with open(os.environ['STUB_LOG'], 'a') as f:
    f.write(json.dumps({{'op': op, 'request': req}}) + '\\n')
answers = json.load(open(os.environ['STUB_SCENARIO'])).get(op) or [{{'ok': False, 'error': 'unexpected operation ' + op}}]
n = sum(1 for line in open(os.environ['STUB_LOG']) if json.loads(line)['op'] == op) - 1
answer = answers[min(n, len(answers) - 1)]
print(json.dumps(answer))
sys.exit(0 if answer.get('ok') else 1)
"""
NOW = int(time.time())
EPOCH = 157


def status(eligible=True, unit='inactive', address=True, lease=True, epoch=EPOCH, mark=EPOCH, mark_read='present', origin=True,
           registered=True, expires_in=3000, transition=None, legacy=False):
    """A publisher_status answer as the node gives it; `legacy` drops the fields V3-1 added."""
    data = {'declared': {'hostname': 'lab.example'}, 'unit': {'state': unit}, 'publisher_eligible': eligible,
            'gates': {'lease': lease, 'policy': True, 'service_address': address, 'credential': True},
            'epoch': epoch if lease else None, 'lease': {'expires_at': NOW + expires_in, 'epoch': epoch},
            'active_manager_mark_epoch': mark if mark_read == 'present' else None, 'active_manager_mark_read': mark_read,
            'origin_ready_at_lease_epoch': origin, 'connector_registered': registered,
            'transition': transition, 'reasons': [] if eligible else ['a gate is closed']}
    if legacy:
        for k in ('gates', 'active_manager_mark_epoch', 'active_manager_mark_read', 'origin_ready_at_lease_epoch', 'connector_registered'):
            data.pop(k)
    return {'ok': True, 'data': data}


OK = {'ok': True, 'data': {}}
MISSING = status(eligible=False, address=False)


def tick(scenario, not_after=4102444800, proof=None, state=None, expect=0):
    """One tick against the stub: the operations it called, in order, with their requests. `state` is a
    directory kept across ticks for the tick's own state file; each tick gets a fresh one otherwise."""
    with tempfile.TemporaryDirectory(prefix='podmesh-follow-stub-') as d:
        d = pathlib.Path(d)
        cli = d / 'podmesh'
        cli.write_text(STUB)
        cli.chmod(0o700)
        (d / 'scenario.json').write_text(json.dumps(scenario))
        proof_path = d / 'proof.json'
        if proof is not None:
            proof_path.write_text(proof if isinstance(proof, str) else json.dumps(proof))
        (d / 'mandate').write_text(f'authorization_ref=ok\nresource={UUID}\nproof={proof_path}\nrenew=1\nnot_after={not_after}\nrenew_below=900\n')
        log = d / 'calls'
        log.touch()
        p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': str(d / 'mandate'), 'PODMESH_CLI': str(cli),
                 'PODMESH_PUBLISHER_FOLLOW_STATE': str(pathlib.Path(state or d) / 'tick-state.json'),
                 'STUB_LOG': str(log), 'STUB_SCENARIO': str(d / 'scenario.json')}, expect)
        calls = [json.loads(line) for line in log.read_text().splitlines()]
    return [c['op'] for c in calls], {c['op']: c['request'] for c in calls}, p


ops, req, _ = tick({'publisher_status': [MISSING, status()], 'network_route_resume': [OK], 'publisher_start': [OK]})
assert ops == ['publisher_status', 'network_route_resume', 'publisher_status', 'publisher_start'], ops
assert req['network_route_resume']['exclusive_resource'] == UUID and 'takeover_proof' not in req['publisher_start'], req
checks.append('only the service address missing: network_route_resume once, the status read again, then a start with no proof (the node resumes)')

proof = {'kind': 'podmesh-takeover-proof/lab-unsigned', 'new_epoch': EPOCH}
ops, req, _ = tick({'publisher_status': [MISSING, status()], 'network_route_resume': [OK], 'publisher_start': [OK]}, proof=proof)
assert ops[-1] == 'publisher_start' and req['publisher_start']['takeover_proof'] == proof, req
ops, req, p = tick({'publisher_status': [status()], 'publisher_start': [OK]}, proof=dict(proof, new_epoch=EPOCH - 1))
assert ops == ['publisher_status', 'publisher_start'] and 'takeover_proof' not in req['publisher_start'] and 'same-epoch resume' in p.stderr, (ops, p.stderr)
checks.append("the installed proof is passed when it is for the lease's epoch, and left out when it is for another")

with tempfile.TemporaryDirectory(prefix='podmesh-follow-state-') as kept:
    for n, warned in [(1, True), (2, False)]:
        ops, req, p = tick({'publisher_status': [status()], 'publisher_start': [OK]}, proof='{"new_epoch": 157', state=kept)
        assert ops == ['publisher_status', 'publisher_start'] and 'takeover_proof' not in req['publisher_start'], (n, ops)
        assert ('cannot be used' in p.stderr) is warned, (n, p.stderr)
    ops, req, _ = tick({'publisher_status': [status()], 'publisher_start': [OK]}, proof='[1, 2]')
    assert 'takeover_proof' not in req['publisher_start'], req
checks.append('an unreadable or non-object proof file: the tick starts without it and says so once per content, never crashes')

ops, _, p = tick({'publisher_status': [status(eligible=False, address=False, unit='active')],
                  'network_route_resume': [{'ok': False, 'error': 'network_route_resume refused (no_carrier_at_via): nothing runs at 10.86.1.10'}],
                  'publisher_stop': [OK]})
assert ops == ['publisher_status', 'network_route_resume', 'publisher_stop'] and 'no_carrier_at_via' in p.stderr, (ops, p.stderr)
checks.append('a refused route resume is reported, and the connector of a host that is not eligible is stopped all the same')

ops, _, _ = tick({'publisher_status': [status(eligible=False, address=False, lease=False, unit='active')], 'publisher_stop': [OK]})
assert ops == ['publisher_status', 'publisher_stop'], ops
ops, _, _ = tick({'publisher_status': [status(eligible=True, address=False)], 'publisher_start': [OK]})
assert ops == ['publisher_status', 'publisher_start'], ops
checks.append('no route resume when another gate is closed too (the lease), nor when the node says eligible')

ops, _, _ = tick({'publisher_status': [MISSING, MISSING], 'network_route_resume': [{'ok': True, 'data': {'already_effective': True}}]})
assert ops == ['publisher_status', 'network_route_resume', 'publisher_status'], ops
ops, _, _ = tick({'publisher_status': [status(eligible=False, address=False, unit='active'), status(unit='active', mark_read='absent')],
                  'network_route_resume': [OK], 'publisher_stop': [OK]})
assert ops == ['publisher_status', 'network_route_resume', 'publisher_status', 'publisher_stop'], ops
checks.append('a replica restarted between two ticks: the route resumed, then the connector stopped on the mark its entrypoint cleared')

for label, kwargs in [('mark one epoch behind', {'mark': EPOCH - 1}), ('no mark', {'mark_read': 'absent'}),
                      ('origin answered at another epoch or not ready', {'origin': False})]:
    ops, _, p = tick({'publisher_status': [status(unit='active', **kwargs)], 'publisher_stop': [OK]})
    assert ops == ['publisher_status', 'publisher_stop'] and 'stopping the connector' in p.stderr, (label, ops, p.stderr)
checks.append('a running connector is stopped at once on a positive mismatch: a mark at another epoch, no mark, an origin that answered otherwise')

for label, kwargs in [('mark unreadable', {'mark_read': 'unknown'}), ('origin not told', {'origin': None}),
                      ('journal unreadable', {'registered': None}), ('no registration in the current run', {'registered': False})]:
    with tempfile.TemporaryDirectory(prefix='podmesh-follow-state-') as kept:
        seen = []
        for n in range(3):
            ops, _, p = tick({'publisher_status': [status(unit='active', **kwargs)], 'publisher_stop': [OK]}, state=kept)
            seen.append(ops[1:])
        assert seen == [[], [], ['publisher_stop']], (label, seen)
        # A good reading in between starts the count again.
        tick({'publisher_status': [status(unit='active', **kwargs)]}, state=kept)
        tick({'publisher_status': [status(unit='active')]}, state=kept)
        ops, _, _ = tick({'publisher_status': [status(unit='active', **kwargs)]}, state=kept)
        assert ops == ['publisher_status'], (label, ops)
checks.append('a reading that could not be made stops a running connector only on the third tick in a row, and a good reading resets the count')

ops, _, _ = tick({'publisher_status': [status(unit='active')]})
assert ops == ['publisher_status'], ops
ops, _, _ = tick({'publisher_status': [status(unit='active', expires_in=100)], 'activation_renew': [OK]})
assert ops == ['publisher_status', 'activation_renew'], ops
ops, _, _ = tick({'publisher_status': [status(unit='active', legacy=True)]})
assert ops == ['publisher_status'], ops
checks.append('a consistent connector is left alone, renewed inside the window only; a node without the new fields is not second-guessed')

with tempfile.TemporaryDirectory(prefix='podmesh-follow-state-') as kept:
    refused = {'ok': False, 'error': 'the connector did not register with Cloudflare within 60 seconds'}
    ops, _, _ = tick({'publisher_status': [status()], 'publisher_start': [refused]}, state=kept, expect=1)
    assert ops == ['publisher_status', 'publisher_start'], ops
    ops, _, p = tick({'publisher_status': [status()], 'publisher_start': [OK]}, state=kept)
    assert ops == ['publisher_status'] and 'not before' in p.stderr, (ops, p.stderr)
    st = json.loads((pathlib.Path(kept) / 'tick-state.json').read_text())
    assert st['start_failures'] == 1 and 15 <= st['next_start_at'] - int(time.time()) <= 20, st
    # Doubling up to the ceiling.
    st.update(start_failures=9, next_start_at=0)
    (pathlib.Path(kept) / 'tick-state.json').write_text(json.dumps(st))
    tick({'publisher_status': [status()], 'publisher_start': [refused]}, state=kept, expect=1)
    st = json.loads((pathlib.Path(kept) / 'tick-state.json').read_text())
    assert st['start_failures'] == 10 and 295 <= st['next_start_at'] - int(time.time()) <= 300, st
    # Another epoch starts the count again, and a success clears it.
    ops, _, _ = tick({'publisher_status': [status(epoch=EPOCH + 1)], 'publisher_start': [OK]}, state=kept)
    assert ops == ['publisher_status', 'publisher_start'], ops
    assert 'start_failures' not in json.loads((pathlib.Path(kept) / 'tick-state.json').read_text())
checks.append('a failed start is not tried again before its backoff (20 s, doubling to 300 s); a new epoch or a success clears it')

PAST = NOW - 60
for label, scenario in [('only the address missing', {'publisher_status': [MISSING]}),
                        ('eligible, nothing running', {'publisher_status': [status()]}),
                        ('eligible, running, inside the renewal window', {'publisher_status': [status(unit='active', expires_in=100)]})]:
    ops, _, _ = tick(scenario, not_after=PAST, proof=proof)
    assert ops == ['publisher_status'], (label, ops)
ops, _, _ = tick({'publisher_status': [status(unit='active', mark=EPOCH - 1)], 'publisher_stop': [OK]}, not_after=PAST)
assert ops == ['publisher_status', 'publisher_stop'], ops
checks.append("after not_after: no route resumed, nothing renewed, nothing started, with or without an installed proof; a mismatch is still withdrawn")

print('PASS')
for c in checks:
    print('-', c)
