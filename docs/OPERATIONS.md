# Operating PodMesh with the experimental tools

Status: experimental, development tree (not packaged). These tools run on a controller that reaches the hosts over
SSH and calls each host's local API; they are the agent's side of operations that the replicated manager is meant to
take over (see [IDEAL-SCENE.md](IDEAL-SCENE.md), policy 5: nothing indispensable on a workstation). Every tool prints
one JSON report and exits 0 on success, 1 on a refusal with its reason.

## Environment

| Variable | Meaning |
| --- | --- |
| `PODMESH_SOCKET` | The API socket on the hosts (default `/run/podmesh/api.sock`) |
| `PODMESH_STATE_DIR` | The service's state directory on the hosts (default `/var/lib/podmesh`) |
| `PODMESH_UNIT` | The service's systemd unit on the hosts (default `podmesh.service`) |
| `PODMESH_HA_LEDGER` | The controller's per-universe ledger directory (default `~/.podmesh-ha`) |
| `PODMESH_FENCE_TIMER`, `PODMESH_FENCE_MANDATE` | The self-fence timer's unit and mandate file on the hosts, read by `guard` and `status` |
| `PODMESH_SSH_CONNECT_TIMEOUT` | SSH connection timeout in seconds (default 15) |

Hosts are named by their SSH targets (`user@host`). A host's service must be reachable over SSH as a user allowed to
run `sudo -n`.

## Moving a universe

    tools/move-universe.py --source user@host-a --destination user@host-b --universe <uuid> [--keep-source]

Runs the migration chain: checkpoint on the source, transfer authorization, carriage with SHA-256 compared on both
sides, destination preflight and restore, completion and source retirement. Only network-disabled, mount-free
universes are qualified. A step that fails leaves the universe where the protocol leaves it, and the report names the
recovery operations.

## Replicating a universe to standby hosts

    tools/replicate-universe.py configure --universe <uuid> --active user@host-a --hosts user@host-a,user@host-b,user@host-c \
                                          --standbys all|N [--interval 900] [--capture stopped|live]
    tools/replicate-universe.py run     --universe <uuid>     # one replication now
    tools/replicate-universe.py start   --universe <uuid>     # a run every interval (a systemd --user timer)
    tools/replicate-universe.py stop    --universe <uuid>     # the schedule disarmed; copies and policy stay
    tools/replicate-universe.py status  --universe <uuid>     # target, schedule, last run, every standby's copy
    tools/replicate-universe.py summary                       # every configured universe, from the ledger alone

- **Stopped capture** stops the universe for the capture, restores the copy into quarantine on each standby, and a
  takeover starts it afresh.
- **Live capture** never stops the universe: it is checkpointed with its memory and resumed in place (interrupted for
  the dump and the resume), the archive is staged on each standby, and a takeover brings it back running with its
  memory. It needs the same image on every standby, no network, no mounts and bounded memory.
- `configure` declares a lease-only activation policy on the active host when none exists and acquires it.

## Taking a universe over

    tools/replicate-universe.py takeover --universe <uuid> --standby user@host-b --planned
    tools/replicate-universe.py takeover --universe <uuid> --standby user@host-b

- **Planned** (the active host is fine): loses nothing. In live mode a final capture (not resumed) is carried and staged,
  the lease moves and the standby promotes it running; in stopped mode the universe is stopped, captured, restored,
  promoted and started. If a step fails before the promotion, the universe is brought back on the active host.
- **Lost host** (without `--planned`): refused while the active host is reachable and holds a live lease, and refused
  while the active host answers SSH and still runs the universe; otherwise the active host is fenced if reachable, the
  lease and the margin are waited out, and the newest copy on the standby is promoted. What the universe did after that
  copy is lost.

## Guarding a universe (continuity of service)

    tools/replicate-universe.py guard   --universe <uuid> [--lease 30 --margin 20 --tick 10] [--keep-stale]
    tools/replicate-universe.py unguard --universe <uuid>

A timer on the controller renews the universe's lease every tick, restarts it in place when its lease holder does not
run it, takes it over on the first standby holding a copy after two failed ticks and the lease plus the margin since the
last renewal attempt, and reintegrates a returning host (its stale copy stopped if it runs, recorded, then deleted or
kept). `guard` refuses a margin that does not cover the hosts' self-fence (its period, accuracy, overhead, the stop grace
of its mandate, and the clock skew).

Limits that matter before arming it: the guardian runs on the controller, so the controller becomes indispensable; and
without the self-fence enabled on the hosts, a network cut can leave the cut host running the universe while the guardian
starts it elsewhere. Read [UNIVERSE-HIGH-AVAILABILITY.md](UNIVERSE-HIGH-AVAILABILITY.md), "Continuity of service under a
mandate", before arming either.
