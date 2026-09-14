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

MIGRATION_TABLES = ('migration_reservations', 'migration_authorizations', 'migration_restore_claims', 'migration_reservation_history',
                    'migration_universe_tombstones', 'migration_collection_history')


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
def n_boot_id():
    """This boot's identity, the incarnation a permit is bound to."""
    return {'boot_id': open('/proc/sys/kernel/random/boot_id').read().strip()}
def n_marker(name, marker):
    """Whether a container's filesystem carries the marker file a running universe wrote: a fresh
    `podman export` of the container, searched for `/marker-<marker>` holding exactly the marker bytes.
    Taken on a container that has never been started here, it can only be satisfied by restored bytes."""
    import io, tarfile
    p = _podman('export', name)
    with tarfile.open(fileobj=io.BytesIO(p.stdout)) as t:
        hit = next((n for n in t.getnames() if n.strip('./') == f'marker-{marker}'), None)
        present = bool(hit) and t.extractfile(hit).read() == marker.encode()
        return {'present': present, 'entries': len(t.getnames())}
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
def _fingerprint(path):
    st = os.stat(path)
    return {'bytes': st.st_size, 'mtime_ns': st.st_mtime_ns, 'inode': st.st_ino, 'mode': oct(st.st_mode & 0o777)}
def n_boxes(authorization=None, fingerprint=False):
    """Every delivered document, by hash: the transport controller's view of both directories. With
    `authorization`, that entry alone. With `fingerprint`, size, mtime, inode and mode instead of the
    digest: what a refusal snapshot compares. Hashing every archive both directories hold made each
    snapshot cost seconds per gigabyte of OTHER suites' leftovers, and a suite under a 20-second lease
    lapsed on its own bookkeeping; a stat fingerprint catches any write PodMesh could make (a new
    file, a rewrite, a chmod), and the digest stays what the transport compares."""
    listing = {}
    for box in ('inbox', 'outbox'):
        entries = {}
        root = os.path.join(_state(), box)
        names = ([authorization] if authorization is not None else sorted(os.listdir(root))) if os.path.isdir(root) else []
        for name in names:
            directory = os.path.join(root, name)
            if not os.path.isdir(directory):
                continue
            entries[name] = {f: (_fingerprint(os.path.join(directory, f)) if fingerprint else
                                 {'sha256': _sha256(os.path.join(directory, f)), 'bytes': os.path.getsize(os.path.join(directory, f)),
                                  'mode': oct(os.stat(os.path.join(directory, f)).st_mode & 0o777)})
                             for f in sorted(os.listdir(directory)) if os.path.isfile(os.path.join(directory, f))}
            entries[name]['_mode'] = oct(os.stat(directory).st_mode & 0o777)
        listing[box] = entries
    return listing
def n_gc_runs():
    """The garbage collector's own run records, without the record bodies: one row per plan or apply run.
    Kept out of n_journal so that a refused request can be compared against the migration tables alone."""
    db = sqlite3.connect(f'file:{_state()}/state.sqlite?mode=ro', uri=True)
    try:
        rows = [dict(zip(('operation_id', 'mode', 'authorization_ref', 'collector_version', 'started_at', 'finished_at'), r))
                for r in db.execute('SELECT operation_id,mode,authorization_ref,collector_version,started_at,finished_at '
                                    'FROM garbage_collection_runs ORDER BY started_at, operation_id')]
    except sqlite3.OperationalError:
        rows = []
    db.close()
    return {'runs': rows, 'count': len(rows)}
def n_journal_row(table, key_column, key):
    db = sqlite3.connect(f'file:{_state()}/state.sqlite?mode=ro', uri=True)
    cursor = db.execute(f'SELECT * FROM {table} WHERE {key_column}=?', (key,))
    names = [d[0] for d in cursor.description]
    rows = [dict(zip(names, r)) for r in cursor.fetchall()]
    db.close()
    return {'rows': rows}
def n_journal_write(table, key_column, key, values):
    """Deliberate, test-owned journal forgery, used only on rows this suite created, to exercise the
    collector's refusal of a malformed or wrongly bound record. The suite restores the original values and
    asserts that it did. This is not a product mechanism: root can always forge a journal, which is why the
    collector re-hashes and re-binds every document it relies on instead of trusting a state column."""
    db = sqlite3.connect(f'{_state()}/state.sqlite', timeout=30)
    with db:
        assignments = ', '.join(f'{c}=?' for c in values)
        db.execute(f'UPDATE {table} SET {assignments} WHERE {key_column}=?', [*values.values(), key])
    db.close()
    return n_journal_row(table, key_column, key)
def n_journal_delete(table, key_column, key):
    """Deliberate, test-owned removal of a row this suite's own operation wrote, to simulate a service that
    died between an effect and the record describing it. Same caveat as n_journal_write."""
    db = sqlite3.connect(f'{_state()}/state.sqlite', timeout=30)
    with db:
        db.execute(f'DELETE FROM {table} WHERE {key_column}=?', (key,))
    db.close()
    return n_journal_row(table, key_column, key)
def n_snapshot():
    return {'podman': n_podman_state(), 'journal': n_journal(), 'boxes': n_boxes(fingerprint=True)}
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
    Simulates a damaged archive that still lists the expected entries (documents are forged separately).

    `member` selects the damage: a name truncates that entry, None truncates the inventory (an immediate
    CRIU failure), and 'largest-pages' truncates the biggest memory image, which is the hard shape that
    makes CRIU spin and write until something stops it."""
    work = target + '.work'
    subprocess.run(['rm', '-rf', work], check=True)
    os.makedirs(work, mode=0o700)
    entries = subprocess.run(['tar', '-tf', source], check=True, capture_output=True, text=True).stdout.splitlines()
    subprocess.run(['tar', '-C', work, '-xf', source], check=True)
    if member == 'largest-pages':
        member = None
    elif member is None:
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
def _libpod_cgroups(container_id):
    return [f'/sys/fs/cgroup/machine.slice/libpod-{container_id}.scope',
            f'/sys/fs/cgroup/machine.slice/libpod-conmon-{container_id}.scope']
def _libpod_ids():
    """Container IDs owning a cgroup under machine.slice, read from the kernel alone."""
    ids = set()
    for entry in os.listdir('/sys/fs/cgroup/machine.slice') if os.path.isdir('/sys/fs/cgroup/machine.slice') else []:
        if entry.startswith('libpod-') and entry.endswith('.scope'):
            candidate = entry[len('libpod-'):-len('.scope')].removeprefix('conmon-')
            if len(candidate) == 64 and all(c in '0123456789abcdef' for c in candidate):
                ids.add(candidate)
    return ids
def _cgroup_members(path):
    found = []
    for root, _dirs, files in os.walk(path):
        if 'cgroup.procs' in files:
            try:
                found += [(int(l), root) for l in open(os.path.join(root, 'cgroup.procs')) if l.strip()]
            except OSError:
                pass
    return sorted(set(found))
def _start_epoch(pid):
    """Start time of a process, from /proc/<pid>/stat, in seconds since the epoch."""
    try:
        boot = next(int(l.split()[1]) for l in open('/proc/stat') if l.startswith('btime '))
        stat = open(f'/proc/{pid}/stat').read()
        return boot + int(stat[stat.rindex(')') + 2:].split()[19]) // 100
    except (OSError, ValueError, StopIteration, IndexError):
        return None
def n_cgroup_facts(container_id):
    """The suite's own reading of the facts a reclaim must be proven on: the two cgroups of a container,
    their members, and each member's start time. Independent of what PodMesh reports about them."""
    facts = {}
    for path in _libpod_cgroups(container_id):
        members = _cgroup_members(path) if os.path.isdir(path) else []
        facts[path] = {'exists': os.path.isdir(path),
                       'processes': [{'pid': pid, 'cgroup_procs_file': root, 'start_epoch': _start_epoch(pid),
                                      'comm': (open(f'/proc/{pid}/comm').read().strip() if os.path.exists(f'/proc/{pid}/comm') else None)}
                                     for pid, root in members]}
    facts['total_processes'] = sum(len(v['processes']) for v in facts.values() if isinstance(v, dict))
    return facts
def n_df(path):
    """Raw `df -B1` for the evidence, exactly as the tool prints it."""
    p = subprocess.run(['df', '-B1', path], capture_output=True, text=True, check=True)
    return {'path': path, 'df': p.stdout, 'free': shutil.disk_usage(path).free, 'at': time.time()}
def n_journal_text(unit=None, since=None, lines=200):
    """Raw journal text of a unit, for a resource incident's evidence. Distinct from n_journal, which
    reads the migration tables."""
    argv = ['journalctl', '--no-pager', '-n', str(lines), '-u', unit or _unit()]
    if since:
        argv += ['--since', f'@{int(since)}']
    p = subprocess.run(argv, capture_output=True, text=True)
    return {'unit': unit or _unit(), 'text': p.stdout, 'exit': p.returncode}
def n_restore_under_watchdog(request, floor_bytes, graph='/var/lib/containers/storage', timeout=900):
    """Sends one API request while a TEST-OWNED free-space watchdog runs beside it.

    The watchdog is this suite's own cleanup, not a product mechanism: if free space on the graph root
    drops below the floor, it ends every process in the cgroups of the container that appeared during the
    attempt and records why. The product's own bound is expected to act long before that; the watchdog
    exists so that a failure of the product's bound cannot fill a laboratory disk."""
    before_ids, box = _libpod_ids(), []
    baseline = shutil.disk_usage(graph).free
    def send():
        box.append(n_api(request, timeout))
    thread = threading.Thread(target=send)
    thread.start()
    samples, fired, killed, container_id = [], None, [], None
    while thread.is_alive():
        free = shutil.disk_usage(graph).free
        samples.append([round(time.time(), 2), free])
        if container_id is None:
            fresh = _libpod_ids() - before_ids
            if len(fresh) == 1:
                container_id = fresh.pop()
        if free < floor_bytes and fired is None:
            fired = {'at': time.time(), 'free': free, 'container_id': container_id}
            for path in _libpod_cgroups(container_id) if container_id else []:
                for pid, _root in _cgroup_members(path):
                    try:
                        os.kill(pid, 9)
                        killed.append(pid)
                    except OSError:
                        pass
            fired['killed'] = killed
        time.sleep(.25)
    thread.join(60)
    return {'response': box[0]['response'] if box else None, 'begin_ns': box[0]['begin_ns'] if box else None,
            'end_ns': box[0]['end_ns'] if box else None, 'watchdog_fired': fired, 'floor_bytes': floor_bytes,
            'baseline_free': baseline, 'minimum_free': min([s[1] for s in samples], default=baseline),
            'maximum_consumed': baseline - min([s[1] for s in samples], default=baseline),
            'attempt_container_id': container_id, 'samples': samples[-60:], 'sample_count': len(samples)}
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
def n_kill_container_cgroups(container_id):
    """Test-owned cleanup by cgroup residency, for a disposable container of this suite: used only when the
    product deliberately left processes alone (an abort without reclaim_processes), so that the laboratory
    host does not keep them."""
    assert len(container_id) == 64 and all(c in '0123456789abcdef' for c in container_id), container_id
    killed = []
    for path in _libpod_cgroups(container_id):
        for pid, _root in _cgroup_members(path):
            try:
                os.kill(pid, 9)
                killed.append(pid)
            except OSError:
                pass
    deadline = time.time() + 30
    while time.time() < deadline and any(os.path.isdir(p) for p in _libpod_cgroups(container_id)):
        time.sleep(.25)
    return {'killed': killed, 'cgroups_gone': not any(os.path.isdir(p) for p in _libpod_cgroups(container_id))}
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

    The kill is timed on Podman's own process, not on the systemd-run wrapper that carries the same
    arguments: waiting for the wrapper alone was observed to lose the race on a fast restore, because the
    request spends its first seconds hashing and decompressing the archive before the command starts. CRIU
    is recorded when it becomes visible, but the kill never waits for it beyond the command's own life."""
    box = []
    def send():
        box.append(n_api(request))
    thread = threading.Thread(target=send)
    thread.start()
    def argv_of(pid):
        try:
            return open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')
        except OSError:
            return []
    def podman_pids():
        found = []
        for pid in filter(str.isdigit, os.listdir('/proc')):
            argv = argv_of(pid)
            if argv and argv[0].endswith(b'/podman') and b'restore' in argv and f'--import={import_path}'.encode() in argv:
                found.append(int(pid))
        return found
    def criu_pids():
        found = []
        for pid in filter(str.isdigit, os.listdir('/proc')):
            argv = argv_of(pid)
            argv0 = argv[0].decode(errors='replace') if argv else ''
            if argv0.endswith('/criu') or argv0 == 'criu':
                found.append({'pid': int(pid), 'executable': argv0})
        return found
    began = time.time()
    deadline = began + 300
    restore_pids = []
    while not restore_pids:
        assert time.time() < deadline and thread.is_alive(), ('the restore command was never observed', box)
        restore_pids = podman_pids()
        time.sleep(.002)
    observed_at = time.time()
    # A short look for CRIU, abandoned the moment the command itself is gone: the kill must land while the
    # command is still running, which is what this test is about.
    criu, criu_deadline = [], time.time() + 3
    while time.time() < criu_deadline and not criu and thread.is_alive() and podman_pids():
        criu = criu_pids()
        time.sleep(.002)
    at_kill = {'restore_command_pids': restore_pids, 'criu_processes': criu, 'scope_before_kill': n_scope(unit)['active_state'],
               'command_seen_after_seconds': round(observed_at - began, 2), 'killed_after_seconds': round(time.time() - began, 2),
               'command_still_running_at_kill': bool(podman_pids()), 'request_still_open_at_kill': thread.is_alive()}
    subprocess.run(['systemctl', 'kill', '--signal=SIGKILL', _unit()], check=True)
    at_kill['restore_command_alive_after_kill'] = bool(podman_pids())
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
    sent = source.call('boxes', authorization=authorization)[box].get(authorization, {})
    arrived = destination.call('boxes', authorization=authorization)[into].get(authorization, {})
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
