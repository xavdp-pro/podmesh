#!/usr/bin/env python3
"""pause, resume and resources on one lab host, verified from Podman and from the kernel's cgroup.

A disposable alpine universe (isolated profile) is created and started. Verified: pause freezes it
(Podman says paused, the cgroup's freezer says frozen, the process count stays); a second pause
answers none_already_paused and sends nothing; resume thaws it (running, frozen 0); a second
resume answers none_already_running; pausing a stopped universe and resuming a running one are
refused at their contract; resources sets memory and cpus on the running universe and the kernel
enforces them at once (memory.max, cpu.max read from the cgroup, and Podman's record agrees);
a limit under 32 MiB, cpus above the host's cores, and an empty request are refused before anything
is sent; resources on a stopped universe is recorded for the next start and applied by it; resume
under an activation policy without a lease is refused as start is. Environment: PODMESH_SOURCE_SSH,
the transient service variables.
"""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
control = tempfile.mkdtemp(prefix='podmesh-pause-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-pause-resources'
checks = []
MIB = 1024 * 1024

def podman(*args):
    return A.call('podman_run', args=list(args))['stdout'].strip()

def inspect(name, fmt):
    return podman('inspect', '--format', fmt, name)

def cgroup(name, leaf):
    """A limit as the kernel holds it, on the container's cgroup (where podman update writes)."""
    path = inspect(name, '{{.State.CgroupPath}}')
    return A.ssh(f'sudo -n cat /sys/fs/cgroup{path}/{leaf}').stdout.decode().strip()

def frozen(name):
    """The effective freeze of the process itself: read from the cgroup its PID sits in
    (the runtime nests one under the container's), whose cgroup.events says frozen 0 or 1
    whether the freeze was written there or on an ancestor."""
    pid = inspect(name, '{{.State.Pid}}')
    path = A.ssh(f'sudo -n cat /proc/{pid}/cgroup').stdout.decode().strip().split('::', 1)[1]
    events = A.ssh(f'sudo -n cat /sys/fs/cgroup{path}/cgroup.events').stdout.decode()
    return 'frozen 1' in events

def refused(r, fragment, label):
    assert not r.get('ok'), (label, 'accepted', r)
    assert fragment in r['error'], (label, 'refused for another reason', r['error'])
    checks.append(f'refused ({fragment}): {label}')

alpine = json.loads(podman('images', '--format', 'json'))
alpine = next(i['Id'] for i in alpine if any('alpine' in n for n in (i.get('Names') or [])))
u = str(uuid.uuid4()); name = 'podmesh-' + u
try:
    A.ok(request('create', u, reference, image='sha256:' + alpine, network_profile='isolated', command=['sleep', '3600']))
    started = A.ok(request('start', u, reference, observe_seconds=2))
    assert started['application_outcome'] == 'running_when_observed', started
    pid_before = inspect(name, '{{.State.Pid}}')

    # pause: frozen from the kernel's point of view, the process still there
    r = A.ok(request('pause', u, reference))
    assert r['action'] == 'paused' and r['observed_state'] == 'paused', r
    assert inspect(name, '{{.State.Status}}') == 'paused'
    assert frozen(name), 'the process is not frozen'
    assert inspect(name, '{{.State.Pid}}') == pid_before, 'the process changed under the pause'
    checks.append('pause: Podman says paused, the cgroup freezer says frozen, the same process is still there')
    r = A.ok(request('pause', u, reference))
    assert r['action'] == 'none_already_paused', r
    checks.append('a second pause answers none_already_paused and sends nothing')
    refused(A.api(request('start', u, reference)), 'outside the start contract', 'start of a paused universe: a paused universe is resumed, not started')

    # resume
    r = A.ok(request('resume', u, reference))
    assert r['action'] == 'resumed' and r['observed_state'] == 'running', r
    assert not frozen(name) and inspect(name, '{{.State.Pid}}') == pid_before
    checks.append('resume: running again, the cgroup thawed, the same process')
    r = A.ok(request('resume', u, reference))
    assert r['action'] == 'none_already_running', r
    checks.append('a second resume answers none_already_running')

    # resources on the running universe: the kernel enforces them at once
    r = A.ok(request('resources', u, reference, memory_bytes=128 * MIB, cpus=0.5))
    assert r['action'] == 'updated' and r['kernel'] and r['kernel']['memory_max_bytes'] == 128 * MIB and r['kernel']['cpus'] == 0.5, r
    assert cgroup(name, 'memory.max') == str(128 * MIB), cgroup(name, 'memory.max')
    quota, period = cgroup(name, 'cpu.max').split()
    assert abs(int(quota) / int(period) - 0.5) < 0.01, (quota, period)
    assert inspect(name, '{{.HostConfig.Memory}} {{.HostConfig.NanoCpus}}') == f'{128 * MIB} 500000000'
    checks.append('resources on a running universe: memory.max and cpu.max in the cgroup enforce 128 MiB and 0.5 core at once, and Podman records the same')
    r = A.ok(request('resources', u, reference, cpus=1.0))
    assert r['kernel']['cpus'] == 1.0 and r['kernel']['memory_max_bytes'] == 128 * MIB, r
    checks.append('changing cpus alone keeps the memory limit')

    # refused before anything is sent
    refused(A.api(request('resources', u, reference, memory_bytes=16 * MIB)), 'memory_bytes must be from', 'a memory limit under 32 MiB')
    refused(A.api(request('resources', u, reference, cpus=512)), 'cpus must be from', 'more cpus than the host has')
    refused(A.api(request('resources', u, reference)), 'requires memory_bytes, cpus or both', 'an empty resources request')
    assert cgroup(name, 'memory.max') == str(128 * MIB), 'a refused request changed the limit'
    refused(A.api(request('pause', 'a' * 8 + '-0000-4000-8000-000000000000', reference)), 'not found', 'pause of a universe that does not exist')

    # stopped: pause refused, resources kept for the next start
    A.ok(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
    refused(A.api(request('pause', u, reference)), 'outside the pause contract', 'pause of a stopped universe')
    r = A.ok(request('resources', u, reference, memory_bytes=96 * MIB))
    assert r['kernel'] is None and r['verification'] == 'deferred' and 'not verified here' in r['note'], r
    A.ok(request('start', u, reference, observe_seconds=2))
    assert cgroup(name, 'memory.max') == str(96 * MIB), 'the deferred limit was not applied by the start'
    checks.append('resources on an exited universe: accepted with the verification deferred and said so, and the next start applies it (memory.max = 96 MiB)')

    # the gate: resume leaves a writer behind, so it passes the same gate as start
    A.ok(request('pause', u, reference))
    A.ok(request('activation_require', u, reference, lease_seconds=30, takeover_margin_seconds=5))
    refused(A.api(request('resume', u, reference)), 'lease', 'resume under an activation policy without a lease')
    assert inspect(name, '{{.State.Status}}') == 'paused'
    A.ok(request('activation_acquire', u, reference))
    r = A.ok(request('resume', u, reference))
    assert r['action'] == 'resumed', r
    checks.append('resume under a policy: refused without the lease, resumed with it')
    print(json.dumps({'result': 'PASS', 'checks': checks}, indent=2))
finally:
    A.api(request('activation_release', u, reference))
    A.api(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', name], check=False)
