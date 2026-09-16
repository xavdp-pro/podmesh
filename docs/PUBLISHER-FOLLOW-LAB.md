# Publisher follow (laboratory)

Status: **lab mandate, 2026-09-16**. Not a production timer and not a second election.
Owner: Xavier de Poorter.

The publishing connector follows the governor (`MANAGER-PUBLISHER-CONTRACT.md`). The daemon
still **does not** start a connector by itself: `publisher_start` needs the authority's
takeover proof. What was missing for a standing laboratory hostname was a **host-side tick**,
under a written mandate, that applies that proof when **this host is eligible**, and stops
the connector when it is not.

## Decision (D1)

On `podmesh-dev-ha` only, the operator authorizes a follow tick analogous to `podmesh-fence`:

- It refuses to run without the mandate file.
- It never invents a proof. The gate's `rotate` / `attest-fence` document is installed on the
  host as a root-only file (`proof=` in the mandate).
- If `publisher_status` says eligible and the unit is already active, the tick is empty.
- If eligible, the unit is not active, and the proof's `new_epoch` matches the live lease
  epoch, it calls `publisher_start` with that proof.
- If not eligible and a connector is recorded or active, it calls `publisher_stop`.
- If `renew=1` and this host is eligible, the tick calls `activation_renew` so a standing
  laboratory hostname does not die with the first lease. A host that is not the holder
  never renews.
- Enabling the timer is the operator's; cleanup stops the timer and removes the mandate.

This does **not** rotate the epoch or publish the exclusive route. Those remain the
agent's (`tools/ha-standby.py`). After a rotation the agent must deliver the new proof
to the hosts (`python3 -B tools/arm-publisher-follow.py --refresh`). A lapsed lease still
withdraws through fence / reconciliation. The takeover proof itself still expires; a
connector that dies after that needs a new rotate, not this tick.

## What it is not

- Not automatic production HA and not a claim that Cloudflare is always on.
- Not a replacement for `check-manager-publisher.py`.
- Not packaged in experimental7; the script lives in `packaging/podmesh-publisher-follow`.

## Arming

Workstation: `python3 -B tools/arm-publisher-follow.py` with the usual M-U2 lab environment
and the private Cloudflare files. Re-apply without new replicas:
`python3 -B tools/arm-publisher-follow.py --refresh`. Evidence:
`podmesh-lab/cursor/publisher-follow/`. `--lease` on `ha-standby.py rotate` overrides the
ledger's previous policy (a 20 s leftover from a suite must not starve a standing demo).
