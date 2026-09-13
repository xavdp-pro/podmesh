# Manager2 three-grant configuration transition

This operator-run harness performs the one local configuration mutation required
before the first G2 durable-store open. It replaces an exact zero-grant manager2
configuration with the same document carrying exactly these three grants:

| Scope | Owner supplied by the private mapping |
| --- | --- |
| `g2/lab-a/observations` | `lab-a` replica |
| `g2/lab-b/observations` | `lab-b` replica |
| `g2/lab-c/observations` | `lab-c` replica |

It does not activate the manager, open SQLite, start networking, contact another
host, or grant any control or external-effect authority. The scopes authorize
only the existing local observation operation.

## Private inputs

Create one root-owned mode `0600` mapping and use the same file on all three
hosts. Replica identifiers are private and never appear in public evidence.

```json
{
  "schema_version": "podmesh-manager-alias-replica-map/v1",
  "aliases": {
    "lab-a": "existing-replica-id-a",
    "lab-b": "existing-replica-id-b",
    "lab-c": "existing-replica-id-c"
  }
}
```

The mapping must describe exactly the three replicas already present in each
configuration. The selected host alias must map to that configuration's local
replica. Also provide one root-owned mode `0600` salt of at least 32 bytes. Keep
the mapping, salt, candidate verification report, backup, ledger and private
result directory out of Git. The backup is deliberately restricted to one
direct child of `/root`; nested backup paths are rejected so the durability
barrier always covers the actual parent directory.

## Apply locally

Run independently as root on each inactive host, changing only the alias and
backup name:

```sh
config-transition/transition-host.sh \
  --mode apply \
  --host-alias lab-a \
  --mapping /root/private-manager2-alias-map.json \
  --salt-file /root/private-manager2-transition-salt \
  --candidate-verification /root/private-manager2-candidate-verification.json \
  --backup-file /root/podmesh-manager2-before-g2-grants-lab-a.json \
  --evidence-directory /root/podmesh-manager2-g2-transition/lab-a
```

The command refuses unless the service is loaded, disabled, inactive and has no
process; the control socket and configured TCP/UDP port have no listener; and
the manager state directory is empty. It checks protected metadata, validates a
candidate as the `podmesh-manager` account through `--validate-config`, binds
the operation to the candidate report, installed package version, binary SHA-256
and clean `dpkg --verify` result, creates
a root-owned mode `0600` backup, atomically replaces the configuration, validates
the final path again, and records salted, secret-free evidence.

The ledger makes an interrupted apply resumable. A replay accepts only the
original configuration or the byte-exact generated candidate and refuses every
other state. It never overwrites an unbound backup.

After all three local applies, compare their secret-free result documents:

```sh
config-transition/compare-three-hosts.py \
  /private/evidence/lab-a/config-transition-result.json \
  /private/evidence/lab-b/config-transition-result.json \
  /private/evidence/lab-c/config-transition-result.json \
  > /private/evidence/config-transition-comparison.json
```

The comparator requires exactly the three aliases, one salted mapping
commitment and one identical ordered set of scope-owner commitments. It also
rechecks every local precondition and offline-validation boundary, and verifies
the SHA-256 sidecar beside every input before parsing it.

## Narrow rollback window

Rollback exists only before any activation and while the state directory remains
empty:

```sh
config-transition/transition-host.sh \
  --mode rollback \
  --host-alias lab-a \
  --mapping /root/private-manager2-alias-map.json \
  --salt-file /root/private-manager2-transition-salt \
  --candidate-verification /root/private-manager2-candidate-verification.json \
  --backup-file /root/podmesh-manager2-before-g2-grants-lab-a.json \
  --evidence-directory /root/podmesh-manager2-g2-transition/lab-a
```

Rollback requires the systemd activation markers captured at apply time to be
unchanged. It validates the retained original before atomic restoration and
again at the final path. It retains the backup and evidence.

Before **any** authorized activation, permanently close the rollback window:

```sh
config-transition/transition-host.sh \
  --mode seal-for-activation \
  --host-alias lab-a \
  --mapping /root/private-manager2-alias-map.json \
  --salt-file /root/private-manager2-transition-salt \
  --candidate-verification /root/private-manager2-candidate-verification.json \
  --backup-file /root/podmesh-manager2-before-g2-grants-lab-a.json \
  --evidence-directory /root/podmesh-manager2-g2-transition/lab-a
```

This writes a durable `sealed-for-activation` ledger state while holding the
same root-only lock as apply, rollback and the reviewed activation harness. The
activation harness additionally writes and fsyncs
`/etc/podmesh-manager/.manager2-activation-started` before `systemctl start`.
Transition rollback requires that permanent marker to be absent. A sealed
ledger can never roll back, even when database, lock and socket artifacts are
absent. This closes the qualified rollback window before first store open. An
out-of-band root process that bypasses the shared lock, marker and activation
workflow is malicious or operationally invalid and must not be represented as
a qualified activation.

## Operational transition after activation: `incoming_workers` 1 → 2

The second exact transition exists because live activation showed what one
incoming worker does: a resident at its limit accepts a second concurrent peer
connection and drops it without a reply, and the peer keeps that exchange as a
permanent uncertain attempt, so three replicas synchronizing with each other
every second never reach zero incomplete attempts. Each replica has one outgoing
worker and two declared peers, so a limit of two admits every peer.

Select it with `--transition incoming-workers` on all three tools. It changes
exactly the top-level key `incoming_workers`, from exactly `1` to exactly `2`,
in a configuration that already carries the three reviewed grants; every other
value must be preserved byte for byte, and the mapping must still bind the host
alias to the local replica.

Because an operational limit does not bind the durable store, this transition
may run after the manager has run: a non-empty state directory and the persistent
activation marker are **disclosed** in the evidence (`durable_state_present`,
`activation_marker_present`), not refused, and an existing marker must still name
this host alias, package version and binary hash. Everything else the first
transition requires still holds — loaded, disabled, inactive, no manager process,
no control socket, no listener on the configured port, protected metadata,
candidate binding and clean `dpkg --verify` — and the quiescence assertion also
requires that the state directory's listing and the marker's presence do not
change during the transition. Rollback restores the protected backup only while
the systemd activation markers captured at apply time are unchanged: once the
manager runs again they change, and the ledger refuses. Seal the ledger before
activation exactly as for the first transition.

```sh
config-transition/transition-host.sh \
  --transition incoming-workers \
  --mode apply \
  --host-alias lab-a \
  --mapping /root/private-manager2-alias-map.json \
  --salt-file /root/private-manager2-transition-salt \
  --candidate-verification /root/private-manager2-candidate-verification.json \
  --backup-file /root/podmesh-manager2-before-incoming-workers-2-lab-a.json \
  --evidence-directory /root/podmesh-manager2-incoming-workers/lab-a

config-transition/compare-three-hosts.py --transition incoming-workers \
  /private/evidence/lab-a/config-transition-result.json \
  /private/evidence/lab-b/config-transition-result.json \
  /private/evidence/lab-c/config-transition-result.json
```

The evidence is schema `podmesh-manager-config-transition-evidence/v2` and the
comparison `podmesh-manager-config-transition-comparison/v2`; each comparator
kind refuses the other kind's evidence. Without `--transition` every tool
behaves exactly as it did for the three-grant transition, byte for byte, except
that the ledger now records its `transition_kind`.

Three boundaries of this kind, stated rather than implied:

- **The closure document names itself.** Only the first-store-open transition can
  assert `state_directory_empty_at_closure`, because only it requires an empty
  store. Its closure stays `podmesh-manager-rollback-window-closure/v1` and is
  emitted by that kind alone; any other kind emits `…/v2`, which carries
  `transition_kind` and discloses `durable_state_present_at_closure` and
  `activation_marker_present_at_closure` instead.
- **The state listing is entry names only.** `state_directory_listing_unchanged`
  and `state_listing_commitment` record that the names in
  `/var/lib/podmesh-manager` were the same before and after the replace. They say
  nothing about the contents of the store, and nothing in this harness reads or
  writes it.
- **The rollback window closes at the seal, not at the next boot.** The activation
  markers are systemd values that reset when the host reboots, so an unsealed
  ledger surviving two reboots around an activation would present an unchanged
  marker hash and permit a rollback after the manager had run. Sealing before
  activation is what makes that impossible, and it is not optional.

## Evidence boundary

The result names the stable lab alias and public scopes. Configuration bodies,
replica identifiers, endpoints, keys and the private mapping are represented
only by salted commitments. A PASS proves the bounded configuration transition;
it does not prove activation, replication, convergence, failover, fencing, DNS,
takeover or high availability.

Run `tests/run-tests.sh` for local transformation and refusal tests. They create
no service, socket, database, systemd state or host connection.
