#!/usr/bin/env python3
"""Two-host laboratory helper for the destination-side migration suites.

Runs in two roles from the same file.

* **Node** (as root on a lab host, through SSH): executes one bounded function against the local
  PodMesh service, Podman or /proc and prints one JSON line. Every API call is timed with the host's
  own clock, so Podman event correlation never compares clocks across machines.
* **Controller** (imported by a suite on the workstation): drives both hosts through SSH, carries
  documents between an outbox and an inbox as the transport controller, and keeps the API windows,
  refusal snapshots and checks of each host.

Direct Podman writes are limited to uniquely named disposable fixtures owned by the suite; every
product mutation goes through the PodMesh API. The controller never holds authority: it moves bytes.
"""
import base64, hashlib, json, os, shutil, socket, sqlite3, subprocess, sys, threading, time, uuid

MIGRATION_TABLES = ('migration_reservations', 'migration_authorizations', 'migration_restore_claims', 'migration_reservation_history')


# ---------------------------------------------------------------- node side

def _endpoint():
    return os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
def _state():
    return os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
def _unit():
    return os.environ.get('PODMESH_UNIT', 'podmesh.service')
def _podman(*args, check=True):
    p = subprocess.run(['podman', *args], capture_output=True)
    if check and p.returncode:
        raise RuntimeError(f'podman {args}: {p.stderr.decode()}')
    return p
def _out(*args):
    return _podman(*args).stdout.decode().strip()
def _sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()
def _raw_api(request, timeout=600):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(timeout)
        s.connect(_endpoint())
        s.sendall(json.dumps(request).encode() + b'\n')
        return json.loads(s.makefile('rb').readline())


def n_api(request, timeout=600):
    """One API request, with the window measured by this host's clock."""
    begin = time.time_ns()
    try:
        response = _raw_api(request, timeout)
    except Exception as e:
        return {'response': {'ok': False, 'error': f'transport: {type(e).__name__}: {e}', 'interrupted': True},
                'begin_ns': begin, 'end_ns': time.time_ns()}
    return {'response': response, 'begin_ns': begin, 'end_ns': time.time_ns()}
def n_ready(seconds=60):
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            if _raw_api({'operation': 'capabilities'}, 5)['ok']:
                return {'ready': True}
        except (OSError, ValueError):
            pass
        time.sleep(.1)
    raise RuntimeError('Service did not become ready')
def n_time():
    return {'time': time.time(), 'time_ns': time.time_ns()}
def n_podman_state():
    """Independent view of every container, image and volume on this host."""
    containers = {c['Id']: [tuple(c.get('Names') or []), c.get('State'), c.get('StartedAt'), c.get('ExitedAt'), c.get('ExitCode'), c.get('ImageID')]
                  for c in json.loads(_out('ps', '--all', '--format', 'json'))}
    images = {i['Id']: sorted(i.get('Names') or []) for i in json.loads(_out('images', '--all', '--format', 'json'))}
    return {'containers': containers, 'images': images, 'volumes': sorted(_out('volume', 'ls', '--quiet').split())}
def n_journal():
    """The migration tables, which a refused request must leave untouched."""
    db = sqlite3.connect(f'file:{_state()}/state.sqlite?mode=ro', uri=True)
    rows = {}
    for table in MIGRATION_TABLES:
        try:
            cursor = db.execute(f'SELECT * FROM {table}')
            names = [d[0] for d in cursor.description]
            rows[table] = [dict(zip(names, r)) for r in cursor.fetchall()]
        except sqlite3.OperationalError:
            # A table the service has not created yet holds no rows either: the first request of a fresh
            # journal creates them, which is not an effect of the request under test.
            rows[table] = []
    db.close()
    return rows
def n_boxes():
    """Every delivered document, by hash: the transport controller's view of both directories."""
    listing = {}
    for box in ('inbox', 'outbox'):
        entries = {}
        root = os.path.join(_state(), box)
        for authorization in sorted(os.listdir(root)) if os.path.isdir(root) else []:
            directory = os.path.join(root, authorization)
            if not os.path.isdir(directory):
                continue
            entries[authorization] = {f: {'sha256': _sha256(os.path.join(directory, f)), 'bytes': os.path.getsize(os.path.join(directory, f)),
                                          'mode': oct(os.stat(os.path.join(directory, f)).st_mode & 0o777)}
                                      for f in sorted(os.listdir(directory)) if os.path.isfile(os.path.join(directory, f))}
            entries[authorization]['_mode'] = oct(os.stat(directory).st_mode & 0o777)
        listing[box] = entries
    return listing
def n_snapshot():
    return {'podman': n_podman_state(), 'journal': n_journal(), 'boxes': n_boxes()}
def n_inspect(name):
    if _podman('container', 'exists', name, check=False).returncode:
        return {'container': None}
    return {'container': json.loads(_out('container', 'inspect', name))[0]}
def n_labelled(uuid_value):
    return {'containers': [{'id': c['Id'], 'names': c.get('Names'), 'state': c.get('State')}
                           for c in json.loads(_out('ps', '--all', '--format', 'json'))
                           if (c.get('Labels') or {}).get('io.podmesh.universe') == uuid_value]}
def n_image_id(reference):
    return {'image': next((i['Id'] for i in json.loads(_out('images', '--all', '--format', 'json')) if reference in (i.get('Names') or [])), None)}
def n_podman_run(args, check=True):
    p = _podman(*args, check=check)
    return {'exit': p.returncode, 'stdout': p.stdout.decode().strip(), 'stderr': p.stderr.decode().strip()}
def n_events(since, until):
    text = _out('events', '--since', str(since), '--until', str(until), '--stream=false', '--format', 'json')
    return {'events': [json.loads(line) for line in text.splitlines() if line.strip()]}
def n_counter(uuid_value, seconds=3.0, interval=0.25):
    """Memory-continuity observation from outside the universe: the application's own /tmp/state,
    read through /proc/<pid>/root as root. No podman exec, no write to the universe."""
    name = 'podmesh-' + uuid_value
    if _podman('container', 'exists', name, check=False).returncode:
        return {'samples': [], 'pid': None, 'error': 'container absent'}
    pid = json.loads(_out('container', 'inspect', name))[0]['State']['Pid']
    samples, deadline = [], time.time() + float(seconds)
    while time.time() < deadline:
        try:
            samples.append([round(time.time(), 3), open(f'/proc/{pid}/root/tmp/state').read().strip()])
        except OSError as e:
            samples.append([round(time.time(), 3), f'unreadable: {e}'])
        time.sleep(float(interval))
    return {'samples': samples, 'pid': pid}
def n_memory(uuid_value):
    c = json.loads(_out('container', 'inspect', 'podmesh-' + uuid_value))[0]
    path = '/sys/fs/cgroup' + c['State']['CgroupPath'] + '/memory.current'
    return {'memory_current_bytes': int(open(path).read().strip()), 'pid': c['State']['Pid']}
def n_scope(unit):
    state = subprocess.run(['systemctl', 'show', '--property=ActiveState', '--value', unit], capture_output=True, text=True).stdout.strip()
    return {'unit': unit, 'active_state': state}
def n_conmon(name):
    """Where conmon of a running universe lives, and whether its scope exists."""
    if _podman('container', 'exists', name, check=False).returncode:
        return {'conmon_cgroup': None, 'conmon_scope_exists': None, 'container': None}
    c = json.loads(_out('container', 'inspect', name))[0]
    pid, cid = c['State'].get('ConmonPid'), c['Id']
    cgroup = None
    if pid:
        try:
            cgroup = open(f'/proc/{pid}/cgroup').read().strip()
        except OSError:
            cgroup = None
    scope = f'/sys/fs/cgroup/machine.slice/libpod-conmon-{cid}.scope'
    return {'conmon_cgroup': cgroup, 'conmon_scope_exists': os.path.exists(scope), 'conmon_scope': scope, 'container': cid}
def n_path(path):
    if not os.path.exists(path):
        return {'exists': False}
    return {'exists': True, 'is_dir': os.path.isdir(path), 'mode': oct(os.stat(path).st_mode & 0o777),
            'bytes': os.path.getsize(path) if os.path.isfile(path) else None,
            'sha256': _sha256(path) if os.path.isfile(path) else None,
            'entries': sorted(os.listdir(path)) if os.path.isdir(path) else None}
def n_space(path):
    usage = shutil.disk_usage(path)
    return {'path': path, 'total': usage.total, 'used': usage.used, 'free': usage.free}
def n_read(path, limit=200000):
    with open(path, 'rb') as f:
        data = f.read(limit)
    return {'text': data.decode(errors='replace'), 'sha256': _sha256(path), 'bytes': os.path.getsize(path)}
def n_write_document(box, authorization, name, text):
    """Transport-controller write into a delivery directory (used to deliver, tamper or forge)."""
    directory = os.path.join(_state(), box, authorization)
    os.makedirs(directory, mode=0o700, exist_ok=True)
    path = os.path.join(directory, name)
    with open(path, 'w') as f:
        f.write(text)
    os.chmod(path, 0o600)
    return {'path': path, 'sha256': _sha256(path), 'bytes': os.path.getsize(path)}
def n_corrupt(path, mode='append', data='tampered'):
    size = os.path.getsize(path)
    if mode == 'append':
        with open(path, 'ab') as f:
            f.write(data.encode())
    elif mode == 'truncate':
        os.truncate(path, size - len(data.encode()))
    else:
        raise ValueError(mode)
    return {'path': path, 'previous_bytes': size, 'bytes': os.path.getsize(path), 'sha256': _sha256(path)}
def n_corrupt_archive(source, target, member):
    """A self-consistent archive defect: one checkpoint image truncated, the archive rebuilt in order.
    Simulates a damaged archive that still lists the expected entries (documents are forged separately)."""
    work = target + '.work'
    subprocess.run(['rm', '-rf', work], check=True)
    os.makedirs(work, mode=0o700)
    entries = subprocess.run(['tar', '-tf', source], check=True, capture_output=True, text=True).stdout.splitlines()
    subprocess.run(['tar', '-C', work, '-xf', source], check=True)
    if member is None:
        # Truncating the inventory leaves every entry and every other file intact, so the archive still passes
        # every structural check and only CRIU discovers the damage, failing early. Truncating a memory image
        # instead was observed to make CRIU spin and write a multi-gigabyte restore log.
        member = 'checkpoint/inventory.img' if 'checkpoint/inventory.img' in entries else None
    if member is None:
        pages = [e for e in entries if e.startswith('checkpoint/pages-') and e.endswith('.img')]
        assert pages, entries
        member = max(pages, key=lambda e: os.path.getsize(os.path.join(work, e)))
    victim = os.path.join(work, member)
    before = os.path.getsize(victim)
    os.truncate(victim, before // 2)
    listing = os.path.join(work, '.entries')
    with open(listing, 'w') as f:
        f.write('\n'.join(e for e in entries if e != './') + '\n')
    subprocess.run(['tar', '-C', work, '--zstd', '--no-recursion', '-cf', target, '-T', listing], check=True)
    os.chmod(target, 0o600)
    subprocess.run(['rm', '-rf', work], check=True)
    return {'target': target, 'member': member, 'member_bytes_before': before, 'member_bytes_after': before // 2,
            'bytes': os.path.getsize(target), 'sha256': _sha256(target)}
def n_kill_container_processes(container_id):
    """Test-owned cleanup: PodMesh reports the processes a failed restore leaves behind but never kills them,
    so the suite kills the ones that still name its own disposable container."""
    assert len(container_id) == 64 and all(c in '0123456789abcdef' for c in container_id), container_id
    killed = []
    for pid in n_processes(container_id):
        try:
            os.kill(pid, 9)
            killed.append(pid)
        except OSError:
            pass
    return {'killed': killed}
def n_kill_service():
    subprocess.run(['systemctl', 'kill', '--signal=SIGKILL', _unit()], check=True)
    return {'killed': _unit()}
def n_restart_service():
    subprocess.run(['systemctl', 'restart', _unit()], check=True)
    return n_ready()
def n_processes(*needles):
    found = []
    for pid in filter(str.isdigit, os.listdir('/proc')):
        try:
            argv = open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')
        except OSError:
            continue
        if all(n.encode() in argv for n in needles):
            found.append(int(pid))
    return found
def n_interrupt_restore(request, import_path, unit):
    """Sends migration_restore and kills the service while the restore command runs in its own scope.

    The kill is timed on the command's own process: it is only sent once `podman container restore`
    of this operation is running, and the CRIU process it spawns is recorded when it is visible."""
    box = []
    def send():
        box.append(n_api(request))
    thread = threading.Thread(target=send)
    thread.start()
    deadline = time.time() + 120
    restore_pids = []
    while not restore_pids:
        assert time.time() < deadline and thread.is_alive(), ('the restore command was never observed', box)
        restore_pids = n_processes('restore', f'--import={import_path}')
        time.sleep(.002)
    criu_deadline = time.time() + 10
    criu = []
    def criu_pids():
        found = []
        for pid in filter(str.isdigit, os.listdir('/proc')):
            try:
                argv0 = open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')[0].decode()
            except (OSError, UnicodeDecodeError):
                continue
            if argv0.endswith('/criu') or argv0 == 'criu':
                found.append({'pid': int(pid), 'executable': argv0})
        return found
    while time.time() < criu_deadline and not criu:
        criu = criu_pids()
        if criu or not thread.is_alive():
            break
        time.sleep(.002)
    at_kill = {'restore_command_pids': restore_pids, 'criu_processes': criu, 'scope_before_kill': n_scope(unit)['active_state']}
    subprocess.run(['systemctl', 'kill', '--signal=SIGKILL', _unit()], check=True)
    at_kill['restore_command_alive_after_kill'] = bool(n_processes('restore', f'--import={import_path}'))
    at_kill['criu_alive_after_kill'] = bool(criu_pids())
    at_kill['scope_after_kill'] = n_scope(unit)['active_state']
    thread.join(120)
    n_ready(120)
    deadline = time.time() + 300
    while n_scope(unit)['active_state'] not in ('inactive', 'failed'):
        assert time.time() < deadline, 'the restore scope did not finish'
        time.sleep(.2)
    return {'first_response': box[0] if box else None, 'at_kill': at_kill, 'scope_after_finish': n_scope(unit)['active_state']}


def _node(argv):
    function = globals()['n_' + argv[0]]
    arguments = json.loads(base64.b64decode(argv[1])) if len(argv) > 1 else {}
    if isinstance(arguments, list):
        return function(*arguments)
    return function(**arguments)


# ---------------------------------------------------------- controller side

class Host:
    """One lab host, reached through SSH, with its API windows and its checks."""

    def __init__(self, role, target, control, socket_path, state_dir, unit):
        self.role, self.target, self.control = role, target, control
        self.socket, self.state_dir, self.unit = socket_path, state_dir, unit
        self.windows, self.fixtures, self.removed_by_test = [], [], {}
        self.source = open(os.path.abspath(__file__), 'rb').read()
        self.identity = self.call('api', request={'operation': 'identity'})['response']['data']['host_uuid']

    def ssh(self, command, input_bytes=None, check=True):
        argv = ['ssh', '-o', 'BatchMode=yes', '-o', 'ControlMaster=auto', '-o', f'ControlPath={self.control}/%C',
                '-o', 'ControlPersist=180', '-o', 'ConnectTimeout=15', self.target, command]
        p = subprocess.run(argv, input=input_bytes, capture_output=True)
        if check and p.returncode:
            raise RuntimeError(f'{self.role} ssh failed ({p.returncode}): {command}\n{p.stderr.decode()}')
        return p

    def call(self, function, **arguments):
        payload = base64.b64encode(json.dumps(arguments).encode()).decode()
        command = (f'sudo env PODMESH_SOCKET={self.socket} PODMESH_STATE_DIR={self.state_dir} PODMESH_UNIT={self.unit} '
                   f'python3 -B - {function} {payload}')
        p = self.ssh(command, input_bytes=self.source)
        lines = [l for l in p.stdout.decode().splitlines() if l.strip()]
        if not lines:
            raise RuntimeError(f'{self.role}.{function} returned nothing: {p.stderr.decode()}')
        return json.loads(lines[-1])

    # --- API
    def api(self, request):
        result = self.call('api', request=request)
        self.windows.append((result['begin_ns'], result['end_ns'], request.get('operation'), request.get('operation_id')))
        return result['response']
    def ok(self, request, check=None, checks=None):
        response = self.api(request)
        assert response.get('ok'), (self.role, request, response)
        if check and checks is not None:
            checks.append(f'[{self.role}] {check}')
        return response['data']
    def refused(self, request, check, expected, checks):
        """A refusal must change no Podman state, no migration journal row and no delivered document."""
        before = self.call('snapshot')
        response = self.api(request)
        assert response.get('ok') is False and expected in json.dumps(response), (self.role, check, expected, response)
        after = self.call('snapshot')
        assert after == before, (self.role, check, 'a refused request changed state', _difference(before, after))
        checks.append(f'[{self.role}] refused without effect: {check}')
        return response
    def status(self, uuid_value):
        return self.ok({'operation': 'migration_status', 'universe_uuid': uuid_value})

    # --- fixtures owned by the suite
    def fixture(self, name, *args):
        self.fixtures.append(name)
        self.call('podman_run', args=['create', '--pull=never', '--network=none', '--name', name, *args])
        return name
    def remove_fixture(self, name, note=None):
        self.call('podman_run', args=['rm', '--force', '--time', '0', name], check=False)
        if name in self.fixtures:
            self.fixtures.remove(name)
        if note:
            self.removed_by_test[name] = note
    def cleanup(self):
        for name in list(self.fixtures):
            self.remove_fixture(name)


def _difference(before, after):
    """A short description of what a refused request changed, for the assertion message."""
    changed = []
    for key in before:
        if before[key] != after.get(key):
            changed.append(key)
    return changed


def request(operation, universe, reference, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), universe_uuid=universe, authorization_ref=reference, **extra)


def transfer(source, destination, authorization, files=('handoff.json', 'manifest.json', 'checkpoint.tar.zst'), box='outbox', into='inbox'):
    """Transport controller: copies documents from one host's outbox to the other's inbox, through this
    workstation, and verifies that every byte arrived by comparing SHA-256 on both sides."""
    members = ' '.join(f"{authorization}/{f}" for f in files)
    out = source.ssh(f'sudo tar -C {source.state_dir}/{box} -cf - {members}')
    destination.ssh(f'sudo mkdir -p -m 0700 {destination.state_dir}/{into} && sudo tar -C {destination.state_dir}/{into} -xf -',
                    input_bytes=out.stdout)
    sent = source.call('boxes')[box].get(authorization, {})
    arrived = destination.call('boxes')[into].get(authorization, {})
    for f in files:
        assert sent[f]['sha256'] == arrived[f]['sha256'], (f, sent.get(f), arrived.get(f))
    return {'authorization_id': authorization, 'files': {f: arrived[f] for f in files}}


def counter_values(samples):
    """(token, n) pairs from /tmp/state samples, ignoring unreadable reads."""
    values = []
    for _, text in samples:
        parts = text.split()
        if len(parts) == 2 and parts[1].isdigit():
            values.append((parts[0], int(parts[1])))
    return values


def memory_continued(before, after):
    """Continuity as the protocol defines it: same memory-only token, the first value after the restore at
    least the last one observed before the checkpoint, and strictly increasing afterwards."""
    b, a = counter_values(before), counter_values(after)
    assert b and a, ('no counter samples', before, after)
    token, last = b[-1]
    tokens = {t for t, _ in a}
    assert tokens == {token}, ('the token changed: a fresh start, not restored memory', token, tokens)
    assert a[0][1] >= last, ('the counter restarted below its checkpointed value', last, a[0][1])
    progression = [n for _, n in a]
    assert progression[-1] > progression[0], ('the counter did not progress after the restore', progression)
    assert all(y >= x for x, y in zip(progression, progression[1:])), ('the counter went backwards', progression)
    return {'token': token, 'last_before_checkpoint': last, 'first_after_restore': a[0][1], 'last_after_restore': progression[-1],
            'distinct_values_after_restore': len(set(progression))}


def event_report(host, since, until, universes, fixture_ids, extra_windows=(), removed_by_test=()):
    """Every Podman container event on API-managed universes must fall inside an API request window of that
    host. Events of the suite's own direct fixtures are identified by container ID and reported separately, as
    is the documented direct removal of a container the API deliberately refuses to delete (a reserved source)."""
    events = host.call('events', since=since, until=until)['events']
    windows = [(b, e) for b, e, *_ in host.windows] + list(extra_windows)
    managed = {'podmesh-' + u for u in universes}
    statuses, outside, fixture_events, test_removals = {}, [], 0, 0
    for e in events:
        if e.get('Type') != 'container' or e.get('Name') not in managed or e.get('Status') == 'cleanup':
            continue
        if e.get('ID') in fixture_ids:
            fixture_events += 1
            continue
        if e.get('Status') == 'remove' and e.get('ID') in removed_by_test:
            test_removals += 1
            continue
        statuses[e['Status']] = statuses.get(e['Status'], 0) + 1
        if not any(b <= e['timeNano'] <= f for b, f in windows):
            outside.append(e)
    return {'statuses': statuses, 'outside_api_windows': outside, 'suite_fixture_events': fixture_events,
            'documented_test_removals': test_removals, 'events_examined': len(events)}


if __name__ == '__main__':
    print(json.dumps(_node(sys.argv[1:])))
