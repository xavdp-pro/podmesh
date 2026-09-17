#!/usr/bin/env python3
"""The Ed25519-signed takeover document (Codex's signature milestone, after P0): one lab host,
the tool's gate signing every proof it issues and every policy it declares naming the
authority's public key. Verified at `publisher_start`, the document's origin checked before its
binding: an unsigned laboratory proof refused under a keyed policy; an altered document (one
field changed after signing) refused as not verifying; a document signed by an unknown key
refused as not the policy's; documents signed by the real key but bound to another resource,
expired, or naming another host refused at their binding; the genuine document accepted, the
answer recording it as signed; a policy declaring an invalid key, or a key without an authority,
refused. Then the stop. Environment as `check-manager-publisher-readiness.py`.
"""
import importlib.util, json, os, pathlib, subprocess, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402
from podmesh_manager_lab import declare_replica_config, remove_replica_config, replica_create, prove_takeover  # noqa: E402

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
BINARY = os.environ.get('PODMESH_DAEMON_BINARY')  # needed only for a transient unit systemd has already forgotten
PREFIX = os.environ.get('PODMESH_NETWORK_PREFIX', '10.86.0.0/16')
SERVICE = os.environ.get('PODMESH_MANAGER_SERVICE_ADDRESS', '10.86.0.100')
VIAS = os.environ['PODMESH_NETWORK_PEER_VIAS'].split(',')
ALIAS = os.environ.get('PODMESH_HOST_ALIAS', 'lab-a')
replica_set = json.load(open(os.environ['PODMESH_REPLICA_SET']))
TUNNEL_ID = os.environ['PODMESH_TUNNEL_ID']
HOSTNAME = os.environ['PODMESH_PUBLIC_HOSTNAME']
CREDENTIALS = open(os.environ['PODMESH_TUNNEL_CREDENTIALS'], 'rb').read()
control = tempfile.mkdtemp(prefix='podmesh-pubsig-')
A = Host('host', os.environ['PODMESH_SOURCE_SSH'], control, socket_path, state_dir, unit)
reference = 'disposable-lab-proof-signature'
NET = str(uuid.uuid4())
POOLS = {'lab-a': '10.86.1.0/24', 'lab-b': '10.86.2.0/24', 'lab-c': '10.86.3.0/24'}
peers = [{'pool': POOLS[o], 'via': v} for o, v in zip([a for a in POOLS if a != ALIAS], VIAS)]
LOGICAL = replica_set['logical_manager_id']
address = next(r['address'] for r in replica_set['replicas'] if r['alias'] == ALIAS)
CREDENTIAL = 'cloudflare-tunnel-podmesh-lab'
checks = []

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tools', 'ha-standby.py')
sys.path.insert(0, os.environ['PODMESH_FENCING_LAB'])
import fencing_lab  # noqa: E402
LEDGER = os.environ.get('PODMESH_HA_LEDGER') or os.path.expanduser('~/.podmesh-ha')
os.makedirs(LEDGER, mode=0o700, exist_ok=True)
GATE = os.environ.get('PODMESH_GATE') or os.path.join(LEDGER, 'gate-m-u2.sqlite')
env = dict(os.environ, PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
env.pop('PODMESH_HA_UNSIGNED', None)
os.environ.update(PODMESH_GATE=GATE, PODMESH_HA_LEDGER=LEDGER)
spec = importlib.util.spec_from_file_location('ha_standby', TOOL)
ha = importlib.util.module_from_spec(spec); spec.loader.exec_module(ha)  # the tool's signing helpers, the same code the gate runs

def tool(*argv, expect=0):
    p = subprocess.run([sys.executable, '-B', TOOL, '--reference', reference, *argv], env=env, capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout[-800:], p.stderr[-800:])
    return json.loads(p.stdout)

def gate_ready():
    if not os.path.exists(GATE):
        tool('gate', 'init')
    gate = fencing_lab.Authority(pathlib.Path(GATE))
    try:
        gate.inspect(LOGICAL)
    except fencing_lab.Refused:
        gate.declare(LOGICAL)
    s = A.api(request('activation_status', LOGICAL, reference))
    seen = (s.get('data') or {}).get('highest_epoch_seen') or 0
    while gate.inspect(LOGICAL)['epoch'] < seen:
        gate.transfer(LOGICAL, gate.inspect(LOGICAL)['epoch'], 'gate-recovery', 'gate-recovery')
    authority_id = gate.authority_id
    gate.close()
    return authority_id

def hostwide(operation, **extra):
    return dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, **extra)

def pub(operation, **extra):
    return A.api(dict(operation=operation, operation_id=str(uuid.uuid4()), authorization_ref=reference, resource=LOGICAL, **extra))

def daemon():
    # Through the harness: an installed unit gets a runtime drop-in that carries the fault and turns systemd's own
    # restart off; a transient unit is stopped completely and launched again.
    A.call('restart_with_fault', binary=BINARY)
    for _ in range(50):
        if A.ssh(f'sudo -n test -S {socket_path}', check=False).returncode == 0 and A.call('ready', seconds=20).get('ready'):
            return
        time.sleep(0.2)
    raise AssertionError('the daemon did not come up')

def origin():
    return A.ssh(f'curl -s -m 5 -o /dev/null -w "%{{http_code}}" http://{SERVICE}:8080/ready', check=False).stdout.decode().strip()

def unit_active():
    return A.ssh(f'systemctl is-active podmesh-publisher-{LOGICAL}.service', check=False).stdout.decode().strip() == 'active'

def refused(document, fragment, label):
    r = pub('publisher_start', takeover_proof=document)
    assert not r.get('ok'), (label, 'accepted', r)
    assert fragment in r['error'], (label, 'refused for another reason', r['error'])
    assert not unit_active() and origin() == '503', (label, 'the refusal left something')
    st = pub('publisher_status')['data']
    assert st['transition'] is None and st['active_manager_mark'] is False, (label, st)
    checks.append(f'refused ({fragment}): {label}')

u = str(uuid.uuid4())
declared = False
try:
    daemon()
    A.ok(hostwide('network_declare', network_uuid=NET, prefix=PREFIX, pool=POOLS[ALIAS], peer_pools=peers)); declared = True
    declare_replica_config(A, ALIAS, reference, state_dir)
    A.ssh(f'sudo -n mkdir -p -m 0700 {state_dir}/inbox/secrets && sudo -n install -m 0600 -o root -g root /dev/stdin {state_dir}/inbox/secrets/{CREDENTIAL}', input_bytes=CREDENTIALS)
    A.ok(hostwide('secret_declare', name=CREDENTIAL, source=CREDENTIAL))
    replica_create(A, u, ALIAS, reference, address, request)
    started = A.ok(request('start', u, reference, observe_seconds=3))
    assert started['application_outcome'] == 'running_when_observed', started['application_outcome']
    authority_id = gate_ready()
    rot = tool('rotate', '--universe', LOGICAL, '--host', os.environ['PODMESH_SOURCE_SSH'], '--lease', '60', '--margin', '5')
    proof, how = prove_takeover(rot, {'host': A}, tool, request, reference, LOGICAL)
    A.ok(request('activation_acquire', LOGICAL, reference, permit=rot['permit']))
    key = ha.signing_key(authority_id)
    assert key is not None, 'the gate signs nothing: PODMESH_HA_UNSIGNED is set'
    public = ha.public_hex(key)
    assert proof['kind'] == 'podmesh-takeover-proof/ed25519' and proof['signer'] == public and len(proof['signature']) == 128, proof
    st = A.ok(request('activation_status', LOGICAL, reference))
    assert st['authority_key'] == public and 'Ed25519' in st['takeover_proof_verification'], st
    A.ok(hostwide('network_route_publish', universe_uuid=LOGICAL, ip=SERVICE, via=address, exclusive_resource=LOGICAL))
    assert pub('publisher_declare', hostname=HOSTNAME, tunnel_uuid=TUNNEL_ID, credential=CREDENTIAL, origin_port=8080).get('ok')
    checks.append(f'the policy names the authority\'s key; the rotation\'s proof is signed by it ({how}); the service address and the publisher declared')

    # the policy cannot be given a key that is not one, nor a key without an authority
    r = A.api(request('activation_require', LOGICAL, reference, lease_seconds=60, takeover_margin_seconds=5, desired_standbys=0, authority_id=authority_id, authority_key='zz'))
    assert not r.get('ok') and 'authority_key must be 32 bytes' in r['error'], r
    r = A.api(request('activation_require', LOGICAL, reference, lease_seconds=60, takeover_margin_seconds=5, desired_standbys=0, authority_id=authority_id, authority_key='02' + '00' * 31))  # y = 2: not on the curve
    assert not r.get('ok') and 'not a valid Ed25519 public key' in r['error'], r
    r = A.api(request('activation_require', LOGICAL, reference, lease_seconds=60, takeover_margin_seconds=5, desired_standbys=0, authority_key=public))
    assert not r.get('ok') and 'without authority_id' in r['error'], r
    assert A.ok(request('activation_status', LOGICAL, reference))['authority_key'] == public, 'a refused declaration changed the policy'
    checks.append('a policy with a malformed key, a key that is not an Ed25519 point, or a key without an authority: refused, the policy unchanged')

    # origin before binding
    unsigned = {k: v for k, v in proof.items() if k not in ('signature', 'signer')}
    unsigned['kind'] = 'podmesh-takeover-proof/lab-unsigned'
    refused(unsigned, 'requires podmesh-takeover-proof/ed25519', 'an unsigned laboratory proof under a policy that names the key')
    refused(dict(proof, issued_at=proof['issued_at'] - 1), 'does not verify', 'the signed document with one field altered after signing (issued_at, which no binding refuses)')
    refused(dict(proof, note='altered'), 'does not verify', 'the signed document with its note altered')
    ed25519, _ = ha._crypto()
    stranger = ed25519.Ed25519PrivateKey.generate()
    refused(ha.sign(unsigned, stranger), 'does not name', 'the same document signed by an unknown key')
    refused(ha.sign(dict(unsigned, resource=str(uuid.uuid4())), key), 'bound to another resource', 'a document signed by the real key for another resource')
    refused(ha.sign(dict(unsigned, expires_at=int(time.time()) - 10), key), 'expired', 'a document signed by the real key, expired')
    refused(ha.sign(dict(unsigned, new_holder=str(uuid.uuid4())), key), 'another host as the new holder', 'a document signed by the real key for another holder')
    refused(ha.sign(dict(unsigned, new_epoch=proof['new_epoch'] + 1, previous_epoch=proof['previous_epoch'] + 1), key), 'for epoch', 'a document signed by the real key for the next epoch')
    refused(dict(proof, signature='00' * 64), 'does not verify', 'the document with a zero signature')
    refused({k: v for k, v in proof.items() if k != 'signature'}, 'carries no signature', 'the signed kind without its signature')

    # the genuine document
    A.ok(request('activation_renew', LOGICAL, reference))
    r = pub('publisher_start', takeover_proof=proof)
    assert r.get('ok') and r['data']['published'] is True and r['data']['takeover_proof']['signed'] is True and 'verified' in r['data']['takeover_proof']['note'], r
    assert origin() == '200'
    checks.append('the genuine signed document: accepted, the answer records the signature verified; the origin ready')
    assert pub('publisher_stop').get('ok') and origin() == '503'
    print(json.dumps({'result': 'PASS', 'checks': checks, 'authority_key': public}, indent=2))
finally:
    daemon()
    A.api(dict(operation='publisher_stop', operation_id=str(uuid.uuid4()), authorization_ref=reference, resource=LOGICAL))
    A.api(hostwide('network_route_withdraw', universe_uuid=LOGICAL))
    A.api(request('stop', u, reference, timeout_seconds=15, on_timeout='kill'))
    A.api(request('delete', u, reference))
    A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + u], check=False)
    remove_replica_config(A, ALIAS, reference)
    A.api(hostwide('secret_remove', name=CREDENTIAL))
    if declared:
        r = A.api(hostwide('network_undeclare', network_uuid=NET))
        if not r.get('ok'):
            print(f'undeclare refused: {r.get("error")}', file=sys.stderr)
    print(f'connector unit active: {unit_active()}', file=sys.stderr)
