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
    tools/replicate-universe.py guard   --universe U [--lease 30 --margin 20 --tick 10] [--keep-stale]
                                                         continuity: the guardian renews the lease and fails over
    tools/replicate-universe.py unguard --universe U      the guardian disarmed (read the warning it prints)
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

`guard` is continuity of service under the operator's mandate. A `systemd --user` timer on this workstation
runs `guard-tick` every `tick` seconds: it renews the universe's lease on the active host (re-acquiring a lease
the guardian itself let lapse); when the active host cannot be reached, it waits for two failed ticks and
the last renewal attempt plus the lease plus the margin, then takes the universe over on the first standby that is reachable and
holds a copy, in the standbys' order; and it reintegrates a host that comes back after a takeover: a stale
copy still running there is a split-brain observed, stopped through the API and recorded; the stale copy is
then deleted (or kept with --keep-stale) so that the next replication stages a fresh one. The takeover never
starts a second instance while the old active host can be observed running the universe: a host reachable
over SSH whose service does not answer, with the universe still running there, is an incident for the
operator, not a failover. The hosts' own self-fence timer (packaging/podmesh-fence, enabled under a mandate)
is what stops the universe on a host cut from this workstation once its lease lapses; the margin is what
keeps the two apart. Recovery point objective: the replication interval. Recovery time objective: about
lease + margin + tick + the promotion. The guardian is one process on this workstation: if it stops, no lease
is renewed and every guarded universe is stopped by its host's fence after the lease -- never a second
instance, and no instance at all until the guardian is back.

One JSON report on stdout; exit 0 on success, 1 on a refusal with its reason.
"""
import argparse, contextlib, fcntl, json, os, pathlib, subprocess, sys, tempfile, time, uuid

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'tests'))
from podmesh_two_hosts import Host, request, transfer, control_dir  # noqa: E402

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


@contextlib.contextmanager
def locked(u, wait_seconds=30):
    """One mutation of a universe's ledger and hosts at a time: a run, a takeover and a guardian tick never
    interleave. A caller that cannot take the lock within the wait reports so rather than acting."""
    path = ledger_path(u).with_suffix('.lock')
    with open(path, 'a+') as f:
        deadline = time.monotonic() + wait_seconds
        while True:
            try:
                fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise Refused(f'another run, takeover or guardian tick holds the universe {u[:8]} for more than {wait_seconds} s; nothing was done')
                time.sleep(0.5)
        try:
            yield
        finally:
            fcntl.flock(f, fcntl.LOCK_UN)


_control = control_dir('podmesh-replicate-')


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
    with locked(args.universe):
        return run_once(args)


def run_once(args):
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
    try:
        p = subprocess.run(argv, capture_output=True, text=True, env=dict(os.environ), timeout=max(120, int(rep.get('interval_seconds') or 900) * 5))
    except subprocess.TimeoutExpired as e:
        p = subprocess.CompletedProcess(argv, 124, stdout=e.stdout or '', stderr='the cycle exceeded its bound and was killed')
    # The cycle renews the lease on the active host; the instant it ended bounds when that renewal was committed.
    write_renewal(u, rep['active'], time.time(), attempt_only=True)
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
    with locked(args.universe):
        return takeover_once(args)


def observe_container(target, u, seconds=8):
    """What Podman on a host says of the universe's container, read over SSH without the service: 'running',
    'stopped' (any non-running state), 'absent' (Podman says there is no such container), 'unreachable' (SSH
    cannot connect), or 'unknown' (anything else: a timeout, a Podman error). Only 'absent', 'stopped' and
    'unreachable' let a lost-host takeover proceed."""
    try:
        p = subprocess.run(['ssh', '-o', 'BatchMode=yes', '-o', f'ConnectTimeout={seconds}', '-o', 'ServerAliveInterval=3', '-o', 'ServerAliveCountMax=2', target,
                            f"sudo -n podman inspect --format '{{{{.State.Status}}}}' podmesh-{u}"], capture_output=True, text=True, timeout=seconds + 15)
    except subprocess.TimeoutExpired:
        return 'unknown'
    if p.returncode == 255:
        return 'unreachable'
    if p.returncode:
        return 'absent' if 'no such' in (p.stderr or '').lower() else 'unknown'
    return 'running' if p.stdout.strip() == 'running' else 'stopped'


def api_request(operation, u, reference, operation_id=None, **extra):
    """A typed request with a chosen operation ID, so that a resumed takeover replays instead of repeating."""
    return dict(operation=operation, operation_id=operation_id or str(uuid.uuid4()), universe_uuid=u, authorization_ref=reference, **extra)


def takeover_once(args):
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
    A = None if getattr(args, 'active_unreachable', False) else try_host(rep['active'])
    report = {'result': 'taken_over', 'universe': u, 'from': rep['active'], 'to': args.standby, 'planned': bool(args.planned),
              'active_reachable': A is not None, 'schedule_was_armed': armed}
    since = getattr(args, 'since', None)
    waited = 0.0
    if args.planned:
        if A is None:
            raise Refused('a planned switchover needs the active host; it cannot be reached, so this is a takeover of a lost host (without --planned)')
        switched = planned_switchover(args, u, rep, A, B, lease_seconds, margin)
        ledger = load(u)
        save_after_takeover(u, ledger, rep, args, t0, switched['point'], switched['capture'], renewed_at=time.time() - switched.get('promotion_seconds', 0))
        if armed:
            cmd_start(args)
        report.update(switched, schedule_rearmed=armed, seconds=round(time.time() - t0, 1))
        return report
    if A is not None:
        status = ok(A, request('activation_status', u, args.reference), 'activation_status on the active host')
        if status.get('live') and status.get('holder_host_uuid') == A.identity:
            raise Refused('the active host is reachable and holds a live lease: that is a planned switchover, not the takeover of a lost host')
        fenced = ok(A, {'operation': 'activation_fence', 'operation_id': str(uuid.uuid4()), 'authorization_ref': args.reference,
                        'timeout_seconds': FENCE_STOP_SECONDS}, 'activation_fence on the active host')
        report['fence'] = next((e for e in fenced.get('fenced', []) + fenced.get('left_running_or_absent', []) if e.get('universe_uuid') == u), None)
        if observe_container(rep['active'], u) == 'running':
            raise Refused('the active host still runs the universe after its fence; nothing was promoted')
        until = time.time() + 1
    else:
        # The service does not answer. Before assuming the host is gone, look at it without the service: a host
        # that answers SSH and still runs the universe is not fenced (a sick service cannot fence itself), and
        # starting elsewhere would make a second instance. That is the operator's call, never this tool's.
        seen = getattr(args, 'observed', None) or observe_container(rep['active'], u)
        report['old_active_observed'] = seen
        if seen == 'running':
            raise Refused('the active host answers SSH and still runs the universe, but its PodMesh service does not answer: '
                          'it cannot fence itself, so a takeover would start a second instance; fence it out of band (power it off) or repair its service first')
        if seen == 'unknown':
            raise Refused('the active host answers SSH but whether it still runs the universe cannot be read; nothing was promoted')
        # Any lease the host holds was last extended no later than the base: the last renewal attempt this
        # workstation made (or now, when nothing is recorded). The margin covers the host's fence period, its
        # stop, and the clock skew; the wait is on this workstation's clock, the base's own.
        base = float(since) if since else time.time()
        until = base + lease_seconds + margin + 1
    began = time.time()
    while time.time() < until:
        time.sleep(min(1.0, max(0.0, until - time.time())))
    waited = time.time() - began
    # Look at the old host again, after the wait and right before promoting: a host that came back during the wait
    # may be running the universe again, and a second instance is never this tool's decision.
    seen_after_wait = observe_container(rep['active'], u)
    report['old_active_observed_before_promotion'] = seen_after_wait
    if seen_after_wait in ('running', 'unknown'):
        raise Refused(f'the old active host is back and the universe there is {seen_after_wait} after the wait; nothing was promoted')
    copy = newest_copy(ledger, B.identity)
    if copy is None:
        raise Refused('the standby holds no copy of this universe; run a replication to it first')
    # The intent, durable before the standby is touched: a takeover interrupted after the promotion is completed
    # by the next attempt from what the standby shows, with the same operation IDs, never repeated.
    intent = ledger.get('takeover_intent')
    if not intent or intent.get('to') != args.standby or intent.get('point') != copy['point']:
        intent = {'to': args.standby, 'from': rep['active'], 'point': copy['point'], 'since': since, 'started_at': int(t0), 'schedule_was_armed': armed,
                  'promote_operation_id': str(uuid.uuid4()), 'start_operation_id': str(uuid.uuid4())}
        ledger['takeover_intent'] = intent
        save(u, ledger)
    ok(B, request('activation_require', u, args.reference, lease_seconds=lease_seconds, takeover_margin_seconds=margin,
                  desired_standbys=len(rep['standbys'])), 'activation_require on the standby')
    acquired_at = time.time()
    ok(B, request('activation_acquire', u, args.reference), 'activation_acquire on the standby')
    live = copy.get('capture') == 'live'
    began = time.time()
    if live:
        promoted = ok(B, api_request('recovery_point_promote', u, args.reference, intent['promote_operation_id'], recovery_point_uuid=copy['point']), 'recovery_point_promote')
    else:
        promoted = ok(B, api_request('recovery_point_promote', u, args.reference, intent['promote_operation_id'], restored_universe_uuid=copy['quarantined_uuid'],
                                     network_profile='isolated'), 'recovery_point_promote')
        ok(B, api_request('start', u, args.reference, intent['start_operation_id'], observe_seconds=1), 'start on the standby')
    promotion_seconds = time.time() - began
    ledger = load(u)
    rep = ledger['replication']
    copy = next(c for c in reversed(ledger['cycles']) if c.get('point') == copy['point'] and c.get('standby') == B.identity)
    copy['promoted'] = int(time.time())
    ledger.pop('takeover_intent', None)
    save_after_takeover(u, ledger, rep, args, t0, copy['point'], copy.get('capture', 'stopped'), renewed_at=acquired_at)
    if armed:
        cmd_start(args)
    prepared = copy.get('prepared_at') or 0
    report.update({'capture': copy.get('capture', 'stopped'), 'point': copy['point'], 'generation': copy['generation'],
                   'copy_age_seconds': int(time.time() - prepared) if prepared else None, 'data_lost': 'what the universe did after its copy was taken',
                   'waited_seconds': round(waited, 1), 'promotion_seconds': round(promotion_seconds, 3), 'with_memory': live,
                   'promoted_at': round(began + promotion_seconds, 3), 'acquired_at': round(acquired_at, 3),
                   'started': bool(promoted.get('started')) if live else True, 'old_active_copy': None, 'replayed': bool(promoted.get('replayed')),
                   'schedule_rearmed': armed, 'seconds': round(time.time() - t0, 1)})
    return report


def save_after_takeover(u, ledger, rep, args, t0, point, capture, renewed_at=None):
    old_active = rep['active']
    rep['active'] = args.standby
    rep['standbys'] = [s for s in rep['standbys'] if s != args.standby] + [old_active]
    ledger['replication'] = rep
    ledger.setdefault('takeovers', []).append({'at': int(t0), 'from': old_active, 'to': args.standby, 'planned': bool(args.planned),
                                               'point': point, 'capture': capture, 'reintegrated_at': int(t0) if args.planned else None})
    guard = ledger.get('guard')
    if guard:
        # The failover order follows the swap: the old active host goes last, where the replication put it.
        guard['order'] = [s for s in guard.get('order') or [] if s in rep['standbys']] + [s for s in rep['standbys'] if s not in (guard.get('order') or [])]
        # The new holder's lease was taken just now: that is the base of any later wait, never nothing.
        guard['last_renewed_at'] = int(renewed_at or time.time())
        guard['last_renewed_host'] = args.standby
        guard['failed_ticks'] = 0
        guard['state'] = 'guarding'
        write_renewal(u, rep['active'], renewed_at or time.time())
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


GUARD_UNIT_PREFIX = 'podmesh-guard-'
FENCE_TIMER = os.environ.get('PODMESH_FENCE_TIMER', 'podmesh-fence')
FENCE_MANDATE = os.environ.get('PODMESH_FENCE_MANDATE', '/etc/podmesh/fence-mandate')
TICKS_KEPT = 30
# The timing contract (docs: the continuity section of UNIVERSE-HIGH-AVAILABILITY.md). A lost host's universe is
# last writing at: its last committed renewal (no later than this workstation's last renewal ATTEMPT) + the lease
# + the fence timer's period and accuracy + the fence's own round trips + the stop grace its mandate gives. The
# standby is promoted at: the last attempt + the lease + the margin. So the margin must cover the fence's period,
# accuracy, overhead, stop grace, and the clock skew between the host and this workstation.
FENCE_PERIOD_SECONDS = 5
FENCE_ACCURACY_SECONDS = 1
FENCE_OVERHEAD_SECONDS = 2
CLOCK_SKEW_BUDGET_SECONDS = 5
FENCE_STOP_SECONDS = 2
FAILED_TICKS_BEFORE_FAILOVER = 2


def minimum_margin(stop_seconds):
    return FENCE_PERIOD_SECONDS + FENCE_ACCURACY_SECONDS + FENCE_OVERHEAD_SECONDS + int(stop_seconds) + CLOCK_SKEW_BUDGET_SECONDS


def guard_unit(u):
    return f'{GUARD_UNIT_PREFIX}{u[:8]}'


def guard_timer_state(u):
    unit = guard_unit(u)
    active = subprocess.run(['systemctl', '--user', 'is-active', f'{unit}.timer'], capture_output=True, text=True).stdout.strip()
    return {'unit': unit, 'armed': active == 'active'}


def renewal_path(u):
    return ledger_path(u).with_suffix('.renewal.json')


def read_renewal(u):
    try:
        return json.loads(renewal_path(u).read_text())
    except (OSError, ValueError):
        return {}


def write_renewal(u, active, instant, attempt_only=False, confirmed=False):
    """The instants the guardian's wait is based on, in a small file of their own: a renewal never waits for the
    ledger's lock, and a run or a takeover records its own renewals here too. `attempt` only ever moves forward
    for the same active host; a new active host starts a new record."""
    r = read_renewal(u)
    if r.get('active') != active:
        r = {'active': active}
    r['attempt'] = max(float(r.get('attempt') or 0), float(instant))
    if confirmed or not attempt_only:
        r['confirmed'] = max(float(r.get('confirmed') or 0), float(instant))
    tmp = renewal_path(u).with_suffix('.partial')
    tmp.write_text(json.dumps(r))
    os.replace(tmp, renewal_path(u))
    return r


def fence_state(target):
    """Whether a host fences itself: the mandate present (with the stop grace it gives) and the fence timer active,
    read over SSH; and what a fence would act on now, read through the service (activation_fence_preview)."""
    out = {'host': target, 'mandate_present': None, 'timer_active': None, 'stop_seconds': None, 'preview': None}
    try:
        p = subprocess.run(['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', '-o', 'ServerAliveInterval=3', '-o', 'ServerAliveCountMax=2', target,
                            f"sudo -n sed -n 's/^timeout_seconds=//p' {FENCE_MANDATE} 2>/dev/null | head -1 | sed 's/^/timeout=/'; sudo -n test -f {FENCE_MANDATE} && echo mandate; systemctl is-active {FENCE_TIMER}.timer"],
                           capture_output=True, text=True, timeout=40)
    except subprocess.TimeoutExpired:
        out['error'] = 'timed out'
        return out
    if p.returncode == 255:
        out['error'] = 'unreachable'
        return out
    lines = p.stdout.split()
    out['mandate_present'] = 'mandate' in lines
    out['timer_active'] = 'active' in lines
    timeout = next((l.split('=', 1)[1] for l in lines if l.startswith('timeout=')), '')
    out['stop_seconds'] = int(timeout) if timeout.isdigit() else (10 if out['mandate_present'] else None)
    try:
        h = host(target)
        out['preview'] = h.api({'operation': 'activation_fence_preview'}).get('data')
    except Exception as e:  # noqa: BLE001
        out['error'] = str(e)[-160:]
    return out


def record_tick(ledger, entry):
    ledger.setdefault('guard_ticks', []).append(entry)
    ledger['guard_ticks'] = ledger['guard_ticks'][-TICKS_KEPT:]


def incident(ledger, kind, **detail):
    entry = {'at': int(time.time()), 'kind': kind, **detail}
    incidents = ledger.setdefault('incidents', [])
    # A condition that persists tick after tick is one incident, counted, not one per tick.
    last = incidents[-1] if incidents else None
    same = lambda a, b: {k: v for k, v in a.items() if k not in ('at', 'first_at', 'repeated')} == {k: v for k, v in b.items() if k not in ('at', 'first_at', 'repeated')}
    if last and kind in ('failover_refused', 'no_standby_for_failover', 'takeover_resume_refused') and same(last, entry):
        last['repeated'] = int(last.get('repeated') or 1) + 1
        last['first_at'] = last.get('first_at') or last['at']
        last['at'] = entry['at']
        return last
    incidents.append(entry)
    ledger['incidents'] = ledger['incidents'][-50:]
    return entry


def cmd_guard(args):
    u = args.universe
    with locked(u):
        ledger = load(u)
        rep = ledger.get('replication')
        if not rep:
            raise Refused('the universe has no replication configured; configure it first (the guardian promotes the copies it makes)')
        lease, margin, tick = args.lease, args.margin, args.tick
        if not (5 <= lease <= 3600 and 5 <= margin <= 3600 and 2 <= tick and lease >= 3 * tick):
            raise Refused('lease 5-3600 s, margin 5-3600 s, tick at least 2 s and at most a third of the lease')
        fences = [fence_state(h) for h in [rep['active'], *rep['standbys']]]
        grace = max([f['stop_seconds'] for f in fences if f.get('stop_seconds') is not None] or [FENCE_STOP_SECONDS])
        needed = minimum_margin(grace)
        if margin < needed:
            raise Refused(f'a margin of {margin} s does not cover the fence: {FENCE_PERIOD_SECONDS} s period + {FENCE_ACCURACY_SECONDS} s accuracy + '
                          f'{FENCE_OVERHEAD_SECONDS} s overhead + {grace} s stop grace (the largest mandate here) + {CLOCK_SKEW_BUDGET_SECONDS} s clock skew = {needed} s at least')
        active = host(rep['active'])
        ok(active, request('activation_require', u, args.reference, lease_seconds=lease, takeover_margin_seconds=margin,
                           desired_standbys=len(rep['standbys'])), 'activation_require')
        attempt = time.time()
        write_renewal(u, rep['active'], attempt, attempt_only=True)
        status = ok(active, request('activation_status', u, args.reference), 'activation_status')
        if not status.get('live') or status.get('holder_host_uuid') != active.identity:
            ok(active, request('activation_acquire', u, args.reference), 'activation_acquire')
        else:
            ok(active, request('activation_renew', u, args.reference), 'activation_renew')
        write_renewal(u, rep['active'], attempt, confirmed=True)
        now = int(time.time())
        rep['lease_seconds'], rep['takeover_margin_seconds'] = lease, margin
        ledger['guard'] = {'lease_seconds': lease, 'takeover_margin_seconds': margin, 'tick_seconds': tick, 'keep_stale': bool(args.keep_stale),
                           'order': list(rep['standbys']), 'armed_at': now, 'last_tick': None, 'last_renewed_at': int(attempt), 'last_renewed_host': rep['active'],
                           'state': 'guarding', 'failed_ticks': 0, 'mandate': args.reference, 'minimum_margin_seconds': needed, 'fence_stop_seconds': grace}
        save(u, ledger)
        unit = guard_unit(u)
        subprocess.run(['systemctl', '--user', 'stop', f'{unit}.timer', f'{unit}.service'], capture_output=True)
        subprocess.run(['systemctl', '--user', 'reset-failed', f'{unit}.timer', f'{unit}.service'], capture_output=True)
        env = [f'--setenv={k}={os.environ[k]}' for k in ('PODMESH_SOCKET', 'PODMESH_STATE_DIR', 'PODMESH_UNIT', 'PODMESH_HA_LEDGER', 'PODMESH_FENCE_TIMER', 'PODMESH_FENCE_MANDATE', 'PODMESH_SSH_CONNECT_TIMEOUT') if k in os.environ]
        p = subprocess.run(['systemd-run', '--user', f'--unit={unit}', f'--on-active={tick}', f'--on-unit-active={tick}', '--timer-property=AccuracySec=1s',
                            f'--property=RuntimeMaxSec={lease + margin + 600}', *env,
                            sys.executable, '-B', str(HERE / 'replicate-universe.py'), '--reference', args.reference, 'guard-tick', '--universe', u], capture_output=True, text=True)
        if p.returncode:
            raise Refused(f'the guardian could not be armed: {p.stderr.strip()[-300:]}')
        unfenced = [f['host'] for f in fences if not (f.get('mandate_present') and f.get('timer_active'))]
        return {'result': 'guarded', 'universe': u, 'active': rep['active'], 'order': rep['standbys'], 'lease_seconds': lease, 'takeover_margin_seconds': margin,
                'minimum_margin_seconds': needed, 'tick_seconds': tick, 'keep_stale': bool(args.keep_stale), 'timer': guard_timer_state(u), 'fence': fences,
                'warning': (f'no self-fence on {", ".join(unfenced)}: a host cut from this workstation would keep the universe running while the '
                            'guardian starts it elsewhere; enable the fence timer under a mandate there before trusting the failover') if unfenced else None,
                'recovery_point_objective_seconds': rep.get('interval_seconds'),
                'recovery_time_objective_seconds': tick * FAILED_TICKS_BEFORE_FAILOVER + lease + margin + 20}


def cmd_unguard(args):
    u = args.universe
    with locked(u):
        ledger = load(u)
        unit = guard_unit(u)
        subprocess.run(['systemctl', '--user', 'stop', f'{unit}.timer', f'{unit}.service'], capture_output=True)
        subprocess.run(['systemctl', '--user', 'reset-failed', f'{unit}.timer', f'{unit}.service'], capture_output=True)
        rep = ledger.get('replication') or {}
        guard = ledger.get('guard') or {}
        restored = None
        if rep:
            # Back to the replication's own lease, which its scheduled runs renew, so that the fence does not stop
            # a universe nobody guards at the guardian's short lease.
            lease = max(60, min(3600, int(rep.get('interval_seconds') or 900) * 3))
            try:
                active = host(rep['active'])
                ok(active, request('activation_require', u, args.reference, lease_seconds=lease, takeover_margin_seconds=30,
                                   desired_standbys=len(rep.get('standbys', []))), 'activation_require')
                status = ok(active, request('activation_status', u, args.reference), 'activation_status')
                if not status.get('live') or status.get('holder_host_uuid') != active.identity:
                    ok(active, request('activation_acquire', u, args.reference), 'activation_acquire')
                else:
                    ok(active, request('activation_renew', u, args.reference), 'activation_renew')
                rep['lease_seconds'], rep['takeover_margin_seconds'] = lease, 30
                restored = {'lease_seconds': lease, 'takeover_margin_seconds': 30}
            except Refused as e:
                restored = {'error': str(e)}
        if guard:
            guard['state'] = 'disarmed'
            guard['disarmed_at'] = int(time.time())
        save(u, ledger)
        return {'result': 'unguarded', 'universe': u, 'timer': guard_timer_state(u), 'policy': restored,
                'warning': ('the universe stays under an activation policy: with the hosts\' fence timer enabled it is stopped when its lease '
                            f'lapses, which the replication schedule renews every run while armed (lease {restored.get("lease_seconds") if restored else "?"} s); '
                            'nothing takes over for it any more')}


def cmd_guard_tick(args):
    u = args.universe
    ledger = load(u)
    rep, guard = ledger.get('replication'), ledger.get('guard')
    if not rep or not guard or guard.get('state') == 'disarmed':
        raise Refused('the universe is not guarded')
    # 1. The renewal, outside the lock: it touches only the active host's lease and its own small file, so a
    # replication run holding the lock never makes the lease lapse on a healthy host. Skipped while a takeover
    # intent is pending: renewing the old active host then would re-arm the very lease the takeover waited out.
    renewal = {}
    if not ledger.get('takeover_intent'):
        renewal = renew_active(args, u, rep)
    try:
        with locked(u, wait_seconds=max(2, args.wait)):
            return guard_tick_once(args, renewal)
    except Refused as e:
        if 'holds the universe' in str(e):
            return {'result': 'skipped', 'universe': u, 'reason': str(e), 'renewal': renewal}
        raise


def renew_active(args, u, rep):
    out = {'active': rep['active']}
    A = try_host(rep['active'])
    if A is None:
        out['unreachable'] = True
        return out
    attempt = time.time()
    write_renewal(u, rep['active'], attempt, attempt_only=True)
    try:
        r = A.api(request('activation_renew', u, args.reference))
        if not r.get('ok') and 'expired' in (r.get('error') or ''):
            attempt = time.time()
            write_renewal(u, rep['active'], attempt, attempt_only=True)
            r = A.api(request('activation_acquire', u, args.reference))
            out['reacquired'] = True
    except Exception as e:  # noqa: BLE001 -- a session cut mid-call: the attempt is recorded, the answer is not known
        out['error'] = str(e)[-200:]
        return out
    if r.get('ok'):
        write_renewal(u, rep['active'], attempt, confirmed=True)
        out['renewed'] = True
        out['expires_at'] = (r.get('data') or {}).get('expires_at')
        try:
            out['universe'] = observed_state(A, u)
        except Exception as e:  # noqa: BLE001
            out['universe'] = f'unknown ({str(e)[-80:]})'
    else:
        out['error'] = r.get('error')
    return out


def observed_state(A, u):
    c = A.call('inspect', name='podmesh-' + u)['container']
    if not c:
        return 'absent'
    return 'running' if c['State'].get('Running') else c['State'].get('Status') or 'stopped'


def guard_tick_once(args, renewal):
    u = args.universe
    ledger = load(u)
    rep, guard = ledger.get('replication'), ledger.get('guard')
    if not rep or not guard or guard.get('state') == 'disarmed':
        raise Refused('the universe is not guarded')
    now = time.time()
    L, M = int(guard['lease_seconds']), int(guard['takeover_margin_seconds'])
    tick = {'at': int(now), 'active': rep['active'], 'renewal': renewal}
    outcome = 'renewed' if renewal.get('renewed') else 'failed'

    # 0. A takeover interrupted after it began: finished from what the standby shows, with the same operation IDs.
    intent = ledger.get('takeover_intent')
    if intent:
        outcome = resume_intent(args, u, ledger, rep, guard, intent, tick)
        ledger = load(u)
        rep, guard = ledger['replication'], ledger['guard']
    elif renewal.get('renewed'):
        guard['last_renewed_at'], guard['last_renewed_host'], guard['failed_ticks'] = int(read_renewal(u).get('confirmed') or now), rep['active'], 0
        # 1b. The universe must be running where it holds its lease. A late guardian (its lease lapsed and the host's
        # fence stopped it) or an application that exited are both brought back in place, and recorded.
        state = renewal.get('universe')
        if state and state != 'running' and not str(state).startswith('unknown') and renewal.get('active') == rep['active']:
            # Seen outside the lock, where a live capture's dump stops the universe for about a second: looked at
            # again now that no run, capture or switchover can be in flight, before anything is started.
            try:
                state = observed_state(host(rep['active']), u)
            except Exception as e:  # noqa: BLE001
                state = f'unknown ({str(e)[-80:]})'
            tick['observed_under_lock'] = state
        if state and state != 'running' and not str(state).startswith('unknown') and renewal.get('active') == rep['active']:
            kind = 'self_fenced_by_late_guardian' if renewal.get('reacquired') else 'universe_not_running'
            try:
                started = host(rep['active']).api(request('start', u, args.reference, observe_seconds=2))
                inc = incident(ledger, kind, host=rep['active'], observed=state, restarted=bool(started.get('ok')), error=started.get('error'),
                               memory='lost: started afresh from its disk state' if started.get('ok') else None)
            except Exception as e:  # noqa: BLE001
                inc = incident(ledger, kind, host=rep['active'], observed=state, restarted=False, error=str(e)[-200:])
            tick['incident'] = inc
            outcome = 'restarted_in_place' if inc.get('restarted') else 'down'
            guard['state'] = 'guarding' if inc.get('restarted') else 'down'
        else:
            guard['state'] = 'guarding'
    else:
        # 2 and 3. A failed tick counts; the failover waits for the lease and the margin since the last renewal
        # ATTEMPT (a renewal the host committed but whose answer was lost is covered), and needs consecutive failures.
        guard['failed_ticks'] = int(guard.get('failed_ticks') or 0) + 1
        r = read_renewal(u)
        since = float(r['attempt']) if r.get('active') == rep['active'] and r.get('attempt') else None
        tick['since'] = since
        if since is None:
            # Nothing recorded for this active host: the wait starts now, never from an older instant.
            write_renewal(u, rep['active'], now, attempt_only=True)
            since = now
        deadline = since + L + M
        tick['failover_deadline'] = round(deadline, 1)
        if now >= deadline and guard['failed_ticks'] >= FAILED_TICKS_BEFORE_FAILOVER:
            outcome = fail_over(args, u, ledger, rep, guard, since, renewal, tick)
            ledger = load(u)
            rep, guard = ledger['replication'], ledger['guard']
        elif guard.get('state') == 'guarding':
            guard['state'] = 'degraded'

    # 4. Reintegrate every old active host that answers again.
    reintegrate(args, u, ledger, rep, guard, tick)
    guard['last_tick'] = int(time.time())
    tick['outcome'] = outcome
    record_tick(ledger, tick)
    save(u, ledger)
    return {'result': outcome, 'universe': u, **tick}


def fail_over(args, u, ledger, rep, guard, since, renewal, tick):
    # The operator's order first, then every other current standby (a host reintegrated after a takeover included).
    candidates = [s for s in guard.get('order') or [] if s in rep['standbys']] + [s for s in rep['standbys'] if s not in (guard.get('order') or [])]
    seen = observe_container(rep['active'], u)
    tick['old_active_observed'] = seen
    if seen in ('running', 'unknown'):
        guard['state'] = 'failing_over'
        inc = incident(ledger, 'failover_refused', active=rep['active'], observed=seen,
                       error=('the active host answers SSH and still runs the universe, but its PodMesh service does not answer: a takeover would start a second instance'
                              if seen == 'running' else 'the active host answers SSH but whether it still runs the universe cannot be read'))
        tick['incident'] = inc
        save(u, ledger)
        return 'failover_refused'
    chosen, why = None, []
    for target in candidates:
        B = try_host(target)
        if B is None:
            why.append(f'{target}: unreachable')
            continue
        if newest_copy(ledger, B.identity) is None:
            why.append(f'{target}: no copy')
            continue
        chosen = target
        break
    if chosen is None:
        guard['state'] = 'failing_over'
        tick['incident'] = incident(ledger, 'no_standby_for_failover', active=rep['active'], reasons=why)
        save(u, ledger)
        return 'no_standby'
    guard['state'] = 'failing_over'
    save(u, ledger)
    t_args = argparse.Namespace(universe=u, standby=chosen, planned=False, reference=args.reference, since=since,
                                active_unreachable=not renewal.get('renewed') and renewal.get('unreachable', False), observed=seen)
    try:
        report = takeover_once(t_args)
    except Refused as e:
        ledger2 = load(u)
        ledger2['guard']['state'] = 'failing_over'
        tick['incident'] = incident(ledger2, 'failover_refused', active=rep['active'], standby=chosen, error=str(e)[-600:])
        save(u, ledger2)
        ledger.clear(); ledger.update(ledger2)
        return 'failover_refused'
    ledger2 = load(u)
    tick['failover'] = {k: report.get(k) for k in ('from', 'to', 'waited_seconds', 'promotion_seconds', 'with_memory', 'copy_age_seconds', 'promoted_at', 'replayed')}
    incident(ledger2, 'lost_host_failover', **{'from': report['from'], 'to': report['to'], 'copy_age_seconds': report.get('copy_age_seconds'),
             'waited_seconds': report.get('waited_seconds'), 'promotion_seconds': report.get('promotion_seconds'), 'with_memory': report.get('with_memory'),
             'old_active_observed': seen, 'since': since, 'promoted_at': report.get('promoted_at')})
    save(u, ledger2)
    return 'failed_over'


def resume_intent(args, u, ledger, rep, guard, intent, tick):
    """Decide an interrupted takeover from what its target shows: finish it (same operation IDs, replayed by the
    target), forget it (nothing happened there), or hold it for the operator."""
    target = intent['to']
    tick['intent'] = intent
    B = try_host(target)
    if B is None:
        tick['intent_target'] = 'unreachable'
        return 'intent_pending'
    seen = observed_state(B, u)
    status = B.api(request('activation_status', u, args.reference)).get('data') or {}
    holds = status.get('live') and status.get('holder_host_uuid') == B.identity
    if seen == 'absent' and not holds:
        ledger.pop('takeover_intent', None)
        save(u, ledger)
        tick['intent_resolved'] = 'nothing happened on the target; forgotten, the failover path decides again'
        return 'intent_forgotten'
    if seen in ('running', 'absent') or holds:
        # Promoted or partly done: the same takeover again, which replays what was done and finishes the rest.
        save(u, ledger)
        t_args = argparse.Namespace(universe=u, standby=target, planned=False, reference=args.reference, since=intent.get('since'),
                                    active_unreachable=True, observed='unreachable')
        try:
            report = takeover_once(t_args)
        except Refused as e:
            ledger2 = load(u)
            tick['incident'] = incident(ledger2, 'takeover_resume_refused', target=target, error=str(e)[-600:])
            save(u, ledger2)
            return 'intent_held'
        ledger2 = load(u)
        incident(ledger2, 'lost_host_failover', **{'from': report['from'], 'to': report['to'], 'resumed': True, 'replayed': report.get('replayed')})
        save(u, ledger2)
        return 'failed_over'
    ledger2 = load(u)
    tick['incident'] = incident(ledger2, 'takeover_resume_refused', target=target, observed=seen, holds_lease=bool(holds))
    save(u, ledger2)
    return 'intent_held'


def reintegrate(args, u, ledger, rep, guard, tick):
    for t in ledger.get('takeovers', []):
        if t.get('reintegrated_at') or t['from'] not in rep['standbys'] or t['from'] == rep['active']:
            continue
        seen = observe_container(t['from'], u)
        if seen in ('unreachable', 'unknown'):
            continue
        entry = {'host': t['from'], 'observed': seen}
        H = None
        try:
            H = host(t['from'])
            c = H.call('inspect', name='podmesh-' + u)['container']
            if c:
                entry['evidence'] = {k: c['State'].get(k) for k in ('Status', 'ExitCode', 'StartedAt', 'FinishedAt', 'Restored', 'Checkpointed')}
        except Exception as e:  # noqa: BLE001 -- SSH answers, the service does not: the stale copy waits for the service
            entry['error'] = str(e)[-200:]
            tick.setdefault('reintegration', []).append(entry)
            continue
        if seen == 'running':
            entry['incident'] = incident(ledger, 'split_brain_observed', host=t['from'], since_takeover_seconds=int(time.time()) - t['at'], evidence=entry.get('evidence'))
            stopped = H.api(request('stop', u, args.reference, timeout_seconds=FENCE_STOP_SECONDS, on_timeout='kill'))
            entry['stopped'] = {'ok': stopped.get('ok'), 'error': stopped.get('error'), 'forced': (stopped.get('data') or {}).get('forced')}
            if not stopped.get('ok'):
                tick.setdefault('reintegration', []).append(entry)
                continue
        if seen in ('running', 'stopped'):
            if guard.get('keep_stale'):
                entry['stale'] = 'kept for forensics (keep_stale)'
            else:
                deleted = H.api(request('delete', u, args.reference))
                entry['stale'] = {'deleted': deleted.get('ok'), 'error': deleted.get('error')}
                if not deleted.get('ok'):
                    tick.setdefault('reintegration', []).append(entry)
                    continue
        t['reintegrated_at'] = int(time.time())
        t['stale_copy'] = entry.get('stale', 'absent')
        incident(ledger, 'host_reintegrated', host=t['from'], observed=seen, stale_copy=entry.get('stale', 'absent'), evidence=entry.get('evidence'))
        tick.setdefault('reintegration', []).append(entry)


def guard_view(ledger):
    guard = ledger.get('guard')
    if not guard:
        return None
    now = int(time.time())
    r = read_renewal(ledger['universe'])
    armed = guard_timer_state(ledger['universe'])['armed']
    last_tick_age = now - guard['last_tick'] if guard.get('last_tick') else None
    tick_s = guard.get('tick_seconds') or 10
    health = ('disarmed' if guard.get('state') == 'disarmed' or not armed else
              'stale' if last_tick_age is None or last_tick_age > 3 * tick_s else
              'failing_over' if guard.get('state') == 'failing_over' else
              'down' if guard.get('state') == 'down' else
              'degraded' if (guard.get('failed_ticks') or 0) > 0 or guard.get('state') == 'degraded' else 'ok')
    base = float(r['attempt']) if r.get('active') == (ledger.get('replication') or {}).get('active') and r.get('attempt') else None
    return {**{k: guard.get(k) for k in ('lease_seconds', 'takeover_margin_seconds', 'minimum_margin_seconds', 'fence_stop_seconds', 'tick_seconds', 'keep_stale',
                                          'order', 'armed_at', 'last_tick', 'last_renewed_at', 'last_renewed_host', 'state', 'failed_ticks')},
            'armed': armed, 'health': health, 'takeover_intent': ledger.get('takeover_intent'),
            'last_tick_age_seconds': last_tick_age,
            'last_renewal_age_seconds': now - int(r['confirmed']) if r.get('confirmed') and r.get('active') == (ledger.get('replication') or {}).get('active') else None,
            'failover_after_seconds': round(base + guard['lease_seconds'] + guard['takeover_margin_seconds'] - now) if base else None,
            'incidents': (ledger.get('incidents') or [])[-5:], 'ticks': (ledger.get('guard_ticks') or [])[-3:]}


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
    fences = [fence_state(h) for h in ([rep['active'], *rep['standbys']] if rep else [])] if getattr(args, 'with_fence', True) else []
    return {'result': 'status', 'universe': u, 'configured': bool(rep), 'replication': rep, 'schedule': timer_state(u),
            'last_run': runs[-1] if runs else None, 'runs': runs[-5:], 'standbys': standbys, 'guard': guard_view(ledger),
            'fence': fences, 'takeovers': (ledger.get('takeovers') or [])[-5:]}


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
        guard = ledger.get('guard') or {}
        view = guard_view(ledger) if guard else None
        guarded = bool(view) and view['health'] not in ('disarmed',)
        universes[u] = {'mode': rep.get('mode'), 'capture': rep.get('capture', 'stopped'), 'standbys': len(rep.get('standbys', [])), 'interval_seconds': rep.get('interval_seconds'),
                        'armed': timer_state(u)['armed'], 'last_copy_age_seconds': now - max(copies) if copies else None,
                        'last_run': {k: last.get(k) for k in ('at', 'ok', 'capture', 'stopped_for_seconds', 'error')} if last else None,
                        'active': rep.get('active'), 'guarded': guarded, 'guard_state': guard.get('state') if guard else None, 'guard_health': view['health'] if view else None,
                        'guard_last_tick_age_seconds': now - guard['last_tick'] if guard.get('last_tick') else None,
                        'incidents': len(ledger.get('incidents') or []), 'last_incident': (ledger.get('incidents') or [None])[-1]}
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
    k.add_argument('--since', type=int, default=None, help='lost host only: the workstation-clock instant of the last renewal seen; the wait runs from it')
    g = sub.add_parser('guard'); g.add_argument('--universe', required=True); g.add_argument('--lease', type=int, default=30); g.add_argument('--margin', type=int, default=20)
    g.add_argument('--tick', type=int, default=10); g.add_argument('--keep-stale', action='store_true', help='keep a returning host\'s stale copy stopped for forensics instead of deleting it')
    ug = sub.add_parser('unguard'); ug.add_argument('--universe', required=True)
    gt = sub.add_parser('guard-tick'); gt.add_argument('--universe', required=True); gt.add_argument('--wait', type=int, default=5, help='seconds to wait for the universe lock')
    sub.add_parser('summary')
    args = p.parse_args()
    if args.command == 'configure' and not 60 <= args.interval <= 86400:
        done(1, {'result': 'refused', 'error': '--interval must be from 60 to 86400 seconds'})
    try:
        report = {'configure': cmd_configure, 'run': cmd_run, 'start': cmd_start, 'stop': cmd_stop, 'status': cmd_status, 'summary': cmd_summary, 'takeover': cmd_takeover,
                  'guard': cmd_guard, 'unguard': cmd_unguard, 'guard-tick': cmd_guard_tick}[args.command](args)
        done(0, report)
    except Refused as e:
        done(1, {'result': 'refused', 'error': str(e)})


if __name__ == '__main__':
    main()
