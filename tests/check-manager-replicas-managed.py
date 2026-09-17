#!/usr/bin/env python3
"""Step 4 of M-U2: three manager universes run concurrently on the managed network, one logical
manager, three replicas, three owned scopes, explicit authenticated endpoints -- and their facts
converge, proven from outside while all three keep running.

Environment: PODMESH_SOURCE_SSH / PODMESH_DESTINATION_SSH / PODMESH_THIRD_SSH (lab-a, lab-b,
lab-c), the transient service variables, PODMESH_MANAGER_CANDIDATE (the musl inspector on the
workstation, byte-equal to the binary in the images), PODMESH_REPLICA_SET (the private
replica-set.json the generator wrote), PODMESH_NETWORK_PREFIX (default 10.86.0.0/16) and
PODMESH_LAB_HOSTS "lab-a=addr,lab-b=addr,lab-c=addr" (the hosts' on-link addresses, for the
peer-pool routes; never in the repository). Images `localhost/podmesh-manager-universe:m-u2-<alias>`
must already be built on each host from that alias's configuration.

What is verified, from the hosts and never from the daemon's tables: each host's bridge and peer
routes; each replica's effective address; and, with all three running, each replica's store
copied out with `podman cp` and inspected by the frozen inspector -- the fact set must be the
same on all three, hold exactly three facts (one boot fact per scope), and every replica must
carry authenticated import receipts from both peers and audit rows for real exchanges. Cleanup
returns every host's routes and networks to their initial state.
"""
import io, json, os, sys, tarfile, tempfile, time, uuid, hashlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, secrets_for  # noqa: E402  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
CANDIDATE = os.environ['PODMESH_MANAGER_CANDIDATE']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
control = tempfile.mkdtemp(prefix='podmesh-mu2-')
targets = {'lab-a': os.environ['PODMESH_SOURCE_SSH'], 'lab-b': os.environ['PODMESH_DESTINATION_SSH'], 'lab-c': os.environ['PODMESH_THIRD_SSH']}
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
reference = 'disposable-lab-m-u2'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
checks, report = [], {'hosts': {a: h.identity for a, h in hosts.items()}, 'logical_manager_id': replica_set['logical_manager_id']}
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
for r in replica_set['replicas']:
    assert hosts[r['alias']].identity == r['host_id'], f"the replica set binds {r['alias']} to another host UUID than the one running there"

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def routes(h):
    return h.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def networks(h):
    return sorted(h.call('podman_run', args=['network', 'ls', '--format', '{{.Name}}'])['stdout'].split())

def image_on(h, alias):
    tag = f'localhost/podmesh-manager-universe:m-u2-{alias}'
    return next(l.split()[0] for l in h.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines() if l.endswith(' ' + tag))

def inspect_running(h, u):
    """The store of a RUNNING replica, copied out of the container with podman cp (the WAL with it),
    fetched, inspected on the workstation by the frozen inspector. The binary in the image is
    attested against the inspector first, from the same copy."""
    d = tempfile.mkdtemp(prefix='podmesh-mu2-store-'); os.chmod(d, 0o700)
    remote = h.ssh(f'sudo -n mktemp -d').stdout.decode().strip()
    # The store is copied while the resident runs: SQLite's -shm/-wal files come, go and grow between
    # the copier's listing and its read, so a copy the copier could not complete is retried, never trusted.
    for attempt in range(5):
        try:
            h.ssh(f'sudo -n podman cp podmesh-{u}:/var/lib/podmesh-manager {remote}/state && sudo -n podman cp podmesh-{u}:/etc/podmesh-manager/config.json {remote}/config.json && sudo -n podman cp podmesh-{u}:/usr/lib/podmesh-manager/podmesh-managerd {remote}/podmesh-managerd')
            break
        except RuntimeError as e:
            if attempt == 4 or 'copying from container' not in str(e):
                raise
            h.ssh(f'sudo -n rm -rf {remote}/state {remote}/config.json {remote}/podmesh-managerd'); time.sleep(1)
    tar = h.ssh(f'sudo -n tar -C {remote} -cf - .').stdout
    h.ssh(f'sudo -n rm -rf {remote}')
    with tarfile.open(fileobj=io.BytesIO(tar)) as t:
        for m in t.getmembers():
            if m.isfile() and not m.name.startswith('/') and '..' not in m.name.split('/'):
                target = os.path.join(d, m.name)
                os.makedirs(os.path.dirname(target), exist_ok=True)
                with open(target, 'wb') as f:
                    f.write(t.extractfile(m).read())
    in_image = hashlib.sha256(open(os.path.join(d, 'podmesh-managerd'), 'rb').read()).hexdigest()
    inspector = hashlib.sha256(open(CANDIDATE, 'rb').read()).hexdigest()
    assert in_image == inspector, f'binary in the universe {in_image} is not the inspector {inspector}'
    config = json.load(open(os.path.join(d, 'config.json')))
    config['network']['database_path'] = os.path.join(d, 'state', 'manager.sqlite')
    config['control_socket'] = os.path.join(d, 'control.sock')
    for root, _, files in os.walk(d):
        for f in files:
            os.chmod(os.path.join(root, f), 0o600)
    with open(os.path.join(d, 'config.json'), 'w') as f:
        json.dump(config, f)
    os.chmod(os.path.join(d, 'config.json'), 0o600)
    import subprocess
    p = subprocess.run([CANDIDATE, '--inspect-store', '--config', os.path.join(d, 'config.json'), '--state-dir', os.path.join(d, 'state')], capture_output=True, text=True)
    assert p.returncode == 0, ('inspect-store', p.stderr[-400:])
    i = json.loads(p.stdout)
    facts = sorted(json.dumps(f, sort_keys=True) for f in i['ordered_facts'])
    return {'logical_history_sha256': i['logical_history_sha256'], 'history_count': i['history_count'], 'receipt_count': i['receipt_count'],
            'audit_event_count': i['audit_event_count'], 'integrity': i['sqlite_integrity_result'], 'incomplete': len(i['incomplete_attempts']),
            'scopes': sorted({f['scope'] for f in i['ordered_facts']}), 'origins': len({f['origin_replica_id'] for f in i['ordered_facts']}),
            'imports': sum(1 for r in i['ordered_receipts'] if r.get('kind') == 'authenticated_import'),
            'import_sources': len({r.get('source_replica_id') for r in i['ordered_receipts'] if r.get('kind') == 'authenticated_import'}),
            'fact_set_sha256': hashlib.sha256('\n'.join(facts).encode()).hexdigest()}

initial = {a: {'routes': routes(h), 'networks': networks(h)} for a, h in hosts.items()}
for a, h in hosts.items():
    assert 'podmesh-managed' not in initial[a]['networks'], f'{a} already carries the bridge'
universes = {a: str(uuid.uuid4()) for a in hosts}
declared = set()
try:
    # 1. the managed network on every host: its own pool, the two others routed through their hosts
    for a, h in hosts.items():
        peers = [{'pool': POOLS[o], 'via': lab_hosts[o]} for o in hosts if o != a]
        d = h.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[a], peer_pools=peers))
        declared.add(a)
        assert d['declaration']['state'] == 'effective' and all(p['effective'] for p in d['effective']['peer_pool_routes']), d
    checks.append('the managed network declared on three hosts: one bridge per pool, two peer routes each, all effective')
    report['network'] = {a: hosts[a].ok(hostwide('network_status'))['effective'] for a in hosts}

    # 2. three replicas created at the addresses their configurations name, and started together
    for a, h in hosts.items():
        declare_replica_config(h, a, reference, state_dir)
        c = replica_create(h, universes[a], a, reference, addresses[a], request)
        assert c['network']['requested']['ip'] == addresses[a], c['network']
    for a, h in hosts.items():
        s = h.ok(request('start', universes[a], reference, observe_seconds=3))
        assert s['application_outcome'] == 'running_when_observed', (a, s)
        insp = json.loads(h.call('podman_run', args=['inspect', 'podmesh-' + universes[a]])['stdout'])[0]
        assert insp['NetworkSettings']['Networks']['podmesh-managed']['IPAddress'] == addresses[a], (a, insp['NetworkSettings']['Networks'])
    checks.append('three manager replicas running concurrently, each at its declared managed address, verified from Podman')

    # 3. convergence, from outside, with all three running: poll the stores until the fact sets agree
    deadline = time.time() + 240
    views = {}
    while time.time() < deadline:
        views = {a: inspect_running(h, universes[a]) for a, h in hosts.items()}
        digests = {v['fact_set_sha256'] for v in views.values()}
        if len(digests) == 1 and all(v['history_count'] == 3 for v in views.values()):
            break
        time.sleep(5)
    report['replicas'] = views
    assert len({v['fact_set_sha256'] for v in views.values()}) == 1, f'fact sets differ: {views}'
    assert all(v['history_count'] == 3 and v['origins'] == 3 and len(v['scopes']) == 3 for v in views.values()), views
    assert all(v['integrity'] == 'ok' for v in views.values())
    assert all(v['imports'] >= 2 and v['import_sources'] == 2 for v in views.values()), f'not every replica imported from both peers: {views}'
    assert all(v['audit_event_count'] > 0 for v in views.values()), 'no exchange audit rows: nothing was exchanged'
    assert all(hosts[a].call('podman_run', args=['inspect', '--format', '{{.State.Running}}', 'podmesh-' + universes[a]])['stdout'].strip() == 'true' for a in hosts)
    checks.append('facts converged on all three replicas while all three kept running: three boot facts, one per scope, byte-identical sets, authenticated imports from both peers on every replica, exchange audit rows present')
    checks.append('the digests are %s' % views['lab-a']['logical_history_sha256'][:16])
    print(json.dumps({'result': 'PASS', 'checks': checks, 'report': report,
                      'not_proven': ['agent access to the control API: the resident exposes none; the only writer inside is the entrypoint',
                                     'exclusive active manager role, takeover, partition, route withdrawal and publication across hosts: steps 5 and 6',
                                     'the source NAT of Podman\'s firewall on inter-host traffic: identity is the HMAC pair key, not the address']}, indent=2))
finally:
    for a, h in hosts.items():
        h.api(request('stop', universes[a], reference, timeout_seconds=15, on_timeout='kill'))
        h.api(request('delete', universes[a], reference))
        h.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + universes[a]], check=False)
        remove_replica_config(h, a, reference)
        if a in declared:
            r = h.api(hostwide('network_undeclare', network_uuid=NET))
            if not r.get('ok'):
                print(f'{a}: undeclare refused: {r.get("error")}', file=sys.stderr)
    for a, h in hosts.items():
        same = routes(h) == initial[a]['routes'] and networks(h) == initial[a]['networks']
        print(f'{a}: network state restored: {same}', file=sys.stderr)
