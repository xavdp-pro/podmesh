#!/usr/bin/env python3
"""The hard test of the publisher contract (item 6) and of the operator's decision 4: the governor's
host cut from its peer and from the agent while it keeps its Internet egress, so that its connector
stays reachable by Cloudflare; its lease lapses on its own clock; its own timer stops the
connector, removes the governor mark, withdraws the alias and the route -- BEFORE the standby,
which waited lease plus margin, publishes and starts its connector; and the public hostname then
answers with the standby's replica and the new epoch. After the dead man's switch reconnects the
old governor, its journal shows the timer's fence in that order and at a time before the standby's
start; it is superseded, follows the role, converges as a standby and its connector stays stopped.

Environment: PODMESH_PUBLISHER_HOSTS="<governor alias>=<ssh>,<standby alias>=<ssh>", the
transient service variables, PODMESH_CLI (the CLI beside the daemon on the governor's host),
PODMESH_MANAGER_CANDIDATE, PODMESH_REPLICA_SET, PODMESH_REPLICA_CONFIGS, PODMESH_LAB_HOSTS,
PODMESH_FENCING_LAB, PODMESH_TUNNEL_CREDENTIALS, PODMESH_TUNNEL_ID, PODMESH_PUBLIC_HOSTNAME.
The cut drops only the standby's host, its pool and the workstation: Cloudflare's edge stays
reachable. The dead man's switch is armed and verified BEFORE the cut.
"""
import io, json, os, pathlib, subprocess, sys, tarfile, tempfile, time, uuid, hashlib, urllib.request, urllib.error

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, prove_takeover  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
FENCE = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'packaging', 'podmesh-fence')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
CLI = os.environ['PODMESH_CLI']
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
TUNNEL_ID = os.environ['PODMESH_TUNNEL_ID']
HOSTNAME = os.environ['PODMESH_PUBLIC_HOSTNAME']
CREDENTIALS = open(os.environ['PODMESH_TUNNEL_CREDENTIALS'], 'rb').read()
control = tempfile.mkdtemp(prefix='podmesh-pubcut-')
targets = dict(kv.split('=', 1) for kv in os.environ['PODMESH_PUBLISHER_HOSTS'].split(','))
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
aliases = list(hosts)
G, S = aliases[0], aliases[1]
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
reference = 'disposable-lab-publisher-agent-cut'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
LOGICAL = replica_set['logical_manager_id']
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
replica_ids = {r['alias']: r['replica_id'] for r in replica_set['replicas']}
CREDENTIAL = 'cloudflare-tunnel-podmesh-lab'
TABLE = 'podmesh-lab-partition'
TIMER = 'podmesh-fence-lab'
LEASE, MARGIN, CUT_SECONDS = 20, 5, 120
checks = []

def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', reference, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def pub(operation, alias, **extra):
    return hosts[alias].api(dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, resource=LOGICAL, **extra))

def routes(h):
    return h.ssh('ip -4 route show').stdout.decode().strip().splitlines()

def networks(h):
    return sorted(h.call('podman_run', args=['network', 'ls', '--format', '{{.Name}}'])['stdout'].split())

def declare_credential(h):
    h.ssh(f'sudo -n mkdir -p -m 0700 {state_dir}/inbox/secrets && sudo -n install -m 0600 -o root -g root /dev/stdin {state_dir}/inbox/secrets/{CREDENTIAL}', input_bytes=CREDENTIALS)
    r = h.api(hostwide('secret_declare', name=CREDENTIAL, source=CREDENTIAL, replace=True))
    assert r.get('ok'), (h.role, r)

def gate_ready():
    if not os.path.exists(GATE):
        tool('gate', 'init')
    gate = fencing_lab.Authority(pathlib.Path(GATE))
    try:
        gate.inspect(LOGICAL)
    except fencing_lab.Refused:
        gate.declare(LOGICAL)
    seen = 0
    for h in hosts.values():
        s = h.api(request('activation_status', LOGICAL, reference))
        seen = max(seen, (s.get('data') or {}).get('highest_epoch_seen') or 0)
    while gate.inspect(LOGICAL)['epoch'] < seen:
        gate.transfer(LOGICAL, gate.inspect(LOGICAL)['epoch'], 'gate-recovery', 'gate-recovery')
    state = {'authority_id': gate.authority_id, 'epoch': gate.inspect(LOGICAL)['epoch']}
    gate.close()
    return state

def public_ready(seconds=90):
    deadline = time.time() + seconds
    last = None
    while time.time() < deadline:
        try:
            req = urllib.request.Request(f'https://{HOSTNAME}/ready', headers={'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) PodMesh-lab-check/1.0'})
            with urllib.request.urlopen(req, timeout=10) as r:
                return r.status, json.loads(r.read().decode())
        except urllib.error.HTTPError as e:
            last = (e.code, e.read().decode()[:200])
        except Exception as e:  # noqa: BLE001
            last = (None, str(e)[:200])
        time.sleep(3)
    return last

def wait_connector(alias, seconds=60):
    deadline = time.time() + seconds
    st = None
    while time.time() < deadline:
        st = pub('publisher_status', alias)['data']
        if st.get('connector_id') and st['unit']['state'] == 'active':
            return st
        time.sleep(3)
    return st

def inspect_running(h, u):
    d = tempfile.mkdtemp(prefix='podmesh-pubcut-store-'); os.chmod(d, 0o700)
    remote = h.ssh('sudo -n mktemp -d').stdout.decode().strip()
    for attempt in range(5):
        try:
            h.ssh(f'sudo -n podman cp podmesh-{u}:/var/lib/podmesh-manager {remote}/state && sudo -n podman cp podmesh-{u}:/etc/podmesh-manager/config.json {remote}/config.json')
            break
        except RuntimeError as e:
            if attempt == 4 or 'copying from container' not in str(e):
                raise
            h.ssh(f'sudo -n rm -rf {remote}/state {remote}/config.json'); time.sleep(1)
    tar = h.ssh(f'sudo -n tar -C {remote} -cf - .').stdout
    h.ssh(f'sudo -n rm -rf {remote}')
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
    p = subprocess.run([os.environ['PODMESH_MANAGER_CANDIDATE'], '--inspect-store', '--config', os.path.join(d, 'config.json'), '--state-dir', os.path.join(d, 'state')], capture_output=True, text=True)
    assert p.returncode == 0, ('inspect-store', p.stderr[-400:])
    i = json.loads(p.stdout)
    facts = sorted(json.dumps(f, sort_keys=True) for f in i['ordered_facts'])
    return {'history_count': i['history_count'], 'fact_set_sha256': hashlib.sha256('\n'.join(facts).encode()).hexdigest()}

def converged(expected_facts, among, seconds=240):
    deadline = time.time() + seconds
    views = {}
    while time.time() < deadline:
        views = {a: inspect_running(hosts[a], universes[a]) for a in among}
        if len({v['fact_set_sha256'] for v in views.values()}) == 1 and all(v['history_count'] == expected_facts for v in views.values()):
            return views
        time.sleep(5)
    raise AssertionError(f'not converged to {expected_facts} facts among {among}: {views}')

def cut(h, others, seconds):
    """The dead man's switch first, verified armed; then the cut in one nftables transaction whose
    SSH session is expected to die under it."""
    peers = ', '.join(others)
    h.ssh(f'sudo -n systemd-run --quiet --on-active={seconds} --unit=podmesh-lab-partition-deadman /usr/sbin/nft delete table inet {TABLE}')
    armed = h.ssh('systemctl is-active podmesh-lab-partition-deadman.timer', check=False).stdout.decode().strip()
    assert armed == 'active', f'the dead man\'s switch is not armed ({armed}); refusing to cut'
    ruleset = (f'table inet {TABLE} {{\n chain prerouting {{ type filter hook prerouting priority -300; ip saddr {{ {peers} }} drop }}\n'
               f' chain output {{ type filter hook output priority -300; ip daddr {{ {peers} }} drop }}\n}}\n')
    h.ssh(f'sudo -n install -m 0600 /dev/stdin /run/podmesh-lab-partition.nft', input_bytes=ruleset.encode())
    h.ssh(f'sudo -n sh -c "nohup nft -f /run/podmesh-lab-partition.nft >/dev/null 2>&1 &"', check=False)
    time.sleep(2)

def reconnect_cleanup(h):
    h.ssh(f'sudo -n nft delete table inet {TABLE}', check=False)
    h.ssh('sudo -n systemctl stop podmesh-lab-partition-deadman.timer podmesh-lab-partition-deadman.service 2>/dev/null; sudo -n systemctl reset-failed podmesh-lab-partition-deadman.service 2>/dev/null; sudo -n rm -f /run/podmesh-lab-partition.nft', check=False)

def is_cut(h):
    return TABLE in h.ssh('sudo -n nft list tables', check=False).stdout.decode()

def reset_master(h):
    subprocess.run(['ssh', '-o', f'ControlPath={h.control}/%C', '-O', 'exit', h.target], capture_output=True)

def timer_start(h, mandate):
    h.ssh('sudo -n install -D -m 0755 /dev/stdin /usr/local/lib/podmesh-fence-lab/podmesh-fence', input_bytes=open(FENCE, 'rb').read())
    h.ssh(f'sudo -n sh -c \'umask 077; printf "authorization_ref=mandate:lab-publisher-agent-cut\\ntimeout_seconds=5\\n" > {mandate}\'')
    h.ssh(f'sudo -n systemd-run --quiet --unit={TIMER} --on-active=1 --on-unit-active=2 --timer-property=AccuracySec=1s '
          f'--setenv=PODMESH_SOCKET={socket_path} --setenv=PODMESH_CLI={CLI} --setenv=PODMESH_FENCE_MANDATE={mandate} /usr/local/lib/podmesh-fence-lab/podmesh-fence')

def timer_stop(h, mandate):
    h.ssh(f'sudo -n systemctl stop {TIMER}.timer {TIMER}.service 2>/dev/null; sudo -n systemctl reset-failed {TIMER}.service {TIMER}.timer 2>/dev/null; sudo -n rm -rf {mandate} /usr/local/lib/podmesh-fence-lab', check=False)

A, B = hosts[G], hosts[S]
initial = {a: {'routes': routes(h), 'networks': networks(h)} for a, h in hosts.items()}
universes = {a: str(uuid.uuid4()) for a in hosts}
mandate = f'/run/podmesh-fence-lab-{uuid.uuid4()}'
declared = set()
assert not is_cut(A), f'the partition table already exists on {G}; refusing to run on top of it'
A.ssh(f'sudo -n systemctl stop {TIMER}.timer {TIMER}.service 2>/dev/null; sudo -n systemctl reset-failed {TIMER}.service {TIMER}.timer 2>/dev/null', check=False)
try:
    gate_state = gate_ready()
    for a, h in hosts.items():
        peers = [{'pool': POOLS[o], 'via': lab_hosts[o]} for o in POOLS if o != a]
        h.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[a], peer_pools=peers)); declared.add(a)
    for a, h in hosts.items():
        declare_replica_config(h, a, reference, state_dir)
        declare_credential(h)
        replica_create(h, universes[a], a, reference, addresses[a], request)
        started = h.ok(request('start', universes[a], reference, observe_seconds=3))
        assert started['application_outcome'] == 'running_when_observed', (started['application_outcome'], h.call('podman_run', args=['logs', 'podmesh-' + universes[a]], check=False))
    converged(len(aliases), aliases)
    rot = tool('rotate', '--universe', LOGICAL, '--host', targets[G], '--lease', str(LEASE), '--margin', str(MARGIN))
    B.ok(request('activation_require', LOGICAL, reference, lease_seconds=LEASE, takeover_margin_seconds=MARGIN, desired_standbys=1, authority_id=rot['permit']['authority_id']))
    e1 = rot['epoch']
    for a in aliases:
        assert pub('publisher_declare', a, hostname=HOSTNAME, tunnel_uuid=TUNNEL_ID, credential=CREDENTIAL, origin_port=8080).get('ok')
    A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses[G], exclusive_resource=LOGICAL))
    proof1, how = prove_takeover(rot, hosts, tool, request, reference, LOGICAL)
    assert pub('publisher_start', G, takeover_proof=proof1).get('ok')
    st = wait_connector(G)
    assert st['unit']['state'] == 'active' and st['connector_id'], st
    status, body = public_ready()
    assert status == 200 and body['replica_id'] == replica_ids[G] and body['epoch'] == e1, (status, body)
    timer_start(A, mandate)
    time.sleep(4)
    assert pub('publisher_status', G)['data']['unit']['state'] == 'active', 'the timer stopped a live governor\'s connector'
    lease_expires = A.ok(request('activation_status', LOGICAL, reference))['expires_at']
    checks.append(f'{G} the governor under epoch {e1}, its connector registered, the public hostname answering with its replica; its self-withdrawal timer running under a mandate; lease to expire at {lease_expires}')

    # the cut: the governor alone with the Internet -- the standby, its pool and the workstation dropped
    agent = A.ssh('echo $SSH_CLIENT').stdout.decode().split()[0]
    clock_g_at_cut = A.call('time')['time']
    cut(A, [lab_hosts[S], POOLS[S], agent], CUT_SECONDS)
    reset_master(A)
    checks.append(f'{G} cut from {S}, its pool and the agent for {CUT_SECONDS} s, its Internet egress kept; the dead man\'s switch armed first')
    wait = LEASE + MARGIN + 1
    time.sleep(wait)
    # by now the governor's own timer must have stopped its connector: the public hostname answers nothing usable
    status, body = public_ready(20)
    assert status != 200 or body.get('ready') is not True, f'the cut governor still publishes after its lease lapsed: {status} {body}'
    checks.append(f'{wait} s after the cut, without reaching {G}: the public hostname no longer answers ready ({status}) -- its own timer withdrew the connector')
    rot2 = tool('rotate', '--universe', LOGICAL, '--host', targets[S], '--lease', str(LEASE), '--margin', str(MARGIN))
    e2 = rot2['epoch']
    B.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses[S], exclusive_resource=LOGICAL))
    publish_at_s = B.call('time')['time']
    # the previous holder is out of reach: the authority's barrier, waited on this clock
    proof2 = rot2['takeover_proof']; assert proof2['method'] == 'lease_barrier', proof2
    refused_early = pub('publisher_start', S, takeover_proof=proof2)
    assert not refused_early.get('ok') and 'barrier' in refused_early['error'], refused_early
    while time.time() < proof2['eligible_after']:
        time.sleep(1)
    # the barrier (the previous lease plus the margin, from the rotation) outlasts a 20-second
    # lease: acquired again with the rotation's permit, idempotent for the holder, before starting
    B.ok(request('activation_acquire', LOGICAL, reference, permit=rot2['permit']))
    started_s = pub('publisher_start', S, takeover_proof=proof2, previous={'waited_seconds': wait})
    assert started_s.get('ok'), started_s
    st = wait_connector(S)
    assert st['unit']['state'] == 'active' and st['connector_id'], st
    deadline = time.time() + 120
    while time.time() < deadline:
        status, body = public_ready(30)
        if status == 200 and body.get('epoch') == e2 and body.get('replica_id') == replica_ids[S]:
            break
        time.sleep(3)
    assert status == 200 and body['epoch'] == e2 and body['replica_id'] == replica_ids[S], (status, body)
    pub('publisher_observed', S, observation={'hostname': HOSTNAME, 'status': status, 'body': body, 'from': 'workstation'})
    checks.append(f'the role rotated to {S} after the wait; it published and started its connector; the public hostname answers with {S}\'s replica and epoch {e2}')

    # the reconnection, by the dead man's switch only
    deadline = time.time() + CUT_SECONDS + 60
    while time.time() < deadline:
        try:
            reset_master(A)
            if not is_cut(A):
                break
        except Exception:
            pass
        time.sleep(10)
    assert not is_cut(A), f'{G} did not come back'
    reconnect_cleanup(A)
    log = A.ssh(f'sudo -n journalctl -u {TIMER}.service --no-pager -o short-unix --since @{int(clock_g_at_cut)}', check=False).stdout.decode()
    fence_lines = [l for l in log.splitlines() if '"publishers_withdrawn"' in l or '"withdrawn": true' in l]
    withdrawn_lines = [l for l in log.splitlines() if '"withdrawn": true' in l]
    assert withdrawn_lines, log[-2000:]
    withdrawn_at_g = float(withdrawn_lines[0].split()[0])
    st = pub('publisher_status', G)['data']
    assert st['unit']['state'] != 'active' and st['publisher_eligible'] is False and st['governor_mark'] in (False, None), st
    assert withdrawn_at_g < publish_at_s + 1.0, (withdrawn_at_g, publish_at_s)
    stops = [l for l in log.splitlines() if '"event": "fence"' in l or 'podmesh-publisher' in l]
    checks.append(f'{G}\'s own journal: its timer withdrew at {withdrawn_at_g} ({round(withdrawn_at_g - lease_expires, 1)} s after the lapse, {round(publish_at_s - withdrawn_at_g, 1)} s before {S} published); its connector inactive, no mark, not eligible')
    over = A.ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    assert over['superseded'] is True
    A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=lab_hosts[S]))
    converged(len(aliases), aliases)
    assert pub('publisher_status', G)['data']['unit']['state'] != 'active'
    checks.append(f'{G} reconnected: superseded, following the role, converged as a standby, its connector still stopped')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'hostname': HOSTNAME, 'epochs': [e1, e2], 'gate': gate_state,
                      'clocks': {'cut_at_g': clock_g_at_cut, 'lease_expires_g': lease_expires, 'withdrawn_at_g': withdrawn_at_g, 'publish_at_s': publish_at_s},
                      'not_proven': ['clocks that lie: the lapse on the governor\'s clock, the wait on the agent\'s, compared with a one-second allowance',
                                     'a wedged daemon on the cut side: lease-expiry self-withdrawal needs the daemon alive']}, indent=2))
finally:
    try:
        reset_master(A); reconnect_cleanup(A); timer_stop(A, mandate)
    except Exception as e:
        print(f'{G} cleanup: {e}', file=sys.stderr)
    for a, h in hosts.items():
        try:
            h.api(dict(operation='publisher_stop', operation_id=str(uuid.uuid4()), authorization_ref=reference, resource=LOGICAL))
            h.api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
            h.api(request('stop', universes[a], reference, timeout_seconds=15, on_timeout='kill'))
            h.api(request('delete', universes[a], reference))
            h.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + universes[a]], check=False)
            remove_replica_config(h, a, reference)
            h.api(hostwide('secret_remove', name=CREDENTIAL))
            if a in declared:
                r = h.api(hostwide('network_undeclare', network_uuid=NET))
                if not r.get('ok'):
                    print(f'{a}: undeclare refused: {r.get("error")}', file=sys.stderr)
        except Exception as e:
            print(f'{a} cleanup: {e}', file=sys.stderr)
    for a, h in hosts.items():
        left = h.ssh(f'systemctl is-active podmesh-publisher-{LOGICAL}.service', check=False).stdout.decode().strip()
        print(f'{a}: network state restored: {routes(h) == initial[a]["routes"] and networks(h) == initial[a]["networks"]}; cut: {is_cut(h)}; connector unit: {left}', file=sys.stderr)
