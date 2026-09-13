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

## Evidence boundary

The result names the stable lab alias and public scopes. Configuration bodies,
replica identifiers, endpoints, keys and the private mapping are represented
only by salted commitments. A PASS proves the bounded configuration transition;
it does not prove activation, replication, convergence, failover, fencing, DNS,
takeover or high availability.

Run `tests/run-tests.sh` for local transformation and refusal tests. They create
no service, socket, database, systemd state or host connection.
