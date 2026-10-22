#!/usr/bin/env python3
"""Bounded Ubuntu collector draft; refuses unimplemented signed-index binding."""
import argparse
import hashlib
import json
import os
import pwd
from pathlib import Path, PurePosixPath
import re
import resource
import shutil
import signal
import stat
import subprocess
import time
from urllib.parse import urlsplit, unquote

BASE = {'registry': 'sha256:93fc3fe333b6cdfb061425869c2c3a5bb2851c0c74dbebc2c940f024e7482d76',
        'image': 'sha256:53ef799caed285438d88678b529b4ff24406d4788f47ab8a74bb2212c707899a',
        'archive': '087a378584383840073ebc347e383229f9765572785b7b4cbf09f22a274b4efb'}
NAME = re.compile(r'[a-z0-9][a-z0-9+.-]+\Z')
VERSION = re.compile(r'[0-9][A-Za-z0-9.+:~\-]*\Z')
SHA = re.compile(r'[0-9a-f]{64}\Z')
SOURCES = [('https://archive.ubuntu.com/ubuntu', 'noble'),
           ('https://archive.ubuntu.com/ubuntu', 'noble-updates'),
           ('https://security.ubuntu.com/ubuntu', 'noble-security')]
TARGETS = ['python3', 'libpython3-stdlib']


def need(ok, message):
    if not ok:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as stream:
        while True:
            block = stream.read(1024*1024)
            if not block:
                return h.hexdigest()
            h.update(block)


def image_read(root, name, maximum):
    pending = list(PurePosixPath(name).parts)[1:]
    held = [os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)]
    links = 0
    try:
        while pending:
            part = pending.pop(0)
            if part in ('', '.'):
                continue
            if part == '..':
                need(len(held)>1, 'image symlink escapes held root')
                os.close(held.pop()); continue
            m = os.stat(part, dir_fd=held[-1], follow_symlinks=False)
            if stat.S_ISLNK(m.st_mode):
                links += 1; need(links<=40, 'image symlink loop')
                target = PurePosixPath(os.readlink(part, dir_fd=held[-1]))
                if target.is_absolute():
                    while len(held)>1:os.close(held.pop())
                    parts=list(target.parts)[1:]
                else:parts=list(target.parts)
                pending=parts+pending; continue
            fd=os.open(part,os.O_RDONLY|os.O_NOFOLLOW|(os.O_DIRECTORY if pending else 0),dir_fd=held[-1])
            actual=os.fstat(fd)
            need((m.st_dev,m.st_ino)==(actual.st_dev,actual.st_ino), 'image component replaced')
            if pending:held.append(fd);continue
            try:
                need(stat.S_ISREG(actual.st_mode) and actual.st_size<=maximum, 'image metadata size/type bound')
                with os.fdopen(os.dup(fd),'rb') as stream:content=stream.read(maximum+1)
                after=os.fstat(fd)
                need(len(content)<=maximum and (actual.st_size,actual.st_ctime_ns)==(after.st_size,after.st_ctime_ns), 'image metadata changed')
                return content
            finally:os.close(fd)
        raise ValueError('image path is not a regular file')
    finally:
        for fd in held:os.close(fd)


def protected_absolute(path, absent=False):
    path = Path(path)
    need(path.is_absolute() and '..' not in path.parts and re.fullmatch(r'/[A-Za-z0-9_./-]+', str(path)), 'normalized absolute path required')
    for part in reversed((path.parent if absent else path).parents):
        m = part.lstat()
        need(stat.S_ISDIR(m.st_mode) and m.st_uid in (0, os.geteuid()) and not m.st_mode & 0o022,
             'unprotected/symlink parent')
    if absent:
        m = path.parent.lstat()
        need(stat.S_ISDIR(m.st_mode) and m.st_uid in (0, os.geteuid()) and not m.st_mode & 0o022,
             'unprotected parent')
        need(not os.path.lexists(path), 'output/work must be absent, not reused or symlinked')
    else:
        m = path.lstat()
        need(not stat.S_ISLNK(m.st_mode) and m.st_uid in (0, os.geteuid()) and not m.st_mode & 0o022,
             'unprotected input')
    return path


def image_file(root, name):
    """Resolve absolute/relative image symlinks within image root, never host root."""
    pending = list(PurePosixPath(name).parts)
    if pending and pending[0] == '/':
        pending.pop(0)
    held = []
    links = 0
    while pending:
        part = pending.pop(0)
        if part in ('', '.'):
            continue
        if part == '..':
            need(bool(held), 'image symlink escapes root')
            held.pop()
            continue
        current = root.joinpath(*held, part)
        m = current.lstat()
        if stat.S_ISLNK(m.st_mode):
            links += 1
            need(links <= 40, 'image symlink loop')
            target = PurePosixPath(os.readlink(current))
            if target.is_absolute():
                held = []
                parts = list(target.parts)[1:]
            else:
                parts = list(target.parts)
            pending = parts + pending
        else:
            if pending:
                need(stat.S_ISDIR(m.st_mode), 'image path component is not a directory')
            held.append(part)
    path = root.joinpath(*held)
    need(stat.S_ISREG(path.lstat().st_mode), 'image input is not a regular file')
    return path


def stanzas(text):
    records = []
    for block in re.split(r'\n\s*\n', text.strip()):
        item = {}
        key = None
        for line in block.splitlines():
            if line.startswith((' ', '\t')) and key:
                item[key] += '\n' + line
            else:
                key, separator, value = line.partition(':')
                need(bool(separator) and key not in item, 'unsupported Debian metadata')
                item[key] = value.strip()
        if item:
            records.append(item)
    return records


def installed(status):
    out = {}
    for item in stanzas(status):
        if item.get('Status') != 'install ok installed':
            need(item.get('Status') in ('deinstall ok config-files', 'purge ok not-installed'),
                 'base dpkg database has pending/unconfigured packages')
            continue
        name, version, arch = (item[k] for k in ('Package', 'Version', 'Architecture'))
        need(NAME.fullmatch(name) and VERSION.fullmatch(version) and arch in ('amd64', 'all'),
             'unsupported base package/architecture')
        need(name not in out, 'multiarch/duplicate base package unsupported')
        out[name] = (version, arch)
    need(bool(out), 'base status is empty')
    return out


def verify_base(root, binding, expected_hash):
    need(SHA.fullmatch(expected_hash) and digest(binding) == expected_hash, 'external base binding hash differs')
    b = json.loads(binding)
    need(b.get('schema') == 'podmesh.base-root-binding/v1' and b.get('base') == BASE,
         'unsupported externally verified base binding')
    need(b.get('suite') == 'noble' and b.get('arch') == 'amd64', 'unsupported base suite/architecture')
    need(b.get('verification') == 'oci-config-diffids-checked', 'external OCI root verification missing')
    paths = {'os_release': '/etc/os-release', 'dpkg_status': '/var/lib/dpkg/status',
             'ubuntu_key': '/usr/share/keyrings/ubuntu-archive-keyring.gpg', 'dpkg_elf': '/usr/bin/dpkg'}
    data = {k: image_read(root, p, 16*1024*1024 if k=='dpkg_status' else 4*1024*1024) for k,p in paths.items()}
    need(all(SHA.fullmatch(b['files'].get(k, '')) and digest(v) == b['files'][k] for k, v in data.items()),
         'base file binding differs')
    release = dict(line.split('=', 1) for line in data['os_release'].decode().splitlines() if '=' in line and not line.startswith('#'))
    need(release.get('ID', '').strip('"') == 'ubuntu' and release.get('VERSION_CODENAME', '').strip('"') == 'noble'
         and release.get('VERSION_ID', '').strip('"') == '24.04', 'observed base is not Ubuntu noble')
    elf = data['dpkg_elf'][:20]
    need(elf[:6] == b'\x7fELF\x02\x01' and int.from_bytes(elf[18:20], 'little') == 62,
         'observed base dpkg is not amd64 ELF')
    return b, data, installed(data['dpkg_status'].decode())


def apt_config(work):
    # All mutable configuration/state/cache/log/status/key/source paths are private.
    paths = {'Dir': work, 'Dir::Etc': work/'etc', 'Dir::Etc::main': work/'etc/empty.conf',
             'Dir::Etc::parts': work/'etc/parts', 'Dir::Etc::sourcelist': work/'etc/sources.list',
             'Dir::Etc::sourceparts': work/'etc/sources.d', 'Dir::Etc::preferences': work/'etc/preferences',
             'Dir::Etc::preferencesparts': work/'etc/preferences.d', 'Dir::Etc::trusted': work/'etc/empty.gpg',
             'Dir::Etc::trustedparts': work/'etc/trusted.d', 'Dir::Etc::netrc': work/'etc/auth.conf',
             'Dir::Etc::netrcparts': work/'etc/auth.d', 'Dir::State': work/'state',
             'Dir::State::status': work/'state/status', 'Dir::State::extended_states': work/'state/extended_states',
             'Dir::State::lists': work/'state/lists', 'Dir::Cache': work/'cache',
             'Dir::Cache::archives': work/'cache/archives', 'Dir::Cache::pkgcache': work/'cache/pkgcache.bin',
             'Dir::Cache::srcpkgcache': work/'cache/srcpkgcache.bin', 'Dir::Log': work/'log',
             'Dir::Log::Terminal': work/'log/terminal.log', 'Dir::Log::History': work/'log/history.log'}
    for path in paths.values():
        need(path.is_relative_to(work), 'APT mutable path escaped work')
    lines = [f'{key} "{value}";' for key, value in paths.items()]
    lines += ['Dir::Bin::methods "/usr/lib/apt/methods";', 'Dir::Bin::dpkg "/usr/bin/dpkg";',
              'APT::Architecture "amd64";', '#clear APT::Architectures;', 'APT::Architectures { "amd64"; };',
              'APT::Sandbox::User "_apt";', 'APT::Install-Recommends "false";',
              'APT::Install-Suggests "false";', 'Acquire::AllowInsecureRepositories "false";',
              'Acquire::AllowDowngradeToInsecureRepositories "false";', 'APT::Get::AllowUnauthenticated "false";',
              'Acquire::Check-Valid-Until "true";', 'Acquire::Check-Date "true";',
              'Acquire::https::AllowRedirect "false";', 'Acquire::https::Proxy "DIRECT";', 'Acquire::http::Proxy "DIRECT";', 'Acquire::GzipIndexes "false";', 'Acquire::Retries "0";',
              'Acquire::https::Timeout "20";', 'APT::Update::Error-Mode "any";',
              '#clear DPkg::Pre-Invoke;', '#clear DPkg::Post-Invoke;', '#clear DPkg::Pre-Install-Pkgs;',
              '#clear APT::Update::Post-Invoke;', '#clear APT::Update::Post-Invoke-Success;']
    return '\n'.join(lines) + '\n'


def planned(output, base):
    need(not re.search(r'^(Remv|Purg)\s', output, re.M), 'resolution removes packages')
    rows = []
    for line in output.splitlines():
        if not line.startswith('Inst '):
            continue
        match = re.fullmatch(r'Inst ([a-z0-9+.:\-]+) (?:\[([^]]+)\] )?\(([^\s()]+) [^()]+\)', line)
        need(match is not None, 'unsupported APT plan syntax')
        name = match[1].split(':')[0]
        need(NAME.fullmatch(name) and VERSION.fullmatch(match[3]) and name not in base and not match[2],
             'unplanned base upgrade/reinstall or bad package identity')
        need(name not in [n for n, _ in rows], 'duplicate planned package')
        rows.append((name, match[3]))
    need(0 < len(rows) <= 64 and {'python3', 'libpython3-stdlib'} <= {n for n, _ in rows},
         'plan is not complete new Python distribution/stdlib')
    return rows


def package_record(text, name, version):
    matches = [p for p in stanzas(text) if p.get('Package') == name and p.get('Version') == version]
    need(matches and len({(p.get('SHA256'), p.get('Size'), p.get('Architecture')) for p in matches}) == 1,
         'ambiguous/unavailable authenticated package metadata')
    p = matches[0]
    need(p['Architecture'] in ('amd64', 'all') and SHA.fullmatch(p.get('SHA256', ''))
         and p.get('Size', '').isdigit() and p.get('Installed-Size', '').isdigit(), 'unsupported package metadata')
    path = PurePosixPath(unquote(p.get('Filename', '')))
    need(not path.is_absolute() and path.parts and path.parts[0] == 'pool' and '..' not in path.parts,
         'package filename escapes official archive')
    return {'name': name, 'version': version, 'arch': p['Architecture'], 'sha256': p['SHA256'],
            'bytes': int(p['Size']), 'installedKiB': int(p['Installed-Size']), 'archivePath': p['Filename'],
            'file': p['SHA256'] + '.deb', 'authenticatedMetadata': p}


def verify_bundle(output, expected_hash):
    raw = (output/'closure.lock').read_bytes()
    need(SHA.fullmatch(expected_hash) and len(raw) <= 131072 and digest(raw) == expected_hash,
         'closure lock hash/size differs')
    lines = raw.decode().splitlines()
    need(lines and lines[0] == 'PODMESH_PYTHON_CLOSURE_V1' and raw.endswith(b'\n'), 'closure schema differs')
    fields, packages = {}, []
    ended = False
    for line in lines[1:]:
        row = line.split('\t')
        need(not ended, 'records after END')
        if row[0] == 'PACKAGE':
            need(len(row) == 8 and NAME.fullmatch(row[1]) and VERSION.fullmatch(row[2])
                 and row[3] in ('all', 'amd64') and SHA.fullmatch(row[4]) and row[5].isdigit()
                 and int(row[5]) > 0 and row[6] == row[4]+'.deb' and row[7].isdigit(), 'bad package lock row')
            need(row[1] not in [p[1] for p in packages], 'duplicate package')
            packages.append(row)
        elif row[0] == 'END':
            need(len(row) == 3 and row[1] == str(len(packages))
                 and row[2] == str(sum(int(p[5]) for p in packages)), 'bad closure totals')
            ended = True
        else:
            need(len(row) == 2 and row[0] not in fields, 'duplicate/malformed lock field')
            fields[row[0]] = row[1]
    need(ended and 0 < len(packages) <= 64 and {'python3', 'libpython3-stdlib'} <= {p[1] for p in packages}, 'incomplete Python closure')
    required = {'REGISTRY','IMAGE','ARCHIVE','SUITE','ARCH','STATUS','OS_RELEASE','KEY','BINDING','EVIDENCE'}
    need(set(fields) == required and fields['REGISTRY'] == BASE['registry'] and fields['IMAGE'] == BASE['image']
         and fields['ARCHIVE'] == BASE['archive'] and fields['SUITE'] == 'noble' and fields['ARCH'] == 'amd64', 'wrong locked base')
    need(all(SHA.fullmatch(fields[k]) for k in ('STATUS','OS_RELEASE','KEY','BINDING','EVIDENCE')), 'bad evidence hash')
    need(digest((output/'evidence.json').read_bytes()) == fields['EVIDENCE'], 'evidence hash differs')
    need(not any(p.is_symlink() for p in output.rglob('*')), 'symlink in closure bundle')
    expected_files = {p[6] for p in packages}
    need({p.name for p in (output/'debs').iterdir()} == expected_files, 'extra/missing artifacts')
    need({str(p.relative_to(output)) for p in output.rglob('*.deb')} == {'debs/'+p for p in expected_files}, 'deb outside declared artifacts')
    for package in packages:
        artifact = output/'debs'/package[6]
        need(artifact.is_file() and artifact.stat().st_size == int(package[5])
             and file_digest(artifact) == package[4], 'package hash/size tampering')
    return packages


def download_argv(versions):
    need(all(NAME.fullmatch(n) and VERSION.fullmatch(v) for n, v in versions), 'invalid package argument')
    return ['/usr/bin/apt-get', '--download-only', '--no-upgrade', '--no-remove', '--no-install-recommends',
            '--yes', 'install', *[n + '=' + v for n, v in versions]]


class Commands:
    def __init__(self, work, maximum):
        self.work, self.maximum = work, maximum
        self.deadline = time.monotonic() + 600
        self.number = 0
        self.env = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'HOME': str(work), 'LC_ALL': 'C',
                    'APT_CONFIG': str(work/'apt.conf'), 'DEBIAN_FRONTEND': 'noninteractive'}

    def run(self, args, timeout=60):
        need(args[0] in ('/usr/bin/apt-get', '/usr/bin/apt-cache', '/usr/bin/gpgv', '/usr/bin/dpkg-deb'),
             'unsupported collector command')
        if args[0].endswith('apt-get'):
            need('update' in args or '--simulate' in args or '--download-only' in args or '--print-uris' in args, 'host install prohibited')
        self.number += 1
        output = self.work/f'command-{self.number}.log'
        def limits():
            resource.setrlimit(resource.RLIMIT_FSIZE, (self.maximum, self.maximum))
        with output.open('xb') as stream:
            process = subprocess.Popen(args, env=self.env, stdin=subprocess.DEVNULL, stdout=stream,
                                       stderr=subprocess.STDOUT, start_new_session=True, preexec_fn=limits)
            until = min(self.deadline, time.monotonic()+timeout)
            while process.poll() is None:
                used = sum(p.stat().st_size for p in self.work.rglob('*') if p.is_file() and not p.is_symlink())
                if time.monotonic() >= until or used > self.maximum or output.stat().st_size > 8*1024*1024:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    raise ValueError('collector command exceeded finite time/output/disk bound; retain work')
                time.sleep(0.2)
        need(process.returncode == 0 and output.stat().st_size <= 8*1024*1024, 'collector command failed; retain work')
        return output.read_text()


def collect(args):
    need(os.geteuid() == 0, 'collector requires an isolated root build authority, never host package installation')
    root = protected_absolute(args.base_root)
    binding_path = protected_absolute(args.base_binding)
    work, output = protected_absolute(args.work, True), protected_absolute(args.output, True)
    need(work not in output.parents and output not in work.parents and work != output, 'work/output overlap')
    binding, data, base = verify_base(root, binding_path.read_bytes(), args.base_binding_sha256)
    need(0 < args.max_download_bytes <= 256*1024*1024 and 0 < args.max_metadata_bytes <= 768*1024*1024,
         'invalid finite byte budgets')
    need(shutil.disk_usage(work.parent).free > args.max_metadata_bytes + 2*args.max_download_bytes,
         'insufficient measured work filesystem free bytes')
    sandbox = pwd.getpwnam('_apt')
    need(sandbox.pw_uid != 0 and shutil.which('setfacl') == '/usr/bin/setfacl', 'unprivileged APT sandbox/setfacl dependency missing')
    need(all(parent.stat().st_mode & 0o001 for parent in [work.parent,*work.parent.parents]),
         'sandbox cannot traverse protected work ancestors; choose a private build root below searchable parents')
    work.mkdir(mode=0o700)
    for name in ['etc/parts', 'etc/sources.d', 'etc/preferences.d', 'etc/trusted.d', 'etc/auth.d',
                 'state/lists/partial', 'cache/archives/partial', 'log']:
        (work/name).mkdir(mode=0o700, parents=True, exist_ok=True)
    for name in ['etc/empty.conf', 'etc/sources.list', 'etc/empty.gpg', 'etc/auth.conf', 'state/extended_states']:
        (work/name).write_text('')
    (work/'state/status').write_bytes(data['dpkg_status'])
    (work/'etc/ubuntu-key.gpg').write_bytes(data['ubuntu_key'])
    (work/'apt.conf').write_text(apt_config(work))
    prefs = ''.join(f'Package: {n}\nPin: version {v}\nPin-Priority: 1001\n\n' for n, (v, _) in sorted(base.items()))
    (work/'etc/preferences').write_text(prefs)
    sources = ''.join(f'Types: deb\nURIs: {uri}\nSuites: {suite}\nComponents: main\nArchitectures: amd64\nSigned-By: {work}/etc/ubuntu-key.gpg\n\n' for uri, suite in SOURCES)
    (work/'etc/sources.d/ubuntu.sources').write_text(sources)
    # Named-user ACLs change only NEW work files, never host ancestors/config.
    subprocess.run(['/usr/bin/setfacl','-R','-m',f'u:{sandbox.pw_uid}:r-X',str(work)],
                   env={'PATH':'/usr/bin:/bin','LC_ALL':'C'},check=True,timeout=10)
    for partial in ('state/lists/partial','cache/archives/partial'):
        os.chown(work/partial,sandbox.pw_uid,sandbox.pw_gid)
    cmd = Commands(work, args.max_metadata_bytes + 2*args.max_download_bytes)
    cmd.run(['/usr/bin/apt-get', 'update'], 180)
    releases = sorted((work/'state/lists').glob('*_InRelease'))
    need(len(releases) == len(SOURCES), 'missing/extra signed Ubuntu release metadata')
    observed_suites = set()
    signed_releases = {}
    fingerprints = set(binding.get('ubuntu_archive_fingerprints',[]))
    need(fingerprints and all(re.fullmatch('[0-9A-F]{40}', f) for f in fingerprints), 'externally verified Ubuntu archive fingerprints required')
    for path in releases:
        payload = work/('verified-'+path.name)
        status = cmd.run(['/usr/bin/gpgv','--status-fd','1','--output',str(payload),'--keyring',str(work/'etc/ubuntu-key.gpg'),str(path)],15)
        valid=set(re.findall(r'\[GNUPG:\] VALIDSIG ([0-9A-F]{40}) ',status))
        need(valid and valid <= fingerprints, 'unexpected or missing Ubuntu archive signer')
        release = stanzas(payload.read_text())[0]
        need(release.get('Origin') == 'Ubuntu' and release.get('Codename') == 'noble'
             and release.get('Suite') in {s for _, s in SOURCES}
             and 'amd64' in release.get('Architectures', '').split(), 'signed release suite/origin/architecture drift')
        observed_suites.add(release['Suite'])
        signed_releases[release['Suite']] = release
    need(observed_suites == {s for _, s in SOURCES}, 'incomplete signed suite coverage')
    # Signed index-to-Release normalization must precede trusting apt-cache records.
    # This guard is deliberate until the exact APT index encoding path is bound.
    need(False, 'UNIMPLEMENTED_AUTHENTICATED_PACKAGES_INDEX_BINDING: retain signed metadata; no package download or install authorized')
    plan_text = cmd.run(['/usr/bin/apt-get', '--simulate', '--no-upgrade', '--no-remove', '--no-install-recommends',
                         'install', *TARGETS])
    versions = planned(plan_text, base)
    packages = [package_record(cmd.run(['/usr/bin/apt-cache', 'show', n+'='+v]), n, v) for n, v in versions]
    total = sum(p['bytes'] for p in packages)
    need(total <= args.max_download_bytes and total < shutil.disk_usage(work).free, 'planned download exceeds measured budget')
    need(digest((work/'state/status').read_bytes()) == binding['files']['dpkg_status'], 'base status changed before download')
    uri_text = cmd.run(['/usr/bin/apt-get', '--print-uris', '--no-upgrade', '--no-remove', '--no-install-recommends',
                        '--yes', 'install', *[n+'='+v for n, v in versions]])
    uris = re.findall(r"^'([^']+)'", uri_text, re.M)
    need(len(uris) == len(packages), 'unsupported URI resolution')
    for uri in uris:
        u = urlsplit(uri)
        need(u.scheme == 'https' and u.netloc in ('archive.ubuntu.com', 'security.ubuntu.com')
             and u.path.startswith('/ubuntu/pool/') and '..' not in PurePosixPath(unquote(u.path)).parts
             and not u.query and not u.fragment, 'download URI outside official Ubuntu allowlist')
    cmd.run(download_argv(versions), 180)
    need(digest((work/'state/status').read_bytes()) == binding['files']['dpkg_status'], 'download command modified package status')
    debs = list((work/'cache/archives').glob('*.deb'))
    need(len(debs) == len(packages), 'missing/extra downloaded packages')
    by_hash = {file_digest(p): p for p in debs}
    for p in packages:
        archive = by_hash.get(p['sha256'])
        need(archive is not None and archive.stat().st_size == p['bytes'], 'signed package hash/size differs')
        fields = cmd.run(['/usr/bin/dpkg-deb', '--field', str(archive), 'Package', 'Version', 'Architecture'])
        actual = stanzas(fields)[0]
        need((actual['Package'], actual['Version'], actual['Architecture']) == (p['name'], p['version'], p['arch']),
             'artifact package identity differs from signed metadata')
    metadata_files = [p for p in (work/'state/lists').iterdir() if p.is_file() and p.name != 'lock']
    metadata_size = sum(p.stat().st_size for p in metadata_files)
    need(metadata_size <= args.max_metadata_bytes and shutil.disk_usage(output.parent).free > metadata_size + total,
         'output evidence copy exceeds measured free bytes')
    output.mkdir(mode=0o700)
    (output/'debs').mkdir(mode=0o700)
    (output/'evidence').mkdir(mode=0o700)
    for p in packages:
        shutil.copyfile(by_hash[p['sha256']], output/'debs'/p['file'])
    for p in metadata_files:
        shutil.copyfile(p, output/'evidence'/p.name)
    shutil.copyfile(work/'etc/ubuntu-key.gpg', output/'evidence/ubuntu-key.gpg')
    evidence = {'schema': 'podmesh.python-closure-evidence/v1', 'base_binding': binding,
                'base_binding_sha256': args.base_binding_sha256, 'sources': SOURCES,
                'packages': packages, 'copied_base_status_unchanged': True, 'host_install_or_unpack': False,
                'installed_size_KiB': sum(p['installedKiB'] for p in packages),
                'metadata': {p.name: {'sha256': file_digest(p), 'bytes': p.stat().st_size} for p in metadata_files}}
    encoded = (json.dumps(evidence, sort_keys=True, indent=2)+'\n').encode()
    (output/'evidence.json').write_bytes(encoded)
    lines = ['PODMESH_PYTHON_CLOSURE_V1', 'REGISTRY\t'+BASE['registry'], 'IMAGE\t'+BASE['image'],
             'ARCHIVE\t'+BASE['archive'], 'SUITE\tnoble', 'ARCH\tamd64',
             'STATUS\t'+binding['files']['dpkg_status'], 'OS_RELEASE\t'+binding['files']['os_release'],
             'KEY\t'+binding['files']['ubuntu_key'], 'BINDING\t'+args.base_binding_sha256, 'EVIDENCE\t'+digest(encoded)]
    for p in sorted(packages, key=lambda p: p['name']):
        lines.append('\t'.join(['PACKAGE', p['name'], p['version'], p['arch'], p['sha256'], str(p['bytes']), p['file'], str(p['installedKiB'])]))
    lines.append('END\t'+str(len(packages))+'\t'+str(total))
    (output/'closure.lock').write_text('\n'.join(lines)+'\n')
    lock_hash = digest((output/'closure.lock').read_bytes())
    verify_bundle(output, lock_hash)
    return {'lockSHA256': lock_hash, 'packages': len(packages), 'downloadBytes': total,
            'output': str(output), 'workRetained': str(work), 'hostInstall': False}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('base-root', 'base-binding', 'work', 'output'):
        p.add_argument('--'+name, type=Path, required=True)
    p.add_argument('--base-binding-sha256', required=True)
    p.add_argument('--max-download-bytes', type=int, default=128*1024*1024)
    p.add_argument('--max-metadata-bytes', type=int, default=512*1024*1024)
    args = p.parse_args()
    os.umask(0o077)
    print(json.dumps(collect(args)))


if __name__ == '__main__':
    main()
