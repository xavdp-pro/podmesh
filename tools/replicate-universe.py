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
    tools/replicate-universe.py takeover --universe U --standby lab@… [--planned]
                                                         the standby becomes the active host
    tools/replicate-universe.py summary                  every configured universe from the ledger alone, no host reached

`--standbys all` replicates to every host but the active one; `--standbys N` to the N others with the most
available memory at configuration time (host_status), named in the report. `configure` declares a
lease-only activation policy on the active host when none exists, with a lease that outlives the
interval, and acquires it: a cycle renews a lease its active host holds, it does not take one.

The schedule is a timer on this workstation (systemd --user), because the workstation is the transport
controller and PodMesh never acts on its own: armed and disarmed by the operator, visible in `status`.
Settings, runs and copies live in the universe's ledger (PODMESH_HA_LEDGER, default ~/.podmesh-ha).
Environment: PODMESH_SOCKET, PODMESH_STATE_DIR, PODMESH_UNIT for the hosts' service.

`takeover` makes a standby the active host. `--planned` is a switchover chosen while the active host is fine,
and it loses nothing: in live mode a FINAL capture (checkpointed with its memory, not resumed) is carried and
staged on the standby, the lease moves, and the standby promotes it running; in stopped mode the universe is
stopped, captured, restored into quarantine on the standby, promoted and started. The universe is interrupted
from the capture to the promotion, and the report measures it. If a step fails before the promotion, the
universe is brought back on the active host (resumed with its memory, or started) and the report says so.
The stopped copy left on the old active host is deleted after the promotion. Without it, the active host
is taken as lost: the tool refuses while that host is reachable and holds a live lease, fences it if it is
reachable, waits out the lease and the margin, then promotes the newest copy the standby holds. A live copy
comes back running with its memory; a stopped copy is promoted network-disabled and started afresh. The
ledger then names the standby as the active host and the old active host as a standby.

One JSON report on stdout; exit 0 on success, 1 on a refusal with its reason.
"""
import argparse, json, os, pathlib, subprocess, sys, tempfile, time, uuid

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'tests'))
from podmesh_two_hosts import Host, request, transfer  # noqa: E402

REF = 'replicate-universe-tool'
MAX_RUNS_KEPT = 20


class Refused(Exception):
    pass


class RolledBack(Refused):
    """A switchover that failed and has already been undone as far as it could be; never undone twice."""


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
                             'lease_seconds': status.get('lease_seconds') or lease, 'takeover_margin_seconds': status.get('takeover_margin_seconds') or 30,
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


def try_host(target):
    try:
        return host(target)
    except Exception:  # noqa: BLE001 -- an unreachable host is a fact the takeover acts on
        return None


def newest_copy(ledger, identity):
    mine = [c for c in ledger.get('cycles', []) if c.get('standby') == identity
            and not c.get('pruned') and not c.get('discarded') and not c.get('promoted')]
    return mine[-1] if mine else None


def cmd_takeover(args):
    u = args.universe
    ledger = load(u)
    rep = ledger.get('replication')
    if not rep:
        raise Refused('the universe has no replication configured; nothing records where its copies are')
    if args.standby not in rep['standbys']:
        raise Refused(f'{args.standby} is not a standby of this universe ({", ".join(rep["standbys"])})')
    t0 = time.time()
    B = try_host(args.standby)
    if B is None:
        raise Refused(f'the standby {args.standby} cannot be reached; nothing was changed')
    armed = timer_state(u)['armed']
    if armed:
        cmd_stop(args)
    lease_seconds = int(rep.get('lease_seconds') or max(60, min(3600, rep['interval_seconds'] * 3)))
    margin = int(rep.get('takeover_margin_seconds') or 30)
    A = try_host(rep['active'])
    report = {'result': 'taken_over', 'universe': u, 'from': rep['active'], 'to': args.standby, 'planned': bool(args.planned),
              'active_reachable': A is not None, 'schedule_was_armed': armed}
    waited = 0.0
    if args.planned:
        if A is None:
            raise Refused('a planned switchover needs the active host; it cannot be reached, so this is a takeover of a lost host (without --planned)')
        switched = planned_switchover(args, u, rep, A, B, lease_seconds, margin)
        ledger = load(u)
        save_after_takeover(u, ledger, rep, args, t0, switched['point'], switched['capture'])
        if armed:
            cmd_start(args)
        report.update(switched, schedule_rearmed=armed, seconds=round(time.time() - t0, 1))
        return report
    if A is not None:
        status = ok(A, request('activation_status', u, args.reference), 'activation_status on the active host')
        if status.get('live') and status.get('holder_host_uuid') == A.identity:
            raise Refused('the active host is reachable and holds a live lease: that is a planned switchover, not the takeover of a lost host')
        fenced = ok(A, {'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': args.reference,
                        'timeout_seconds': 10}, 'activation_fence on the active host')
        report['fence'] = next((e for e in fenced.get('fenced', []) + fenced.get('left_running_or_absent', []) if e.get('universe_uuid') == u), None)
        until = (status.get('expires_at') or 0) + (status.get('takeover_margin_seconds') or margin) + 1
        clock = A
    else:
        # Unreachable: any lease it holds ends at most lease_seconds after now; the margin is the clock-skew budget.
        until = B.call('time')['time'] + lease_seconds + margin + 1
        clock = B
    began = time.time()
    while clock.call('time')['time'] < until:
        time.sleep(1)
    waited = time.time() - began
    copy = newest_copy(ledger, B.identity)
    if copy is None:
        raise Refused('the standby holds no copy of this universe; run a replication to it first')
    ok(B, request('activation_require', u, args.reference, lease_seconds=lease_seconds, takeover_margin_seconds=margin,
                  desired_standbys=len(rep['standbys'])), 'activation_require on the standby')
    ok(B, request('activation_acquire', u, args.reference), 'activation_acquire on the standby')
    live = copy.get('capture') == 'live'
    began = time.time()
    if live:
        promoted = ok(B, request('recovery_point_promote', u, args.reference, recovery_point_uuid=copy['point']), 'recovery_point_promote')
    else:
        promoted = ok(B, request('recovery_point_promote', u, args.reference, restored_universe_uuid=copy['quarantined_uuid'],
                                 network_profile='isolated'), 'recovery_point_promote')
        ok(B, request('start', u, args.reference, observe_seconds=1), 'start on the standby')
    promotion_seconds = time.time() - began
    copy['promoted'] = int(time.time())
    save_after_takeover(u, ledger, rep, args, t0, copy['point'], copy.get('capture', 'stopped'))
    if armed:
        cmd_start(args)
    prepared = copy.get('prepared_at') or 0
    report.update({'capture': copy.get('capture', 'stopped'), 'point': copy['point'], 'generation': copy['generation'],
                   'copy_age_seconds': int(time.time() - prepared) if prepared else None, 'data_lost': 'what the universe did after its copy was taken',
                   'waited_seconds': round(waited, 1), 'promotion_seconds': round(promotion_seconds, 3), 'with_memory': live,
                   'started': bool(promoted.get('started')) if live else True, 'old_active_copy': None,
                   'schedule_rearmed': armed, 'seconds': round(time.time() - t0, 1)})
    return report


def save_after_takeover(u, ledger, rep, args, t0, point, capture):
    old_active = rep['active']
    rep['active'] = args.standby
    rep['standbys'] = [s for s in rep['standbys'] if s != args.standby] + [old_active]
    ledger['replication'] = rep
    ledger.setdefault('takeovers', []).append({'at': int(t0), 'from': old_active, 'to': args.standby, 'planned': bool(args.planned),
                                               'point': point, 'capture': capture})
    save(u, ledger)


def planned_switchover(args, u, rep, A, B, lease_seconds, margin):
    """A switchover that loses nothing. The universe is interrupted from its final capture to its promotion; a
    failure before the promotion brings it back on the active host, and is raised with what was done."""
    capture = rep.get('capture', 'stopped')
    status = ok(A, request('activation_status', u, args.reference), 'activation_status')
    if not status.get('live'):
        ok(A, request('activation_acquire', u, args.reference), 'activation_acquire (the lease had lapsed)')
    began = time.time()
    released = False
    step = 'final capture'

    def roll_back(reason, point=None, quarantined=None):
        done = {'failed_at': step, 'reason': reason}
        # Only while the standby runs nothing of the universe: the way back must never make a second instance.
        on_standby = B.call('inspect', name='podmesh-' + u)['container']
        if on_standby is not None:
            done['brought_back'] = 'not attempted: the standby holds a container for the universe; decide from what each host shows'
            raise RolledBack(f'the switchover failed at {step} and nothing was undone: {json.dumps(done)}')
        if released:
            done['lease_reacquired'] = bool(A.api(request('activation_acquire', u, args.reference)).get('ok'))
        if capture == 'live' and point:
            r = A.api(request('recovery_point_resume', u, args.reference, recovery_point_uuid=point))
        else:
            r = A.api(request('start', u, args.reference, observe_seconds=1))
        done['brought_back'] = {'ok': bool(r.get('ok')), 'error': r.get('error')}
        if quarantined:
            B.api(request('delete', quarantined, args.reference))
        raise RolledBack(f'the switchover failed at {step}; the universe was brought back on the active host: {json.dumps(done)}')

    if capture == 'live':
        final = A.api(request('recovery_point_prepare', u, args.reference, capture='live', resume=False))
        if not final.get('ok'):
            raise Refused(f"the final capture refused, and the universe is where it was: {final.get('error')}")
        point, data = final['data']['recovery_point_uuid'], final['data']
        dump_seconds = (data.get('capture') or {}).get('dump_seconds')
        try:
            step = 'transfer'
            transfer(A, B, point, files=('recovery-point-manifest.json', 'checkpoint.tar.zst'))
            step = 'staging'
            staged = B.api(request('recovery_point_stage', u, args.reference, recovery_point_uuid=point))
            if not staged.get('ok'):
                roll_back(staged.get('error'), point)
            step = 'lease move'
            ok(A, request('activation_release', u, args.reference), 'activation_release on the active host')
            released = True
            ok(B, request('activation_require', u, args.reference, lease_seconds=lease_seconds, takeover_margin_seconds=margin,
                          desired_standbys=len(rep['standbys'])), 'activation_require on the standby')
            ok(B, request('activation_acquire', u, args.reference), 'activation_acquire on the standby')
            step = 'promotion'
            promote_began = time.time()
            promoted = B.api(request('recovery_point_promote', u, args.reference, recovery_point_uuid=point))
        except RolledBack:
            raise
        except Exception as e:  # noqa: BLE001 -- any failure before the promotion rolls back
            roll_back(str(e)[-400:], point)
        if not promoted.get('ok'):
            roll_back(promoted.get('error'), point)
        promotion_seconds = time.time() - promote_began
        generation = data['generation']
    else:
        stopped = A.api(request('stop', u, args.reference, timeout_seconds=10, on_timeout='kill'))
        if not stopped.get('ok') or stopped['data'].get('forced') is not False:
            if stopped.get('ok'):
                A.api(request('start', u, args.reference, observe_seconds=1))
            raise Refused(f"the stop for the final capture escalated or refused; the universe was started again: {stopped.get('error') or stopped.get('data')}")
        dump_seconds = None
        quarantined = None
        try:
            step = 'final capture'
            prepared = ok(A, request('recovery_point_prepare', u, args.reference), 'recovery_point_prepare')
            point, generation = prepared['recovery_point_uuid'], prepared['generation']
            step = 'transfer'
            transfer(A, B, point, files=('recovery-point-manifest.json', 'rootfs.tar'))
            step = 'restore on the standby'
            quarantined = str(uuid.uuid4())
            ok(B, request('recovery_point_restore', quarantined, args.reference, recovery_point_uuid=point), 'recovery_point_restore')
            step = 'lease move'
            ok(A, request('activation_release', u, args.reference), 'activation_release on the active host')
            released = True
            ok(B, request('activation_require', u, args.reference, lease_seconds=lease_seconds, takeover_margin_seconds=margin,
                          desired_standbys=len(rep['standbys'])), 'activation_require on the standby')
            ok(B, request('activation_acquire', u, args.reference), 'activation_acquire on the standby')
            step = 'promotion'
            promote_began = time.time()
            promoted = B.api(request('recovery_point_promote', u, args.reference, restored_universe_uuid=quarantined, network_profile='isolated'))
            if promoted.get('ok'):
                step = 'start on the standby'
                started = B.api(request('start', u, args.reference, observe_seconds=1))
                if not started.get('ok'):
                    raise Refused(f"the universe was promoted on the standby but did not start there: {started.get('error')}; the active host was left stopped")
        except RolledBack:
            raise
        except Refused as e:
            if step == 'start on the standby':
                raise
            roll_back(str(e), None, quarantined)
        except Exception as e:  # noqa: BLE001
            roll_back(str(e)[-400:], None, quarantined)
        if not promoted.get('ok'):
            roll_back(promoted.get('error'), None, quarantined)
        promotion_seconds = time.time() - promote_began
    interruption = time.time() - began
    retired = A.api(request('delete', u, args.reference))
    return {'capture': capture, 'point': point, 'generation': generation, 'with_memory': capture == 'live', 'started': True,
            'copy_age_seconds': 0, 'data_lost': 'nothing: the capture was final', 'interruption_seconds': round(interruption, 2),
            'dump_seconds': dump_seconds, 'promotion_seconds': round(promotion_seconds, 3), 'waited_seconds': 0,
            'old_active_copy': {'deleted': bool(retired.get('ok')), 'error': retired.get('error')}}


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
    k = sub.add_parser('takeover'); k.add_argument('--universe', required=True); k.add_argument('--standby', required=True)
    k.add_argument('--planned', action='store_true', help='a switchover while the active host is fine: fresh copy, stop, promote, retire the old copy')
    sub.add_parser('summary')
    args = p.parse_args()
    if args.command == 'configure' and not 60 <= args.interval <= 86400:
        done(1, {'result': 'refused', 'error': '--interval must be from 60 to 86400 seconds'})
    try:
        report = {'configure': cmd_configure, 'run': cmd_run, 'start': cmd_start, 'stop': cmd_stop, 'status': cmd_status, 'summary': cmd_summary, 'takeover': cmd_takeover}[args.command](args)
        done(0, report)
    except Refused as e:
        done(1, {'result': 'refused', 'error': str(e)})


if __name__ == '__main__':
    main()
