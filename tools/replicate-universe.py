#!/usr/bin/env python3
"""Replicate a universe to standby hosts: what to replicate to, run it now, run it on a schedule, stop.

A replication here is the warm-standby cycle of `tools/ha-standby.py cycle`: the universe is stopped for
a capture on its active host, captured as a recovery point, started again, and the point is carried to
each standby and restored there into quarantine, ready for a takeover. A stopped run therefore STOPS the
universe for the capture -- seconds for a small universe; the report measures it. A live run
(`--capture live`) never stops it: the universe is checkpointed with its memory by the qualified runtime
and resumed in place, the archive staged on each standby and promoted running at a takeover; the report
measures the interruption (the dump plus the resume). A live run needs the source image on each standby
and a universe the migration checks accept (network none, no mounts, bounded memory).

    tools/replicate-universe.py configure --universe U --active lab@… --hosts lab@…,lab@…,lab@… \\
                                          --standbys all|N [--interval 900] [--capture stopped|live]
    tools/replicate-universe.py run    --universe U      one replication now, to the configured standbys
    tools/replicate-universe.py start  --universe U      the schedule armed: a run every interval
    tools/replicate-universe.py stop   --universe U      the schedule disarmed; copies and policy stay
    tools/replicate-universe.py status --universe U      target, schedule, last run, every standby's copy
    tools/replicate-universe.py summary                  every configured universe from the ledger alone, no host reached

`--standbys all` replicates to every host but the active one; `--standbys N` to the N others with the most
available memory at configuration time (host_status), named in the report. `configure` declares a
lease-only activation policy on the active host when none exists, with a lease that outlives the
interval, and acquires it: a cycle renews a lease its active host holds, it does not take one.

The schedule is a timer on this workstation (systemd --user), because the workstation is the transport
controller and PodMesh never acts on its own: armed and disarmed by the operator, visible in `status`.
Settings, runs and copies live in the universe's ledger (PODMESH_HA_LEDGER, default ~/.podmesh-ha).
Environment: PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT for the hosts' service.

One JSON report on stdout; exit 0 on success, 1 on a refusal with its reason.
"""
import argparse, json, os, pathlib, subprocess, sys, tempfile, time, uuid

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'tests'))
from podmesh_two_hosts import Host, request  # noqa: E402

REF = 'replicate-universe-tool'
MAX_RUNS_KEPT = 20


class Refused(Exception):
    pass


def done(code, report):
    print(json.dumps(report, indent=2, sort_keys=True))
    sys.exit(code)


def ledger_path(u):
    root = pathlib.Path(os.environ.get('PODMESH_HA_LEDGER', pathlib.Path.home() / '.podmesh-ha'))
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    return root / f'{u}.json'


def load(u):
    p = ledger_path(u)
    return json.loads(p.read_text()) if p.is_file() else {'universe': u, 'cycles': [], 'rotations': []}


def save(u, ledger):
    p = ledger_path(u)
    tmp = p.with_suffix('.json.partial')
    tmp.write_text(json.dumps(ledger, indent=2, sort_keys=True))
    os.replace(tmp, p)


_control = tempfile.mkdtemp(prefix='podmesh-replicate-')


def host(target):
    return Host(target, target, _control, os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock'),
                os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh'), os.environ.get('PODMESH_UNIT', 'podmesh.service'))


def ok(h, req, what):
    r = h.api(req)
    if not r.get('ok'):
        raise Refused(f'{h.role}: {what}: {r.get("error")}')
    return r['data']


def unit_name(u):
    return f'podmesh-replicate-{u[:8]}'


def timer_state(u):
    unit = unit_name(u)
    active = subprocess.run(['systemctl', '--user', 'is-active', f'{unit}.timer'], capture_output=True, text=True).stdout.strip()
    show = subprocess.run(['systemctl', '--user', 'show', f'{unit}.timer', '-p', 'NextElapseUSecRealtime', '-p', 'LastTriggerUSec', '--value'],
                          capture_output=True, text=True).stdout.strip().splitlines()
    return {'unit': unit, 'armed': active == 'active', 'next': (show[0] if show else '') or None, 'last_trigger': (show[1] if len(show) > 1 else '') or None}


def cmd_configure(args):
    u = args.universe
    active = host(args.active)
    others = [t for t in dict.fromkeys(args.hosts.split(',')) if t and t != args.active]
    if not others:
        raise Refused('no standby host besides the active one')
    inv = ok(active, {'operation': 'inventory', 'operation_id': str(uuid.uuid4()), 'authorization_ref': args.reference}, 'inventory')
    if not any((c.get('Labels') or {}).get('io.podmesh.universe') == u for c in inv['containers']):
        raise Refused('the universe is not on the active host')
    if args.standbys == 'all':
        chosen, why = others, 'every host but the active one'
    else:
        n = int(args.standbys)
        if not 1 <= n <= len(others):
            raise Refused(f'--standbys must be all or 1 to {len(others)}')
        free = []
        for t in others:
            hs = ok(host(t), {'operation': 'host_status', 'operation_id': str(uuid.uuid4()), 'authorization_ref': args.reference}, 'host_status')
            free.append((hs.get('memory_available_bytes') or 0, t))
        chosen = [t for _, t in sorted(free, reverse=True)[:n]]
        why = f'the {n} other host(s) with the most available memory now'
    interval = args.interval
    lease = max(60, min(3600, interval * 3))
    status = ok(active, request('activation_status', u, args.reference), 'activation_status')
    if not status.get('requires_lease'):
        ok(active, request('activation_require', u, args.reference, lease_seconds=lease, takeover_margin_seconds=30,
                           desired_standbys=len(chosen)), 'activation_require')
        status = ok(active, request('activation_status', u, args.reference), 'activation_status')
    if not status.get('live') or status.get('holder_host_uuid') != active.identity:
        ok(active, request('activation_acquire', u, args.reference), 'activation_acquire')
    ledger = load(u)
    ledger['replication'] = {'active': args.active, 'hosts': args.hosts.split(','), 'standbys': chosen, 'mode': args.standbys, 'capture': args.capture,
                             'chosen_because': why, 'interval_seconds': interval, 'configured_at': int(time.time())}
    save(u, ledger)
    return {'result': 'configured', 'universe': u, 'active': args.active, 'standbys': chosen, 'chosen_because': why, 'capture': args.capture,
            'interval_seconds': interval, 'lease_seconds': status.get('lease_seconds') or lease}


def cmd_run(args):
    u = args.universe
    ledger = load(u)
    rep = ledger.get('replication')
    if not rep:
        raise Refused('the universe has no replication configured; configure it first')
    active = host(rep['active'])
    status = ok(active, request('activation_status', u, args.reference), 'activation_status')
    if not status.get('live'):
        ok(active, request('activation_acquire', u, args.reference), 'activation_acquire (the lease had lapsed)')
    argv = [sys.executable, '-B', str(HERE / 'ha-standby.py'), '--reference', args.reference, 'cycle', '--universe', u,
            '--active', rep['active'], '--standby', rep['standbys'][0], '--capture', rep.get('capture', 'stopped')] + sum((['--also', s] for s in rep['standbys'][1:]), [])
    t0 = time.time()
    p = subprocess.run(argv, capture_output=True, text=True, env=dict(os.environ))
    ledger = load(u)  # the cycle wrote its copies into the ledger
    try:
        report = json.loads(p.stdout)
    except ValueError:
        report = {'error': (p.stderr or p.stdout)[-600:]}
    entry = {'at': int(t0), 'seconds': round(time.time() - t0, 1), 'ok': p.returncode == 0, 'capture': rep.get('capture', 'stopped'),
             'stopped_for_seconds': report.get('stopped_for_seconds'), 'point': report.get('point'),
             'error': None if p.returncode == 0 else (report.get('refused') or report.get('error') or (p.stderr or p.stdout)[-400:])}
    ledger.setdefault('replication_runs', []).append(entry)
    ledger['replication_runs'] = ledger['replication_runs'][-MAX_RUNS_KEPT:]
    save(u, ledger)
    if p.returncode:
        raise Refused(f"the replication refused: {entry['error']}")
    return {'result': 'replicated', 'universe': u, 'capture': rep.get('capture', 'stopped'),
            **{k: report.get(k) for k in ('point', 'generation', 'copies', 'stopped_for_seconds', 'pruned_on_standbys', 'discarded_on_standbys')},
            'seconds': entry['seconds']}


def cmd_start(args):
    u = args.universe
    rep = load(u).get('replication')
    if not rep:
        raise Refused('the universe has no replication configured; configure it first')
    unit = unit_name(u)
    subprocess.run(['systemctl', '--user', 'stop', f'{unit}.timer', f'{unit}.service'], capture_output=True)
    subprocess.run(['systemctl', '--user', 'reset-failed', f'{unit}.timer', f'{unit}.service'], capture_output=True)
    env = [f'--setenv={k}={os.environ[k]}' for k in ('PODMESH_SOCKET', 'PODMESH_STATE_DIR', 'PODMESH_UNIT', 'PODMESH_HA_LEDGER') if k in os.environ]
    interval = rep['interval_seconds']
    p = subprocess.run(['systemd-run', '--user', f'--unit={unit}', f'--on-active={interval}', f'--on-unit-active={interval}',
                        '--timer-property=AccuracySec=5s', *env, sys.executable, '-B', str(HERE / 'replicate-universe.py'),
                        '--reference', args.reference, 'run', '--universe', u], capture_output=True, text=True)
    if p.returncode:
        raise Refused(f'the schedule could not be armed: {p.stderr.strip()[-300:]}')
    return {'result': 'armed', 'universe': u, 'interval_seconds': interval, 'timer': timer_state(u)}


def cmd_stop(args):
    u = args.universe
    unit = unit_name(u)
    subprocess.run(['systemctl', '--user', 'stop', f'{unit}.timer', f'{unit}.service'], capture_output=True)
    subprocess.run(['systemctl', '--user', 'reset-failed', f'{unit}.timer', f'{unit}.service'], capture_output=True)
    return {'result': 'disarmed', 'universe': u, 'timer': timer_state(u),
            'note': 'no more runs are scheduled; the copies on the standbys and the activation policy stay'}


def cmd_status(args):
    u = args.universe
    ledger = load(u)
    rep = ledger.get('replication')
    now = int(time.time())
    standbys = []
    for t in (rep or {}).get('standbys', []):
        mine = [c for c in ledger.get('cycles', []) if c.get('standby') and not c.get('pruned') and not c.get('discarded') and not c.get('promoted')]
        entry = {'host': t, 'copy': None}
        try:
            h = host(t)
            mine = [c for c in mine if c['standby'] == h.identity]
            inv = ok(h, {'operation': 'inventory', 'operation_id': str(uuid.uuid4()), 'authorization_ref': args.reference}, 'inventory')
            present = {(c.get('Labels') or {}).get('io.podmesh.universe') for c in inv['containers']}
            if mine:
                last = mine[-1]
                if last.get('capture') == 'live':
                    listed = ok(h, request('recovery_point_status', u, args.reference), 'recovery_point_status')
                    staged = {s['recovery_point_uuid']: s for s in listed.get('staged') or []}
                    s = staged.get(last['point']) or {}
                    entry['copy'] = {'point': last['point'], 'generation': last['generation'], 'capture': 'live', 'staged_at': last['staged_at'],
                                     'age_seconds': now - last['staged_at'], 'bytes': last.get('archive_bytes'),
                                     'present_on_host': bool((s.get('archive') or {}).get('present')) and not s.get('discarded_at')}
                else:
                    entry['copy'] = {'point': last['point'], 'generation': last['generation'], 'capture': 'stopped', 'restored_at': last['restored_at'],
                                     'age_seconds': now - last['restored_at'], 'bytes': last.get('rootfs_bytes'),
                                     'quarantined_uuid': last['quarantined_uuid'], 'present_on_host': last['quarantined_uuid'] in present}
            entry['copies_kept'] = len(mine)
        except Exception as e:  # noqa: BLE001 -- an unreachable standby is a fact to report
            entry['error'] = str(e)[-200:]
        standbys.append(entry)
    runs = ledger.get('replication_runs', [])
    return {'result': 'status', 'universe': u, 'configured': bool(rep), 'replication': rep, 'schedule': timer_state(u),
            'last_run': runs[-1] if runs else None, 'runs': runs[-5:], 'standbys': standbys}


def cmd_summary(args):
    root = ledger_path('probe').parent
    now = int(time.time())
    universes = {}
    for p in sorted(root.glob('*.json')):
        try:
            ledger = json.loads(p.read_text())
        except (OSError, ValueError):
            continue
        rep = ledger.get('replication')
        u = ledger.get('universe')
        if not rep or not isinstance(u, str):
            continue
        runs = ledger.get('replication_runs', [])
        copies = [c.get('restored_at') or c.get('staged_at') for c in ledger.get('cycles', [])
                  if c.get('standby') and not c.get('pruned') and not c.get('discarded') and not c.get('promoted') and (c.get('restored_at') or c.get('staged_at'))]
        last = runs[-1] if runs else None
        universes[u] = {'mode': rep.get('mode'), 'capture': rep.get('capture', 'stopped'), 'standbys': len(rep.get('standbys', [])), 'interval_seconds': rep.get('interval_seconds'),
                        'armed': timer_state(u)['armed'], 'last_copy_age_seconds': now - max(copies) if copies else None,
                        'last_run': {k: last.get(k) for k in ('at', 'ok', 'capture', 'stopped_for_seconds', 'error')} if last else None}
    return {'result': 'summary', 'universes': universes}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--reference', default=REF)
    sub = p.add_subparsers(dest='command', required=True)
    c = sub.add_parser('configure'); c.add_argument('--universe', required=True); c.add_argument('--active', required=True)
    c.add_argument('--hosts', required=True); c.add_argument('--standbys', required=True); c.add_argument('--interval', type=int, default=900)
    c.add_argument('--capture', default='stopped', choices=('stopped', 'live'), help='live: never stopped, checkpointed with memory and resumed in place')
    for name in ('run', 'start', 'stop', 'status'):
        s = sub.add_parser(name); s.add_argument('--universe', required=True)
    sub.add_parser('summary')
    args = p.parse_args()
    if args.command == 'configure' and not 60 <= args.interval <= 86400:
        done(1, {'result': 'refused', 'error': '--interval must be from 60 to 86400 seconds'})
    try:
        report = {'configure': cmd_configure, 'run': cmd_run, 'start': cmd_start, 'stop': cmd_stop, 'status': cmd_status, 'summary': cmd_summary}[args.command](args)
        done(0, report)
    except Refused as e:
        done(1, {'result': 'refused', 'error': str(e)})


if __name__ == '__main__':
    main()
