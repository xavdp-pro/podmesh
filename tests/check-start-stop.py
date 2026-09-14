#!/usr/bin/env python3
"""Explicit start and stop through the PodMesh API, verified independently with Podman.

Run as root on a disposable lab host. Every mutation of a PodMesh universe goes through the
API. Direct Podman writes are limited to named fixtures: an unrelated running container, and
unmanaged, forged, replaced or paused containers. Universes touched by a fixture write are
excluded from the Podman event correlation and listed in the output."""
import json, os, socket, subprocess, tempfile, threading, time, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
TRAP = ['sh', '-c', 'echo start >> /data.log; trap "echo term >> /data.log; exit 0" TERM; while true; do sleep 1; done']
checks, windows, universes, fixtures, tampered = [], [], [], [], {}

def raw_api(r, timeout=400):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(timeout); s.connect(endpoint)
        s.sendall(json.dumps(r).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())
def api(r):
    """Mutating call. Its time window is kept for the Podman event correlation."""
    begin = time.time_ns()
    try:
        return raw_api(r)
    finally:
        windows.append((begin, time.time_ns()))
def podman(*args, check=True):
    p = subprocess.run(['podman', *args], capture_output=True)
    if check and p.returncode: raise RuntimeError(f'podman {args}: {p.stderr.decode()}')
    return p
def out(*args): return podman(*args).stdout.decode().strip()
def inspect(name): return json.loads(out('container', 'inspect', name))[0]
def exists(name): return podman('container', 'exists', name, check=False).returncode == 0
def images(): return json.loads(out('images', '--all', '--format', 'json'))
def state():
    """Independent view of every container, image and volume on the host."""
    containers = {c['Id']: (tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'), c.get('ExitCode'), c.get('ImageID'))
                  for c in json.loads(out('ps', '--all', '--format', 'json'))}
    return containers, {i['Id']: tuple(sorted(i.get('Names') or [])) for i in images()}, sorted(out('volume', 'ls', '--quiet').split())
def request(op, u, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), universe_uuid=u, authorization_ref='disposable-lab-start-stop-test', **extra)
def ok(r, check=None):
    result = api(r); assert result['ok'], (r, result)
    if check: checks.append(check)
    return result['data']
def refused(r, check, expected):
    before = state()
    result = api(r)
    assert result['ok'] is False and expected in result['error'], (check, result)
    assert state() == before, (check, 'refused request changed Podman state')
    checks.append('refused without effect: ' + check)
    return result
def ready():
    for _ in range(300):
        try:
            if raw_api({'operation': 'capabilities'}, 5)['ok']: return
        except (OSError, ValueError): pass
        time.sleep(.1)
    raise RuntimeError('Service did not become ready')
def get(name, path):
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, 'f'); out('cp', f'{name}:{path}', f); return open(f, 'rb').read()
def universe(command):
    u = str(uuid.uuid4()); universes.append(u)
    r = request('create', u, image='sha256:' + alpine, network_profile='isolated', command=command); ok(r)
    return u, 'podmesh-' + u, r['operation_id']
def fixture(name, *args):
    fixtures.append(name)
    out('create', '--pull=never', '--network=none', '--name', name, *args)
def process_running(*needles):
    for pid in filter(str.isdigit, os.listdir('/proc')):
        try: argv = open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')
        except OSError: continue
        if all(n.encode() in argv for n in needles): return True
    return False
def background(r):
    box = []
    def send():
        try: box.append(api(r))
        except Exception as e: box.append({'interrupted': type(e).__name__})
    t = threading.Thread(target=send); t.start()
    return t, box
def kill_service():
    subprocess.run(['systemctl', 'kill', '--signal=SIGKILL', unit], check=True)
def wait_for(predicate, what, thread):
    deadline = time.time() + 15
    while not predicate():
        assert time.time() < deadline and thread.is_alive(), what
        time.sleep(.05)

alpine = next(i['Id'] for i in images() if any('alpine' in n for n in i.get('Names') or []))
baseline = state()
since = int(time.time()) - 1
unrelated = 'pmfixture-unrelated-' + str(uuid.uuid4())
try:
    out('run', '-d', '--pull=never', '--network=none', '--name', unrelated, alpine, 'sleep', '3600')
    unrelated_view = (inspect(unrelated)['Id'], inspect(unrelated)['State']['StartedAt'])
    unrelated_ready_ns = time.time_ns()

    # Validation and ownership refusals, before any effect.
    l, L, l_op = universe(TRAP)
    anonymous = request('start', l); del anonymous['authorization_ref']
    refused(anonymous, 'start without authorization reference', 'Missing authorization_ref')
    refused(request('start', 'not-a-uuid'), 'invalid universe UUID', 'Invalid universe UUID')
    refused(request('start', str(uuid.uuid4())), 'start of an absent universe', 'not found')
    refused(request('start', l, observe_seconds=31), 'observation window above its bound', 'observe_seconds')
    refused(request('stop', l, timeout_seconds=10), 'stop without declared timeout behaviour', 'on_timeout')
    refused(request('stop', l, on_timeout='kill'), 'stop without graceful timeout', 'timeout_seconds')
    refused(request('stop', l, timeout_seconds=301, on_timeout='kill'), 'graceful timeout above its bound', 'timeout_seconds')
    refused(request('stop', l, timeout_seconds='10', on_timeout='kill'), 'non-integer graceful timeout', 'timeout_seconds')
    refused(request('stop', l, timeout_seconds=10, on_timeout='sigkill'), 'unknown timeout behaviour', 'on_timeout')
    x = str(uuid.uuid4())
    fixture('podmesh-' + x, '--label', f'io.podmesh.universe={x}', '--label', 'io.podmesh.creation-operation=' + str(uuid.uuid4()), alpine, 'sleep', '3600')
    refused(request('start', x), 'start of a labelled container unknown to the journal', 'not recorded')
    out('start', 'podmesh-' + x)
    refused(request('stop', x, timeout_seconds=0, on_timeout='kill'), 'stop of a labelled running container unknown to the journal', 'not recorded')
    y = str(uuid.uuid4())
    fixture('podmesh-' + y, '--label', f'io.podmesh.universe={y}', '--label', f'io.podmesh.creation-operation={l_op}', alpine, 'sleep', '3600')
    refused(request('start', y), 'start of a container borrowing another universe creation', 'does not match')
    o = str(uuid.uuid4())
    fixture('podmesh-' + o, alpine, 'sleep', '3600'); out('start', 'podmesh-' + o)
    refused(request('start', o), 'start of an unmanaged container', 'not managed')
    refused(request('stop', o, timeout_seconds=0, on_timeout='kill'), 'stop of an unmanaged running container', 'not managed')
    r, R, r_op = universe(['sleep', '3600'])
    out('rm', R); tampered[r] = 'container removed and replaced by a forged container with the same name and labels'
    fixture(R, '--label', f'io.podmesh.universe={r}', '--label', f'io.podmesh.creation-operation={r_op}', alpine, 'sleep', '3600')
    refused(request('start', r), 'start of a replaced universe container', 'does not match')
    out('start', R)
    refused(request('stop', r, timeout_seconds=0, on_timeout='kill'), 'stop of a replaced running universe container', 'does not match')

    # Long-running application: start, no-op start, graceful stop, historical replay, data.
    start_l = request('start', l)
    s1 = ok(start_l)
    c = inspect(L)
    assert s1['action'] == 'started' and s1['running'] is True and s1['observed_state'] == 'running' and s1['exit_code'] is None, s1
    assert c['State']['Status'] == 'running' and c['State']['StartedAt'] == s1['started_at'] and c['Id'] == s1['container_id']
    checks.append('start reports running only as observed; Podman agrees on state, start time and container ID')
    conmon_cgroup = open(f"/proc/{c['State']['ConmonPid']}/cgroup").read().strip()
    assert unit not in conmon_cgroup, ('conmon must not live in the PodMesh service cgroup', conmon_cgroup)
    checks.append('conmon of a started universe runs outside the PodMesh service cgroup: ' + conmon_cgroup)
    again = ok(request('start', l))
    assert again['action'] == 'none_already_running' and inspect(L)['State']['StartedAt'] == s1['started_at'], again
    checks.append('start of a running universe is a verified no-op without restart')
    refused(dict(start_l, observe_seconds=5), 'operation ID reused for a different request', 'different request')
    graceful = ok(request('stop', l, timeout_seconds=10, on_timeout='kill'))
    c = inspect(L)
    assert graceful['action'] == 'stopped' and graceful['forced'] is False and graceful['exit_code'] == 0 and graceful['elapsed_ms'] < 10000, graceful
    assert c['State']['Status'] in ('exited', 'stopped') and c['State']['ExitCode'] == 0 and c['State']['StartedAt'] == s1['started_at']
    checks.append('stop within the declared graceful timeout reports forced=false and exit code 0')
    history = ok(start_l)
    assert history['replayed'] and history['historical'] and history['original_result'] == s1 and history['original_result']['running'] is True, history
    assert history['current']['running'] is False and history['current']['state'] == inspect(L)['State']['Status'], history
    assert history['current_matches_recorded_container'] is True
    checks.append('retried start returns a result labelled historical, with a fresh observation that contradicts it')
    s2 = ok(request('start', l))
    assert s2['action'] == 'started' and s2['running'] is True
    assert get(L, '/data.log').split() == [b'start', b'term', b'start']
    checks.append('container data persists across stop and start')

    # Service restart while the universe runs.
    identity = raw_api({'operation': 'identity'})['data']['host_uuid']
    subprocess.run(['systemctl', 'restart', unit], check=True)
    ready()
    c = inspect(L)
    assert c['State']['Status'] == 'running' and c['State']['StartedAt'] == s2['started_at'], 'service restart changed a running universe'
    assert raw_api({'operation': 'identity'})['data']['host_uuid'] == identity
    after_restart = ok(start_l)
    assert after_restart['historical'] and after_restart['current']['running'] is True
    polite = ok(request('stop', l, timeout_seconds=10, on_timeout='leave_running'))
    assert polite['action'] == 'stopped' and polite['forced'] is False and polite['stop_signal'] == 'SIGTERM' and inspect(L)['State']['ExitCode'] == 0, polite
    assert get(L, '/data.log').split() == [b'start', b'term', b'start', b'term']
    checks.append('running universe, identity, journal and ownership survive a service restart; stop signal only with leave_running')

    # An application that ignores SIGTERM: declared timeout behaviour.
    k, K, _ = universe(['sleep', '3600'])
    ok(request('start', k))
    k_started = inspect(K)['State']['StartedAt']
    time.sleep(1.1)
    left = request('stop', k, timeout_seconds=2, on_timeout='leave_running')
    failed = api(left)
    assert failed['ok'] is False and 'left running' in failed['error'] and failed['details']['observed']['running'] is True, failed
    c = inspect(K)
    assert c['State']['Status'] == 'running' and c['State']['StartedAt'] == k_started
    checks.append('on_timeout=leave_running: timeout reported as a failure, container still running, never killed')
    retried = api(left)
    assert retried['ok'] is False and 'data' not in retried and 'left running' in retried['error'] and inspect(K)['State']['StartedAt'] == k_started, retried
    checks.append('a failed operation ID is re-evaluated on retry, not replayed as success')
    kill = request('stop', k, timeout_seconds=1, on_timeout='kill')
    forced = ok(kill)
    c = inspect(K)
    assert forced['forced'] is True and forced['exit_code'] == 137 and c['State']['ExitCode'] == 137 and c['State']['Status'] in ('exited', 'stopped'), forced
    checks.append('on_timeout=kill: escalation after the declared timeout reported as forced=true, exit code 137')
    forced_history = ok(kill)
    assert forced_history['historical'] and forced_history['original_result']['forced'] is True and forced_history['current']['running'] is False

    # Short-lived and failing applications.
    s, S, _ = universe(['sh', '-c', 'echo short; exit 3'])
    short = ok(request('start', s, observe_seconds=3))
    c = inspect(S)
    assert short['action'] == 'started' and short['running'] is False and short['observed_state'] == 'exited' and short['exit_code'] == 3, short
    assert c['State']['Status'] == 'exited' and c['State']['ExitCode'] == 3
    checks.append('short-lived application reported as not running, with its exit code; no false running claim')
    nothing = ok(request('stop', s, timeout_seconds=5, on_timeout='kill'))
    assert nothing['action'] == 'none_already_stopped' and nothing['forced'] is False and nothing['stop_signal'] is None
    m, M, _ = universe(['/nonexistent'])
    bad = api(request('start', m))
    assert bad['ok'] is False and bad['error'].startswith('Start failed') and bad['details']['observed']['state'] == inspect(M)['State']['Status'] == 'created', bad
    checks.append('start failure reported with the observed created state and runtime error')

    # Paused state (reached only by a direct Podman fixture write) is outside both contracts.
    p, P, _ = universe(['sleep', '3600'])
    ok(request('start', p))
    out('pause', P); tampered[p] = 'paused and unpaused directly with Podman'
    refused(request('stop', p, timeout_seconds=1, on_timeout='kill'), 'stop of a paused universe', 'outside the stop contract')
    refused(request('start', p), 'start of a paused universe', 'outside the start contract')
    out('unpause', P)

    # Service killed during a start observation: the retry must not run the application twice.
    # The application runs briefly so that the service is still observing when it is killed;
    # it has exited before the retry, which a naive retry would start again.
    q, Q, _ = universe(['sh', '-c', 'echo run >> /runs.log; sleep 4'])
    first = request('start', q, observe_seconds=20)
    thread, box = background(first)
    wait_for(lambda: inspect(Q)['State']['Status'] == 'running', ('start not observed', box), thread)
    time.sleep(1)
    assert thread.is_alive(), ('start finished before the kill', box)
    kill_service(); thread.join(30); ready()
    assert 'interrupted' in box[0], box
    deadline = time.time() + 15
    while inspect(Q)['State']['Status'] != 'exited':
        assert time.time() < deadline; time.sleep(.2)
    q_started = inspect(Q)['State']['StartedAt']
    resumed = ok(first)
    assert 'replayed' not in resumed and resumed['action'] == 'none_start_observed_since_first_attempt', resumed
    assert inspect(Q)['State']['StartedAt'] == q_started and get(Q, '/runs.log') == b'run\n'
    checks.append('retry after a service kill during start does not start the application again')

    # Service killed during a stop wait. Podman then records the container as 'stopping' while the
    # application is still alive; PodMesh must say so, and a retry of the same operation completes it.
    i, I, _ = universe(['sleep', '3600'])
    def interrupt_stop():
        ok(request('start', i))
        started = inspect(I)['State']['StartedAt']
        time.sleep(1.1)
        stop = request('stop', i, timeout_seconds=4, on_timeout='kill')
        thread, box = background(stop)
        wait_for(lambda: process_running('stop', I), ('stop not observed', box), thread)
        time.sleep(1)
        assert thread.is_alive(), ('stop finished before the kill', box)
        kill_service(); thread.join(30); ready()
        c = inspect(I)
        assert c['State']['Status'] in ('stopping', 'running') and c['State']['StartedAt'] == started, c['State']
        assert c['State']['Pid'] > 0 and os.path.exists(f"/proc/{c['State']['Pid']}"), 'application must still be alive'
        return stop, c['State']['Status']
    interrupted, podman_state = interrupt_stop()
    checks.append(f'service kill during a stop wait: no escalation, application alive, Podman state {podman_state}')
    honest = api(request('start', i))
    assert honest['ok'] is False and honest['details']['observed']['running'] is True and honest['details']['observed']['state'] == podman_state, honest
    checks.append('PodMesh reports the interrupted stop as a still-running application, not as stopped')
    completed = ok(interrupted)
    assert 'replayed' not in completed and completed['action'] == 'stopped' and completed['forced'] is True and inspect(I)['State']['ExitCode'] == 137, completed
    checks.append('retry of the interrupted stop operation completes it with the declared escalation')
    assert ok(interrupted)['historical']
    # A second interrupted stop must not be retried against a newer run.
    interrupted, _ = interrupt_stop()
    ok(request('stop', i, timeout_seconds=1, on_timeout='kill'))
    time.sleep(1.1)
    ok(request('start', i))
    newer = inspect(I)['State']['StartedAt']
    refused(interrupted, 'retry of an interrupted stop after the universe was started again', 'newer run')
    assert inspect(I)['State']['StartedAt'] == newer

    # Cleanup of universes through the API only.
    for u in (l, k, s, m, p, q, i):
        if inspect('podmesh-' + u)['State']['Status'] == 'running':
            ok(request('stop', u, timeout_seconds=1, on_timeout='kill'))
        ok(request('delete', u))
    refused(request('delete', r), 'delete of a replaced universe container', 'does not match')
    for name in fixtures: podman('rm', '--force', '--time', '0', name)
    fixtures.clear()
    ok(request('delete', r))
    c = inspect(unrelated)
    assert (c['Id'], c['State']['StartedAt']) == unrelated_view and c['State']['Status'] == 'running'
    checks.append('unrelated running container untouched')

    # Every Podman event on API-managed universes happened inside a PodMesh API request.
    until = int(time.time()) + 1
    events = [json.loads(e) for e in out('events', '--since', str(since), '--until', str(until), '--stream=false', '--format', 'json').splitlines() if e.strip()]
    managed = {'podmesh-' + u for u in universes if u not in tampered}
    roles = {'podmesh-' + u: role for u, role in ((l, 'L trap'), (k, 'K sleep'), (s, 'S short-lived'), (m, 'M missing executable'),
                                                  (p, 'P paused fixture'), (q, 'Q brief application'), (i, 'I sleep'), (r, 'R replaced'))}
    # Applications that exit by themselves may die outside a request; every other event, including
    # the death of applications that never exit on their own, must come from a PodMesh request.
    self_exiting = {'podmesh-' + s, 'podmesh-' + q}
    correlated, natural_exits, outside, statuses = 0, [], [], {}
    for e in events:
        if e.get('Type') != 'container' or e.get('Name') not in managed or e.get('Status') == 'cleanup': continue
        statuses[e['Status']] = statuses.get(e['Status'], 0) + 1
        if any(b <= e['timeNano'] <= f for b, f in windows): correlated += 1
        elif e['Status'] == 'died' and e['Name'] in self_exiting: natural_exits.append(roles[e['Name']])
        else: outside.append((roles.get(e['Name']), e))
    assert correlated and not outside, outside
    checks.append('every Podman container event on API-managed universes falls inside a PodMesh API request window, '
                  'except post-exit cleanup and natural exits of self-exiting applications: ' + ', '.join(natural_exits or ['none']))
    stray = [e for e in events if e.get('Name') == unrelated and e['timeNano'] > unrelated_ready_ns]
    assert not stray, stray
    podman('rm', '--force', '--time', '0', unrelated)
    assert state() == baseline, 'Podman containers, images or volumes differ from the baseline'
    checks.append('all pre-existing containers, images and volumes unchanged; no leftovers')
finally:
    for u in universes:
        if exists('podmesh-' + u): podman('rm', '--force', '--time', '0', 'podmesh-' + u, check=False)
    for name in fixtures + [unrelated]:
        if exists(name): podman('rm', '--force', '--time', '0', name, check=False)

print(json.dumps({'status': 'PASS', 'version': raw_api({'operation': 'capabilities'})['data']['version'], 'podman': out('--version'), 'unit': unit,
                  'checks': checks, 'api_windows': len(windows), 'events_correlated': correlated, 'event_statuses': statuses,
                  'excluded_from_event_correlation': tampered,
                  'results': {'start_running': s1, 'stop_graceful_kill_mode': graceful, 'stop_leave_running_timeout': failed, 'stop_forced': forced,
                              'start_short_lived': short, 'start_failure': bad, 'historical_replay': history, 'interrupted_start_retry': resumed}}))
