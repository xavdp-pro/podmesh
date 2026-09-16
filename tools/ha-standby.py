#!/usr/bin/env python3
"""The agent's side of a warm standby: the capture cycle and the takeover, as a tool and not a timer.

PodMesh never acts on its own. Everything level 2 needs outside the two hosts -- deciding when a
capture happens, carrying the bytes, restoring on the standby, deciding that the active host has
failed, waiting the margin, rotating the epoch, promoting and starting -- is the agent's, and this
tool is that side made runnable by a human or an agent from a workstation. It is invoked; it does
one bounded thing; it prints one JSON report; it exits. Whether it may ever run on a schedule is a
production mandate, exactly as for the collector.

Every product mutation goes through the PodMesh API of the host concerned, reached over SSH as
the two-host suites reach it. Bytes move outbox -> inbox over SSH with digests compared on both
sides. The epoch gate is the fencing laboratory's `Authority` (experiments/manager-fencing in the
web tree, reviewed candidate 0c3756fb...), imported from PODMESH_FENCING_LAB and never copied:
one SQLite compare-and-swap gate on the host this tool runs on, with the laboratory's own
precondition -- one current copy, never cloned or rolled back -- as the operator's obligation.

Environment: PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT (the service on both hosts),
PODMESH_FENCING_LAB (directory holding fencing_lab.py), PODMESH_GATE (the gate's SQLite file),
PODMESH_HA_LEDGER (directory for this tool's per-universe ledger; default ~/.podmesh-ha),
PODMESH_HA_KEYS (directory for the authority's Ed25519 signing key, one file per authority,
0600, generated at first use; default <ledger>/keys), PODMESH_HA_UNSIGNED=1 (laboratory
proofs, unsigned, for a policy that names no key).

The takeover proof is signed: every policy this tool declares names the authority's public key,
and every proof it prints (rotate, attest-fence) carries the signature over the proof's canonical
form (keys sorted, compact JSON); the host verifies the signature before the binding.

Subcommands:
  gate init                      create the gate; prints its authority_id
  gate declare  --universe U     declare the universe as a gated resource (epoch 0, no owner)
  gate inspect  --universe U
  attest-fence  --universe R --receipt FILE   bind the previous holder's fence answer to the current epoch's
                                 takeover proof (method fence_receipt); FILE = {host, operation_id, fence}
  rotate        --universe R --host SSH [--lease S --margin S --standbys N]
                                 rotate the epoch of a resource (a universe, or a role such as a logical
                                 manager whose replicas all keep running) to a host and acquire it there;
                                 nothing promoted or started; prints the permit, which the agent delivers
                                 to the other hosts as `activation_supersede` before they fence
  activate      --universe U --host SSH [--lease S --margin S --standbys N]
                                 declare the policy on the host under the gate's authority, rotate the
                                 epoch to it, acquire; starting is the operator's (the API's `start`)
  cycle         --universe U --active SSH --standby SSH [--also SSH]... [--keep N --keep-points N --minimum-age S]
                [--capture stopped|live]
                                 one capture: declare the collector's retention on the active host,
                                 stop, prepare, renew, start again; carry; restore into quarantine on
                                 the standby; prune older copies. `--capture live` never stops the
                                 universe: it is checkpointed with its memory and resumed in place
                                 (interrupted for the dump, under a second for a small universe), the
                                 archive is staged on each standby, and older staged points discarded
  takeover      --universe U --active SSH --standby SSH [--also SSH]... [--no-start]
                                 [--network-profile isolated|managed --network-address IP]
                                 (a live copy is promoted running, with its memory: no start, no network flags)
                                 the standby takes over, under the lease and margin recorded by
                                 `activate` (never this invocation's defaults): refuses while the active host is reachable
                                 and entitled (that is a planned handoff, not a takeover); otherwise
                                 fences it if reachable, waits the margin on its clock (or lease +
                                 margin on the standby's clock if it is not), rotates the epoch,
                                 acquires, promotes the newest quarantined copy, starts, and
                                 supersedes the active host if it can be reached
"""
import argparse, json, os, pathlib, sys, tempfile, time, uuid

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'tests'))
from podmesh_two_hosts import Host, request, transfer  # noqa: E402

LAB = os.environ.get('PODMESH_FENCING_LAB')
if LAB:
    sys.path.insert(0, LAB)
try:
    import fencing_lab  # noqa: E402
except ImportError:
    fencing_lab = None

REF_DEFAULT = 'ha-standby-tool'


class Refusal(Exception):
    """The tool refuses; the report says why and nothing was changed after the refusal."""


def out(report, code=0):
    print(json.dumps(report, indent=2, sort_keys=True))
    sys.exit(code)


def gate_or_refuse(create=False):
    if fencing_lab is None:
        raise Refusal('the fencing laboratory is not importable: set PODMESH_FENCING_LAB to the directory holding fencing_lab.py')
    path = os.environ.get('PODMESH_GATE')
    if not path:
        raise Refusal('PODMESH_GATE must name the gate\'s SQLite file')
    try:
        return fencing_lab.Authority(pathlib.Path(path), create=create)
    except fencing_lab.Refused as e:
        raise Refusal(f'gate: {e}') from e


def ledger_path(universe):
    root = pathlib.Path(os.environ.get('PODMESH_HA_LEDGER', pathlib.Path.home() / '.podmesh-ha'))
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    return root / f'{universe}.json'


def load_ledger(universe):
    p = ledger_path(universe)
    if p.is_file():
        return json.loads(p.read_text())
    return {'universe': universe, 'cycles': [], 'rotations': []}


def save_ledger(universe, ledger):
    p = ledger_path(universe)
    tmp = p.with_suffix('.json.partial')
    tmp.write_text(json.dumps(ledger, indent=2, sort_keys=True))
    os.replace(tmp, p)


def save_current_proof(universe, proof):
    """The follow tick on a host reads this file after the agent copies it; rotate never starts the connector."""
    root = pathlib.Path(os.environ.get('PODMESH_HA_LEDGER', pathlib.Path.home() / '.podmesh-ha'))
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = root / f'{universe}.current-proof.json'
    tmp = path.with_suffix('.json.partial')
    tmp.write_text(json.dumps(proof, indent=2, sort_keys=True))
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)
    return str(path)


# ------------------------------------------------------------------ signing
def _crypto():
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric import ed25519
    return ed25519, serialization


def key_path(authority_id):
    root = pathlib.Path(os.environ.get('PODMESH_HA_KEYS') or (pathlib.Path(os.environ.get('PODMESH_HA_LEDGER', pathlib.Path.home() / '.podmesh-ha')) / 'keys'))
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    return root / f'{authority_id}.ed25519'


def signing_key(authority_id):
    """The authority's Ed25519 private key: raw bytes as hex in a 0600 file named by the authority,
    generated at first use and never printed; None under PODMESH_HA_UNSIGNED=1."""
    if os.environ.get('PODMESH_HA_UNSIGNED') == '1':
        return None
    ed25519, serialization = _crypto()
    p = key_path(authority_id)
    if p.is_file():
        return ed25519.Ed25519PrivateKey.from_private_bytes(bytes.fromhex(p.read_text().strip()))
    key = ed25519.Ed25519PrivateKey.generate()
    raw = key.private_bytes(serialization.Encoding.Raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
    fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as f:
        f.write(raw.hex() + '\n')
    return key


def public_hex(key):
    _, serialization = _crypto()
    return key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()


def canonical(document):
    """What the signature covers: the document without `signature`, keys sorted, compact -- the
    form `serde_json` gives a `Value` on the host."""
    return json.dumps({k: v for k, v in document.items() if k != 'signature'}, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()


def sign(document, key):
    """The document as the authority signs it: kind ed25519, `signer` the public key, `signature`
    over the canonical form. A document signed again is signed over its new content."""
    doc = {k: v for k, v in document.items() if k != 'signature'}
    doc.update(kind='podmesh-takeover-proof/ed25519', signer=public_hex(key),
               note='signed by the authority: the host verifies the signature under the key its policy names, then the binding')
    doc['signature'] = key.sign(canonical(doc)).hex()
    return doc


def key_fields(gate):
    """What a policy declaration carries so that the host requires and verifies signed proofs."""
    key = signing_key(gate.authority_id)
    return {'authority_key': public_hex(key)} if key else {}


def hosts(args, *roles):
    control = tempfile.mkdtemp(prefix='podmesh-ha-')
    socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
    state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
    unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
    return [Host(role, target, control, socket_path, state_dir, unit) for role, target in roles]


def try_host(role, target):
    """A host that may be unreachable: None rather than an exception, and the report says so."""
    try:
        return hosts(None, (role, target))[0]
    except Exception as e:  # noqa: BLE001 -- unreachable is a fact to report, whatever raised it
        return None


def permit_for(gate, universe, host, expected_epoch):
    boot = host.call('boot_id')['boot_id']
    try:
        p = gate.transfer(universe, expected_epoch, host.identity, boot)
    except fencing_lab.Refused as e:
        raise Refusal(f'gate refused the rotation: {e}') from e
    return json.loads(p.encode())


def ok(host, req, what):
    r = host.api(req)
    if not r.get('ok'):
        raise Refusal(f'{host.role}: {what}: {r.get("error")}')
    return r['data']


# ----------------------------------------------------------------- subcommands

def cmd_gate(args):
    if args.gate_command == 'init':
        gate = gate_or_refuse(create=True)
        key = signing_key(gate.authority_id)
        out({'gate': os.environ.get('PODMESH_GATE'), 'authority_id': gate.authority_id, 'created': True,
             'authority_key': public_hex(key) if key else None,
             'precondition': 'one current copy of this file, never cloned or rolled back; that is the operator\'s obligation'})
    gate = gate_or_refuse()
    if args.gate_command == 'declare':
        try:
            gate.declare(args.universe)
        except fencing_lab.Refused as e:
            raise Refusal(f'gate: {e}') from e
        out({'authority_id': gate.authority_id, 'declared': args.universe, 'epoch': 0})
    if args.gate_command == 'inspect':
        try:
            row = gate.inspect(args.universe)
        except fencing_lab.Refused as e:
            raise Refusal(f'gate: {e}') from e
        out({'authority_id': gate.authority_id, 'resource': row})


def cmd_activate(args):
    gate = gate_or_refuse()
    (host,) = hosts(args, ('host', args.host))
    u = args.universe
    status = ok(host, request('activation_status', u, args.reference), 'status')
    if status['requires_lease'] and status.get('authority_id') not in (None, gate.authority_id):
        raise Refusal(f'the universe is under another authority on {host.role}: {status.get("authority_id")}')
    ok(host, request('activation_require', u, args.reference, lease_seconds=args.lease, takeover_margin_seconds=args.margin,
                     desired_standbys=args.standbys, authority_id=gate.authority_id, **key_fields(gate)), 'activation_require')
    current = gate.inspect(u)
    permit = permit_for(gate, u, host, current['epoch'])
    lease = ok(host, request('activation_acquire', u, args.reference, permit=permit), 'activation_acquire')
    ledger = load_ledger(u)
    # The policy as declared, kept where the takeover reads it: the wait for an unreachable host
    # is computed from THIS lease and margin, never from a later invocation's defaults.
    ledger['policy'] = {'lease_seconds': args.lease, 'takeover_margin_seconds': args.margin, 'desired_standbys': args.standbys,
                        'authority_id': gate.authority_id, 'declared_on': host.identity, 'declared_at': int(time.time())}
    ledger['rotations'].append({'epoch': permit['epoch'], 'to': host.identity, 'at': int(time.time()), 'by': 'activate'})
    save_ledger(u, ledger)
    out({'universe': u, 'host': host.identity, 'epoch': permit['epoch'], 'lease': {k: lease[k] for k in ('generation', 'expires_at', 'live')},
         'started': False, 'note': 'starting is the operator\'s: the API\'s start goes through the gate'})


def cmd_rotate(args):
    """Rotate the epoch of a resource to a host and acquire it there: the exclusive ROLE moves,
    nothing is promoted or started -- the running replicas keep running. The permit is printed
    so that the agent can deliver it to the other hosts as a supersession."""
    gate = gate_or_refuse()
    (host,) = hosts(args, ('host', args.host))
    u = args.universe
    ledger = load_ledger(u)
    policy = dict(ledger.get('policy') or {})
    if args.lease is not None:
        policy['lease_seconds'] = args.lease
    if args.margin is not None:
        policy['takeover_margin_seconds'] = args.margin
    if args.standbys is not None:
        policy['desired_standbys'] = args.standbys
    policy.setdefault('lease_seconds', 20)
    policy.setdefault('takeover_margin_seconds', 5)
    policy.setdefault('desired_standbys', 2)
    policy['authority_id'] = gate.authority_id
    ok(host, request('activation_require', u, args.reference, lease_seconds=policy['lease_seconds'], takeover_margin_seconds=policy['takeover_margin_seconds'],
                     desired_standbys=policy['desired_standbys'], authority_id=gate.authority_id, **key_fields(gate)), 'activation_require')
    current = gate.inspect(u)
    permit = permit_for(gate, u, host, current['epoch'])
    lease = ok(host, request('activation_acquire', u, args.reference, permit=permit), 'activation_acquire')
    proof = takeover_proof(gate, u, current, host.identity, permit['epoch'], policy)
    ledger['policy'] = dict(policy, authority_id=gate.authority_id, declared_on=host.identity, declared_at=int(time.time()))
    ledger['rotations'].append({'epoch': permit['epoch'], 'to': host.identity, 'at': int(time.time()), 'by': 'rotate'})
    ledger.setdefault('proofs', {})[str(permit['epoch'])] = proof
    save_ledger(u, ledger)
    proof_path = save_current_proof(u, proof)
    out({'resource': u, 'host': host.identity, 'epoch': permit['epoch'], 'permit': permit, 'takeover_proof': proof,
         'current_proof_path': proof_path,
         'lease': {k: lease[k] for k in ('generation', 'expires_at', 'live')},
         'note': 'the role moved; no universe was promoted or started, and the other hosts learn the epoch only when the permit is delivered to them; '
                 'the takeover proof is what an exclusive publication needs, upgraded by attest-fence once the previous holder is fenced'})


def takeover_proof(gate, universe, before, new_holder, new_epoch, policy):
    """The authority's account of the transition, bound to the resource, both epochs and both
    holders (Codex, P0). Its method says what makes the new holder eligible: `first` when no
    epoch existed, `same_holder` when this host held the previous one, else `lease_barrier` --
    the previous lease plus the margin from now, on this gate's clock, since the previous
    holder may have renewed right up to this rotation. `attest-fence` turns it into a
    `fence_receipt` once the previous holder's fence is in hand. Signed with the authority's
    Ed25519 key over its canonical form; a laboratory proof, unsigned and labelled so, only under
    PODMESH_HA_UNSIGNED=1 (the host then accepts it on its binding alone)."""
    previous_epoch = before['epoch']
    previous_holder = before.get('replica_id') or None
    now = int(time.time())
    if previous_epoch == 0 or previous_holder is None:
        method, eligible = 'first', now
        previous_holder = None
    elif previous_holder == new_holder:
        method, eligible = 'same_holder', now
    else:
        method, eligible = 'lease_barrier', now + int(policy['lease_seconds']) + int(policy['takeover_margin_seconds'])
    proof = {'kind': 'podmesh-takeover-proof/lab-unsigned', 'authority_id': gate.authority_id, 'resource': universe,
             'previous_epoch': previous_epoch, 'new_epoch': new_epoch, 'previous_holder': previous_holder, 'new_holder': new_holder,
             'method': method, 'eligible_after': eligible, 'issued_at': now, 'expires_at': now + 3600,
             'note': 'laboratory proof, unsigned: binding checked by the host, origin not verified'}
    key = signing_key(gate.authority_id)
    return sign(proof, key) if key else proof


def cmd_attest_fence(args):
    """Upgrade the current epoch's takeover proof with the previous holder's fence: the fence's
    answer (as the host returned it, with the host's identity) is bound to the transition and the
    method becomes `fence_receipt`, eligible at once."""
    gate = gate_or_refuse()
    u = args.universe
    ledger = load_ledger(u)
    current = gate.inspect(u)
    proof = (ledger.get('proofs') or {}).get(str(current['epoch']))
    if not proof:
        raise Refusal(f'no takeover proof recorded for epoch {current["epoch"]}; rotate first')
    receipt = json.loads(pathlib.Path(args.receipt).read_text())
    host = receipt.get('host')
    answer = receipt.get('fence') or {}
    if host != proof['previous_holder']:
        raise Refusal(f'the receipt is from {host}, not from the previous holder {proof["previous_holder"]}')
    # The fence names every resource it found the host not entitled to; a withdrawal it made for this
    # resource must have taken, and one it had nothing to withdraw for still binds: the host is fenced.
    if u not in (answer.get('unentitled') or []):
        raise Refusal('the fence answer does not name this resource among those the host is not entitled to')
    attempted = [r for r in (answer.get('publishers_withdrawn') or []) if r.get('resource') == u] + \
        [r for r in (answer.get('routes_withdrawn') or []) if r.get('exclusive_resource') == u]
    if any(r.get('withdrawn') is not True for r in attempted):
        raise Refusal('the fence answer records a withdrawal for this resource that did not take')
    proof = dict(proof, method='fence_receipt', eligible_after=int(time.time()), issued_at=int(time.time()),
                 receipt={'host': host, 'operation_id': receipt.get('operation_id'), 'resource': u, 'withdrawn': True})
    key = signing_key(gate.authority_id)
    proof = sign(proof, key) if key else {k: v for k, v in proof.items() if k not in ('signature', 'signer')}
    ledger['proofs'][str(current['epoch'])] = proof
    save_ledger(u, ledger)
    proof_path = save_current_proof(u, proof)
    out({'resource': u, 'epoch': current['epoch'], 'takeover_proof': proof, 'current_proof_path': proof_path})


def cmd_cycle(args):
    targets = [args.standby] + list(args.also or [])
    all_hosts = hosts(args, ('active', args.active), *[(f'standby-{i+1}', s) for i, s in enumerate(targets)])
    A, standbys = all_hosts[0], all_hosts[1:]
    if len({h.identity for h in all_hosts}) != len(all_hosts):
        raise Refusal('the active host and the standbys must be distinct hosts')
    u = args.universe
    ledger = load_ledger(u)
    status = ok(A, request('activation_status', u, args.reference), 'status')
    if not status['live'] or status['holder_host_uuid'] != A.identity:
        raise Refusal('the active host does not hold a live lease for the universe; a capture cycle renews a lease it holds, it does not take one')
    # The active host's archives are the collector's, after a declared retention: the cycle declares
    # it every time with the same values, so a universe under this tool is never left without one.
    ok(A, request('collection_retention_declare', u, args.reference, keep_latest=args.keep_points, minimum_age_seconds=args.minimum_age),
       'collection_retention_declare')
    if args.capture == 'live':
        return cycle_live(args, A, standbys, ledger)
    began = A.call('time')['time']
    stopped = ok(A, request('stop', u, args.reference, timeout_seconds=args.stop_timeout, on_timeout='kill'), 'stop for capture')
    if stopped.get('forced') is not False:
        # The universe is left stopped and the report says so: a capture after an escalated stop has no class.
        raise Refusal(f'the stop escalated ({stopped}); no capture was taken and the universe is stopped on the active host')
    if stopped.get('exit_code') not in (0, None):
        # A universe that reports its own stop as failed (a non-zero exit under the stop signal) is not a
        # quiescent capture either: what it left on disk is whatever a failed shutdown leaves.
        raise Refusal(f'the universe did not stop cleanly (exit code {stopped.get("exit_code")}); no capture was taken and the universe is stopped on the active host')
    prepared = ok(A, request('recovery_point_prepare', u, args.reference), 'recovery_point_prepare')
    point = prepared['recovery_point_uuid']
    # The point's own record on the active host, for the time it was prepared at on that host's clock.
    listed = ok(A, request('recovery_point_status', u, args.reference), 'recovery_point_status')
    row = next(r for r in listed['recovery_points'] if r['recovery_point_uuid'] == point)
    ok(A, request('activation_renew', u, args.reference), 'activation_renew')
    ok(A, request('start', u, args.reference, observe_seconds=1), 'start after capture')
    stopped_for = A.call('time')['time'] - began
    # One capture, carried to every standby and restored into quarantine on each; the ledger keeps
    # every copy under the standby it lives on, so a takeover can pick the newest copy of its host.
    copies, pruned, kept = [], [], []
    for B in standbys:
        carried = transfer(A, B, point, files=('recovery-point-manifest.json', 'rootfs.tar'))
        q = str(uuid.uuid4())
        restored = ok(B, request('recovery_point_restore', q, args.reference, recovery_point_uuid=point), f'recovery_point_restore on {B.role}')
        cycle = {'point': point, 'generation': prepared['generation'], 'prepared_at': row['prepared_at'],
                 'rootfs_sha256': prepared['rootfs_sha256'], 'rootfs_bytes': prepared['rootfs_bytes'],
                 'standby': B.identity, 'quarantined_uuid': q, 'restored_at': int(time.time()),
                 'carried_bytes': carried['files']['rootfs.tar']['bytes'], 'manifest_signed': restored['manifest_signed']}
        ledger['cycles'].append(cycle)
        copies.append({'standby': B.identity, 'quarantined_uuid': q})
        # Prune: the newest `keep` quarantined copies on THIS standby stay; older ones are deleted through
        # the API, and a refusal is reported rather than forced -- a promoted copy is a universe now.
        mine = [c for c in ledger['cycles'] if c.get('standby') == B.identity]
        for old in mine[:max(len(mine) - args.keep, 0)]:
            if old.get('pruned') or old.get('promoted'):
                continue
            r = B.api(request('delete', old['quarantined_uuid'], args.reference))
            if r.get('ok'):
                old['pruned'] = int(time.time())
                pruned.append(old['quarantined_uuid'])
            else:
                old['prune_refused'] = r.get('error')
                kept.append({'quarantined_uuid': old['quarantined_uuid'], 'refused': r.get('error')})
    save_ledger(u, ledger)
    points = listed
    out({'universe': u, 'active': A.identity, 'standbys': [B.identity for B in standbys], 'point': point,
         'generation': prepared['generation'], 'copies': copies, 'quarantined': copies[0]['quarantined_uuid'],
         'stopped_for_seconds': round(stopped_for, 2), 'manifest_signed': copies and restored['manifest_signed'],
         'pruned_on_standbys': pruned, 'prune_refused': kept,
         'points_on_active_outbox': len(points['recovery_points']),
         'retention_declared_on_active': {'keep_latest': args.keep_points, 'minimum_age_seconds': args.minimum_age},
         'note': 'the active host\'s archives are the collector\'s (class 5, under the retention declared here); this tool never deletes them'})


def cycle_live(args, A, standbys, ledger):
    """One live capture: the universe is checkpointed with its memory and resumed in place, never stopped by a
    `stop`; the archive is carried to each standby and staged there, ready to be promoted running."""
    u = args.universe
    prepared = ok(A, request('recovery_point_prepare', u, args.reference, capture='live'), 'recovery_point_prepare (live)')
    point = prepared['recovery_point_uuid']
    capture = prepared.get('capture') or {}
    if not prepared.get('resumed'):
        raise Refusal(f'the live capture was recorded but the universe did not resume in place ({capture.get("resume_failure")}); '
                      'it is stopped on the active host with its checkpoint files kept, and nothing was carried')
    listed = ok(A, request('recovery_point_status', u, args.reference), 'recovery_point_status')
    row = next(r for r in listed['recovery_points'] if r['recovery_point_uuid'] == point)
    ok(A, request('activation_renew', u, args.reference), 'activation_renew')
    copies, discarded, kept = [], [], []
    for B in standbys:
        carried = transfer(A, B, point, files=('recovery-point-manifest.json', 'checkpoint.tar.zst'))
        staged = ok(B, request('recovery_point_stage', u, args.reference, recovery_point_uuid=point), f'recovery_point_stage on {B.role}')
        ledger['cycles'].append({'point': point, 'generation': prepared['generation'], 'prepared_at': row['prepared_at'],
                                 'capture': 'live', 'archive_sha256': prepared['archive']['sha256'], 'archive_bytes': prepared['archive']['bytes'],
                                 'standby': B.identity, 'staged_at': staged['staged_at'], 'carried_bytes': carried['files']['checkpoint.tar.zst']['bytes'],
                                 'interruption_seconds': capture.get('interruption_seconds')})
        copies.append({'standby': B.identity, 'staged_point': point})
        mine = [c for c in ledger['cycles'] if c.get('standby') == B.identity and c.get('capture') == 'live']
        for old in mine[:max(len(mine) - args.keep, 0)]:
            if old.get('discarded') or old.get('promoted'):
                continue
            r = B.api(request('recovery_point_discard', u, args.reference, recovery_point_uuid=old['point']))
            if r.get('ok'):
                old['discarded'] = int(time.time())
                discarded.append(old['point'])
            else:
                old['discard_refused'] = r.get('error')
                kept.append({'point': old['point'], 'refused': r.get('error')})
    save_ledger(u, ledger)
    out({'universe': u, 'active': A.identity, 'standbys': [B.identity for B in standbys], 'point': point, 'capture': 'live',
         'generation': prepared['generation'], 'copies': copies, 'consistency_class': prepared.get('consistency_class'),
         'stopped_for_seconds': capture.get('interruption_seconds'),
         'interruption': {k: capture.get(k) for k in ('dump_seconds', 'resume_seconds', 'interruption_seconds')},
         'archive_bytes': prepared['archive']['bytes'], 'discarded_on_standbys': discarded, 'discard_refused': kept,
         'points_on_active_outbox': len(listed['recovery_points']),
         'retention_declared_on_active': {'keep_latest': args.keep_points, 'minimum_age_seconds': args.minimum_age},
         'note': 'never stopped: the universe was checkpointed with its memory and resumed in place; the interruption is the dump plus the resume'})


def cmd_takeover(args):
    gate = gate_or_refuse()
    (B,) = hosts(args, ('standby', args.standby))
    u = args.universe
    ledger = load_ledger(u)
    copies = [c for c in ledger['cycles'] if not c.get('pruned') and not c.get('discarded') and not c.get('promoted') and c.get('standby', B.identity) == B.identity]
    if not copies:
        raise Refusal('no quarantined copy of this universe is recorded on this standby; run a cycle to it first')
    newest = copies[-1]
    policy = ledger.get('policy')
    if not policy:
        raise Refusal('the ledger records no policy for this universe (no `activate` was run through this tool); the wait for an unreachable host must be computed from the real lease and margin, and this tool will not guess them')
    A = try_host('active', args.active)
    waited = {}
    if A is not None:
        status = ok(A, request('activation_status', u, args.reference), 'status on the active host')
        if status['live'] and status['holder_host_uuid'] == A.identity and not status.get('superseded'):
            raise Refusal('the active host is reachable and holds a live lease: that is a planned handoff (level 1), not a takeover; this tool refuses to start a second writer')
        expires = status.get('expires_at') or 0
        margin = status.get('takeover_margin_seconds') or args.margin
        fenced = ok(A, {'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': args.reference,
                        'timeout_seconds': args.stop_timeout}, 'activation_fence on the active host')
        hit = {e['universe_uuid']: e for e in fenced['fenced']}
        left = {e['universe_uuid']: e for e in fenced['left_running_or_absent']}
        waited['fence'] = hit.get(u) or left.get(u)
        until = expires + margin + 1
        while A.call('time')['time'] < until:
            time.sleep(.5)
        waited['margin'] = {'on': 'the active host\'s clock', 'until': until}
    else:
        # Unreachable: nothing can be observed there. Any lease it holds expires at most lease_seconds
        # after the moment it was last renewed, which is not later than now; lease + margin from now on
        # the standby's clock is the wait, and the margin is the clock-skew budget the design states.
        lease = policy['lease_seconds']
        margin = policy['takeover_margin_seconds']
        until = B.call('time')['time'] + lease + margin + 1
        waited['margin'] = {'on': 'the standby\'s clock, the active host being unreachable', 'seconds': lease + margin + 1,
                            'lease_seconds': lease, 'takeover_margin_seconds': margin, 'from': 'the ledger\'s record of the policy as activated'}
        while B.call('time')['time'] < until:
            time.sleep(.5)
    ok(B, request('activation_require', u, args.reference, lease_seconds=policy['lease_seconds'], takeover_margin_seconds=policy['takeover_margin_seconds'],
                  desired_standbys=policy['desired_standbys'], authority_id=gate.authority_id, **key_fields(gate)), 'activation_require on the standby')
    current = gate.inspect(u)
    permit = permit_for(gate, u, B, current['epoch'])
    lease = ok(B, request('activation_acquire', u, args.reference, permit=permit), 'activation_acquire on the standby')
    live = newest.get('capture') == 'live'
    if live:
        # A staged live point comes back running with its memory: the promotion is the start.
        promotion = {'recovery_point_uuid': newest['point']}
    else:
        promotion = {'restored_universe_uuid': newest['quarantined_uuid'], 'network_profile': args.network_profile}
        if args.network_address:
            promotion['network_address'] = args.network_address
    promoted = ok(B, request('recovery_point_promote', u, args.reference, **promotion), 'recovery_point_promote')
    newest['promoted'] = int(time.time())
    ledger['rotations'].append({'epoch': permit['epoch'], 'to': B.identity, 'at': int(time.time()), 'by': 'takeover'})
    save_ledger(u, ledger)
    started = None
    if live:
        started = promoted
    elif not args.no_start:
        started = ok(B, request('start', u, args.reference, observe_seconds=1), 'start on the standby')
    # The new grant is delivered to every other host that can be reached -- the old active, and any
    # other standby -- so that each screen learns the epoch and a stale permit bound to it is refused.
    superseded = None
    others = []
    if A is not None:
        r = A.api(request('activation_supersede', u, args.reference, permit=permit))
        superseded = {'delivered': bool(r.get('ok')), 'highest_epoch_seen': (r.get('data') or {}).get('highest_epoch_seen'), 'error': r.get('error')}
    for target in (args.also or []):
        C = try_host('other', target)
        if C is None:
            others.append({'target': 'unreachable'}); continue
        ok(C, request('activation_require', u, args.reference, lease_seconds=policy['lease_seconds'], takeover_margin_seconds=policy['takeover_margin_seconds'],
                      desired_standbys=policy['desired_standbys'], authority_id=gate.authority_id, **key_fields(gate)), f'activation_require on {C.role}')
        r = C.api(request('activation_supersede', u, args.reference, permit=permit))
        others.append({'host': C.identity, 'delivered': bool(r.get('ok')), 'highest_epoch_seen': (r.get('data') or {}).get('highest_epoch_seen'), 'error': r.get('error')})
    out({'universe': u, 'standby': B.identity, 'active': A.identity if A else None, 'active_reachable': A is not None,
         'waited': waited, 'epoch': permit['epoch'], 'lease': {k: lease[k] for k in ('generation', 'expires_at', 'live')},
         'promoted_from': {'point': newest['point'], 'generation': newest['generation'], 'prepared_at': newest['prepared_at'],
                           'capture': newest.get('capture', 'stopped'), 'quarantined_uuid': newest.get('quarantined_uuid'),
                           'with_memory': live},
         'data_lost_since_seconds': int(time.time()) - newest['prepared_at'],
         'started': started is not None, 'active_superseded': superseded, 'other_standbys_informed': others,
         'not_proven': ['mutual exclusion beyond the epoch: the gate is this workstation\'s file and PodMesh cannot verify a permit\'s origin',
                        'that the active host is stopped when it is unreachable: the wait is the design\'s margin, not a proof']})


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--reference', default=REF_DEFAULT, help='authorization_ref recorded on every operation (provenance, never a credential)')
    sub = p.add_subparsers(dest='command', required=True)
    g = sub.add_parser('gate'); g.add_argument('gate_command', choices=['init', 'declare', 'inspect']); g.add_argument('--universe')
    a = sub.add_parser('activate'); a.add_argument('--universe', required=True); a.add_argument('--host', required=True)
    af = sub.add_parser('attest-fence'); af.add_argument('--universe', required=True); af.add_argument('--receipt', required=True, help='JSON: {host, operation_id, fence: <the fence answer>}')
    ro = sub.add_parser('rotate'); ro.add_argument('--universe', required=True); ro.add_argument('--host', required=True)
    ro.add_argument('--lease', type=int, default=None, help='seconds; when omitted, the ledger policy is kept (else 20)')
    ro.add_argument('--margin', type=int, default=None); ro.add_argument('--standbys', type=int, default=None)
    c = sub.add_parser('cycle'); c.add_argument('--universe', required=True); c.add_argument('--active', required=True); c.add_argument('--standby', required=True)
    c.add_argument('--also', action='append', help='a further standby (repeatable): one capture, restored on each')
    c.add_argument('--capture', default='stopped', choices=('stopped', 'live'), help='live: checkpoint with memory and resume in place, no stop')
    c.add_argument('--keep', type=int, default=3, help='quarantined copies kept on each standby')
    c.add_argument('--keep-points', type=int, default=3, help='recovery points the collector keeps on the active host whatever their age')
    c.add_argument('--minimum-age', type=int, default=3600, help='seconds a recovery point must be old before the collector may take it')
    t = sub.add_parser('takeover'); t.add_argument('--universe', required=True); t.add_argument('--active', required=True); t.add_argument('--standby', required=True)
    t.add_argument('--no-start', action='store_true', help='promote but leave the start to the operator')
    t.add_argument('--network-profile', default='isolated', choices=('isolated', 'managed'), help='the promoted universe\'s network profile')
    t.add_argument('--network-address', help='managed only: the address to put the universe back at, as the restore reported it')
    t.add_argument('--also', action='append', help='another standby to inform of the new epoch (repeatable)')
    a.add_argument('--lease', type=int, default=20); a.add_argument('--margin', type=int, default=5); a.add_argument('--standbys', type=int, default=1)
    for s in (c, t):
        s.add_argument('--stop-timeout', type=int, default=10)
    args = p.parse_args()
    try:
        {'gate': cmd_gate, 'activate': cmd_activate, 'rotate': cmd_rotate, 'attest-fence': cmd_attest_fence, 'cycle': cmd_cycle, 'takeover': cmd_takeover}[args.command](args)
    except Refusal as e:
        out({'refused': str(e)}, 2)


if __name__ == '__main__':
    main()
