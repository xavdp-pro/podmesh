#!/usr/bin/env python3
"""Move a universe from one host to another: the migration chain run from the workstation, which is
the transport controller the protocol names (MIGRATION-PROTOCOL.md).

    tools/move-universe.py --source lab@… --destination lab@… --universe <uuid> [--keep-source]

In order, each step verified by the host that performs it and reported: the source's container
inspected (it must be network-disabled and mount-free -- the only shape the destination restore is
qualified for; a managed-network universe is refused here before anything is touched);
`migration_checkpoint` on the source (its memory and file state captured, the container stopped);
`migration_authorize_transfer` naming the destination host; the documents carried outbox to inbox
through this workstation with every byte's SHA-256 compared on both sides; `migration_destination_preflight`
then `migration_restore` on the destination (the universe runs there from where it was);
the outcome carried back; `migration_complete_transfer` on the source (its entitlement surrendered);
`migration_retire_source` (the stopped checkpointed container removed) unless --keep-source.

Measured on 2026-09-16: a universe whose only process is `sleep` may exit the instant it is
restored on a host whose monotonic clock is further along than the source's -- the sleep's deadline
is already past -- and the destination then refuses to verify a restore that left nothing running.
That is the protocol being honest about the workload, not a fault in the chain; a universe with a
process that does work (the suites' counters) moves and continues.

One JSON report on stdout, exit 0 on a completed move, 1 with the step that refused. A step that
fails leaves the universe where the protocol leaves it -- stopped and checkpointed on the source
until the destination has verified its restore -- and the report says which. Environment: the
service variables (PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT).
"""
import argparse, json, os, sys, tempfile, time, uuid

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '..', 'tests'))
from podmesh_two_hosts import Host, request, transfer  # noqa: E402


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--source', required=True)
    p.add_argument('--destination', required=True)
    p.add_argument('--universe', required=True)
    p.add_argument('--reference', default='move-universe-tool')
    p.add_argument('--keep-source', action='store_true', help='leave the stopped, checkpointed container on the source')
    args = p.parse_args()
    control = tempfile.mkdtemp(prefix='podmesh-move-')
    socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
    state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
    unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
    A = Host('source', args.source, control, socket_path, state_dir, unit)
    B = Host('destination', args.destination, control, socket_path, state_dir, unit)
    if A.identity == B.identity:
        return done(1, 'refused', 'the source and the destination are the same host', [])
    u, ref = args.universe, args.reference
    steps = []

    def step(name, host, req, **note):
        t0 = time.time()
        r = host.api(req)
        entry = {'step': name, 'host': host.role, 'operation': req.get('operation'), 'operation_id': req.get('operation_id'),
                 'ok': bool(r.get('ok')), 'seconds': round(time.time() - t0, 2), **note}
        if not r.get('ok'):
            entry['error'] = r.get('error')
            steps.append(entry)
            raise Refused(name, r.get('error') or 'refused')
        steps.append(entry)
        return r['data']

    class Refused(Exception):
        def __init__(self, step, error):
            super().__init__(error)
            self.step = step

    try:
        try:
            c = A.call('inspect', name='podmesh-' + u)['container']
        except RuntimeError:
            return done(1, 'refused', 'no such universe on the source', steps)
        if c['HostConfig'].get('NetworkMode') != 'none' or (c.get('Mounts') or []):
            return done(1, 'refused', 'only a network-disabled, mount-free universe is moved today: the destination restore is qualified for that shape alone; a managed-network universe is not moved yet', steps)
        if c['State']['Status'] != 'running':
            return done(1, 'refused', f"the universe is {c['State']['Status']}; a move captures a running universe", steps)
        steps.append({'step': 'inspect', 'host': 'source', 'ok': True, 'container_id': c['Id'], 'image': c['Image'], 'network_mode': 'none'})
        checkpoint_request = request('migration_checkpoint', u, ref, container_id=c['Id'], image='sha256:' + c['Image'].removeprefix('sha256:'),
                                     source_host_uuid=A.identity, destination_host_uuid=B.identity)
        step('checkpoint', A, checkpoint_request)
        authorization = step('authorize', A, request('migration_authorize_transfer', u, ref, checkpoint_operation_id=checkpoint_request['operation_id'],
                                                     destination_host_uuid=B.identity))
        auth_id = authorization['authorization_id']
        t0 = time.time()
        delivery = transfer(A, B, auth_id)
        steps.append({'step': 'carry', 'host': 'workstation', 'ok': True, 'seconds': round(time.time() - t0, 2),
                      'files': {f: v.get('sha256', '')[:12] for f, v in delivery['files'].items()}})
        step('preflight', B, request('migration_destination_preflight', u, ref, authorization_id=auth_id))
        restored = step('restore', B, request('migration_restore', u, ref, authorization_id=auth_id))
        transfer(B, A, auth_id, files=('outcome.json',))
        steps.append({'step': 'carry-outcome', 'host': 'workstation', 'ok': True})
        step('complete', A, request('migration_complete_transfer', u, ref, authorization_id=auth_id))
        if not args.keep_source:
            step('retire', A, request('migration_retire_source', u, ref, authorization_id=auth_id))
        now = B.call('inspect', name='podmesh-' + u)['container']
        return done(0, 'moved', f"the universe runs on the destination ({now['State']['Status']}) and its source is {'kept, stopped' if args.keep_source else 'retired'}",
                    steps, destination_state=now['State']['Status'], restored=restored.get('outcome') or restored.get('status'))
    except Refused as e:
        return done(1, 'refused', f'{e.step}: {e}', steps, note='the universe stays where the protocol leaves it: checkpointed and stopped on the source until a destination verifies its restore; migration_status on the source says which')


def done(code, result, message, steps, **extra):
    print(json.dumps({'result': result, 'message': message, 'steps': steps, **extra}, indent=2))
    sys.exit(code)


if __name__ == '__main__':
    main()
