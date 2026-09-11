#!/usr/bin/env python3
"""Kill the PodMesh service while a source checkpoint is being dumped, then retry the same operation.

Development test for a disposable lab host with an isolated PodMesh service that restarts on failure.
The checkpoint must survive the service kill (it runs in its own scope), and the retry must finalize
the existing checkpoint without capturing again. Universe mutations go through the API; the test
removes its own reserved fixture container directly at the end. No restore is run."""
import hashlib, json, os, socket, subprocess, threading, time, uuid

endpoint = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
# Holds about 512 MiB of process memory so that the CRIU dump lasts long enough to be interrupted.
HOG = ['awk', 'BEGIN{s="0123456789abcdefghijklmnopqrstuv"; while (length(s) < 536870912) s = s s; while (1) system("sleep 5")}']

def api(r, timeout=400):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(timeout); s.connect(endpoint)
        s.sendall(json.dumps(r).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())
def out(*args): return subprocess.run(['podman', *args], check=True, capture_output=True).stdout.decode().strip()
def inspect(name): return json.loads(out('container', 'inspect', name))[0]
def exists(name): return subprocess.run(['podman', 'container', 'exists', name]).returncode == 0
def state():
    containers = {c['Id']: (tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'))
                  for c in json.loads(out('ps', '--all', '--format', 'json'))}
    return containers, {i['Id'] for i in json.loads(out('images', '--all', '--format', 'json'))}, sorted(out('volume', 'ls', '--quiet').split())
def request(op, u, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), universe_uuid=u, authorization_ref='disposable-lab-migration-interrupt-test', **extra)
def ok(r):
    result = api(r); assert result['ok'], (r, result); return result['data']
def ready():
    for _ in range(300):
        try:
            if api({'operation': 'capabilities'}, 5)['ok']: return
        except (OSError, ValueError): pass
        time.sleep(.1)
    raise RuntimeError('Service did not return after the kill')
def processes(*needles):
    found = []
    for pid in filter(str.isdigit, os.listdir('/proc')):
        try: argv = open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')
        except OSError: continue
        if all(n.encode() in argv for n in needles): found.append(int(pid))
    return found
def scope_active(operation_id):
    return subprocess.run(['systemctl', 'is-active', '--quiet', f'podmesh-checkpoint-{operation_id}.scope']).returncode == 0
def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''): h.update(chunk)
    return h.hexdigest()

alpine = next(i['Id'] for i in json.loads(out('images', '--format', 'json')) if 'docker.io/library/alpine:3.22' in (i.get('Names') or []))
host = api({'operation': 'identity'})['data']['host_uuid']
baseline = state()
u = str(uuid.uuid4()); name = 'podmesh-' + u
try:
    ok(request('create', u, image='sha256:' + alpine, command=HOG))
    ok(request('start', u))
    c = inspect(name)
    cgroup = '/sys/fs/cgroup' + c['State']['CgroupPath']
    deadline = time.time() + 60
    while int(open(cgroup + '/memory.current').read()) < 400 * 1024 * 1024:
        assert time.time() < deadline, 'memory fixture did not grow'
        time.sleep(.2)
    memory = int(open(cgroup + '/memory.current').read())
    started_at = inspect(name)['State']['StartedAt']
    checkpoint = request('migration_checkpoint', u, container_id=c['Id'], image='sha256:' + alpine,
                         source_host_uuid=host, destination_host_uuid=str(uuid.uuid4()))
    first = []
    def send():
        try: first.append(api(checkpoint))
        except Exception as e: first.append({'interrupted': type(e).__name__})
    thread = threading.Thread(target=send); thread.start()
    deadline = time.time() + 60
    app = c['State']['Pid']
    def tracer():
        try:
            return int(next(l.split()[1] for l in open(f'/proc/{app}/status') if l.startswith('TracerPid:')))
        except (OSError, StopIteration):
            return 0
    # Kill as soon as a tracer has seized the application: CRIU is dumping it. (Matching the CRIU
    # binary alone is not enough: preflight also runs `criu check` inside the service.)
    while not tracer():
        assert time.time() < deadline and thread.is_alive(), ('no tracer seized the application', first)
        time.sleep(.002)
    subprocess.run(['systemctl', 'kill', '--signal=SIGKILL', unit], check=True)
    tracer_pid = tracer()
    tracer_argv = open(f'/proc/{tracer_pid}/cmdline', 'rb').read().split(b'\0')[0].decode() if tracer_pid else None
    at_kill = {'application_pid': app, 'tracer_pid_after_service_kill': tracer_pid, 'tracer_executable': tracer_argv,
               'checkpoint_command_running': bool(processes('container', 'checkpoint', c['Id'])),
               'scope_active': scope_active(checkpoint['operation_id']), 'container_status': inspect(name)['State']['Status'],
               'checkpointed_yet': inspect(name)['State'].get('Checkpointed')}
    # Proof that the kill landed mid-dump: CRIU still held the application after the service was
    # killed. Podman may already report the checkpoint by the time it is inspected (about 0.5 s dump).
    assert at_kill['scope_active'] and at_kill['tracer_pid_after_service_kill'] and 'dump' in (at_kill['tracer_executable'] or ''), at_kill
    thread.join(30)
    ready()
    deadline = time.time() + 120
    while scope_active(checkpoint['operation_id']):
        assert time.time() < deadline, 'checkpoint scope did not finish'
        time.sleep(.2)
    before_retry = inspect(name)['State']
    reservation = api({'operation': 'migration_status', 'universe_uuid': u})['data']['reservation']
    after_kill = {'first_response': first[0], 'container_status': before_retry['Status'], 'checkpointed': before_retry.get('Checkpointed'),
                  'checkpointed_at': before_retry.get('CheckpointedAt'), 'reservation_state': reservation and reservation['state'],
                  'started_at_unchanged': before_retry['StartedAt'] == started_at}
    assert 'interrupted' in first[0], first
    assert reservation and reservation['state'] == 'checkpointing', reservation
    # The kill hit the service while the checkpoint command ran in its own scope: the application
    # must not have been destroyed, and the checkpoint must have completed.
    assert before_retry.get('Checkpointed') is True and before_retry['Status'] == 'exited' and before_retry['StartedAt'] == started_at, before_retry
    directory = os.path.join(state_dir, 'migrations', checkpoint['operation_id'])
    archive = os.path.join(directory, 'checkpoint.tar.zst')
    archive_mtime = os.stat(archive).st_mtime_ns
    retry = ok(checkpoint)
    assert 'replayed' not in retry and retry['finalized_after_interruption'] is True, retry
    assert inspect(name)['State']['CheckpointedAt'] == before_retry['CheckpointedAt'] and os.stat(archive).st_mtime_ns == archive_mtime
    assert sha256(archive) == retry['archive']['sha256']
    replay = ok(checkpoint)
    assert replay['historical'] and replay['current_artifacts']['archive_sha256_matches'] is True
    final = api({'operation': 'migration_status', 'universe_uuid': u})['data']
    assert final['reservation']['state'] == 'checkpointed'
    assert api(request('start', u))['ok'] is False
    files = sorted(os.listdir(directory))
finally:
    if exists(name): subprocess.run(['podman', 'rm', '--force', '--time', '0', name], capture_output=True)
assert state() == baseline, 'Podman containers, images or volumes differ from the baseline'

print(json.dumps({'status': 'PASS', 'universe_uuid': u, 'memory_current_bytes': memory, 'at_kill': at_kill, 'after_kill': after_kill,
                  'retry_result': retry, 'artifact_directory': directory, 'artifact_files': files,
                  'checks': ['service killed while the checkpoint command ran', 'application not destroyed; checkpoint completed in its own scope',
                             'reservation persisted in state checkpointing before the kill', 'retry finalized without recapture (same CheckpointedAt and archive)',
                             'archive hash verified independently', 'historical replay re-hashes the archive', 'start refused on the reserved universe',
                             'test-owned fixture removed; pre-existing containers, images and volumes unchanged'],
                  'reserved_universe_left_in_journal': u}))
