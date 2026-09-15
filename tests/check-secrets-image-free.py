#!/usr/bin/env python3
"""No replica key in any image layer (Codex's finding B2). One lab host, PODMESH_SOURCE_SSH, the
transient service variables, PODMESH_NETWORK_PEER_VIAS, PODMESH_REPLICA_CONFIGS (the private
per-alias configurations on the workstation) and the generic image
`localhost/podmesh-manager-universe:m-u2-generic` on the host.

Verified from outside: `podman image save` of the generic image, every layer scanned, holds none
of the replica set's pair keys, replica identities or logical manager identity; the secret
declared from the host's inbox is in Podman's store with the digest of the operator's copy, the
inbox copy is gone, `secret_status` shows a digest and never content; the universe created with
the secret carries it at the resident's path, root-only, with the operator's bytes, and the
resident started (the boot fact observed, `manager_status` answering with the replica of that
configuration); the container's export (what a recovery point captures) holds no key either; the
secret cannot be removed while the universe carries it and is removed once the universe is gone;
the operator's copy on the workstation is untouched throughout. Refusals: an inbox file that is
not root-only, a create naming an undeclared secret, a target that is not an absolute path.
"""
import hashlib, io, json, os, sys, tarfile, tempfile, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import CONFIG_TARGET, ENTRYPOINT, GENERIC_TAG, configs_dir, declare_replica_config, generic_image, remove_replica_config, secret_name, secrets_for  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
VIAS = os.environ['PODMESH_NETWORK_PEER_VIAS'].split(',')
control = tempfile.mkdtemp(prefix='podmesh-secrets-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-secrets'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
ALIAS = 'lab-a'
checks = []

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def refused(req, fragment, label):
    r = A.api(req)
    assert not r.get('ok'), (label, 'accepted, expected refusal', r)
    assert fragment in json.dumps(r), (label, 'refused for another reason', r.get('error'))
    checks.append(f'refused ({fragment}): {label}')

def secret_bytes_in(tar_bytes):
    """Every needle found in any file of a tar, recursing into layer tars."""
    found = set()
    with tarfile.open(fileobj=io.BytesIO(tar_bytes)) as t:
        for m in t.getmembers():
            if not m.isfile():
                continue
            data = t.extractfile(m).read()
            for label, needle in NEEDLES.items():
                if needle in data:
                    found.add(label)
            if m.name.endswith('.tar') or '/layer' in m.name or m.name.endswith('layer.tar'):
                try:
                    found |= secret_bytes_in(data)
                except tarfile.TarError:
                    pass
    return found

config_path = os.path.join(configs_dir(), ALIAS, 'config.json')
operator_copy = open(config_path, 'rb').read()
operator_digest = hashlib.sha256(operator_copy).hexdigest()
config = json.loads(operator_copy)
NEEDLES = {f'pair key {i}': p['shared_key_hex'].encode() for i, p in enumerate(config['network']['peers'])}
NEEDLES['replica id'] = config['network']['replica_id'].encode()
NEEDLES['logical manager id'] = config['network']['manager']['logical_manager_id'].encode()
routes0 = A.ssh('ip -4 route show').stdout.decode()
u = str(uuid.uuid4())
declared = False
try:
    # 1. the image: no key, no identity, in any layer
    image_id = generic_image(A)
    saved = A.ssh(f'sudo -n podman image save --format oci-archive {image_id}').stdout
    found = secret_bytes_in(saved)
    assert not found, f'the generic image carries {sorted(found)}'
    checks.append(f'the generic image ({len(saved)} bytes saved, every layer scanned) carries none of the {len(NEEDLES)} needles: pair keys, replica identity, logical manager identity')

    # 2. the inbox must be root-only; then the declaration, its digest, the inbox copy gone
    name = secret_name(ALIAS)
    A.ssh(f'sudo -n mkdir -p -m 0700 {state_dir}/inbox/secrets && sudo -n install -m 0644 -o root -g root /dev/stdin {state_dir}/inbox/secrets/{name}', input_bytes=operator_copy)
    refused(hostwide('secret_declare', name=name, source=name), 'no group or other permission', 'an inbox file readable by others')
    A.ssh(f'sudo -n rm -f {state_dir}/inbox/secrets/{name}')
    d = declare_replica_config(A, ALIAS, reference, state_dir)
    assert d['sha256'] == operator_digest and d['bytes'] == len(operator_copy) and d['inbox_copy_removed'] and d['in_store'], d
    assert A.ssh(f'sudo -n test -e {state_dir}/inbox/secrets/{name}', check=False).returncode != 0, 'the inbox copy remains'
    st = A.ok(hostwide('secret_status'))
    row = next(s for s in st['secrets'] if s['name'] == name)
    assert row['sha256'] == operator_digest and row['in_store'] is True and 'shared_key_hex' not in json.dumps(st), st
    checks.append('the secret declared from a root-only inbox copy: in Podman\'s store with the operator copy\'s digest, the inbox copy removed, the status naming digest and size and never content')

    # 3. a universe with the secret: the file at the resident's path, root-only, the operator's bytes; the resident up
    A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS['lab-a'], peer_pools=[{'pool': POOLS['lab-b'], 'via': VIAS[0]}, {'pool': POOLS['lab-c'], 'via': VIAS[1]}])); declared = True
    refused(request('create', str(uuid.uuid4()), reference, image=image_id, command=ENTRYPOINT, network_profile='managed', secrets=[{'name': 'no-such-secret', 'target': CONFIG_TARGET}]),
            'not declared', 'a create naming an undeclared secret')
    refused(request('create', str(uuid.uuid4()), reference, image=image_id, command=ENTRYPOINT, network_profile='managed', secrets=[{'name': name, 'target': 'etc/relative'}]),
            'absolute file path', 'a secret target that is not an absolute path')
    A.ok(request('create', u, reference, image=image_id, command=ENTRYPOINT, network_profile='managed', secrets=secrets_for(ALIAS)))
    insp = json.loads(A.call('podman_run', args=['inspect', 'podmesh-' + u])['stdout'])[0]
    assert insp['Config']['Labels']['io.podmesh.secrets'] == f'{name}:{CONFIG_TARGET}', insp['Config']['Labels']
    started = A.ok(request('start', u, reference, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], A.call('podman_run', args=['logs', 'podmesh-' + u], check=False))
    inside = A.call('podman_run', args=['exec', 'podmesh-' + u, 'sh', '-c', f'stat -c "%u %a" {CONFIG_TARGET}; sha256sum {CONFIG_TARGET}'])['stdout'].split()
    assert inside[0] == '0' and inside[1] == '600' and inside[2] == operator_digest, inside
    status = A.ok(request('manager_status', u, reference))
    assert status['resident_status']['replica_id'] == config['network']['replica_id'], status['resident_status']
    checks.append('the universe carries the configuration at the resident\'s path, root-only 0600, the operator\'s bytes; the resident started on it and answers with its replica identity')

    # 4. what a recovery point captures: the container's export holds no key
    #    The resident's store legitimately holds the replica and logical manager identities (its
    #    facts and receipts name them; they are the public topology); what must be absent is every key.
    export = A.ssh(f'sudo -n podman export podmesh-{u}').stdout
    found = secret_bytes_in(export)
    keys_found = {f for f in found if f.startswith('pair key')}
    assert not keys_found, f'the container export carries {sorted(keys_found)}'
    checks.append(f'the container\'s export ({len(export)} bytes, what a recovery point captures) carries no pair key: the mounted secret is not part of the rootfs (the store inside names the identities, which are public topology)')

    # 5. lifecycle: no removal while carried; removed once the universe is gone; the operator's copy untouched
    refused(hostwide('secret_remove', name=name), 'carried by', 'removing a secret a universe carries')
    A.ok(request('stop', u, reference, timeout_seconds=15, on_timeout='kill'))
    refused(hostwide('secret_remove', name=name), 'carried by', 'removing a secret a stopped universe still carries')
    A.ok(request('delete', u, reference))
    r = remove_replica_config(A, ALIAS, reference)
    assert r.get('ok') and r['data']['in_store'] is False, r
    assert A.ssh(f'sudo -n podman secret exists {name}', check=False).returncode != 0
    assert hashlib.sha256(open(config_path, 'rb').read()).hexdigest() == operator_digest
    checks.append('the secret refused removal while a universe carried it (running or stopped), removed once the universe was deleted, gone from Podman\'s store; the operator\'s copy untouched')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'image': image_id, 'needles': sorted(NEEDLES),
                      'not_proven': ['Podman\'s secret store at rest: root-only files on the host, the laboratory\'s accepted boundary',
                                     'the single-replica portability fixture image (M-U1) still bakes its fixture configuration; it carries no pair key and is not the replicated manager\'s image']}, indent=2))
finally:
    A.api(request('stop', u, reference, timeout_seconds=5, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
    remove_replica_config(A, ALIAS, reference)
    A.ssh(f'sudo -n rm -f {state_dir}/inbox/secrets/{secret_name(ALIAS)}', check=False)
    if declared:
        r = A.api(hostwide('network_undeclare', network_uuid=NET))
        if not r.get('ok'):
            print(f'undeclare refused: {r.get("error")}', file=sys.stderr)
    print(f'network state restored: {A.ssh("ip -4 route show").stdout.decode() == routes0}', file=sys.stderr)
