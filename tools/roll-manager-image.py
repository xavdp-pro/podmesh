#!/usr/bin/env python3
"""Roll a new manager universe image onto the laboratory replicas, one host at a time, each keeping its store.

For each host of PODMESH_ROLL_HOSTS, in that order:
1. the replica universe (the one whose command is the manager entrypoint; nothing else on the host is touched)
   is stopped through PodMesh, with the typed shutdown;
2. its store directory is copied out of the stopped container into a root-only directory on the same host,
   /var/lib/podmesh-lab-roll/<stamp>-<alias>/state, with the digest of every file, and kept there as the
   roll's backup;
3. the image is rebuilt on the host from the web tree's build context and the resident PODMESH_MANAGER_BINARY;
   the resident and the entrypoint inside the image are attested against the files sent. The previous image
   keeps the tag `m-u2-generic-previous`. A failed build or attestation stops the roll before the replica is
   touched;
4. the old universe is deleted, a new one is created from the new image with the same configuration secret
   and address, and the store is copied into it before its first start;
5. the start is observed for 30 seconds -- the entrypoint ends a boot it cannot observe within 25 -- and the
   replica's log must show its boot fact observed.

A roll therefore keeps every fact, receipt and audit row of every replica, including facts that never
replicated because the replica's links were failing. (Recreating a replica with an empty store, as this tool
did until 2026-09-17, reuses the replica's event sequence numbers: after a reboot of its host, the new facts
collide with the ones the peers hold and exchanges are refused both ways.)

Only the active manager's host interrupts the public page: before its replica goes, that host's follow tick
stops, the connector stops and the service address is withdrawn; once its new replica runs, the service
address is published again and the tick re-armed. The tick can only start the connector again with a
takeover proof that is still valid, and a proof lives one lease (an hour in the laboratory): the roll reads
the installed proof before it cuts anything and stops if it has less than PODMESH_ROLL_PROOF_MARGIN seconds
left (default 600), naming `tools/arm-publisher-follow.py --refresh`, which issues a new one under a new
epoch. Warn anyone looking at the page first. Rolling another host interrupts nothing.

Environment: PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT, PODMESH_REPLICA_CONFIGS, PODMESH_REPLICA_SET,
PODMESH_PUBLISHER_HOSTS (alias=ssh-target pairs, comma-separated), PODMESH_WEB_TREE (the web tree holding
packaging/podmesh-manager), PODMESH_MANAGER_BINARY (the resident the image carries), PODMESH_ROLL_HOSTS (the
aliases to roll, in order; default every host of PODMESH_PUBLISHER_HOSTS), PODMESH_ACTIVE_MANAGER_ALIAS (default
lab-a; PODMESH_GOVERNOR_ALIAS is still read), PODMESH_ROLL_REFERENCE, PODMESH_ROLL_REPORT (a path for the JSON
report). Skip the origin's build with PODMESH_ROLL_SKIP_ORIGIN_BUILD=1 when origin.tar is already current.
"""
import hashlib, json, os, shlex, subprocess, sys, tempfile, time, uuid
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '..', 'tests'))
from podmesh_two_hosts import Host, request, control_dir  # noqa: E402
from podmesh_manager_lab import GENERIC_TAG, ENTRYPOINT, declare_replica_config, replica_create  # noqa: E402

WEB = os.environ['PODMESH_WEB_TREE']
UNIVERSE = os.path.join(WEB, 'packaging', 'podmesh-manager', 'universe')
BINARY = os.environ['PODMESH_MANAGER_BINARY']
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
LOGICAL, SERVICE = replica_set['logical_manager_id'], os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
targets = dict(pair.split('=', 1) for pair in os.environ['PODMESH_PUBLISHER_HOSTS'].split(','))
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
ACTIVE = os.environ.get('PODMESH_ACTIVE_MANAGER_ALIAS') or os.environ.get('PODMESH_GOVERNOR_ALIAS') or 'lab-a'
order = [a.strip() for a in os.environ.get('PODMESH_ROLL_HOSTS', ','.join(targets)).split(',') if a.strip()]
assert order and all(a in targets for a in order) and len(set(order)) == len(order), ('PODMESH_ROLL_HOSTS', order)
state_dir = os.environ['PODMESH_STATE_DIR']
control = control_dir('podmesh-roll-')
hosts = {a: Host(a, targets[a], control, os.environ['PODMESH_SOCKET'], state_dir, os.environ['PODMESH_UNIT']) for a in order}
ref = os.environ.get('PODMESH_ROLL_REFERENCE', 'lab-manager-image-roll')
BUILD_DIR = '/root/manager-universe-m-u2'
BACKUPS = '/var/lib/podmesh-lab-roll'
STORE = '/var/lib/podmesh-manager'
PREVIOUS_TAG = GENERIC_TAG + '-previous'
PROOF_REMOTE = '/run/podmesh-publisher-follow/proof.json'
PROOF_MARGIN = int(os.environ.get('PODMESH_ROLL_PROOF_MARGIN', 600))
OBSERVE = 30
TIMER = 'podmesh-publisher-follow-lab'
stamp = time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())
report = {'stamp': stamp, 'reference': ref, 'active_manager': ACTIVE, 'order': order, 'hosts': {}}


def say(m):
    print(f'{time.strftime("%H:%M:%S")} {m}', flush=True)


def hostwide(op, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), authorization_ref=ref, **extra)


def sha256(path):
    return hashlib.sha256(open(path, 'rb').read()).hexdigest()


def root(h, command, check=True, input_bytes=None):
    """One command as root on the host, its output as text."""
    p = h.ssh('sudo -n sh -c ' + shlex.quote(command), input_bytes=input_bytes, check=check)
    return p.stdout.decode().strip()


def replica_of(h):
    """The manager replica on this host: the one universe whose command is the manager entrypoint."""
    names = [n for n in root(h, "podman ps -a --format '{{.Names}}'").split() if n.startswith('podmesh-')]
    found = []
    for name in names:
        inspected = json.loads(root(h, f"podman inspect --format '{{{{json .Config.Cmd}}}}' {name}"))
        if inspected == ENTRYPOINT:
            found.append(name)
    assert len(found) == 1, (h.role, 'expected exactly one manager replica', found)
    return found[0][len('podmesh-'):]


def container_state(h, u):
    return root(h, f"podman inspect --format '{{{{.State.Status}}}}' podmesh-{u}", check=False)


def logs(h, u, lines=40):
    return root(h, f'podman logs --tail {lines} podmesh-{u} 2>&1', check=False)


def start_old(h, u):
    r = h.api(request('start', u, ref, observe_seconds=OBSERVE))
    say(f'{h.role}: old replica started again: {r.get("ok")} {(r.get("data") or {}).get("application_outcome", r.get("error", ""))}')


def proof_left(h):
    """Seconds left on the takeover proof installed on this host, None when there is none to read."""
    out = root(h, f'test -f {PROOF_REMOTE} && python3 -c "import json,time; print(int(json.load(open(\'{PROOF_REMOTE}\'))[\'expires_at\'] - time.time()))"', check=False)
    try:
        return int(out.split()[-1])
    except (ValueError, IndexError):
        return None


binary_sha = sha256(BINARY)
entrypoint_path = os.path.join(UNIVERSE, 'entrypoint.sh')
entrypoint_sha = sha256(entrypoint_path)
if os.environ.get('PODMESH_ROLL_SKIP_ORIGIN_BUILD') != '1':
    subprocess.run([os.path.join(UNIVERSE, 'build-origin.sh')], check=True)
origin_tar = open(os.path.join(UNIVERSE, 'origin.tar'), 'rb').read()
report.update(binary_sha256=binary_sha, entrypoint_sha256=entrypoint_sha, origin_tar_sha256=hashlib.sha256(origin_tar).hexdigest())
say(f'resident {binary_sha[:16]}, entrypoint {entrypoint_sha[:16]}, origin.tar {report["origin_tar_sha256"][:16]}; order {order}, active manager {ACTIVE}')

for a in order:
    h = hosts[a]
    entry = report['hosts'][a] = {}
    old = replica_of(h)
    entry['old_universe'] = old
    say(f'{a}: replica {old[:8]} ({container_state(h, old)})')

    # The build first: a host whose image cannot be built keeps its replica untouched.
    h.ssh(f'sudo -n install -d -m 0700 {BUILD_DIR}')
    h.ssh(f'sudo -n install -m 0755 /dev/stdin {BUILD_DIR}/podmesh-managerd', input_bytes=open(BINARY, 'rb').read())
    h.ssh(f'sudo -n install -m 0644 /dev/stdin {BUILD_DIR}/entrypoint.sh', input_bytes=open(entrypoint_path, 'rb').read())
    h.ssh(f'sudo -n install -m 0644 /dev/stdin {BUILD_DIR}/Containerfile.generic', input_bytes=open(os.path.join(UNIVERSE, 'Containerfile.alpine'), 'rb').read())
    h.ssh(f'sudo -n rm -rf {BUILD_DIR}/origin {BUILD_DIR}/origin.py && sudo -n tar -xf - -C {BUILD_DIR}', input_bytes=origin_tar)
    previous = root(h, f"podman image inspect --format '{{{{.Id}}}}' {GENERIC_TAG}", check=False)
    if previous:
        root(h, f'podman tag {GENERIC_TAG} {PREVIOUS_TAG}')
    entry['previous_image'] = previous
    # the whole build under sudo: /root is not readable by the lab user
    build = root(h, f'cd {BUILD_DIR} && podman build --quiet -f Containerfile.generic -t {GENERIC_TAG} . 2>&1 | tail -2', check=False)
    image = build.split()[-1] if build.split() else ''
    assert len(image) >= 12 and 'rror' not in build, (a, 'image build failed', build[-300:])
    attested = root(h, f'podman run --rm --network none --entrypoint sha256sum {image} /usr/lib/podmesh-manager/podmesh-managerd /usr/local/bin/manager-universe')
    digests = dict(reversed(line.split()) for line in attested.splitlines())
    assert digests.get('/usr/lib/podmesh-manager/podmesh-managerd') == binary_sha, (a, 'resident in the image', digests)
    assert digests.get('/usr/local/bin/manager-universe') == entrypoint_sha, (a, 'entrypoint in the image', digests)
    entry['image'] = image
    say(f'{a}: image {image[:12]} built, resident and entrypoint attested')

    if a == ACTIVE:
        # Nothing publishes again without a valid proof: a cut taken with an expired one lasts until an
        # operator runs the refresh (measured on 2026-09-17: 4 min 33 s instead of the 20 s announced).
        left = proof_left(h)
        entry['takeover_proof_seconds_left'] = left
        if left is None or left < PROOF_MARGIN:
            raise SystemExit(f'{a}: the installed takeover proof has {left} s left (margin {PROOF_MARGIN}); '
                             'run tools/arm-publisher-follow.py --refresh first, then roll again')
        say(f'{a}: takeover proof valid for {left} s')
        h.ssh(f'sudo -n systemctl stop {TIMER}.timer {TIMER}.service 2>/dev/null', check=False)
        r = h.api(dict(operation='publisher_stop', operation_id=str(uuid.uuid4()), authorization_ref=ref, resource=LOGICAL))
        say(f'{a}: follow tick stopped; publisher_stop: {r.get("ok")} {r.get("error", "")[:80]}')
        r = h.api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
        say(f'{a}: service address withdrawn: {r.get("ok")} {r.get("error", "")[:80]}')
        entry['public_page_cut_at'] = int(time.time())

    # The typed stop, then the store out of the stopped container.
    r = h.api(request('stop', old, ref, timeout_seconds=20, on_timeout='kill'))
    state = container_state(h, old)
    assert state in ('exited', 'stopped', 'created'), (a, 'replica still', state, r)
    say(f'{a}: replica stopped ({state}); stop: {r.get("ok")} {(r.get("data") or {}).get("stop_outcome", r.get("error", ""))}')
    backup = f'{BACKUPS}/{stamp}-{a}'
    root(h, f'install -d -m 0700 {BACKUPS} && test ! -e {backup} && install -d -m 0700 {backup} && podman cp podmesh-{old}:{STORE} {backup}/state')
    files = root(h, f"cd {backup}/state && sha256sum -- * | tee {backup}/state.sha256 && stat -c '%n %s' -- *")
    assert 'manager.sqlite' in files, (a, 'no store in the copy', files)
    entry['backup'] = backup
    entry['backup_files'] = files.splitlines()
    say(f'{a}: store kept in {backup}')

    # The old universe goes; the new one comes with the store before its first start.
    deleted = h.api(request('delete', old, ref))
    if not deleted.get('ok'):
        start_old(h, old)
        raise SystemExit(f'{a}: the old replica could not be deleted, so no second one was created: {deleted.get("error")}')
    root(h, f'podman rm --force --time 0 podmesh-{old} >/dev/null 2>&1 || true')
    declare_replica_config(h, a, ref, state_dir)
    new = str(uuid.uuid4())
    entry['new_universe'] = new
    replica_create(h, new, a, ref, addresses[a], request)
    root(h, f'podman cp {backup}/state/. podmesh-{new}:{STORE}/')
    copied = root(h, f"podman cp podmesh-{new}:{STORE}/manager.sqlite - | tar -xOf - manager.sqlite | sha256sum | cut -d' ' -f1")
    kept = next(line.split()[0] for line in files.splitlines() if line.endswith(' manager.sqlite'))
    assert copied == kept, (a, 'store in the new universe differs from the backup', copied, kept)
    started = h.ok(request('start', new, ref, observe_seconds=OBSERVE))
    log = logs(h, new)
    entry['start'] = started.get('application_outcome')
    entry['log'] = log.splitlines()[-12:]
    assert started['application_outcome'] == 'running_when_observed' and 'boot fact observed' in log, (a, started.get('application_outcome'), log[-1500:])
    say(f'{a}: replica {new[:8]} running at {addresses[a]} with its store: ' + next(l for l in log.splitlines() if 'boot fact observed' in l)[:140])

    if a == ACTIVE:
        h.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses[a], exclusive_resource=LOGICAL))
        h.ssh(f'sudo -n systemctl reset-failed {TIMER}.timer {TIMER}.service 2>/dev/null', check=False)
        d = h.ssh('dirname $(sudo -n readlink -f /proc/$(systemctl show -p ExecMainPID --value ' + os.environ['PODMESH_UNIT'] + ')/exe)').stdout.decode().strip()
        h.ssh(f'sudo -n systemd-run --quiet --unit={TIMER} --on-active=2 --on-unit-active=10 --timer-property=AccuracySec=1s '
              f'--setenv=PODMESH_SOCKET={os.environ["PODMESH_SOCKET"]} --setenv=PODMESH_CLI={d}/podmesh '
              f'--setenv=PODMESH_PUBLISHER_FOLLOW_MANDATE=/run/podmesh-publisher-follow/mandate '
              f'/usr/local/lib/podmesh-publisher-follow-lab/podmesh-publisher-follow')
        say(f'{a}: service address published again, follow tick re-armed; run tools/arm-publisher-follow.py --refresh')

if os.environ.get('PODMESH_ROLL_REPORT'):
    with open(os.environ['PODMESH_ROLL_REPORT'], 'w') as f:
        json.dump(report, f, indent=2, sort_keys=True)
print(json.dumps({a: {k: v for k, v in e.items() if k in ('old_universe', 'new_universe', 'image', 'backup', 'start')} for a, e in report['hosts'].items()}, indent=2))
