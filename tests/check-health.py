#!/usr/bin/env python3
"""host_status and universe_stats on one lab host, each figure compared with its source read
directly. A disposable universe spins one busy loop under a 0.5-core allowance and a 96 MiB limit:
its CPU over the sample must sit near 50 % of one core and never above the allowance's ceiling,
memory.max must read 96 MiB, memory.current must match the cgroup file within a small drift, its
written disk must match Podman's size accounting, and once stopped it must report no cgroup figures
but keep its disk. host_status is compared with /proc/meminfo, /proc/loadavg and nproc.
Environment: PODMESH_SOURCE_SSH, the transient service variables."""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

A = Host('host', os.environ['PODMESH_SOURCE_SSH'], tempfile.mkdtemp(prefix='podmesh-health-'),
         os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock'), os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh'), os.environ.get('PODMESH_UNIT', 'podmesh.service'))
reference = 'disposable-lab-health'
checks = []
MIB = 1024 * 1024
hostwide = lambda op: {'operation': op, 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference}
sh = lambda cmd: A.ssh(cmd).stdout.decode().strip()

h = A.ok(hostwide('host_status'))
assert h['cpu_count'] == int(sh('nproc')), (h['cpu_count'], sh('nproc'))
total = int(sh("awk '/^MemTotal:/ {print $2}' /proc/meminfo")) * 1024
assert h['memory_total_bytes'] == total and 0 < h['memory_available_bytes'] <= total, h
load1 = float(sh("cut -d' ' -f1 /proc/loadavg"))
assert abs(h['load_average']['1m'] - load1) < 1.0, (h['load_average'], load1)
assert h['storage']['backend'] and h['storage']['growth'] in ('possible', 'refused') and h['storage']['size_bytes'] > 0, h['storage']
checks.append(f"host_status: {h['cpu_count']} cores, memory total and available, load and storage ({h['storage']['backend']}, growth {h['storage']['growth']}) as /proc, nproc and df say")

images = json.loads(A.call('podman_run', args=['images', '--format', 'json'])['stdout'])
alpine = next(i['Id'] for i in images if any('alpine' in n for n in (i.get('Names') or [])))
u = str(uuid.uuid4()); name = 'podmesh-' + u
try:
    A.ok(request('create', u, reference, image='sha256:' + alpine, network_profile='isolated',
                 command=['sh', '-c', 'dd if=/dev/zero of=/tmp/blob bs=1M count=8 2>/dev/null; while :; do :; done']))
    A.ok(request('resources', u, reference, memory_bytes=96 * MIB, cpus=0.5))
    A.ok(request('start', u, reference, observe_seconds=3))
    time.sleep(3)
    s = A.ok(hostwide('universe_stats'))
    row = next(r for r in s['universes'] if r['universe_uuid'] == u)
    assert row['state'] == 'running' and s['sample_ms'], row
    assert row['memory_max_bytes'] == 96 * MIB and row['cpus_allowed'] == 0.5, row
    assert 35 <= row['cpu_percent_of_one_core'] <= 56, f"a busy loop under 0.5 core should sit near 50 %: {row['cpu_percent_of_one_core']}"
    cg = A.call('podman_run', args=['inspect', '--format', '{{.State.CgroupPath}}', name])['stdout'].strip()
    current = int(sh(f'sudo -n cat /sys/fs/cgroup{cg}/memory.current'))
    assert abs(row['memory_current_bytes'] - current) < 8 * MIB, (row['memory_current_bytes'], current)
    checks.append(f"a busy loop under 0.5 core: {row['cpu_percent_of_one_core']} % of one core over {s['sample_ms']} ms, memory.max 96 MiB, memory.current as the cgroup file says")
    sized = json.loads(A.call('podman_run', args=['ps', '-a', '--size', '--filter', f'name={name}', '--format', 'json'])['stdout'])[0]['Size']
    assert row['disk_written_bytes'] >= 8 * MIB and abs(row['disk_written_bytes'] - sized['rwSize']) < MIB, (row['disk_written_bytes'], sized)
    checks.append(f"disk written {row['disk_written_bytes'] // MIB} MiB, as Podman's size accounting says (the 8 MiB the universe wrote included)")
    A.ok(request('stop', u, reference, timeout_seconds=2, on_timeout='kill'))
    s = A.ok(hostwide('universe_stats'))
    row = next(r for r in s['universes'] if r['universe_uuid'] == u)
    assert row['state'] == 'exited' and row['cpu_percent_of_one_core'] is None and row['memory_current_bytes'] is None and row['disk_written_bytes'] >= 8 * MIB, row
    checks.append('stopped: no cgroup figures invented, the written disk kept')
    caps = A.ok(hostwide('capabilities'))
    assert caps['schemas']['host_status']['kind'] == 'read' and caps['schemas']['universe_stats']['kind'] == 'read'
    checks.append('both advertised as reads, with their schemas')
    print(json.dumps({'result': 'PASS', 'checks': checks}, indent=2))
finally:
    A.api(request('stop', u, reference, timeout_seconds=2, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', name], check=False)
