#!/usr/bin/python3
"""Planned manual logical restore of a bounded canonical NODE; root administration."""
import argparse
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import signal
import stat
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
LABEL = 'io.podmesh.logical-restore'
FORMAT = 'podmesh.canonical-logical-export/v1'
ADOPTION_ENTRYPOINT_SHA256 = '2490cd272725d53dc341c7056cbf0a1817449f83119e0a42119f3c9d5ca22f22'
LIMIT = 16 * 1024 * 1024
ENV = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'HOME': '/root', 'LANG': 'C.UTF-8'}


def need(ok, reason):
    if not ok:
        raise RuntimeError(reason)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def protected(path, directory=False):
    path = Path(path)
    need(path.is_absolute(), 'absolute protected path required')
    for p in [*reversed(path.parents), path]:
        m = p.lstat()
        need(m.st_uid == 0 and not m.st_mode & 0o022 and not stat.S_ISLNK(m.st_mode),
             'root-owned nonreplaceable path required')
        need(stat.S_ISDIR(m.st_mode) if p != path or directory else stat.S_ISREG(m.st_mode),
             'path type differs')
    if not directory:
        need(path.stat().st_nlink == 1, 'hard-linked protected file refused')
    return path


def absent(path):
    path = Path(path)
    protected(path.parent, True)
    need(not path.exists() and not path.is_symlink(), 'new path already exists; preserve it')
    return path


def write_new(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def json_new(path, document):
    write_new(path, (json.dumps(document, sort_keys=True, indent=2) + '\n').encode())


def configuration(path):
    path = protected(path)
    need(stat.S_IMODE(path.stat().st_mode) == 0o600, 'configuration must be root0600')
    c = json.loads(path.read_text())
    need(set(c) == {'container', 'image', 'etc_root', 'sav_root', 'provider_dir'},
         'configuration fields differ')
    need(re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9_.-]{0,63}', c['container']), 'invalid container name')
    c['image'] = c['image'].removeprefix('sha256:')
    need(re.fullmatch(r'[0-9a-f]{64}', c['image']), 'exact cached image ID required')
    for key in ('etc_root', 'sav_root', 'provider_dir'):
        need(isinstance(c[key], str) and Path(c[key]).is_absolute()
             and str(Path(c[key])) == c[key] and '..' not in Path(c[key]).parts
             and ':' not in c[key] and ',' not in c[key], 'unsafe explicit path')
    roots = [Path(c[k]) for k in ('etc_root', 'sav_root', 'provider_dir')]
    need(all(a != b and a not in b.parents and b not in a.parents
             for i, a in enumerate(roots) for b in roots[i + 1:]), 'owned/provider roots overlap')
    protected(c['provider_dir'], True)
    return c


class Node:
    def __init__(self, config):
        self.c = config
        self.env = dict(ENV)
        self.cid = None
        self.lifecycle = None
        self.cleanup = None
        self.token = secrets.token_hex(16)
        protected(HERE / 'logical-transfer-native.py')
        self.worker = (HERE / 'logical-transfer-native.py').read_text()

    def podman(self, *args, data=None, okay=(0,), timeout=120):
        r = subprocess.run(['/usr/bin/podman', '--remote=false', *map(str, args)], input=data,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=self.env, timeout=timeout)
        need(r.returncode in okay, 'Podman operation failed; owned state and native logs retained')
        need(len(r.stdout) <= 4 * LIMIT, 'response exceeds bound')
        return r

    def inspect(self, identity=None):
        return json.loads(self.podman('inspect', identity or self.cid or self.c['container']).stdout)[0]

    def validate(self):
        i = self.inspect()
        need(i['Image'].removeprefix('sha256:') == self.c['image'], 'container image differs')
        h = i['HostConfig']
        need(not h['Privileged'] and h['NetworkMode'] == 'none' and not h.get('PortBindings')
             and h.get('PidMode', '') in ('', 'private') and h.get('IpcMode', '') != 'host',
             'private container boundary differs')
        mounts = {m['Destination']: m for m in i['Mounts']}
        need(set(mounts) == {'/apps/podmesh-node/etc', '/apps/podmesh-node/sav',
                             '/run/podmesh-host', '/run'}, 'undeclared mount or inherited image volume')
        for key, dest, rw in [('etc_root', '/apps/podmesh-node/etc', True),
                              ('sav_root', '/apps/podmesh-node/sav', True),
                              ('provider_dir', '/run/podmesh-host', False)]:
            m = mounts[dest]
            need(m['Type'] == 'bind' and m['Source'] == self.c[key] and bool(m['RW']) == rw,
                 'persistent/provider binding differs')
        need(mounts['/run']['Type'] == 'tmpfs', 'fresh runtime tmpfs required')
        need(i['Config']['Entrypoint'] == ['/usr/bin/python3', '-I',
             '/usr/lib/podmesh-node-canonical/entrypoint.py'], 'canonical entrypoint required')
        return i

    def native(self, mode, **fields):
        request = dict(mode=mode, token=self.token, **fields)
        r = self.podman('exec', '--user', '0:0', '-i', self.cid, '/usr/bin/python3', '-I',
                        '-c', self.worker, str(LIMIT), data=json.dumps(request).encode(), timeout=480)
        return json.loads(r.stdout)

    def resume(self):
        self.native('resume')

    def birth(self):
        path = Path(self.c['sav_root']) / 'admin/birth.json'
        # PID1 creates its own truthful ledger asynchronously during a fresh start.
        return json.loads(path.read_text()) if path.exists() else {}

    def create(self, adopt=False):
        self.lifecycle = None
        self.cleanup = None
        args = ['create', '--name', self.c['container'], '--label', LABEL + '=' + self.token,
                '--pull=never', '--image-volume=ignore', '--network=none', '--stop-timeout=60',
                '--tmpfs', '/run:rw,nosuid,nodev,mode=755']
        for key, dest, access in [('etc_root', '/apps/podmesh-node/etc', 'rw'),
                                  ('sav_root', '/apps/podmesh-node/sav', 'rw'),
                                  ('provider_dir', '/run/podmesh-host', 'ro')]:
            args += ['--volume', self.c[key] + ':' + dest + ':' + access]
        args += [self.c['image']]
        if adopt:
            args += ['--adopt-restored-host-identity']
        self.cid = self.podman(*args).stdout.decode().strip()
        self.lifecycle = {'container_id': self.cid, 'start_command_succeeded': False,
                          'started_at': None, 'running_birth_observed': False}
        self.validate()
        self.podman('start', self.cid)
        self.lifecycle['start_command_succeeded'] = True
        until = time.monotonic() + 90
        while time.monotonic() < until:
            i = self.inspect()
            need(i['Id'] == self.cid and i['State']['Running'] and i['State']['Pid'] > 0,
                 'canonical startup failed; ledger/logs preserved')
            started = i['State'].get('StartedAt')
            need(isinstance(started, str) and started and not started.startswith('0001-'),
                 'current engine start was not observed')
            if self.lifecycle['started_at'] is None:
                self.lifecycle['started_at'] = started
            need(started == self.lifecycle['started_at'], 'engine incarnation changed during startup')
            if self.birth().get('phase') == 'running':
                self.lifecycle['running_birth_observed'] = True
                return
            time.sleep(.25)
        raise RuntimeError('canonical startup timed out; owned target retained')

    def stop(self):
        result = {'status': 'NO_CURRENT_CONTAINER_ID', 'container_id': self.cid,
                  'container_stopped': False, 'clean_lifecycle': False, 'errors': []}
        self.cleanup = result
        if not self.cid:
            return result
        result['status'] = 'UNSTABLE_OR_UNVERIFIED_TARGET'
        try:
            i = self.inspect()
            need(i['Id'] == self.cid, 'owned target identity changed')
        except Exception:
            result['errors'].append('initial-inspection-failed')
            return result
        if i['State']['Running']:
            try:
                self.resume()
            except Exception:
                result['errors'].append('resume-probe-failed')
            try:
                self.podman('stop', '--time', '60', self.cid, timeout=75)
            except Exception:
                result['errors'].append('engine-stop-failed')
        try:
            i = self.inspect()
            need(i['Id'] == self.cid, 'owned target identity changed')
            state = i['State']
            result['engine_state'] = {k: state.get(k) for k in
                                      ('Status', 'Running', 'Pid', 'ExitCode', 'StartedAt', 'FinishedAt')}
            result['container_stopped'] = not state['Running'] and state['Pid'] == 0
            life = self.lifecycle or {}
            if (result['container_stopped'] and state.get('StartedAt') == '0001-01-01T00:00:00Z'
                    and not life.get('start_command_succeeded')):
                result['status'] = 'NEVER_STARTED'
                return result  # The previous birth record is deliberately not read.
            current = (life.get('container_id') == self.cid and life.get('start_command_succeeded')
                       and life.get('running_birth_observed') and life.get('started_at')
                       and life['started_at'] == state.get('StartedAt'))
            if (current and result['container_stopped'] and state['ExitCode'] == 0
                    and not result['errors'] and self.birth().get('phase') == 'stopped-clean'):
                result.update(status='STOPPED_CLEAN_CURRENT_LIFECYCLE', clean_lifecycle=True)
            elif result['container_stopped']:
                result['status'] = 'STOPPED_WITH_INCOMPLETE_LIFECYCLE'
        except Exception:
            result['errors'].append('final-inspection-or-birth-failed')
        return result

    def recreate(self, adopt=False):
        need(self.stop()['clean_lifecycle'], 'current target lifecycle did not stop cleanly; preserve ledger')
        i = self.inspect()
        need(i['Id'] == self.cid and i['Config']['Labels'].get(LABEL) == self.token,
             'owned stopped container identity differs')
        self.podman('rm', self.cid)
        self.cid = None
        self.lifecycle = None
        self.create(adopt)


def file_metadata(item):
    need(isinstance(item, dict) and set(item) == {'data', 'uid', 'gid', 'mode'},
         'saved file fields differ')
    need(all(type(item[k]) is int and 0 <= item[k] < 4294967295 for k in ('uid', 'gid'))
         and type(item['mode']) is int and 0 <= item['mode'] <= 0o777,
         'plain regular file ownership/mode required')
    return {k: item[k] for k in ('uid', 'gid', 'mode')}


def files_manifest(saved):
    result = {}
    need(isinstance(saved, dict) and len(saved) <= 16, 'saved file count exceeds bound')
    for name, item in saved.items():
        need(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,127}', name) and name not in ('.', '..'),
             'invalid saved file name')
        metadata = file_metadata(item)
        data = base64.b64decode(item['data'], validate=True)
        need(len(data) <= LIMIT, 'saved file exceeds bound')
        result[name] = {'bytes': len(data), 'sha256': sha(data), **metadata}
    need(sum(v['bytes'] for v in result.values()) <= LIMIT, 'saved files exceed bound')
    return result


def load_bundle(path):
    path = protected(path, True)
    need(stat.S_IMODE(path.stat().st_mode) == 0o700, 'bundle must be root0700')
    need({p.name for p in path.iterdir()} == {'manifest.json', 'logical.sql', 'files'}, 'incomplete/extra bundle members')
    protected(path / 'manifest.json')
    need((path / 'manifest.json').stat().st_size <= LIMIT, 'manifest exceeds bound')
    m = json.loads((path / 'manifest.json').read_text())
    need(m['format'] == FORMAT and m['status'] == 'completed-coherent-export', 'completed export required')
    protected(path / 'logical.sql')
    need((path / 'logical.sql').stat().st_size <= LIMIT, 'SQL exceeds fixture bound')
    data = (path / 'logical.sql').read_bytes()
    need(data.strip(), 'SQL dump is empty')
    need({'bytes': len(data), 'sha256': sha(data)} == m['sql'], 'SQL checksum differs')
    protected(path / 'files', True)
    need({p.name for p in (path / 'files').iterdir()} == set(m['files']), 'bundle file inventory differs')
    need(len(m['files']) <= 16, 'saved file count exceeds bound')
    need(sum((path / 'files' / n).lstat().st_size for n in m['files']) <= LIMIT,
         'saved files exceed total bound')
    saved = {}
    for name in m['files']:
        need(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,127}', name), 'invalid member name')
        p = protected(path / 'files' / name)
        need(p.stat().st_size <= LIMIT, 'file exceeds bound')
        saved[name] = {'data': base64.b64encode(p.read_bytes()).decode(),
                       **{k: m['files'][name][k] for k in ('uid', 'gid', 'mode')}}
    need(files_manifest(saved) == m['files'], 'saved file checksum differs')
    return m, data, saved


def adoption_delta(before, after, target, request):
    need(after['identity'] == {'host_uuid': before['identity']['host_uuid'],
                              'machine_id_sha256': target}, 'restored identity differs')
    need(before['schema_sha256'] == after['schema_sha256'] and before['objects'] == after['objects']
         and before['filtered'] == after['filtered'], 'original schema/rows changed')
    for name in before['tables']:
        if name not in ('metadata', 'observations'):
            need(before['tables'][name] == after['tables'][name], 'original table changed')
    need(after['tables']['metadata']['rows'] == before['tables']['metadata']['rows']
         and after['tables']['observations']['rows'] == before['tables']['observations']['rows'] + 1,
         'unexpected adoption row count')
    need(after['audits'][:-1] == before['audits'] and len(after['audits']) == len(before['audits']) + 1
         and after['audits'][-1].get('operation') == 'explicit-restored-host-identity-adoption'
         and all(after['audits'][-1].get(k) == v for k, v in request.items()), 'adoption audit differs')


def restore_artifact(manifest, image):
    need(manifest['image'] == image, 'restore requires the same exact cached image')
    need(manifest['provenance']['sha256']['entrypoint.py'] == ADOPTION_ENTRYPOINT_SHA256,
         'image must embed the pinned adoption entrypoint; an old base ID is insufficient')


def export(node, output, names):
    output = absent(output)
    output.mkdir(mode=0o700)
    (output / 'files').mkdir(mode=0o700)
    protected(node.c['etc_root'], True)
    protected(node.c['sav_root'], True)
    i = node.validate()
    need(i['State']['Running'], 'source must already be running under its own authority')
    node.cid = i['Id']
    # The caller owns this cooperative lock, not the source lifecycle.
    lock_path = Path(node.c['sav_root']) / 'admin/logical-transfer.lock'
    protected(lock_path.parent, True)
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'a+b') as lock:
        protected(lock_path)
        need(stat.S_IMODE(os.fstat(lock.fileno()).st_mode) == 0o600, 'transfer lock permissions differ')
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            value = node.native('export', files=names)
        finally:
            node.resume()
    data = base64.b64decode(value['sql'], validate=True)
    need(len(data) <= LIMIT, 'SQL exceeds fixture bound')
    saved = value['files']
    fm = files_manifest(saved)
    write_new(output / 'logical.sql', data)
    for name, item in saved.items():
        write_new(output / 'files' / name, base64.b64decode(item['data'], validate=True))
    manifest = {'format': FORMAT, 'status': 'completed-coherent-export',
                'sql': {'bytes': len(data), 'sha256': sha(data)}, 'files': fm,
                'inventory': value['inventory'], 'image': node.c['image'],
                'provenance': value['birth']['provenance'],
                'configuration_sha256': value['birth']['configuration'],
                'native_worker_sha256': sha(node.worker.encode()), 'created_at': int(time.time())}
    # Completion is published last; partial directories are retained and not restorable.
    json_new(output / 'manifest.json', manifest)
    return {'status': 'COMPLETED_COHERENT_EXPORT', 'output': str(output)}


def restore(node, bundle, receipt_path, allow_adoption):
    m, data, saved = load_bundle(bundle)  # Reuse this completed bundle; never re-export a source.
    restore_artifact(m, node.c['image'])
    receipt_path = absent(receipt_path)
    for key in ('etc_root', 'sav_root'):
        absent(node.c[key])
    need(node.podman('container', 'exists', node.c['container'], okay=(0, 1)).returncode == 1,
         'target container name exists; no overwrite/adoption')
    image = json.loads(node.podman('image', 'inspect', node.c['image']).stdout)[0]
    need(image['Id'].removeprefix('sha256:') == node.c['image'], 'cached image identity differs')
    need(node.podman('image', 'exists', node.c['image'], okay=(0, 1)).returncode == 0,
         'exact image absent; no implicit pull/import')
    receipt = {'status': 'FAILED_OR_INCOMPLETE', 'bundle_sql': m['sql'],
               'configuration': node.c, 'stage': 'new-target-construction', 'events': []}
    try:
        for key in ('etc_root', 'sav_root'):
            Path(node.c[key]).mkdir(mode=0o755)
            Path(node.c[key]).chmod(0o755)
        node.create()
        fresh = node.native('binding')
        binding = {k: fresh['birth'][k] for k in ('datadir', 'configuration', 'provenance')}
        need(binding['provenance'] == m['provenance'], 'target provenance differs')
        # Both newly generated credentials must differ; matching profile bytes are legitimate.
        for key in ('/apps/podmesh-node/etc/mysql/localhost/passwd', '/apps/podmesh-node/etc/admin/passwd'):
            need(binding['configuration'][key] != m['configuration_sha256'][key], 'source credential reused')
        target = fresh['provider_machine_sha256']  # Explicit real provider query under APP1102.
        different = target != m['inventory']['identity']['machine_id_sha256']
        need(not different or allow_adoption, 'different physical host requires --adopt-restored-host-identity')
        receipt['stage'] = 'absent-application-database-import-and-compare'
        result = node.native('restore', files=sorted(saved), saved=saved,
                             sql=base64.b64encode(data).decode(), inventory=m['inventory'],
                             fresh_identity=fresh['identity'])
        need(result['compared'], 'restore comparison incomplete')
        receipt['events'].append('SQL_AND_DECLARED_FILES_COMPARED_BEFORE_ADOPTION')
        need(node.stop()['clean_lifecycle'], 'current imported target did not stop cleanly; preserve ledger')
        request = None
        if different:
            request = {'status': 'restored-and-compared',
                       'source_machine_sha256': m['inventory']['identity']['machine_id_sha256'],
                       'target_machine_sha256': target, 'host_uuid': m['inventory']['identity']['host_uuid'],
                       'export_sha256': m['sql']['sha256']}
            json_new(Path(node.c['etc_root']) / 'admin/restore-complete.json', request)
        receipt['stage'] = 'explicit-adoption-or-same-host-recreation'
        node.recreate(adopt=different)
        restored = node.native('readback', files=sorted(saved))
        if different:
            adoption_delta(m['inventory'], restored['inventory'], target, request)
            need(node.birth()['host_identity_adoption'] == restored['inventory']['audits'][-1], 'birth/audit mismatch')
        else:
            need(restored['inventory'] == m['inventory'], 'same-host restored inventory differs')
        need(restored['files'] == saved, 'restored saved files differ')
        # A separate ordinary lifecycle must preserve the post-adoption result exactly.
        receipt['stage'] = 'ordinary-recreation-and-readback'
        node.recreate()
        final = node.native('readback', files=sorted(saved))
        actual = node.native('binding')
        need(final == restored and {k: actual['birth'][k] for k in binding} == binding,
             'ordinary restart changed saved state, credentials, datadir or provenance')
        need(actual['api']['identity']['host_uuid'] == m['inventory']['identity']['host_uuid']
             and actual['api']['capabilities']['store_engine'] == 'mariadb', 'ordinary NODE API readback failed')
        receipt.update(status='READBACK_PASSED_FINAL_STOP_PENDING', stage='final-owned-stop', target_container=node.cid,
                       adoption=request, inventory=final['inventory'], files=m['files'],
                       ordinary_api=actual['api'], binding=binding)
    finally:
        try:
            cleanup = node.stop()
        except Exception:
            cleanup = {'status': 'UNSTABLE_OR_UNVERIFIED_TARGET', 'container_stopped': False,
                       'clean_lifecycle': False, 'errors': ['unexpected-cleanup-error']}
        receipt['cleanup'] = cleanup
        receipt['current_lifecycle'] = node.lifecycle
        receipt['owned_target_stopped_cleanly'] = cleanup['clean_lifecycle']
        receipt['never_started_cleanup'] = cleanup['status'] == 'NEVER_STARTED'
        if cleanup['clean_lifecycle'] and receipt['status'] == 'READBACK_PASSED_FINAL_STOP_PENDING':
            receipt['status'] = 'RESTORED_COMPARED_AND_ORDINARY_RESTARTED'
        elif not cleanup['clean_lifecycle']:
            receipt['status'] = 'FAILED_OR_INCOMPLETE'
            receipt['retained_state_warning'] = (
                'Never-started target retained; old birth is not new lifecycle evidence.'
                if receipt['never_started_cleanup'] else
                'Cleanup incomplete or unverified; do not assume retained SQL state is stable.')
        receipt['target_container'] = node.cid
        json_new(receipt_path, receipt)
    need(receipt['owned_target_stopped_cleanly'], 'target cleanup incomplete; retain ledger/logs')
    return {'status': receipt['status'], 'receipt': str(receipt_path)}


def main():
    global LIMIT
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--max-payload-bytes', required=True, type=int,
                        help='explicit per-stream/aggregate-file buffer limit; not a host capacity check')
    sub = parser.add_subparsers(dest='command', required=True)
    exp = sub.add_parser('export')
    exp.add_argument('--output', required=True, type=Path)
    exp.add_argument('--saved-file', action='append', default=[])
    res = sub.add_parser('restore')
    res.add_argument('--bundle', required=True, type=Path)
    res.add_argument('--receipt', required=True, type=Path)
    res.add_argument('--adopt-restored-host-identity', action='store_true')
    args = parser.parse_args()
    need(os.getuid() == 0 and os.getgid() == 0, 'root administration only')
    need(args.max_payload_bytes > 0, 'positive explicit payload limit required')
    LIMIT = args.max_payload_bytes
    os.umask(0o077)
    node = Node(configuration(args.config))
    if args.command == 'export':
        result = export(node, args.output, args.saved_file)
    else:
        result = restore(node, args.bundle, args.receipt, args.adopt_restored_host_identity)
    print(json.dumps(result))


if __name__ == '__main__':
    def interrupted(signum, frame):
        raise RuntimeError('administration interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        main()
    except Exception:
        print('logical transfer incomplete; preserve bundle, target ledger and private diagnostics', file=sys.stderr)
        raise SystemExit(1)
