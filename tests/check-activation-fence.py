#!/usr/bin/env python3
"""Self-fencing: a universe this host is not entitled to run is stopped.

This one needs Podman, because the whole point is the effect on a running container rather
than on a row. It creates one disposable universe, proves the fence leaves it alone while
the lease is live, and proves it stops it once the entitlement is gone.
"""
import json, os, socket, subprocess, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')

def api(request):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(60); s.connect(endpoint)
        s.sendall(json.dumps(request).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())

def op(operation, **extra):
    return api(dict({'operation': operation, 'operation_id': str(uuid.uuid4()),
                     'authorization_ref': 'disposable-lab'}, **extra))

def running(name):
    out = subprocess.run(['podman', 'inspect', '--format', '{{.State.Running}}', name],
                         capture_output=True, text=True)
    return out.returncode == 0 and out.stdout.strip() == 'true'

images = json.loads(subprocess.check_output(['podman', 'images', '--format', 'json']))
image = next(i['Id'] for i in images if any('alpine' in (n or '') for n in (i.get('Names') or [])))
if not image.startswith('sha256:'):
    image = 'sha256:' + image

u = str(uuid.uuid4())
name = 'podmesh-' + u
try:
    # A container that handles its stop signal, so a polite fence is polite and an escalation
    # to SIGKILL means something went wrong rather than that the image ignores signals.
    created = op('create', universe_uuid=u, image=image,
                 command=['sh', '-c', "trap 'exit 0' TERM; sleep 600 & wait"])
    assert created['ok'], created

    assert op('activation_require', universe_uuid=u, lease_seconds=8, takeover_margin_seconds=5)['ok']
    assert op('activation_acquire', universe_uuid=u)['ok']
    started = op('start', universe_uuid=u, observe_seconds=1)
    assert started['ok'], started
    assert running(name), 'the universe did not start'

    # Fencing must take no universe: it acts on every one under a policy.
    scoped = op('activation_fence', universe_uuid=u, timeout_seconds=5)
    assert not scoped['ok'] and 'takes no universe_uuid' in json.dumps(scoped), scoped

    # With a live lease, the fence leaves it alone -- and says so, rather than silently
    # doing nothing.
    quiet = op('activation_fence', timeout_seconds=5)
    assert quiet['ok'], quiet
    left = {e['universe_uuid']: e for e in quiet['data']['left_running_or_absent']}
    assert u in left and left[u]['reason'] == 'lease is live', quiet
    assert not any(e['universe_uuid'] == u for e in quiet['data']['fenced']), quiet
    assert running(name), 'the fence stopped a universe whose lease was live'

    # Give up the entitlement and the same operation must stop it.
    assert op('activation_release', universe_uuid=u)['ok']
    fenced = op('activation_fence', timeout_seconds=10)
    assert fenced['ok'], fenced
    hit = {e['universe_uuid']: e for e in fenced['data']['fenced']}
    assert u in hit, f'the fence left an unentitled universe running: {fenced}'
    assert hit[u]['forced'] is False, f'a container that handles SIGTERM should not need killing: {hit[u]}'
    assert not running(name), 'the fence reported a stop that did not happen'

    # Fencing again is idempotent: nothing left to do, and it says which reason.
    twice = op('activation_fence', timeout_seconds=5)
    assert twice['ok'], twice
    again = {e['universe_uuid']: e for e in twice['data']['left_running_or_absent']}
    assert u in again and again[u]['reason'] == 'not running', twice

    # And the gate refuses to restart it, which is the other half of the same entitlement.
    refused = op('start', universe_uuid=u, observe_seconds=0)
    assert not refused['ok'] and 'requires an activation lease' in json.dumps(refused), refused

    print('PASS: self-fencing — a live lease is left alone, an unentitled universe is stopped '
          'without escalation, the fence is idempotent, and the gate still refuses a restart.')
finally:
    subprocess.run(['podman', 'rm', '-f', name], capture_output=True)
