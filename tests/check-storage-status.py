#!/usr/bin/env python3
"""storage_status on one lab host: what carries Podman's storage, compared with findmnt and df read
directly, and the operator's rule applied -- growth possible only on a dedicated LVM, ZFS or Btrfs
volume, refused on a filesystem shared with the system. Environment: PODMESH_SOURCE_SSH, the
transient service variables."""
import json, os, sys, tempfile, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host  # noqa: E402

A = Host('host', os.environ['PODMESH_SOURCE_SSH'], tempfile.mkdtemp(prefix='podmesh-storage-'),
         os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock'), os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh'), os.environ.get('PODMESH_UNIT', 'podmesh.service'))
checks = []
s = A.ok({'operation': 'storage_status', 'operation_id': str(uuid.uuid4()), 'authorization_ref': 'disposable-lab-storage'})
root = A.ssh('sudo -n podman info --format "{{.Store.GraphRoot}}"').stdout.decode().strip()
assert s['graph_root'] == root, (s['graph_root'], root)
mnt = A.ssh(f'sudo -n findmnt -T {root} -n -o SOURCE,FSTYPE,TARGET').stdout.decode().split()
assert [s['mount']['source'], s['mount']['fstype'], s['mount']['target']] == mnt, (s['mount'], mnt)
checks.append(f'the mount that carries {root} is what findmnt says: {mnt[1]} on {mnt[0]} at {mnt[2]}')
size, used, avail = (int(x) for x in A.ssh(f'sudo -n df -B1 --output=size,used,avail {root} | tail -1').stdout.decode().split())
fs = s['filesystem']
assert fs['size_bytes'] == size and abs(fs['available_bytes'] - avail) < 64 * 1024 * 1024, (fs, size, avail)
checks.append('the sizes are what df says (available within 64 MiB of a concurrent reading)')
dedicated = mnt[2] != '/'
assert s['dedicated'] is dedicated
if not dedicated:
    assert s['backend'] in ('plain', 'lvm', 'lvm-thin', 'zfs', 'btrfs') and s['growth'] == 'refused' and 'shared with the system' in s['reason'], s
    checks.append(f"storage shared with the system's root ({mnt[1]}): growth refused, with the reason")
else:
    expected = 'possible' if s['backend'] in ('zfs', 'btrfs', 'lvm', 'lvm-thin') else 'refused'
    assert s['growth'] == expected, s
    checks.append(f"dedicated {s['backend']} storage: growth {expected}")
caps = A.ok({'operation': 'capabilities', 'operation_id': str(uuid.uuid4()), 'authorization_ref': 'disposable-lab-storage'})
assert 'storage_status' in caps['operations'] and caps['schemas']['storage_status']['kind'] == 'read', 'storage_status is not advertised as a read'
checks.append('storage_status is advertised, as a read, with its schema')
print(json.dumps({'result': 'PASS', 'checks': checks, 'observed': {k: s[k] for k in ('backend', 'dedicated', 'growth')}}, indent=2))
