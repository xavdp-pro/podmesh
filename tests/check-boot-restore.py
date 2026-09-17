#!/usr/bin/env python3
"""Restoring after a boot (vital target V1): a host brings back, by itself and without any peer, exactly the universes
its journal says should run, at most once each per boot -- proven by restarting its service, then by rebooting it.

Run on a controller with SSH access to ONE disposable lab host whose service is an installed unit that starts at boot,
with the restore-after-boot unit enabled under a written mandate on the host:

    PODMESH_REBOOT_SSH=user@host PODMESH_REBOOT_MANDATE=<the authorization_ref of that host's restore mandate> \\
    PODMESH_SOCKET=... PODMESH_STATE_DIR=... PODMESH_UNIT=... PODMESH_RESTORE_UNIT=... python3 -B tests/check-boot-restore.py

THIS SUITE REBOOTS THE HOST. It refuses to begin while any container runs there, unless the service unit is installed
and enabled, and unless the restore unit is enabled with a mandate naming PODMESH_REBOOT_MANDATE. Seven universes,
each in one state a boot must treat differently: running with no policy, stopped, paused, created and never started,
running under a lease longer than the reboot, running under a lease shorter than the reboot, and left stopped by a
final live capture. A lease held before the reboot entitles nothing after it: a universe under a lease comes back
only once whoever decides where it runs has acquired or renewed its lease during this boot. Every product action goes through the API; the reboot and the out-of-band kill of the last leg are
the only actions around it."""
import json, os, sys, tempfile, time, uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from podmesh_two_hosts import Host, request  # noqa: E402

REF = 'disposable-lab-boot-restore'
COUNTER = ['sh', '-c', 'token=$(cat /proc/sys/kernel/random/uuid); n=0; while :; do echo "$token $n" > /tmp/state; n=$((n+1)); sleep 1; done']

socket_path = os.environ.get('PODMESH_SOCKET', '/run/podmesh/api.sock')
state_dir = os.environ.get('PODMESH_STATE_DIR', '/var/lib/podmesh')
unit = os.environ.get('PODMESH_UNIT', 'podmesh.service')
restore_unit = os.environ.get('PODMESH_RESTORE_UNIT', 'podmesh-restore.service')
mandate = os.environ['PODMESH_REBOOT_MANDATE']
control = tempfile.mkdtemp(prefix='podmesh-boot-restore-')
checks, results = [], {}
A = Host('host', os.environ['PODMESH_REBOOT_SSH'], control, socket_path, state_dir, unit)
U = {name: str(uuid.uuid4()) for name in ('running', 'stopped', 'paused', 'created', 'lease_long', 'lease_short', 'final')}


def check(condition, label, detail=None):
    assert condition, (label, detail)
    checks.append(label)


def container(name):
    return A.call('inspect', name='podmesh-' + U[name])['container']


def started_at(name):
    c = container(name)
    return (c['State']['Status'], c['State']['StartedAt'])


def journal_operation(operation_id):
    rows = A.call('journal_row', table='operations', key_column='id', key=operation_id)['rows']
    return rows[0] if rows else None


def shell(command):
    return A.ssh(command, check=False).stdout.decode().strip()


def boot_restore(operation_id, observe_seconds=2):
    return A.api({'operation': 'boot_restore', 'operation_id': operation_id, 'authorization_ref': mandate, 'observe_seconds': observe_seconds})


def decision_of(result, name):
    return next((d for d in result['decisions'] if d['universe_uuid'] == U[name]), None)


try:
    # ------------------------------------------------------------------ preconditions: an idle host, units armed
    running = shell("sudo -n podman ps --format '{{.Names}}'")
    check(running == '', 'no container runs on the host before the suite', running)
    foreign = [p for p in A.ok({'operation': 'boot_restore_status'})['plan'] if p['would'] in ('restore', 'restored_earlier_this_boot')]
    check(not foreign, 'no universe already on the host would be restored by the reboot', foreign)
    check(shell(f'systemctl is-enabled {unit}') == 'enabled' and shell(f'systemctl show -p Transient --value {unit}') == 'no',
          'the service is an installed, enabled unit')
    check(shell(f'systemctl is-enabled {restore_unit}') == 'enabled', 'the restore unit is enabled')
    armed = shell(f"sudo -n systemctl cat {restore_unit} | sed -n 's/^ConditionPathExists=//p' | head -1")
    check(shell(f"sudo -n sed -n 's/^authorization_ref=//p' {armed}") == mandate, 'the restore mandate on the host names the expected authorization', armed)
    alpine = A.call('image_id', reference='docker.io/library/alpine:3.22')['image']

    # ------------------------------------------------------------------ seven universes, seven states
    for name in U:
        A.ok(request('create', U[name], REF, image='sha256:' + alpine, network_profile='isolated', command=COUNTER))
    A.ok(request('start', U['running'], REF))
    A.ok(request('start', U['stopped'], REF))
    A.ok(request('stop', U['stopped'], REF, timeout_seconds=5, on_timeout='kill'))
    A.ok(request('start', U['paused'], REF))
    A.ok(request('pause', U['paused'], REF))
    A.ok(request('activation_require', U['lease_long'], REF, lease_seconds=1800, takeover_margin_seconds=5))
    A.ok(request('activation_acquire', U['lease_long'], REF))
    A.ok(request('start', U['lease_long'], REF))
    A.ok(request('activation_require', U['lease_short'], REF, lease_seconds=10, takeover_margin_seconds=5))
    A.ok(request('activation_acquire', U['lease_short'], REF))
    A.ok(request('start', U['lease_short'], REF))
    A.ok(request('start', U['final'], REF))
    time.sleep(2)
    final = A.ok(request('recovery_point_prepare', U['final'], REF, capture='live', resume=False))
    check(final.get('final') is True and container('final')['State']['Status'] != 'running', 'the final capture left its universe stopped', final)
    states = {name: container(name)['State']['Status'] for name in U}
    check(states == {'running': 'running', 'stopped': 'exited', 'paused': 'paused', 'created': 'created', 'lease_long': 'running',
                     'lease_short': 'running', 'final': states['final']}, 'the seven states are set', states)

    plan = {p['universe_uuid']: p for p in A.ok({'operation': 'boot_restore_status'})['plan']}
    check(all(plan.get(U[n], {}).get('would') == 'already_running' for n in ('running', 'lease_long', 'lease_short')),
          'status: the three running universes are the only ones intended to run, and they already run', plan)
    check(not any(U[n] in plan for n in ('stopped', 'paused', 'created', 'final')), 'status: stopped, paused, created and final are not intended to run', plan)

    # ------------------------------------------------------------------ B3: a service restart touches nothing
    before = {n: started_at(n) for n in ('running', 'lease_long', 'lease_short', 'paused')}
    A.call('restart_with_fault')
    A.call('ready', seconds=60)
    after = {n: started_at(n) for n in before}
    check(after == before, 'a service restart leaves running and paused universes untouched', {'before': before, 'after': after})
    restart_pass = A.ok({'operation': 'boot_restore', 'operation_id': str(uuid.uuid4()), 'authorization_ref': REF, 'observe_seconds': 2})
    check(restart_pass['counts']['restored'] == 0 and all(decision_of(restart_pass, n)['decision'] == 'already_running' for n in ('running', 'lease_long', 'lease_short')),
          'a pass after the service restart starts nothing: the running universes are already running', restart_pass['counts'])
    boot_before = restart_pass['boot_id']
    check(all(journal_operation(f"boot-{boot_before.replace('-', '')}-{U[n].replace('-', '')}") is None for n in U),
          'no start was issued by that pass')
    results['service_restart'] = {'counts': restart_pass['counts']}

    # ------------------------------------------------------------------ B1 and B2: reboot the host
    reboot = A.reboot()
    results['reboot'] = reboot
    boot = reboot['boot_id_after']
    check(boot != boot_before and shell(f'systemctl is-active {unit}') == 'active' and shell(f'systemctl is-enabled {unit}') == 'enabled',
          'B1: the service came back by itself at boot', reboot)
    deadline = time.time() + 300
    while shell(f'systemctl show -p ActiveState --value {restore_unit}') == 'activating' or not shell(f'systemctl show -p ExecMainExitTimestampMonotonic --value {restore_unit}').strip('0'):
        assert time.time() < deadline, 'the restore unit did not finish'
        time.sleep(2)
    unit_state = {k: shell(f'systemctl show -p {k} --value {restore_unit}') for k in ('Result', 'ExecMainStatus')}
    check(unit_state == {'Result': 'success', 'ExecMainStatus': '0'}, 'the restore unit ran once at boot and succeeded', unit_state)
    pass_id = 'boot-restore-' + boot.replace('-', '')
    row = journal_operation(pass_id)
    check(row is not None and row['status'] == 'verified' and json.loads(row['request'])['authorization_ref'] == mandate,
          'the boot pass is journaled, verified, under the mandate', row)
    boot_pass = json.loads(row['result'])
    results['boot_pass_counts'] = boot_pass['counts']
    check(boot_pass['boot_id'] == boot and boot_pass['clock_synchronized'] is True, 'the pass ran in this boot with a synchronized clock', boot_pass.get('clock_synchronized'))

    d = decision_of(boot_pass, 'running')
    start_id = f"boot-{boot.replace('-', '')}-{U['running'].replace('-', '')}"
    start_row = journal_operation(start_id)
    check(d is not None and d['decision'] == 'restored' and d['start_operation_id'] == start_id and container('running')['State']['Status'] == 'running',
          'B2: the universe with no policy is running again, started by the pass', d)
    check(start_row and start_row['status'] == 'verified' and json.loads(start_row['request'])['authorization_ref'] == mandate,
          'its start names the mandate in the journal', start_row)
    for name in ('lease_long', 'lease_short'):
        d = decision_of(boot_pass, name)
        check(d is not None and d['decision'] == 'not_restored' and d['reason'] == 'lease_not_renewed_since_boot'
              and container(name)['State']['Status'] != 'running', f'the {name} universe is not restored on a lease from before the reboot', d)
    for name in ('stopped', 'paused', 'created', 'final'):
        check(decision_of(boot_pass, name) is None and container(name)['State']['Status'] != 'running', f'the {name} universe is not considered and stays stopped')

    # ------------------------------------------------------------------ a lease decided again; at most one start per boot
    replay = boot_restore(pass_id)
    check(replay.get('ok') and replay['data'].get('replayed') is True, 'the boot pass sent again is a replay', replay)
    A.ok(request('activation_renew', U['lease_long'], REF))
    lapsed = A.api(request('activation_renew', U['lease_short'], REF))
    check(not lapsed.get('ok') and 'expired' in lapsed.get('error', ''), 'a lease that lapsed during the reboot cannot be renewed', lapsed)
    A.call('podman_run', args=['kill', 'podmesh-' + U['running']])
    time.sleep(1)
    check(container('running')['State']['Status'] != 'running', 'the restored universe died out of band')
    # Another reference and another observation window: the start this boot already spent is still replayed, not refused.
    second = A.ok({'operation': 'boot_restore', 'operation_id': str(uuid.uuid4()), 'authorization_ref': REF, 'observe_seconds': 3})
    d = decision_of(second, 'running')
    check(d is not None and d['decision'] == 'restored_earlier_this_boot' and container('running')['State']['Status'] != 'running',
          'a second pass in the same boot, under other parameters, does not start it again: its start is replayed', d)
    d = decision_of(second, 'lease_long')
    check(d is not None and d['decision'] == 'restored' and container('lease_long')['State']['Status'] == 'running',
          'once its lease is renewed during this boot, the universe under a lease is restored by the next pass', d)
    d = decision_of(second, 'lease_short')
    check(d is not None and d['decision'] == 'not_restored' and d['reason'] == 'lease_not_renewed_since_boot', 'the lapsed lease still restores nothing', d)
    status = A.ok({'operation': 'boot_restore_status'})
    view = {p['universe_uuid']: p for p in status['plan']}
    check(view[U['running']]['would'] == 'restored_earlier_this_boot' and view[U['lease_long']]['would'] == 'already_running'
          and any(p['operation_id'] == pass_id for p in status['passes_this_boot']), 'status shows the pass of this boot and the starts already spent', view)

    print(json.dumps({'result': 'PASS', 'checks': checks, 'results': results,
                      'not_covered': ['a universe on the managed network, an epoch-gated universe, a migration reservation and an interrupted live capture: refused by construction, not exercised here',
                                      'a boot while the clock is not synchronized']}, indent=2))
finally:
    for name in U:
        A.api(request('stop', U[name], REF, timeout_seconds=5, on_timeout='kill'))
        A.api(request('delete', U[name], REF))
        A.call('podman_run', args=['rm', '--force', '--time', '0', 'podmesh-' + U[name]], check=False)
    for name in ('lease_long', 'lease_short'):
        A.api(request('activation_release', U[name], REF))
