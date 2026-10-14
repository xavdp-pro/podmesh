#!/usr/bin/env python3
"""Explicit stopped-instance root administration: append ONE fresh contained grant.

This separately pinned administrator never changes installed bundle bytes, provider
verbs, existing grants, SQL, resource identities, unit hashes or lifecycle results.
An interrupted two-file commit is retained and refused, never auto-adopted/replayed.
"""
import argparse
import copy
import datetime
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import time
import uuid


class Refusal(RuntimeError):
    pass


def require(ok, message):
    if not ok:
        raise Refusal(message)


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON field')
        result[key] = value
    return result


def decode(data):
    return json.loads(data, object_pairs_hook=unique)


def protected(path, directory=False, mode=None):
    path = Path(path)
    require(path.is_absolute(), 'absolute protected path required')
    for parent in [*reversed(path.parents), path]:
        meta = parent.lstat()
        require(meta.st_uid == 0 and not stat.S_ISLNK(meta.st_mode) and not meta.st_mode & 0o022,
                'root custody changed')
    meta = path.lstat()
    require(stat.S_ISDIR(meta.st_mode) if directory else stat.S_ISREG(meta.st_mode), 'unexpected protected path type')
    if not directory:
        require(meta.st_nlink == 1, 'hard-linked administration input refused')
    if mode is not None:
        require(stat.S_IMODE(meta.st_mode) == mode, 'protected path mode differs')
    return path


def array_span(data):
    """Locate the top-level grant array without rewriting any other policy bytes."""
    text = data.decode('utf8');decoder = json.JSONDecoder(object_pairs_hook=unique)
    def whitespace(index):
        while index < len(text) and text[index].isspace():
            index += 1
        return index
    index = whitespace(0);require(text[index] == '{', 'policy object required');index += 1
    result = None
    while True:
        index = whitespace(index)
        if text[index] == '}':
            break
        key, index = decoder.raw_decode(text, index);index = whitespace(index)
        require(text[index] == ':', 'invalid policy object');index = whitespace(index + 1)
        start = index;value, index = decoder.raw_decode(text, index)
        if key == 'allowed_contained_nested':
            require(result is None and isinstance(value, list), 'one contained grant array required')
            spans = [];cursor = whitespace(start + 1)
            while text[cursor] != ']':
                item_start = cursor;_, cursor = decoder.raw_decode(text, cursor)
                spans.append(text[item_start:cursor]);cursor = whitespace(cursor)
                if text[cursor] == ',':
                    cursor = whitespace(cursor + 1)
                else:
                    require(text[cursor] == ']', 'invalid grant array')
            result = (start, index, spans)
        index = whitespace(index)
        if text[index] == ',':
            index += 1
        else:
            require(text[index] == '}', 'invalid policy separator');break
    require(result is not None, 'existing contained grant list required')
    return text, result


def append_only(before, after, template_uuid):
    old, new = decode(before), decode(after)
    old_text, (a, b, old_raw) = array_span(before)
    new_text, (c, d, new_raw) = array_span(after)
    require(old_text[:a] == new_text[:c] and old_text[b:] == new_text[d:],
            'policy bytes outside the contained grant array changed')
    originals = old['allowed_contained_nested'];proposed = new['allowed_contained_nested']
    require(len(proposed) == len(originals) + 1 and proposed[:-1] == originals and new_raw[:-1] == old_raw,
            'existing descriptors changed or more than one grant added')
    require(0 < len(proposed) <= 1024, 'contained grant history budget exceeded')
    templates = [g for g in originals if g['universe_uuid'] == template_uuid]
    require(len(templates) == 1, 'one existing explicit template grant required')
    template, grant = templates[0], proposed[-1]
    require(set(grant) == set(template), 'new grant field/profile envelope differs')
    changeable = {'universe_uuid', 'uid_start', 'gid_start', 'volume_root', 'mounts'}
    require(all(grant[k] == template[k] for k in template if k not in changeable),
            'image/network/profile/range size changed')
    require(grant['containment'] == 'userns-v1' and str(uuid.UUID(grant['universe_uuid'])) == grant['universe_uuid'],
            'canonical contained UUID required')
    require(grant['universe_uuid'] not in {g['universe_uuid'] for g in originals}
            and grant['universe_uuid'] not in {g['universe_uuid'] for g in old.get('allowed_nested', [])},
            'new universe UUID was already granted')
    count = grant['id_count'];require(type(count) is int and 65536 <= count <= 1048576, 'bounded ID count required')
    for key in ('uid_start', 'gid_start'):
        start = grant[key]
        require(type(start) is int and start >= 65536 and start + count <= 2**32 - 1, 'new mapping exceeds kernel/system boundary')
        for existing in originals:
            require(not (start < existing[key] + existing['id_count'] and existing[key] < start + count), 'mapping overlaps existing grant')
    root = Path(grant['volume_root']);previous_root = Path(template['volume_root'])
    require(root.is_absolute() and len(root.parts) >= 4 and '..' not in root.parts and str(root) == grant['volume_root'],
            'dedicated normalized new volume root required')
    for existing in originals:
        other = Path(existing['volume_root'])
        require(not root.is_relative_to(other) and not other.is_relative_to(root), 'new root overlaps retained grant')
    require(len(grant['mounts']) == len(template['mounts']), 'mount count changed')
    for mount, old_mount in zip(grant['mounts'], template['mounts']):
        require(set(mount) == set(old_mount) and all(mount[k] == old_mount[k] for k in mount if k != 'source'),
                'mount destinations/access changed')
        source = Path(old_mount['source']);require(source.is_relative_to(previous_root), 'template source outside template root')
        require(mount['source'] == str(root / source.relative_to(previous_root)), 'new source layout differs from template')
    require({m['destination'] for m in grant['mounts']} >= {'/etc/shaper', '/var/lib/shaper'},
            'empty configuration and saved-volume bindings required')
    return old, new, grant


def directory_identities(grants):
    result = {}
    for grant in grants:
        for path in [grant['volume_root'], *[m['source'] for m in grant['mounts']]]:
            meta = protected(path, directory=True).stat()
            result[path] = {'device': meta.st_dev, 'inode': meta.st_ino, 'uid': meta.st_uid, 'gid': meta.st_gid,
                            'mode': stat.S_IMODE(meta.st_mode)}
    return result


def check_empty(grant):
    for mount in grant['mounts']:
        if mount['destination'] in ('/etc/shaper', '/var/lib/shaper'):
            for path in Path(mount['source']).rglob('*'):
                meta = path.lstat()
                require(stat.S_ISDIR(meta.st_mode) and not stat.S_ISLNK(meta.st_mode), 'new configuration/saved SQL is not empty')


def check_stopped(instance):
    require(instance.r['phase'] == 'stopped', 'native instance must be explicitly stopped')
    instance.unitcheck()
    env = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C', 'HOME': '/root'}
    for unit in instance.r['units']:
        result = subprocess.run(['/usr/bin/systemctl', 'show', unit, '--property=ActiveState,MainPID,ControlPID'],
                                env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=True)
        values = dict(line.split('=', 1) for line in result.stdout.decode().splitlines() if '=' in line)
        require(values.get('ActiveState') in ('inactive', 'failed'), 'native service/target remains live')
        # systemd targets have no service process properties. Only the exact
        # recorded immutable target may omit them; services must report explicit 0.
        target = unit == instance.prefix + '.target'
        require(all(values.get(field, '0' if target else None) == '0'
                    for field in ('MainPID', 'ControlPID')), 'native service/target process remains live')
    for role in ('app', 'db'):
        container = instance.check_container(role)
        require(container['State']['Running'] is False and not container['State']['Paused']
                and container['State']['Pid'] == 0, 'private app/DB process remains live')


def inventory_guard(instance, grant):
    require(not (instance.root / 'provider-state' / (grant['universe_uuid'] + '.json')).exists(),
            'fresh UUID already has provider history')
    rows = decode(instance.podman('ps', '--all', '--format=json').stdout)
    for row in rows:
        identity = row.get('Id') or row.get('ID');require(identity, 'inventory lacks immutable ID')
        actual = decode(instance.podman('container', 'inspect', identity).stdout)[0]
        labels = actual['Config'].get('Labels', {})
        require(actual['Name'].lstrip('/') != 'podmesh-' + grant['universe_uuid']
                and labels.get('io.podmesh.universe') != grant['universe_uuid'], 'fresh UUID already appears in actual inventory')
        host = actual['HostConfig']
        if host.get('UsernsMode') == 'private':
            for field, start in (('UidMap', 'uid_start'), ('GidMap', 'gid_start')):
                mappings = host['IDMappings'][field];require(isinstance(mappings, list), 'existing private mappings unavailable')
                for mapping in mappings:
                    pieces = mapping.split(':');require(len(pieces) == 3 and all(p.isdecimal() for p in pieces), 'unsupported observed mapping')
                    _, beginning, count = map(int, pieces)
                    require(count > 0 and not (grant[start] < beginning + count and beginning < grant[start] + grant['id_count']),
                            'new mapping overlaps actual scoped container')


def validate_freshness(freshness, snapshots, instance, grant, policy_sha256):
    machine, scope = instance.r['host_machine_id'], instance.scope
    target = grant['universe_uuid']
    require(freshness['scope'] == scope and freshness['host_machine_id'] == machine
            and freshness['universe_uuid'] == target and freshness['policy_sha256'] == policy_sha256
            and type(freshness['actual_sql_entries']) is int and freshness['actual_sql_entries'] == 0
            and freshness['inventory_absent'] is True and freshness['provider_record_absent'] is True,
            'receiving absence summary differs')
    app, db = instance.check_container('app'), instance.check_container('db')
    starts = [datetime.datetime.fromisoformat(c['State']['StartedAt'].replace('Z', '+00:00')).timestamp() for c in (app, db)]
    finishes = [datetime.datetime.fromisoformat(c['State']['FinishedAt'].replace('Z', '+00:00')).timestamp() for c in (app, db)]
    for proof in [freshness, *snapshots.values()]:
        observed = proof.get('observed_at')
        require(type(observed) in (int, float) and max(starts) <= observed <= min(finishes),
                'absence evidence must belong to the last stopped app/DB run')
        require(proof['scope'] == scope and proof['host_machine_id'] == machine,
                'absence evidence machine/scope differs')
    require(freshness['db_container_id'] == instance.r['db'] and freshness['db_started_at'] == db['State']['StartedAt'],
            'summary database generation differs')
    sql, inventory, process = snapshots['sql'], snapshots['inventory'], snapshots['process']
    require(sql['universe_uuid'] == target and sql['database'] == 'podmesh-node' and sql['user'] == 'podmesh-node'
            and sql['db_container_id'] == instance.r['db'] and sql['db_started_at'] == db['State']['StartedAt']
            and sql['app_container_id'] == instance.r['app'] and type(sql['count']) is int and sql['count'] == 0
            and sql['query'] == "SELECT COUNT(*) FROM operations WHERE request LIKE '%" + target + "%';",
            'real private journal query/identity/count differs')
    require(inventory['operation'] == 'inventory' and inventory['ok'] is True
            and inventory['app_container_id'] == instance.r['app'] and isinstance(inventory['containers'], list)
            and target not in json.dumps(inventory['containers'], sort_keys=True),
            'actual API inventory is not an absence observation')
    require(process['app_container_id'] == instance.r['app'] and process['app_started_at'] == app['State']['StartedAt']
            and type(process['host_pid']) is int and process['host_pid'] > 0
            and ((isinstance(process['starttime'], str) and process['starttime'].isdecimal())
                 or (type(process['starttime']) is int and process['starttime'] > 0))
            and process['executable_sha256'] == instance.manifest['payload_sha256']['podmeshd'],
            'actual application process/executable proof differs')


def state_fingerprint(root):
    return {str(path.relative_to(root)): sha(protected(path)) for path in sorted(root.rglob('*')) if path.is_file()}


def write_new(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        os.fchmod(stream.fileno(), 0o600);os.fchown(stream.fileno(), 0, 0)
        stream.write(data);stream.flush();os.fsync(stream.fileno())
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def replace_from(stage, destination):
    require(stage.stat().st_dev == destination.stat().st_dev, 'staged commit must remain on the same filesystem')
    os.replace(stage, destination)
    fd = os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--scope', required=True)
    parser.add_argument('--expected-bundle-manifest-sha256', required=True)
    parser.add_argument('--expected-policy-sha256', required=True)
    parser.add_argument('--expected-receipt-sha256', required=True)
    parser.add_argument('--new-policy', required=True, type=Path)
    parser.add_argument('--expected-new-policy-sha256', required=True)
    parser.add_argument('--template-uuid', required=True)
    parser.add_argument('--enrollment-root', required=True, type=Path)
    parser.add_argument('--operation-id', required=True)
    parser.add_argument('--tool-source-pin', required=True)
    parser.add_argument('--expected-tool-sha256', required=True)
    parser.add_argument('--freshness-receipt', required=True, type=Path,
                        help='actual root-observed private SQL/inventory absence, collected before native stop')
    parser.add_argument('--freshness-sha256', required=True)
    args = parser.parse_args()
    require(os.geteuid() == 0, 'separate root administrator required')
    os.umask(0o077)
    require(re.fullmatch('[a-zA-Z0-9_-]{1,96}', args.operation_id), 'bounded safe enrollment operation ID required')
    require(re.fullmatch('[0-9a-f]{40}', args.tool_source_pin), 'separate full tool source pin required')
    require(sha(Path(__file__).resolve()) == args.expected_tool_sha256, 'pinned administrator bytes differ')
    bundle_manifest = protected(args.bundle / 'bundle.json')
    require(sha(bundle_manifest) == args.expected_bundle_manifest_sha256, 'original installed bundle manifest differs')
    bundle = decode(bundle_manifest.read_bytes())
    instance_path = protected(args.bundle / 'instance.py')
    require(sha(instance_path) == bundle['payload_sha256']['instance.py'], 'original administration payload differs before import')
    spec = importlib.util.spec_from_file_location('original_installed_node', instance_path)
    original = importlib.util.module_from_spec(spec);spec.loader.exec_module(original)
    instance = original.Instance(args.scope)
    protected(instance.root, directory=True, mode=0o700)
    fd = os.open(instance.root / 'operator.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'r+') as lock:
        protected(instance.root / 'operator.lock', mode=0o600)
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        instance.read();check_stopped(instance)
        policy_path = protected(instance.root / 'policy.json', mode=0o600)
        receipt_path = protected(instance.receipt, mode=0o600)
        require(sha(policy_path) == args.expected_policy_sha256 and sha(receipt_path) == args.expected_receipt_sha256,
                'expected stopped preimages changed')
        before_policy, before_receipt = policy_path.read_bytes(), receipt_path.read_bytes()
        proposal = protected(args.new_policy, mode=0o600);require(sha(proposal) == args.expected_new_policy_sha256, 'reviewed append proposal changed')
        after_policy = proposal.read_bytes();old, new, grant = append_only(before_policy, after_policy, args.template_uuid)
        approved_root = protected(args.enrollment_root, directory=True)
        require(len(approved_root.parts) >= 4, 'dedicated enrollment subtree required')
        new_root = Path(grant['volume_root'])
        require(new_root != approved_root and new_root.is_relative_to(approved_root)
                and not new_root.is_relative_to(instance.root) and not new_root.is_relative_to(args.bundle), 'new root outside explicit dedicated enrollment subtree')
        old_inodes = directory_identities(old['allowed_contained_nested'])
        new_inodes = directory_identities([grant]);check_empty(grant)
        require(not { (i['device'], i['inode']) for i in old_inodes.values()} &
                    { (i['device'], i['inode']) for i in new_inodes.values()}, 'new directories reuse retained inode identity')
        inventory_guard(instance, grant)
        freshness_path = protected(args.freshness_receipt, mode=0o600)
        require(sha(freshness_path) == args.freshness_sha256, 'actual fresh receiving proof changed')
        freshness = decode(freshness_path.read_bytes())
        evidence = freshness.get('evidence')
        require(isinstance(evidence, dict) and set(evidence) == {'sql', 'inventory', 'process'},
                'actual SQL/API/process snapshots required, not absence booleans alone')
        snapshots = {}
        for kind, reference in evidence.items():
            proof_path = protected(reference['path'], mode=0o600)
            require(sha(proof_path) == reference['sha256'], 'paired absence evidence checksum differs')
            snapshots[kind] = decode(proof_path.read_bytes())
        validate_freshness(freshness, snapshots, instance, grant, args.expected_policy_sha256)
        provider_before = state_fingerprint(instance.root / 'provider-state')
        enrollment_parent = instance.root / 'enrollments'
        enrollment_parent.mkdir(mode=0o700, exist_ok=True);protected(enrollment_parent, directory=True, mode=0o700)
        for previous in enrollment_parent.iterdir():
            protected(previous, directory=True, mode=0o700)
            completed = protected(previous / 'completed.json', mode=0o600)
            require(decode(completed.read_bytes()).get('status') == 'ROOT_CONFIGURATION_ENROLLED',
                    'prior enrollment incomplete; preserve and inspect before another append')
        transaction = enrollment_parent / args.operation_id
        require(not transaction.exists(), 'enrollment exists/partial: preserve and inspect it; no replay/adoption')
        transaction.mkdir(mode=0o700)
        for directory in (instance.root, enrollment_parent):
            directory_fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            os.fsync(directory_fd);os.close(directory_fd)
        intent = {'schema':'podmesh.contained-grant-enrollment/1','operation_id':args.operation_id,
            'scope':args.scope,'bundle':instance.r['bundle'],'host_machine_id':instance.r['host_machine_id'],
            'original_bundle_manifest_sha256':args.expected_bundle_manifest_sha256,
            'tool_source_pin':args.tool_source_pin,'tool_sha256':args.expected_tool_sha256,'time':time.time(),
            'before_policy_sha256':args.expected_policy_sha256,'before_receipt_sha256':args.expected_receipt_sha256,
            'new_policy_sha256':args.expected_new_policy_sha256,'freshness_sha256':args.freshness_sha256,
            'added_uuid':grant['universe_uuid'],'old_directory_identities':old_inodes,'new_directory_identities':new_inodes,
            'provider_state_sha256':provider_before,'policy_change':'append-one-contained-grant-only'}
        write_new(transaction / 'intent.json', (json.dumps(intent,indent=2,sort_keys=True)+'\n').encode())
        write_new(transaction / 'policy.before.json', before_policy)
        write_new(transaction / 'instance.before.json', before_receipt)
        write_new(transaction / 'policy.after.json', after_policy)
        after = decode(before_receipt);after['configuration_sha256']['policy.json'] = args.expected_new_policy_sha256
        after_receipt = (json.dumps(after,indent=2,sort_keys=True)+'\n').encode()
        write_new(transaction / 'instance.after.json', after_receipt)
        # The receipt's only semantic change is the actual new policy checksum.
        expected = copy.deepcopy(instance.r);expected['configuration_sha256']['policy.json'] = args.expected_new_policy_sha256
        require(after == expected, 'unexpected instance receipt delta')
        write_new(transaction / 'policy.commit', after_policy)
        write_new(transaction / 'instance.commit', after_receipt)
        check_stopped(instance);inventory_guard(instance, grant);check_empty(grant)
        require(sha(policy_path) == args.expected_policy_sha256 and sha(receipt_path) == args.expected_receipt_sha256
                and old_inodes == directory_identities(old['allowed_contained_nested'])
                and new_inodes == directory_identities([grant]), 'preimages/directory custody changed before commit')
        replace_from(transaction / 'policy.commit', policy_path)
        replace_from(transaction / 'instance.commit', receipt_path)
        instance.read();check_stopped(instance)
        require(instance.r == after and old_inodes == directory_identities(old['allowed_contained_nested'])
                and new_inodes == directory_identities([grant]) and provider_before == state_fingerprint(instance.root / 'provider-state'),
                'post-enrollment custody differs; preserve versioned intent and do not start')
        complete = {**intent,'status':'ROOT_CONFIGURATION_ENROLLED','after_receipt_sha256':sha(receipt_path),
            'original_instance_read_passed':True,'native_phase':'stopped','existing_grants_preserved':True,
            'sql_or_lifecycle_effects':False,'runtime_qualified':False}
        write_new(transaction / 'completed.json', (json.dumps(complete,indent=2,sort_keys=True)+'\n').encode())
        print(json.dumps({'status':complete['status'],'added_uuid':grant['universe_uuid'],'record':str(transaction),
                          'runtime_qualified':False}))


if __name__ == '__main__':
    main()
