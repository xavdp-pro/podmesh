#!/usr/bin/env python3
"""The publishing connector follows the governor (docs/MANAGER-PUBLISHER-CONTRACT.md), measured
on two or three lab hosts (PODMESH_SOURCE_SSH is the first governor; PODMESH_DESTINATION_SSH the
standby that takes over; PODMESH_THIRD_SSH, optional, a second standby), with a real Cloudflare
tunnel: PODMESH_TUNNEL_CREDENTIALS (the private credentials JSON on the workstation),
PODMESH_TUNNEL_ID and PODMESH_PUBLIC_HOSTNAME. Same environment otherwise as the manager suites.

Verified from outside: the standby's connector is refused by the lease gate; the governor's start
is refused without an account of the previous publisher; started, its unit is active, cloudflared
registered a connection (its identity from the unit's journal), the origin at the service address
answers ready with the logical manager, the carrier's replica and the epoch, and an external
request through the public hostname answers the same; on the rotation the old governor's fence
stops its connector and removes its mark BEFORE its alias and route go (the fence's report, in
that order), its replica then answers 503 at its own address; the new governor publishes and
starts, the public hostname answers with the new epoch and the new replica; the old governor is no
longer eligible and says why. Cleanup stops the connector, withdraws everything and removes the
secrets; the laboratory tunnel and hostname stay as fixtures. The credential never leaves Podman's
store except into the connector's root-only runtime copy, removed with it.
"""
import io, json, os, pathlib, subprocess, sys, tarfile, tempfile, time, uuid, hashlib, urllib.request, urllib.error

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, prove_takeover  # noqa: E402

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
ORIGIN_PORT = 8080
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
lab_hosts = dict(kv.split('=') for kv in os.environ['PODMESH_LAB_HOSTS'].split(','))
TUNNEL_ID = os.environ['PODMESH_TUNNEL_ID']
HOSTNAME = os.environ['PODMESH_PUBLIC_HOSTNAME']
CREDENTIALS = open(os.environ['PODMESH_TUNNEL_CREDENTIALS'], 'rb').read()
control = tempfile.mkdtemp(prefix='podmesh-pub-')
# PODMESH_PUBLISHER_HOSTS="lab-b=lab@…,lab-c=lab@…" names the aliases in play and their order (the first is the
# first governor, the second takes over), so that a host out of reach can be left out; otherwise the usual three.
targets = (dict(kv.split('=', 1) for kv in os.environ['PODMESH_PUBLISHER_HOSTS'].split(',')) if os.environ.get('PODMESH_PUBLISHER_HOSTS')
           else {a: os.environ[v] for a, v in (('lab-a', 'PODMESH_SOURCE_SSH'), ('lab-b', 'PODMESH_DESTINATION_SSH'), ('lab-c', 'PODMESH_THIRD_SSH')) if os.environ.get(v)})
hosts = {alias: Host(alias, target, control, socket_path, state_dir, unit) for alias, target in targets.items()}
aliases = list(hosts)
G, S = aliases[0], aliases[1]
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
os.environ.update(PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
import importlib.util  # noqa: E402
_spec = importlib.util.spec_from_file_location('ha_standby', TOOL)
ha = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(ha)  # the tool's signing helpers: altered documents are re-signed by the gate's key so that each binding is what refuses
reference = 'disposable-lab-publisher'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
LOGICAL = replica_set['logical_manager_id']
addresses = {r['alias']: r['address'] for r in replica_set['replicas']}
replica_ids = {r['alias']: r['replica_id'] for r in replica_set['replicas']}
CREDENTIAL = 'cloudflare-tunnel-podmesh-lab'
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

LEASE_GATE_REASONS = ('none is held', 'held by another host', 'expired', 'superseded')

def refused(r, fragments, label):
    """Refused for one of the named reasons; which one is recorded, since a host that took part in an
    earlier run may hold an expired or superseded lease rather than none at all."""
    fragments = (fragments,) if isinstance(fragments, str) else fragments
    assert not r.get('ok'), (label, 'accepted, expected refusal', r)
    reason = next((f for f in fragments if f in json.dumps(r)), None)
    assert reason, (label, 'refused for another reason', r.get('error'))
    checks.append(f'refused ({reason}): {label}')

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

def resigned(document):
    """An altered document signed again by the gate's key (unchanged under PODMESH_HA_UNSIGNED=1)."""
    key = ha.signing_key(gate_state['authority_id'])
    return ha.sign(document, key) if key else document

def key_fields():
    key = ha.signing_key(gate_state['authority_id'])
    return {'authority_key': ha.public_hex(key)} if key else {}

def public_ready(seconds=90):
    """The public hostname asked from the workstation until it answers 200 with a JSON body, or the last error."""
    deadline = time.time() + seconds
    last = None
    while time.time() < deadline:
        try:
            # Cloudflare's edge answers 403 (error 1010) to a bare python User-Agent: the request carries a browser's.
            req = urllib.request.Request(f'https://{HOSTNAME}/ready', headers={'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) PodMesh-lab-check/1.0'})
            with urllib.request.urlopen(req, timeout=10) as r:
                return r.status, json.loads(r.read().decode())
        except urllib.error.HTTPError as e:
            last = (e.code, e.read().decode()[:200])
        except Exception as e:  # noqa: BLE001 -- what the edge answered is the observation
            last = (None, str(e)[:200])
        time.sleep(3)
    return last

def origin_at(h, ip):
    r = h.ssh(f'curl -s -m 5 -o /dev/stderr -w "%{{http_code}}" http://{ip}:{ORIGIN_PORT}/ready', check=False)
    return r.stdout.decode().strip(), r.stderr.decode()[-300:]

def wait_connector(alias, seconds=60):
    deadline = time.time() + seconds
    st = None
    while time.time() < deadline:
        st = pub('publisher_status', alias)['data']
        if st.get('connector_id') and st['unit']['state'] == 'active':
            return st
        time.sleep(3)
    return st

initial = {a: {'routes': routes(h), 'networks': networks(h)} for a, h in hosts.items()}
universes = {a: str(uuid.uuid4()) for a in hosts}
declared = set()
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
    rot = tool('rotate', '--universe', LOGICAL, '--host', targets[G], '--lease', '120', '--margin', '5')
    for other in aliases[1:]:
        hosts[other].ok(request('activation_require', LOGICAL, reference, lease_seconds=120, takeover_margin_seconds=5, desired_standbys=len(aliases) - 1, authority_id=rot['permit']['authority_id'], **key_fields()))
    e1 = rot['epoch']
    proof1, how = prove_takeover(rot, hosts, tool, request, reference, LOGICAL)
    for a in aliases:
        r = pub('publisher_declare', a, hostname=HOSTNAME, tunnel_uuid=TUNNEL_ID, credential=CREDENTIAL, origin_port=ORIGIN_PORT)
        assert r.get('ok'), (a, r)
    checks.append(f'{len(aliases)} replicas running with their credential declared on every host, the publisher declared on every host, the role on {G} under epoch {e1}; takeover proof {proof1["method"]} ({how})')

    # a standby may not publish; the governor may not without the service address, nor without the authority's proof
    refused(pub('publisher_start', S, takeover_proof=proof1), LEASE_GATE_REASONS, f'{S} starting a connector without the lease')
    refused(pub('publisher_start', G, takeover_proof=proof1), 'no effective exclusive route', f'{G} starting a connector before publishing the service address')
    hosts[G].ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses[G], exclusive_resource=LOGICAL))
    refused(pub('publisher_start', G), 'requires `takeover_proof`', f'{G} starting a connector without the authority\'s proof')
    refused(pub('publisher_start', G, previous={'waited_seconds': 1}), 'requires `takeover_proof`', f'{G} starting a connector on its own word alone (previous: waited)')
    refused(pub('publisher_start', G, previous={'none': True}), 'requires `takeover_proof`', f'{G} starting a connector on its own word alone (previous: none)')
    refused(pub('publisher_start', G, takeover_proof=resigned(dict(proof1, resource=str(uuid.uuid4())))), 'another resource', 'a proof bound to another resource')
    refused(pub('publisher_start', G, takeover_proof=resigned(dict(proof1, new_holder=hosts[S].identity))), 'another host as the new holder', 'a proof naming another host as the new holder')
    refused(pub('publisher_start', G, takeover_proof=resigned(dict(proof1, expires_at=int(time.time()) - 5))), 'expired', 'an expired proof')
    refused(pub('publisher_start', G, takeover_proof=resigned(dict(proof1, method='fence_receipt', receipt={'host': proof1.get('previous_holder') or hosts[S].identity, 'operation_id': 'x', 'resource': str(uuid.uuid4()), 'withdrawn': True}))),
            'another resource' if proof1.get('previous_holder') else 'not the previous holder', 'a fence receipt for another resource')
    refused(pub('publisher_start', G, takeover_proof=resigned(dict(proof1, method='lease_barrier', eligible_after=int(time.time()) + 600))), 'barrier', 'a barrier not yet reached')
    st = pub('publisher_status', G)['data']
    assert st['publisher_eligible'] is True and st['origin_readiness']['status'] == 503, st
    checks.append('before the start: the governor eligible, its origin answering 503 (no governor mark yet)')
    r = pub('publisher_start', G, takeover_proof=proof1, previous={'none': True})
    assert r.get('ok') and r['data']['published'] is True and r['data']['connector_id'], r
    st = wait_connector(G)
    assert st['unit']['state'] == 'active' and st['connector_id'], st
    assert st['origin_readiness']['status'] == 200 and st['origin_readiness']['body']['epoch'] == e1 and st['origin_readiness']['body']['replica_id'] == replica_ids[G], st['origin_readiness']
    assert st['carrier_replica_id'] == replica_ids[G] and st['governor_mark'] is True, st
    checks.append(f'{G} started its connector: unit active, connection {st["connector_id"][:8]}… registered, the origin ready at epoch {e1} with {G}\'s replica, the mark present')
    status, body = public_ready()
    assert status == 200 and body['logical_manager_id'] == LOGICAL and body['replica_id'] == replica_ids[G] and body['epoch'] == e1, (status, body)
    pub('publisher_observed', G, observation={'hostname': HOSTNAME, 'status': status, 'body': body, 'from': 'workstation'})
    checks.append(f'an external request to https://{HOSTNAME}/ready answered 200 with the logical manager, {G}\'s replica and epoch {e1}; recorded')

    # the rotation: the old governor's fence stops the connector and removes the mark before the address goes
    rot2 = tool('rotate', '--universe', LOGICAL, '--host', targets[S], '--lease', '20', '--margin', '5')  # short: the permit gate is tested alone at the end
    e2 = rot2['epoch']
    assert rot2['takeover_proof']['method'] == 'lease_barrier' and rot2['takeover_proof']['previous_holder'] == hosts[G].identity, rot2['takeover_proof']
    for a in aliases:
        if a not in (S, G):
            hosts[a].ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    refused(pub('publisher_start', S, takeover_proof=rot2['takeover_proof']), 'no effective exclusive route', f'{S} starting before publishing, and before the barrier or the fence')
    hosts[G].ok(request('activation_supersede', LOGICAL, reference, permit=rot2['permit']))
    fence_op = str(uuid.uuid4())
    fence = hosts[G].ok({'operation': 'activation_fence', 'operation_id': fence_op, 'authorization_ref': reference, 'timeout_seconds': 10})
    pw = fence['publishers_withdrawn']
    assert pw and pw[0]['withdrawn'] is True and [s['kind'] for s in pw[0]['steps'] if not s.get('unrecorded')] == ['publisher', 'governor_mark'], fence
    rw = [r for r in fence['routes_withdrawn'] if r['ip'] == SERVICE]
    assert rw and rw[0]['withdrawn'] is True, fence
    st = pub('publisher_status', G)['data']
    assert st['unit']['state'] != 'active' and st['publisher_eligible'] is False and any('superseded' in r for r in st['reasons']), st
    code, body = origin_at(hosts[G], addresses[G])
    assert code == '503', (code, body)
    checks.append(f'{G} fenced after the supersession: its connector stopped and its mark removed first, then its alias and route withdrawn; its origin answers 503 at its own address; no longer eligible ({st["reasons"][0][:60]}…)')
    hosts[S].ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=addresses[S], exclusive_resource=LOGICAL))
    refused(pub('publisher_start', S, takeover_proof=rot2['takeover_proof']), 'barrier', f'{S} starting on the barrier proof before the barrier: the fence must be attested or the barrier reached')
    # the previous epoch's proof, naming this host as its new holder so that its epoch is the one defect
    refused(pub('publisher_start', S, takeover_proof=resigned(dict(proof1, new_holder=hosts[S].identity))), 'for epoch', f'{S} starting on the previous epoch\'s proof (stale)')
    refused(pub('publisher_start', S, takeover_proof=proof1), 'another host as the new holder', f'{S} starting on a proof issued to {G}')
    import tempfile as _tf
    with _tf.NamedTemporaryFile('w', suffix='.json', delete=False, prefix='podmesh-receipt-') as f:
        json.dump({'host': hosts[G].identity, 'operation_id': fence_op, 'fence': fence}, f)
    proof2 = tool('attest-fence', '--universe', LOGICAL, '--receipt', f.name)['takeover_proof']
    os.unlink(f.name)
    assert proof2['method'] == 'fence_receipt' and proof2['receipt']['host'] == hosts[G].identity, proof2
    r = pub('publisher_start', S, takeover_proof=proof2, previous={'fenced': True, 'operation_id': fence_op})
    assert r.get('ok') and r['data']['published'] is True, r
    st = wait_connector(S)
    assert st['unit']['state'] == 'active' and st['connector_id'] and st['origin_readiness']['body']['epoch'] == e2, st
    deadline = time.time() + 120
    while time.time() < deadline:
        status, body = public_ready(30)
        if status == 200 and body.get('epoch') == e2 and body.get('replica_id') == replica_ids[S]:
            break
        time.sleep(3)
    assert status == 200 and body['epoch'] == e2 and body['replica_id'] == replica_ids[S], (status, body)
    pub('publisher_observed', S, observation={'hostname': HOSTNAME, 'status': status, 'body': body, 'from': 'workstation'})
    checks.append(f'{S} published the service address and started its connector; the same public hostname answers with {S}\'s replica and epoch {e2}')

    # the permit gate alone: the connector stopped, the lease left to lapse with the route and the alias
    # still effective (no fence ran), a start is refused by the lease gate and by nothing else
    assert pub('publisher_stop', S).get('ok')
    expires = hosts[S].ok(request('activation_status', LOGICAL, reference))['expires_at']
    while hosts[S].call('time')['time'] <= expires:
        time.sleep(1)
    st = pub('publisher_status', S)['data']
    assert st['service'] and st['publisher_eligible'] is False, st
    refused(pub('publisher_start', S, takeover_proof=proof2), 'expired', f'{S} starting a connector under a lapsed lease while its route and alias are still effective')
    checks.append('the permit gate alone refused: the service address still effective, the lease lapsed, no fence run')
    print(json.dumps({'result': 'PASS', 'checks': checks, 'hostname': HOSTNAME, 'tunnel': TUNNEL_ID[:8], 'epochs': [e1, e2], 'gate': gate_state,
                      'not_proven': ['the partition that cuts the old governor from peers and agent while it keeps its Internet egress: its own suite',
                                     'the manager\'s web interface behind the origin: the origin is the readiness responder',
                                     'a request that reached a standby through Cloudflare: one connector only in this candidate']}, indent=2))
finally:
    for a, h in hosts.items():
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
    for a, h in hosts.items():
        left = h.ssh(f'systemctl is-active podmesh-publisher-{LOGICAL}.service', check=False).stdout.decode().strip()
        print(f'{a}: network state restored: {routes(h) == initial[a]["routes"] and networks(h) == initial[a]["networks"]}; connector unit: {left}', file=sys.stderr)
