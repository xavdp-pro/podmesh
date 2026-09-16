#!/usr/bin/env python3
"""Roll a new manager universe image onto the three laboratory replicas, one host at a time.

Builds the origin's administration app on the workstation (packaging/podmesh-manager/universe/
build-origin.sh in the web tree), ships the build context to each host, rebuilds the generic image
there as root, replaces that host's replica, and re-arms the follow ticks. The role is not rotated:
the lease and the installed proof stay. Replicas go one at a time so the fact set survives through
replication -- the counter-review of 2026-09-16 recorded that recreating all three at once would
lose every fact, administrators included.

Coupure: the public hostname answers 503 (not the governor) from the governor's replacement until
`tools/arm-publisher-follow.py --refresh` puts the mark back -- run it right after, and warn anyone
looking at the page first. Environment: PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT,
PODMESH_REPLICA_CONFIGS, PODMESH_REPLICA_SET, and PODMESH_WEB_TREE (default
../podmesh-lab/worktrees/podmesh-web relative to this repository).
"""
import json, os, subprocess, sys, tempfile, uuid
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '..', 'tests'))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, replica_create  # noqa: E402

WEB = os.environ.get('PODMESH_WEB_TREE') or os.path.normpath(os.path.join(HERE, '..', '..', 'podmesh-lab', 'worktrees', 'podmesh-web'))
UNIVERSE = os.path.join(WEB, 'packaging', 'podmesh-manager', 'universe')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
LOGICAL, SERVICE = replica_set['logical_manager_id'], os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
targets = dict(pair.split('=', 1) for pair in os.environ.get('PODMESH_PUBLISHER_HOSTS', 'lab-a=lab@192.168.10.156,lab-b=lab@192.168.10.157,lab-c=lab@192.168.10.154').split(','))
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
G = os.environ.get('PODMESH_GOVERNOR_ALIAS', 'lab-a')
state_dir = os.environ['PODMESH_STATE_DIR']
control = tempfile.mkdtemp(prefix='podmesh-roll-')
hosts = {a: Host(a, t, control, os.environ['PODMESH_SOCKET'], state_dir, os.environ['PODMESH_UNIT']) for a, t in targets.items()}
ref = os.environ.get('PODMESH_ROLL_REFERENCE', 'lab-manager-image-roll')
BUILD_DIR = '/root/manager-universe-m-u2'


def hostwide(op, **extra):
    return dict(operation=op, operation_id=str(uuid.uuid4()), authorization_ref=ref, **extra)


def say(m):
    print(m, flush=True)


# 0. the origin app, built once here; the image build on a host needs no network
subprocess.run([os.path.join(UNIVERSE, 'build-origin.sh')], check=True)
origin_tar = open(os.path.join(UNIVERSE, 'origin.tar'), 'rb').read()
# 1. the ticks stand down while the replicas are replaced
for a, h in hosts.items():
    h.ssh('sudo -n systemctl stop podmesh-publisher-follow-lab.timer 2>/dev/null', check=False)
say('ticks arrêtés')

# 2 & 3. the connector and the service address go before their carrier does
r = hosts[G].api(dict(operation='publisher_stop', operation_id=str(uuid.uuid4()), authorization_ref=ref, resource=LOGICAL))
say(f'publisher_stop: {r.get("ok")} {r.get("error", "")[:80]}')
r = hosts[G].api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
say(f'route retirée: {r.get("ok")} {r.get("error", "")[:80]}')

# 4. the image, then the replica, host by host
universes = {}
for a, h in hosts.items():
    old = h.call('podman_run', args=['ps', '--format', '{{.Names}}'], check=False)['stdout']
    for name in [n for n in old.split() if n.startswith('podmesh-')]:
        u = name[len('podmesh-'):]
        h.api(request('stop', u, ref, timeout_seconds=20, on_timeout='kill'))
        h.api(request('delete', u, ref))
        h.call('podman_run', args=['rm', '--force', '--time', '0', name], check=False)
    say(f'{a}: ancienne réplique retirée')
    h.ssh(f'sudo -n install -m 0644 /dev/stdin {BUILD_DIR}/entrypoint.sh', input_bytes=open(os.path.join(UNIVERSE, 'entrypoint.sh'), 'rb').read())
    h.ssh(f'sudo -n install -m 0644 /dev/stdin {BUILD_DIR}/Containerfile.generic', input_bytes=open(os.path.join(UNIVERSE, 'Containerfile.alpine'), 'rb').read())
    h.ssh(f'sudo -n rm -rf {BUILD_DIR}/origin {BUILD_DIR}/origin.py && sudo -n tar -xf - -C {BUILD_DIR}', input_bytes=origin_tar)
    # the whole build under sudo: /root is not readable by the lab user, and a `cd` that fails
    # before it would short-circuit the build into silence
    build = h.ssh('sudo -n sh -c "cd ' + BUILD_DIR + ' && podman build --quiet -f Containerfile.generic -t localhost/podmesh-manager-universe:m-u2-generic ." 2>&1 | tail -2', check=False)
    image = build.stdout.decode().strip()
    assert len(image) >= 12 and 'rror' not in image, (a, 'image build failed', image[-300:])
    say(f'{a}: image reconstruite {image[-12:]}')
    declare_replica_config(h, a, ref, state_dir)
    u = str(uuid.uuid4())
    universes[a] = u
    replica_create(h, u, a, ref, addresses[a], request)
    started = h.ok(request('start', u, ref, observe_seconds=4))
    assert started['application_outcome'] == 'running_when_observed', (a, started['application_outcome'])
    say(f'{a}: réplique {u[:8]} démarrée à {addresses[a]}')

# 5. the service address back on the governor's new carrier
hosts[G].ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses[G], exclusive_resource=LOGICAL))
say('adresse de service republiée sur le gouverneur')

# 6. the ticks again, same bounded mandate, same proof
for a, h in hosts.items():
    h.ssh('sudo -n systemctl reset-failed podmesh-publisher-follow-lab.timer podmesh-publisher-follow-lab.service 2>/dev/null', check=False)
    d = h.ssh('dirname $(sudo -n readlink -f /proc/$(systemctl show -p ExecMainPID --value podmesh-dev-ha.service)/exe)').stdout.decode().strip()
    h.ssh(f'sudo -n systemd-run --quiet --unit=podmesh-publisher-follow-lab --on-active=2 --on-unit-active=10 --timer-property=AccuracySec=1s '
          f'--setenv=PODMESH_SOCKET={os.environ["PODMESH_SOCKET"]} --setenv=PODMESH_CLI={d}/podmesh '
          f'--setenv=PODMESH_PUBLISHER_FOLLOW_MANDATE=/run/podmesh-publisher-follow/mandate '
          f'/usr/local/lib/podmesh-publisher-follow-lab/podmesh-publisher-follow')
say('ticks réarmés')
print(json.dumps({'universes': universes, 'governor': G}, indent=2))
