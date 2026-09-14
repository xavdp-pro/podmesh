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
PODMESH_HA_LEDGER (directory for this tool's per-universe ledger; default ~/.podmesh-ha).

Subcommands:
  gate init                      create the gate; prints its authority_id
  gate declare  --universe U     declare the universe as a gated resource (epoch 0, no owner)
  gate inspect  --universe U
  activate      --universe U --host SSH [--lease S --margin S --standbys N]
                                 declare the policy on the host under the gate's authority, rotate the
                                 epoch to it, acquire; starting is the operator's (the API's `start`)
  cycle         --universe U --active SSH --standby SSH [--keep N --keep-points N --minimum-age S]
                                 one capture: declare the collector's retention on the active host,
                                 stop, prepare, renew, start again; carry; restore into quarantine on
                                 the standby; prune older copies
  takeover      --universe U --active SSH --standby SSH [--no-start]
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
        out({'gate': os.environ.get('PODMESH_GATE'), 'authority_id': gate.authority_id, 'created': True,
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
                     desired_standbys=args.standbys, authority_id=gate.authority_id), 'activation_require')
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


def cmd_cycle(args):
    A, B = hosts(args, ('active', args.active), ('standby', args.standby))
    u = args.universe
    ledger = load_ledger(u)
    status = ok(A, request('activation_status', u, args.reference), 'status')
    if not status['live'] or status['holder_host_uuid'] != A.identity:
        raise Refusal('the active host does not hold a live lease for the universe; a capture cycle renews a lease it holds, it does not take one')
    # The active host's archives are the collector's, after a declared retention: the cycle declares
    # it every time with the same values, so a universe under this tool is never left without one.
    ok(A, request('collection_retention_declare', u, args.reference, keep_latest=args.keep_points, minimum_age_seconds=args.minimum_age),
       'collection_retention_declare')
    began = A.call('time')['time']
    stopped = ok(A, request('stop', u, args.reference, timeout_seconds=args.stop_timeout, on_timeout='kill'), 'stop for capture')
    if stopped.get('forced') is not False:
        # The universe is left stopped and the report says so: a capture after an escalated stop has no class.
        raise Refusal(f'the stop escalated ({stopped}); no capture was taken and the universe is stopped on the active host')
    prepared = ok(A, request('recovery_point_prepare', u, args.reference), 'recovery_point_prepare')
    point = prepared['recovery_point_uuid']
    # The point's own record on the active host, for the time it was prepared at on that host's clock.
    listed = ok(A, request('recovery_point_status', u, args.reference), 'recovery_point_status')
    row = next(r for r in listed['recovery_points'] if r['recovery_point_uuid'] == point)
    ok(A, request('activation_renew', u, args.reference), 'activation_renew')
    ok(A, request('start', u, args.reference, observe_seconds=1), 'start after capture')
    stopped_for = A.call('time')['time'] - began
    carried = transfer(A, B, point, files=('recovery-point-manifest.json', 'rootfs.tar'))
    q = str(uuid.uuid4())
    restored = ok(B, request('recovery_point_restore', q, args.reference, recovery_point_uuid=point), 'recovery_point_restore')
    cycle = {'point': point, 'generation': prepared['generation'], 'prepared_at': row['prepared_at'],
             'rootfs_sha256': prepared['rootfs_sha256'], 'rootfs_bytes': prepared['rootfs_bytes'],
             'quarantined_uuid': q, 'restored_at': int(time.time()), 'carried_bytes': carried['files']['rootfs.tar']['bytes']}
    ledger['cycles'].append(cycle)
    # Prune: the newest `keep` quarantined copies stay; older ones are deleted through the API, and a
    # refusal is reported rather than forced -- a copy that was promoted is a universe now, not a copy.
    pruned, kept = [], []
    older = ledger['cycles'][:max(len(ledger['cycles']) - args.keep, 0)]
    for old in older:
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
    out({'universe': u, 'active': A.identity, 'standby': B.identity, 'cycle': cycle,
         'stopped_for_seconds': round(stopped_for, 2), 'quarantined': restored['restored_universe_uuid'],
         'manifest_signed': restored['manifest_signed'], 'pruned_on_standby': pruned, 'prune_refused': kept,
         'points_on_active_outbox': len(points['recovery_points']),
         'retention_declared_on_active': {'keep_latest': args.keep_points, 'minimum_age_seconds': args.minimum_age},
         'note': 'the active host\'s archives are the collector\'s (class 5, under the retention declared here); this tool never deletes them'})


def cmd_takeover(args):
    gate = gate_or_refuse()
    (B,) = hosts(args, ('standby', args.standby))
    u = args.universe
    ledger = load_ledger(u)
    copies = [c for c in ledger['cycles'] if not c.get('pruned') and not c.get('promoted')]
    if not copies:
        raise Refusal('no quarantined copy of this universe is recorded on the standby; run a cycle first')
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
                  desired_standbys=policy['desired_standbys'], authority_id=gate.authority_id), 'activation_require on the standby')
    current = gate.inspect(u)
    permit = permit_for(gate, u, B, current['epoch'])
    lease = ok(B, request('activation_acquire', u, args.reference, permit=permit), 'activation_acquire on the standby')
    promoted = ok(B, request('recovery_point_promote', u, args.reference, restored_universe_uuid=newest['quarantined_uuid']), 'recovery_point_promote')
    newest['promoted'] = int(time.time())
    ledger['rotations'].append({'epoch': permit['epoch'], 'to': B.identity, 'at': int(time.time()), 'by': 'takeover'})
    save_ledger(u, ledger)
    started = None
    if not args.no_start:
        started = ok(B, request('start', u, args.reference, observe_seconds=1), 'start on the standby')
    superseded = None
    if A is not None:
        r = A.api(request('activation_supersede', u, args.reference, permit=permit))
        superseded = {'delivered': bool(r.get('ok')), 'highest_epoch_seen': (r.get('data') or {}).get('highest_epoch_seen'), 'error': r.get('error')}
    out({'universe': u, 'standby': B.identity, 'active': A.identity if A else None, 'active_reachable': A is not None,
         'waited': waited, 'epoch': permit['epoch'], 'lease': {k: lease[k] for k in ('generation', 'expires_at', 'live')},
         'promoted_from': {'point': newest['point'], 'generation': newest['generation'], 'prepared_at': newest['prepared_at'],
                           'quarantined_uuid': newest['quarantined_uuid']},
         'data_lost_since_seconds': int(time.time()) - newest['prepared_at'],
         'started': started is not None, 'active_superseded': superseded,
         'not_proven': ['mutual exclusion beyond the epoch: the gate is this workstation\'s file and PodMesh cannot verify a permit\'s origin',
                        'that the active host is stopped when it is unreachable: the wait is the design\'s margin, not a proof']})


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--reference', default=REF_DEFAULT, help='authorization_ref recorded on every operation (provenance, never a credential)')
    sub = p.add_subparsers(dest='command', required=True)
    g = sub.add_parser('gate'); g.add_argument('gate_command', choices=['init', 'declare', 'inspect']); g.add_argument('--universe')
    a = sub.add_parser('activate'); a.add_argument('--universe', required=True); a.add_argument('--host', required=True)
    c = sub.add_parser('cycle'); c.add_argument('--universe', required=True); c.add_argument('--active', required=True); c.add_argument('--standby', required=True)
    c.add_argument('--keep', type=int, default=3, help='quarantined copies kept on the standby')
    c.add_argument('--keep-points', type=int, default=3, help='recovery points the collector keeps on the active host whatever their age')
    c.add_argument('--minimum-age', type=int, default=3600, help='seconds a recovery point must be old before the collector may take it')
    t = sub.add_parser('takeover'); t.add_argument('--universe', required=True); t.add_argument('--active', required=True); t.add_argument('--standby', required=True)
    t.add_argument('--no-start', action='store_true', help='promote but leave the start to the operator')
    a.add_argument('--lease', type=int, default=20); a.add_argument('--margin', type=int, default=5); a.add_argument('--standbys', type=int, default=1)
    for s in (c, t):
        s.add_argument('--stop-timeout', type=int, default=10)
    args = p.parse_args()
    try:
        {'gate': cmd_gate, 'activate': cmd_activate, 'cycle': cmd_cycle, 'takeover': cmd_takeover}[args.command](args)
    except Refusal as e:
        out({'refused': str(e)}, 2)


if __name__ == '__main__':
    main()
