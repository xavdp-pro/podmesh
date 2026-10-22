#!/usr/bin/env python3
"""Collect a replayable signed Ubuntu Python closure without host installation."""
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


def read_bounded(path, maximum):
    before=path.lstat()
    need(stat.S_ISREG(before.st_mode) and before.st_size<=maximum, 'regular input size bound')
    fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
    try:
        actual=os.fstat(fd)
        need((before.st_dev,before.st_ino)==(actual.st_dev,actual.st_ino), 'input replaced')
        with os.fdopen(os.dup(fd),'rb') as stream:data=stream.read(maximum+1)
        after=os.fstat(fd)
        need(len(data)<=maximum and (actual.st_size,actual.st_ctime_ns)==(after.st_size,after.st_ctime_ns), 'input changed/oversized')
        return data
    finally:os.close(fd)


def bounded_listing(path, maximum):
    items=[]
    for item in path.iterdir():
        need(len(items)<maximum, 'unexpected artifact count')
        need(not item.is_symlink(), 'symlink in bundle')
        items.append(item)
    return items


def signature_signers(status, fingerprints):
    need(not any(('] '+bad+' ') in status for bad in ('BADSIG','ERRSIG','NO_PUBKEY','EXPKEYSIG','REVKEYSIG','EXPSIG')),
         'unsupported/bad/unknown signature status')
    signers=[]
    for line in status.splitlines():
        if not line.startswith('[GNUPG:] VALIDSIG '):continue
        fields=line.split()[2:]
        need(len(fields) in (9,10), 'unsupported VALIDSIG status format')
        signing=fields[0];primary=fields[9] if len(fields)==10 else signing
        need(re.fullmatch('[0-9A-F]{40}',signing) and primary in fingerprints, 'unbound archive primary signer')
        signers.append({'signing':signing,'primary':primary})
    need(signers, 'no valid bound archive signature')
    return sorted(signers,key=lambda s:(s['primary'],s['signing']))


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
                canonical={n.lower():n for n in ('Package','Version','Architecture','SHA256','Size','Installed-Size','Filename','Origin','Codename','Suite','Components','Architectures','Status')}
                key=canonical.get(key.lower(),key)
                need(bool(separator) and key.lower() not in {k.lower() for k in item}, 'unsupported/ambiguous Debian metadata')
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
    fingerprints=b.get('ubuntu_archive_fingerprints', [])
    need(fingerprints and len(fingerprints)<=16 and all(re.fullmatch('[0-9A-F]{40}',f) for f in fingerprints),
         'externally verified Ubuntu archive fingerprints required before collection')
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
              'Acquire::https::AllowRedirect "false";', 'Acquire::https::Proxy "DIRECT";', 'Acquire::http::Proxy "DIRECT";', 'Acquire::GzipIndexes "false";', 'Acquire::IndexTargets::deb::Packages::KeepCompressed "false";', 'Acquire::Retries "0";',
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
        need(':' not in match[1] or match[1].endswith(':amd64'), 'resolved architecture drift')
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


def source_specs():
    return [{'uri': uri, 'suite': suite, 'component': 'main', 'arch': 'amd64',
             'metakey': 'main/binary-amd64/Packages',
             'index_uri': uri+'/dists/'+suite+'/main/binary-amd64/Packages',
             'release_cache': urlsplit(uri).netloc+'_ubuntu_dists_'+suite+'_InRelease'}
            for uri, suite in SOURCES]


def bounded_copy(source, destination, maximum):
    need(not source.is_symlink() and source.is_file(), 'metadata source is not a regular file')
    before = source.stat()
    need(before.st_size <= maximum, 'metadata size bound')
    fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        opened = os.fstat(fd)
        need(stat.S_ISREG(opened.st_mode) and (opened.st_dev, opened.st_ino) == (before.st_dev, before.st_ino),
             'metadata replaced before snapshot')
        with os.fdopen(os.dup(fd), 'rb') as reader, destination.open('xb') as writer:
            size = 0
            while True:
                data = reader.read(1024*1024)
                if not data:
                    break
                size += len(data)
                need(size <= maximum, 'metadata grew past bound')
                writer.write(data)
        after = os.fstat(fd)
        need((before.st_size, before.st_ctime_ns) == (after.st_size, after.st_ctime_ns), 'metadata changed during snapshot')
    finally:
        os.close(fd)


def verify_signed_release(signed_path, key_path, fingerprints):
    """Offline crypto replay. Tests may mock this function, never a CLI bypass."""
    import tempfile
    need(fingerprints and all(re.fullmatch('[0-9A-F]{40}', f) for f in fingerprints), 'archive fingerprints missing')
    need(signed_path.stat().st_size <= 8*1024*1024 and key_path.stat().st_size <= 4*1024*1024,
         'signature/key size bound')
    with tempfile.TemporaryDirectory(prefix='podmesh-gpgv-', dir='/tmp') as temporary:
        temporary = Path(temporary)
        plaintext = temporary/'Release'
        with (temporary/'status').open('xb') as status:
            def limits():
                resource.setrlimit(resource.RLIMIT_FSIZE, (8*1024*1024, 8*1024*1024))
            completed = subprocess.run(['/usr/bin/gpgv', '--homedir', str(temporary), '--status-fd', '1',
                           '--output', str(plaintext), '--keyring', str(key_path), str(signed_path)],
                          env={'PATH':'/usr/bin:/bin','HOME':str(temporary),'LC_ALL':'C'},
                          stdin=subprocess.DEVNULL, stdout=status, stderr=subprocess.DEVNULL,
                          timeout=15, check=False, preexec_fn=limits)
        need(completed.returncode == 0, 'retained signature verification refused')
        status = (temporary/'status').read_text()
        signers = signature_signers(status, fingerprints)
        need(plaintext.is_file() and plaintext.stat().st_size <= 8*1024*1024, 'verified plaintext missing/oversized')
        raw = plaintext.read_bytes()
    records = stanzas(raw.decode('utf-8'))
    need(len(records) == 1, 'ambiguous verified Release document')
    return records[0], signers, digest(raw)


def release_index(release, spec):
    need(release.get('Origin') == 'Ubuntu' and release.get('Codename') == 'noble'
         and release.get('Suite') == spec['suite'] and 'amd64' in release.get('Architectures','').split()
         and 'main' in release.get('Components','').split(), 'signed Release source/suite/architecture mismatch')
    entries = {}
    for line in release.get('SHA256', '').splitlines():
        if not line.strip():
            continue
        fields = line.split()
        need(len(fields) == 3 and SHA.fullmatch(fields[0]) and fields[1].isdigit(), 'unsupported signed SHA256 entry')
        name = fields[2]
        need(name not in entries and not PurePosixPath(name).is_absolute() and '..' not in PurePosixPath(name).parts,
             'ambiguous or unsafe signed index path')
        entries[name] = {'sha256':fields[0], 'bytes':int(fields[1])}
    need(spec['metakey'] in entries, 'uncompressed Packages absent from signed Release')
    return entries[spec['metakey']]


def index_targets(text, lists):
    specs = {p['index_uri']:p for p in source_specs()}
    targets = {}
    for target in stanzas(text):
        # deb822 field names are case-insensitive; reject duplicates differing in case.
        lower = {k.lower():v for k,v in target.items()}
        need(len(lower) == len(target), 'ambiguous index target fields')
        uri = lower.get('uri')
        need(uri in specs and uri not in targets, 'extra/duplicate/wrong-source Packages target')
        spec = specs[uri]
        need(lower.get('metakey') == spec['metakey'] and lower.get('identifier') == 'Packages'
             and lower.get('architecture') == 'amd64' and lower.get('component') == 'main'
             and lower.get('suite') == spec['suite'] and lower.get('codename') == 'noble'
             and lower.get('origin') == 'Ubuntu' and lower.get('trusted') == 'yes'
             and lower.get('keepcompressed') == 'no' and lower.get('release') == spec['suite'], 'index target binding differs')
        filename = Path(lower.get('filename', ''))
        need(filename.is_absolute() and filename.is_relative_to(lists) and filename.parent == lists
             and not filename.is_symlink() and filename.name.endswith('_Packages')
             and not any(os.path.lexists(str(filename)+ext) for ext in ('.lz4','.gz','.xz','.zst','.bz2','.lzma')),
             'unsupported compressed/escaped index filename')
        need(filename not in [p['filename'] for p in targets.values()], 'two sources alias one index file')
        targets[uri] = {'spec':spec, 'filename':filename}
    need(set(targets) == set(specs), 'missing expected Packages index')
    return targets


def authenticated_indices(work, target_text, fingerprints, maximum):
    lists = work/'state/lists'
    targets = index_targets(target_text, lists)
    chain_root = work/'authenticated'
    chain_root.mkdir(mode=0o700)
    key = chain_root/'ubuntu-key.gpg'
    bounded_copy(work/'etc/ubuntu-key.gpg', key, 4*1024*1024)
    chains = []
    expected_release_files = {spec['release_cache'] for spec in source_specs()}
    need({p.name for p in lists.glob('*_InRelease')} == expected_release_files, 'extra/missing/duplicate signed Release source')
    used = 0
    for position, spec in enumerate(source_specs()):
        signed = chain_root/f'{position}-InRelease'
        index = chain_root/f'{position}-Packages'
        bounded_copy(lists/spec['release_cache'], signed, 8*1024*1024)
        release, signers, plaintext_hash = verify_signed_release(signed, key, fingerprints)
        expected = release_index(release, spec)
        used += expected['bytes']
        need(used <= maximum, 'authenticated index aggregate size bound')
        bounded_copy(targets[spec['index_uri']]['filename'], index, expected['bytes'])
        need(index.stat().st_size == expected['bytes'] and file_digest(index) == expected['sha256'],
             'actual uncompressed Packages hash/size differs from signed Release')
        chains.append({'source':spec, 'signed_file':signed.name,'signed_sha256':file_digest(signed),
                       'verified_plaintext_sha256':plaintext_hash,'signers':signers,
                       'index_file':index.name,'index':expected,'encoding':'uncompressed-utf8-deb822'})
    return chain_root, chains


def index_stanzas(path):
    """Bound one control stanza/line in memory; never load a whole Packages index."""
    block = []
    size = 0
    with path.open('rb') as stream:
        for line in iter(lambda:stream.readline(65537), b''):
            need(len(line) <= 65536 and b'\x00' not in line, 'unsupported Packages line encoding/size')
            if line.strip() == b'':
                if block:
                    raw = b''.join(block)
                    records = stanzas(raw.decode('utf-8'))
                    need(len(records) == 1 and 'Package' in records[0], 'unsupported Packages stanza')
                    yield raw, records[0]
                block, size = [], 0
            else:
                size += len(line)
                need(size <= 256*1024, 'Packages stanza size bound')
                block.append(line)
        if block:
            raw = b''.join(block)
            records = stanzas(raw.decode('utf-8'))
            need(len(records) == 1 and 'Package' in records[0], 'unsupported final Packages stanza')
            yield raw, records[0]


def selected_stanzas(chain_root, chains, versions):
    wanted = dict(versions)
    need(len(wanted) == len(versions), 'duplicate resolved package')
    selected = {}
    retained_stanza_bytes = 0
    for chain in chains:
        seen = set()
        for raw, stanza in index_stanzas(chain_root/chain['index_file']):
            name, version = stanza.get('Package'), stanza.get('Version')
            if name not in wanted or wanted[name] != version:
                continue
            key = (name, version, stanza.get('Architecture'))
            need(key not in seen, 'duplicate selected stanza in one authenticated index')
            seen.add(key)
            retained_stanza_bytes += len(raw)
            need(retained_stanza_bytes <= 4*1024*1024, 'selected stanza aggregate size bound')
            candidate = package_record(raw.decode('utf-8'), name, version)
            origin = {'index_file':chain['index_file'],'index_uri':chain['source']['index_uri'],
                      'stanza_sha256':digest(raw),'stanza':raw.decode('utf-8')}
            identity = tuple(candidate[k] for k in ('name','version','arch','sha256','bytes','installedKiB','archivePath'))
            if name in selected:
                prior = selected[name]
                need(identity == tuple(prior[k] for k in ('name','version','arch','sha256','bytes','installedKiB','archivePath')),
                     'ambiguous selected package/version/artifact between authenticated indices')
                # Identical artifact may be mirrored in updates/security: retain ALL proofs.
                prior['index_candidates'].append(origin)
            else:
                candidate['index_candidates'] = [origin]
                selected[name] = candidate
    need(set(selected) == set(wanted), 'resolved package/version absent from authenticated Packages')
    return [selected[name] for name in sorted(selected)]


def replay_chain(evidence_root, evidence, versions, binding):
    chains = evidence.get('authenticated_indices', [])
    expected_specs = source_specs()
    need(len(chains) == len(expected_specs) and [c.get('source') for c in chains] == expected_specs,
         'offline chain has missing/duplicate/wrong source')
    key = evidence_root/'ubuntu-key.gpg'
    need(key.stat().st_size <= 4*1024*1024, 'retained key size bound')
    need(file_digest(key) == binding['files']['ubuntu_key'], 'retained archive key binding differs')
    fingerprints = binding.get('ubuntu_archive_fingerprints', [])
    names = set()
    total = 0
    expected_files = {'ubuntu-key.gpg','base-binding.json'} | {c.get(f, '') for c in chains for f in ('signed_file','index_file')}
    need({p.name for p in evidence_root.iterdir()} == expected_files, 'extra/missing retained chain artifacts')
    for chain, spec in zip(chains, expected_specs):
        for field in ('signed_file','index_file'):
            name = chain.get(field, '')
            need(re.fullmatch(r'[0-9]+-(?:InRelease|Packages)', name) and name not in names, 'unsafe/duplicate retained chain file')
            names.add(name)
        need(chain.get('encoding') == 'uncompressed-utf8-deb822', 'unsupported retained index encoding')
        signed, index = evidence_root/chain['signed_file'], evidence_root/chain['index_file']
        need(not signed.is_symlink() and not index.is_symlink() and signed.stat().st_size <= 8*1024*1024 and file_digest(signed) == chain['signed_sha256'],
             'retained signed Release bytes differ')
        release, signers, plaintext_hash = verify_signed_release(signed, key, fingerprints)
        need(plaintext_hash == chain['verified_plaintext_sha256'] and signers == chain['signers'], 'retained signature/plaintext evidence differs')
        expected = release_index(release, spec)
        total += expected['bytes']
        need(total <= 768*1024*1024, 'offline aggregate index size bound')
        need(expected == chain['index'] and expected['bytes'] <= 768*1024*1024
             and index.stat().st_size == expected['bytes'] and file_digest(index) == expected['sha256'],
             'offline index no longer matches authenticated Release')
    return selected_stanzas(evidence_root, chains, versions)


def verify_bundle(output, expected_hash):
    need(output.is_dir() and not output.is_symlink(), 'bundle root differs')
    top={p.name for p in bounded_listing(output,5)}
    need(top in ({'closure.lock','evidence.json','debs','evidence'}, {'closure.lock','evidence.json','debs','evidence','installed.lock.sha256'}), 'unexpected bundle object')
    for directory,limit in [('debs',64),('evidence',8)]:
        need((output/directory).is_dir(), 'bundle directory missing')
        need(all(p.is_file() for p in bounded_listing(output/directory,limit)), 'non-regular artifact')
    if 'installed.lock.sha256' in top:
        need(read_bounded(output/'installed.lock.sha256',128).decode().strip()==expected_hash, 'installed lock marker differs')
    raw = read_bounded(output/'closure.lock',131072)
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
    evidence_bytes=read_bounded(output/'evidence.json',16*1024*1024)
    need(digest(evidence_bytes)==fields['EVIDENCE'], 'evidence hash differs')
    evidence=json.loads(evidence_bytes)
    need(evidence.get('schema') == 'podmesh.python-closure-evidence/v2', 'authenticated evidence chain missing')
    binding_file = output/'evidence/base-binding.json'
    binding_bytes=read_bounded(binding_file,1024*1024)
    need(digest(binding_bytes)==fields['BINDING'], 'external retained binding hash differs')
    binding=json.loads(binding_bytes)
    need(binding.get('schema') == 'podmesh.base-root-binding/v1' and binding.get('base') == BASE
         and binding.get('suite') == 'noble' and binding.get('arch') == 'amd64'
         and binding.get('verification') == 'oci-config-diffids-checked'
         and binding.get('files', {}).get('ubuntu_key') == fields['KEY']
         and binding['files'].get('os_release') == fields['OS_RELEASE']
         and binding['files'].get('dpkg_status') == fields['STATUS']
         and evidence.get('base_binding') == binding and evidence.get('base_binding_sha256') == fields['BINDING'],
         'retained base/key/suite binding differs')
    selected = replay_chain(output/'evidence', evidence, [(p[1], p[2]) for p in packages], binding)
    need(selected == evidence.get('packages'), 'retained selected stanzas/provenance changed')
    for row, package in zip(packages, selected):
        need(row[1:] == [package['name'],package['version'],package['arch'],package['sha256'],str(package['bytes']),package['file'],str(package['installedKiB'])],
             'closure row differs from authenticated package stanza')
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
            need('update' in args or '--simulate' in args or '--download-only' in args or '--print-uris' in args or 'indextargets' in args, 'host install prohibited')
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
    binding_bytes=read_bounded(binding_path,1024*1024)
    binding, data, base = verify_base(root, binding_bytes, args.base_binding_sha256)
    need(0 < args.max_download_bytes <= 256*1024*1024 and 0 < args.max_metadata_bytes <= 768*1024*1024,
         'invalid finite byte budgets')
    need(shutil.disk_usage(work.parent).free > 2*args.max_metadata_bytes + 2*args.max_download_bytes,
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
    sources = ''.join(f'Types: deb\nURIs: {uri}\nSuites: {suite}\nComponents: main\nTargets: Packages\nArchitectures: amd64\nSigned-By: {work}/etc/ubuntu-key.gpg\n\n' for uri, suite in SOURCES)
    (work/'etc/sources.d/ubuntu.sources').write_text(sources)
    # Named-user ACLs change only NEW work files, never host ancestors/config.
    subprocess.run(['/usr/bin/setfacl','-R','-m',f'u:{sandbox.pw_uid}:r-X',str(work)],
                   env={'PATH':'/usr/bin:/bin','LC_ALL':'C'},check=True,timeout=10)
    for partial in ('state/lists/partial','cache/archives/partial'):
        os.chown(work/partial,sandbox.pw_uid,sandbox.pw_gid)
    cmd = Commands(work, 2*args.max_metadata_bytes + 2*args.max_download_bytes)
    cmd.run(['/usr/bin/apt-get', 'update'], 180)
    target_text = cmd.run(['/usr/bin/apt-get', 'indextargets', 'Identifier: Packages', 'Architecture: amd64', 'Component: main'])
    fingerprints = binding.get('ubuntu_archive_fingerprints', [])
    chain_root, chains = authenticated_indices(work, target_text, fingerprints, args.max_metadata_bytes)
    plan_text = cmd.run(['/usr/bin/apt-get', '--simulate', '--no-upgrade', '--no-remove', '--no-install-recommends',
                         'install', *TARGETS])
    versions = planned(plan_text, base)
    # Package identity/hash/path comes ONLY from matched signed index bytes.
    # apt-cache self-report is not an authentication authority.
    packages = selected_stanzas(chain_root, chains, versions)
    total = sum(p['bytes'] for p in packages)
    need(total <= args.max_download_bytes and total < shutil.disk_usage(work).free, 'planned download exceeds measured budget')
    need(digest((work/'state/status').read_bytes()) == binding['files']['dpkg_status'], 'base status changed before download')
    uri_text = cmd.run(['/usr/bin/apt-get', '--print-uris', '--no-upgrade', '--no-remove', '--no-install-recommends',
                        '--yes', 'install', *[n+'='+v for n, v in versions]])
    uris = re.findall(r"^'([^']+)'", uri_text, re.M)
    need(len(uris) == len(packages), 'unsupported URI resolution')
    matched_packages = set()
    for uri in uris:
        u = urlsplit(uri)
        need(u.scheme == 'https' and u.netloc in ('archive.ubuntu.com', 'security.ubuntu.com')
             and u.path.startswith('/ubuntu/pool/') and '..' not in PurePosixPath(unquote(u.path)).parts
             and not u.query and not u.fragment, 'download URI outside official Ubuntu allowlist')
        matches = []
        for package in packages:
            allowed = [origin['index_uri'].split('/dists/',1)[0]+'/'+package['archivePath'] for origin in package['index_candidates']]
            if any((urlsplit(candidate).netloc,unquote(urlsplit(candidate).path)) == (u.netloc,unquote(u.path)) for candidate in allowed):
                matches.append(package['name'])
        need(len(matches)==1 and matches[0] not in matched_packages, 'download URI is not one exact authenticated package/source')
        matched_packages.add(matches[0])
    need(matched_packages == {p['name'] for p in packages}, 'incomplete authenticated download plan')
    cmd.run(download_argv(versions), 180)
    need(digest((work/'state/status').read_bytes()) == binding['files']['dpkg_status'], 'download command modified package status')
    debs = list((work/'cache/archives').glob('*.deb'))
    need(len(debs) == len(packages), 'missing/extra downloaded packages')
    secure_debs = work/'authenticated-debs'
    secure_debs.mkdir(mode=0o700)
    by_hash = {}
    for position, archive in enumerate(debs):
        snapshot = secure_debs/(str(position)+'.deb')
        bounded_copy(archive, snapshot, args.max_download_bytes)
        checksum = file_digest(snapshot)
        need(checksum not in by_hash, 'duplicate downloaded artifact identity')
        by_hash[checksum] = snapshot
    for p in packages:
        archive = by_hash.get(p['sha256'])
        need(archive is not None and archive.stat().st_size == p['bytes'], 'signed package hash/size differs')
        fields = cmd.run(['/usr/bin/dpkg-deb', '--field', str(archive), 'Package', 'Version', 'Architecture'])
        actual = stanzas(fields)[0]
        need((actual['Package'], actual['Version'], actual['Architecture']) == (p['name'], p['version'], p['arch']),
             'artifact package identity differs from signed metadata')
    metadata_files = [p for p in chain_root.iterdir() if p.is_file()]
    metadata_size = sum(p.stat().st_size for p in metadata_files)
    need(metadata_size <= args.max_metadata_bytes + 28*1024*1024
         and shutil.disk_usage(output.parent).free > metadata_size + total, 'output evidence copy exceeds measured free bytes')
    output.mkdir(mode=0o700)
    (output/'debs').mkdir(mode=0o700)
    (output/'evidence').mkdir(mode=0o700)
    for package in packages:
        bounded_copy(by_hash[package['sha256']], output/'debs'/package['file'], package['bytes'])
    for path in metadata_files:
        bounded_copy(path, output/'evidence'/path.name, args.max_metadata_bytes)
    # Preserve the original external binding bytes, including its actual hash.
    (output/'evidence/base-binding.json').write_bytes(binding_bytes)
    evidence = {'schema': 'podmesh.python-closure-evidence/v2', 'base_binding': binding,
                'base_binding_sha256': args.base_binding_sha256, 'sources': SOURCES,
                'packages': packages, 'authenticated_indices':chains,
                'copied_base_status_unchanged': True, 'host_install_or_unpack': False,
                'installed_size_KiB': sum(p['installedKiB'] for p in packages),
                'metadata': {p.name: {'sha256': file_digest(p), 'bytes':p.stat().st_size} for p in metadata_files}}
    encoded = (json.dumps(evidence, sort_keys=True, indent=2)+'\n').encode()
    need(len(encoded) <= 16*1024*1024, 'evidence serialization size bound')
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
