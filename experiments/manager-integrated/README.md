# Resident replication and external effect integration laboratory

Status: isolated local process qualification. No installed service, host deployment,
automatic takeover, DNS publication, Podman activation or HA claim.

This increment composes the existing Rust `manager-resident`, `manager-network`
and `manager-ha` experiments with the Python `manager-fencing` effect gate. It
changes none of those implementations. The Python standard-library harness is a
test fixture, not the production direction or an activation API for the resident.

## Perimeter and executed inventory

Design inputs: `../../INTENT.md`, `../../docs/CONTROL-SERVICES-UNIVERSE.md`,
`../../docs/MANAGER-HA-ACCEPTANCE.md` and the four dependency experiments.
The acceptance plan still requires real-host G4–G7 and complete G2/G3 qualification;
these local results do not promote any gate or whole scenario to deployed status.

| ID | Integrated capability | Acceptance |
| --- | --- | --- |
| MI-01 | One logical manager, three distinct host-bound resident copies | Three compiled processes, separate SQLite databases, exact immutable history equality and SHA-256 checks from external read-only connections |
| MI-02 | AP facts during peer partition | Six fixed TCP proxies; four links around live r2 are cut, including established sockets; r0/r1 exchange while r2 stores its own local fact |
| MI-03 | Exclusive effects have an external authority | Separate gate process and SQLite; one explicit fixture CAS transfer changes epoch 1/r2 to epoch 2/r0; no timeout or coordinator calls transfer |
| MI-04 | Gate enforcement survives stale callers | Copied permit, wrong fixed actor channel, concurrent stale/current requests and stale receipt replay; only current-epoch owner commits |
| MI-05 | Gate outage and restart | A fixture reachability switch refuses new effects; local observations continue; restarting the intact gate retains epochs and deduplication |
| MI-06 | Ambiguous Maker outcome | Child exits 73 after gate commit and before local fact publication; identical current-permit retry publishes one outcome fact without repeating the counter effect |
| MI-07 | Reconnection and old resident return | Full histories converge; old permit fails before and after SIGKILL/restart of r2; socket removal occurs only after collected child exit |
| MI-08 | Conflict retention and resource-local block | Two exclusive claims remain in all histories; identical conflict appears in all residents; adapter refuses the affected resource and independent facts still converge |
| MI-09 | Durable independent evidence | Retained stores, external effects/conflicts/counts, timestamps, IDs, process results, binary/source/config hashes and raw event log |
| MI-10 | Correlated gate outcomes and unknown-channel quarantine | Exact operation/authority/epoch/resource reply binding; mismatches and a genuinely queued late reply leave the channel unusable across Maker processes |

## Executable boundary

```text
trusted local fixture control ---- explicit transfer ----> external gate process
                                                          |
three short-lived Maker adapters -- fixed actor IPC ------> atomic counter+receipt
             |
             +-- reads private resident status before attempting effect
             +-- records immutable outcome after successful gate result
             |
three compiled resident processes <---- authenticated TCP snapshot exchange ---->
             |
             +-- three separate durable SQLite histories

external observer: fresh read-only SQLite connections to histories and gate log
```

Only the gate performs the modeled exclusive effect: a counter mutation and its
receipt in the same SQLite transaction. Its state is outside every resident and
Maker process. There is exactly one gate process; its database is neither replicated
nor rolled back. This remains an availability dependency for exclusive effects.
Each effect request must contact that gate, including deduplicated retries.

The harness pins an actor pair `(replica ID, Maker incarnation UUID)` to each gate
IPC channel at process creation. Request JSON cannot choose that identity. A
separate controller channel owns transfer. This is trusted local fixture wiring,
**not remote authentication or a security boundary against processes under the
same operating-system user**. The actor channels remain parent-owned across a clean
gate restart; the test sends no in-flight IPC during that restart.

Every effect success or refusal echoes its operation ID, authority ID, epoch and
resource. The client validates the complete typed reply and binding before returning
an outcome. A process-shared lock permits one request per actor channel; shared
uncertainty is set before sending and cleared only after a fully correlated reply.
A timeout, malformed or mismatched reply leaves that channel disabled for the rest
of the fixture. A killed lock holder also leaves it unusable. No later Maker process
may send another request or drain a late reply on that channel. There is no automatic
recovery or unsafe channel reset; a future recovery protocol needs a fresh trusted
channel plus explicit reconciliation of the original operation at the gate.

Maker adapters are new, short-lived processes with distinct per-replica local
SQLite caches. They inspect the configured resident socket, require its fixed
replica identity and `activation_authority=false`, reject a known conflict, then
use the existing Maker API to contact the gate. After success they call the existing
HA CLI to append a non-exclusive result fact. Replica identity is the dependency's
fixed local string; logical manager, hosts and Maker incarnations use run UUIDs.
No claim is made that resident process identity now provides a production
incarnation or enrollment protocol.

The result fact contains the operation ID, authority ID, resource, epoch, result and
a fixture evidence marker. The separately inspected gate receipt is the actual
proof. These facts replicate through the unmodified resident transport. Permit JSON
does not travel as an event or become authority from replication.
The acceptance oracle requires exactly one result fact per gate operation and exact
equality of `(operation_id, epoch, result, authority_id)` with its external receipt.

## Consistency and failure limits

- Reading resident conflict status is a conservative local precondition, not an
  atomic cross-system transaction. A new conflict can arrive after the read. The
  gate alone proves epoch exclusion; this increment does not prove instantaneous
  global conflict revocation or synchronize gate ownership from registry claims.
- An AP observation, replica health, coordinator priority or full history count
  never grants authority. The test compares full persisted histories and digests.
- The effect and its local result fact commit in separate databases. A crash can
  leave a gate receipt with no local fact. The test explicitly demonstrates this
  window and current-permit retry. Recovery after authority transfer, arbitrary
  database rollback, disk loss or loss of operation IDs remains unresolved.
- A stale grant cannot replay an old receipt as fresh authority. An unknown IPC or
  local-publication outcome is recorded as unknown, not converted into a claim that
  no effect happened. A known post-gate failure records `gate_committed=true`.
- The gate-unreachable case uses an explicit fixture link switch (`gate=None`);
  it is not a gate-network partition, one-way partition or timeout ambiguity test.
  Resident traffic uses real loopback TCP proxies, not firewall rules on three hosts.
- The gate restart is clean and uses intact current storage. The resident restart
  is a process kill. Neither proves physical power-loss durability or service-manager
  recovery. The operator-fixture transfer is not a distributed arbitration service.
- A counter protected atomically by its resource gate does not qualify route/IP,
  DNS, storage fencing or an asynchronous Podman start. No command adapter exists.
- Bounded snapshots, pairwise HMAC, static topology, absent key rotation, unsigned
  original provenance, missing receipt replication and finite gate quotas retain
  their dependency limitations. No zero-loss claim follows from the selected run.

## Run and inspect

Build the exact binaries before running. Rust remains the resident implementation;
Python only composes existing APIs into this disposable acceptance fixture.

```sh
cargo build --locked --manifest-path experiments/manager-ha/Cargo.toml
cargo build --locked --manifest-path experiments/manager-resident/Cargo.toml
cd experiments/manager-integrated
python3 -m unittest -v test_integration
python3 -m py_compile integrated_lab.py test_integration.py
```

The test prints a unique `/tmp/pm-integrated-...` directory and retains it on
success and failure. To choose a different parent, set
`PODMESH_INTEGRATED_EVIDENCE=/absolute/short/parent`; resident socket paths must remain
at most 100 bytes. The directory is created `0700` and includes throwaway pair keys
and permits. Keep raw configuration files private; they are not public artifacts.
Nothing under an existing directory is overwritten or cleaned up automatically.

`events.jsonl` records injected cuts, transfers, requests/outcomes and process
events with wall and monotonic timestamps. `external-effects.json` contains the gate
receipt sequence. `external-conflicts.json` records the three observed conflict
views. `external-stores.json` records schema versions, integrity and table counts.
`sha256.json` hashes closed stores, configs, evidence, binaries and participating
source/lockfiles. `run.json` records identities, source commit, dirty-tree status and
runtime versions: a source commit alone cannot identify an uncommitted increment.

The harness bounds process waits, socket operations, proxy workers and message
size. Failed setup and shutdown kill/reap owned processes and close owned proxies.
The fixture is not an untrusted service and does not attempt an adversarial
deployment test of these Python orchestration boundaries.

See [EVIDENCE.md](EVIDENCE.md) for the executed run and remaining qualification.
