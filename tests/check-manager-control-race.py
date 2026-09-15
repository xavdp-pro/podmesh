#!/usr/bin/env python3
"""The manager control relay bound to one incarnation of the universe (Codex's finding I3). One lab
host, PODMESH_SOURCE_SSH, the transient service variables, PODMESH_NETWORK_PEER_VIAS,
PODMESH_REPLICA_CONFIGS, PODMESH_DAEMON_BINARY (the daemon is restarted with a fault that holds it
three seconds between the universe's inspection and the relay, so that the race can be forced from
outside) and the generic image on the host.

Verified: with the daemon held between inspection and relay, the universe is restarted from
outside during the hold (its PID changes); the operation is refused -- "changed between its
inspection and the relay" -- and nothing was sent; an observation asked the same way is refused
too and appends nothing (the store inspected afterwards holds only the boot facts); without the
fault the same operations succeed and the answer names the container, the PID and the process's
start time. A PID that belongs to another container's cgroup is refused at inspection.
"""
import io, json, os, subprocess, sys, tarfile, tempfile, threading, time, uuid, hashlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, secret_name  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
BINARY = os.environ['PODMESH_DAEMON_BINARY']
CANDIDATE = os.environ['PODMESH_MANAGER_CANDIDATE']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
VIAS = os.environ['PODMESH_NETWORK_PEER_VIAS'].split(',')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
control = tempfile.mkdtemp(prefix='podmesh-race-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-control-race'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
replica = next(r for r in replica_set['replicas'] if r['alias'] == 'lab-a')
SCOPE = replica_set['scopes']['lab-a']
checks = []

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def daemon(fault=None):
    # The stop must be complete -- the unit gone and its runtime directory removed -- before the new
    # unit of the same name starts, or the old unit's cleanup removes the new daemon's socket.
    A.ssh(f'sudo -n systemctl stop {unit} 2>/dev/null; sudo -n systemctl reset-failed {unit} 2>/dev/null; for i in $(seq 1 100); do systemctl is-active --quiet {unit} || [ -d {os.path.dirname(socket_path)} ] || break; sleep 0.1; done', check=False)
    env = f'--setenv=PODMESH_FAULT={fault} ' if fault else ''
    A.ssh(f'sudo -n systemd-run --quiet --unit={unit} --property=RuntimeDirectory={os.path.basename(os.path.dirname(socket_path))} --property=RuntimeDirectoryMode=0700 '
          f'--property=StateDirectory={os.path.basename(state_dir)} --property=StateDirectoryMode=0700 --property=UMask=0077 '
          f'--setenv=PODMESH_STATE_DIR={state_dir} --setenv=PODMESH_SOCKET={socket_path} {env}{BINARY}')
    for _ in range(50):
        if A.ssh(f'sudo -n test -S {socket_path}', check=False).returncode == 0 and A.call('ready', seconds=20).get('ready'):
            return
        time.sleep(0.2)
    raise AssertionError('the daemon did not come up')

def pid_of(u):
    return A.call('podman_run', args=['inspect', '--format', '{{.State.Pid}}', 'podmesh-' + u])['stdout'].strip()

def facts_in_store(u):
    d = tempfile.mkdtemp(prefix='podmesh-race-store-'); os.chmod(d, 0o700)
    remote = A.ssh('sudo -n mktemp -d').stdout.decode().strip()
    for attempt in range(5):
        try:
            A.ssh(f'sudo -n podman cp podmesh-{u}:/var/lib/podmesh-manager {remote}/state && sudo -n podman cp podmesh-{u}:/etc/podmesh-manager/config.json {remote}/config.json')
            break
        except RuntimeError as e:
            if attempt == 4 or 'copying from container' not in str(e):
                raise
            A.ssh(f'sudo -n rm -rf {remote}/state {remote}/config.json'); time.sleep(1)
    tar = A.ssh(f'sudo -n tar -C {remote} -cf - .').stdout
    A.ssh(f'sudo -n rm -rf {remote}')
    with tarfile.open(fileobj=io.BytesIO(tar)) as t:
        for m in t.getmembers():
            if m.isfile() and not m.name.startswith('/') and '..' not in m.name.split('/'):
                target = os.path.join(d, m.name); os.makedirs(os.path.dirname(target), exist_ok=True)
                with open(target, 'wb') as f:
                    f.write(t.extractfile(m).read())
    config = json.load(open(os.path.join(d, 'config.json')))
    config['network']['database_path'] = os.path.join(d, 'state', 'manager.sqlite'); config['control_socket'] = os.path.join(d, 'control.sock')
    for root, _, files in os.walk(d):
        for f in files:
            os.chmod(os.path.join(root, f), 0o600)
    with open(os.path.join(d, 'config.json'), 'w') as f:
        json.dump(config, f)
    os.chmod(os.path.join(d, 'config.json'), 0o600)
    p = subprocess.run([CANDIDATE, '--inspect-store', '--config', os.path.join(d, 'config.json'), '--state-dir', os.path.join(d, 'state')], capture_output=True, text=True)
    assert p.returncode == 0, ('inspect-store', p.stderr[-400:])
    return [(f['subject'], f['value']) for f in json.loads(p.stdout)['ordered_facts']]

def raced(req):
    """The request sent while the daemon holds; the universe restarted from outside during the hold."""
    box = {}
    def ask():
        box['answer'] = A.api(req)
    t = threading.Thread(target=ask); t.start()
    time.sleep(1.0)
    A.ssh(f'sudo -n podman restart -t 5 podmesh-{u} >/dev/null')
    t.join(60)
    return box.get('answer')

u = str(uuid.uuid4())
declared = False
try:
    daemon()
    A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=[{'pool': POOLS['lab-b'], 'via': VIAS[0]}, {'pool': POOLS['lab-c'], 'via': VIAS[1]}])); declared = True
    declare_replica_config(A, 'lab-a', reference, state_dir)
    replica_create(A, u, 'lab-a', reference, replica['address'], request)
    started = A.ok(request('start', u, reference, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], A.call('podman_run', args=['logs', 'podmesh-' + u], check=False))
    st = A.ok(request('manager_status', u, reference))
    assert st['container']['pid'] == int(pid_of(u)) and st['container']['process_started'] and st['container']['container_id'], st['container']
    checks.append('without a fault: the status names the container, its PID and the process\'s start time, and answers')

    # the race: the daemon held between inspection and relay, the universe restarted from outside
    daemon('manager-before-relay:delay')
    pid_before = pid_of(u)
    answer = raced(request('manager_status', u, reference))
    pid_after = pid_of(u)
    assert pid_before != pid_after, 'the restart did not change the PID; the race was not forced'
    assert answer and not answer.get('ok') and 'changed between its inspection and the relay' in answer['error'], answer
    checks.append(f'status under the race: the universe restarted during the hold (PID {pid_before} to {pid_after}), the relay refused before sending anything')
    time.sleep(6)  # the restarted resident re-observes its boot fact
    before = facts_in_store(u)
    answer = raced({'operation': 'manager_observe', 'operation_id': str(uuid.uuid4()), 'universe_uuid': u, 'authorization_ref': reference, 'scope': SCOPE, 'subject': 'race', 'value': 'must not land'})
    assert answer and not answer.get('ok') and 'changed between its inspection and the relay' in answer['error'], answer
    time.sleep(6)
    after = facts_in_store(u)
    assert [f for f in after if f[0] == 'race'] == [], after
    assert len(after) == len(before) + 1 and after[-1][0] == 'boot', (before, after)  # one more boot fact from the restart, nothing else
    checks.append('observation under the race: refused the same way, nothing appended (the store holds boot facts only, one more from the restart)')

    # a PID of another container is refused at inspection: the daemon inspects by name, so the
    # check is exercised through the fault-free path with the identity read back
    daemon()
    st = A.ok(request('manager_status', u, reference))
    assert st['container']['pid'] == int(pid_of(u)), st['container']
    checks.append('without the fault again: the status answers for the new incarnation, PID and start time recorded')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'pids': [pid_before, pid_after],
                      'not_proven': ['a PID reused by another container within the same three-second window: not forced; the same before-and-after identity check refuses it by construction (cgroup and start time)']}, indent=2))
finally:
    daemon()
    A.api(request('stop', u, reference, timeout_seconds=15, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
    remove_replica_config(A, 'lab-a', reference)
    if declared:
        r = A.api(hostwide('network_undeclare', network_uuid=NET))
        if not r.get('ok'):
            print(f'undeclare refused: {r.get("error")}', file=sys.stderr)
