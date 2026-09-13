<!-- Relocated into the repository on 2026-09-14 from /tmp/podmesh-claude/, where it was
     cited as load-bearing by docs/BACKUP-SERVER.md. /tmp does not survive a reboot, and an
     operator decision must not rest on a file a restart deletes. Content is byte-identical to
     the original apart from this header; it carries no credential value. -->

# Independent counter-review — PodMesh Backup Server design

Date: 2026-09-13  
Reviewer: OpenAI Codex GPT-6 Astra  
Reviewed commit: `7e4e29440b82360eb5312ad25bff59d4b503b030`  
Verdict: **OPEN — sound direction, blocking design questions before B1**

## Material read

- `podmesh/docs/BACKUP-SERVER.md` at commit `7e4e294`
- `SHAPER-OS-V1.14/software/RULES.md`, read in full
- `SHAPER-OS-V1.14/LAW.md`
- `SHAPER-OS-V1.14/docs/agent/BOOT-CONTRACT.md`
- `shaper-three-layers/90-REVIEW/SCOPE-FEATURE-INVENTORY.md`
- PodMesh collector, migration, manager HA and delivery documents where they constrain
  backup, restore, retention, identity or activation

No service was started and no host was contacted. This is a design and contract review,
not runtime evidence.

## Overall assessment

The design has the right product boundary. It separates backup from manager
replication, treats replication as distinct from backup, requires restoration on a
different host, keeps ShaperOS optional, and refuses to call a successful write a
proven recovery point. Its explicit provisional status is honest.

The delivery order is also broadly correct: begin with one narrow round trip before
volumes, databases, retention, off-site replication and memory checkpoints.

However, B1 currently depends on unresolved semantics in consistency, encryption,
deduplication, capture authority, restore authority, retention and the canonical
transport rule. Those are design boundaries, not implementation details. B1 may be
prepared, but its public contract should not be frozen until BBS-R1 through BBS-R6 are
resolved.

## Blocking findings

### BBS-R1 — `crash-consistent` is currently an unsupported claim

**Classification:** technical design failure / untested assumption  
**Severity:** critical

Capturing pieces from a running universe without quiescing does not by itself produce
a crash-consistent recovery point. Files and volumes read sequentially can represent
different moments. A database dump, volume archive, configuration and overlay gathered
over minutes may never have existed together at any instant.

Required correction:

- reserve `crash-consistent` for an atomic or coordinated storage snapshot that is
  equivalent to one power-loss point;
- call an uncoordinated live multi-piece capture `incoherent` or `best-effort`, and do
  not qualify it as restorable until the application proves it;
- define a quiesce/freeze boundary and maximum freeze duration;
- bind every piece to one recovery-point UUID, capture generation, start/end time,
  source identity and snapshot boundary;
- record pieces that changed during capture as a failure, not as one recovery point.

Likewise, a database dump plus volumes "captured around it" is not automatically
application-consistent. The application must declare and execute its own pre-freeze,
dump, volume-snapshot and post-thaw protocol, with failure recovery.

### BBS-R2 — host-side encryption and cross-universe deduplication need one explicit cryptographic model

**Classification:** unresolved security architecture  
**Severity:** high

The document proposes plaintext chunk hashes, encrypted stored chunks, encryption on
the host, server opacity and cross-universe deduplication within a key domain. These
properties do not compose automatically.

With randomized authenticated encryption, identical plaintext chunks produce different
ciphertexts, so storage deduplication cannot simply retain one ciphertext. Deterministic
or convergent encryption enables deduplication but leaks equality and supports
confirmation attacks. A shared domain key increases deduplication and also increases
the blast radius of one compromised host. A per-universe key limits exposure but loses
cross-universe deduplication.

Required correction:

- define the encryption domain: tenant, fractal, universe, host or datastore;
- define who owns and can recover each domain key;
- define whether ciphertext is randomized or convergent;
- state the exact equality leakage and compromise boundary;
- bind chunk ID, ciphertext digest, algorithm, nonce, key ID, compression parameters
  and plaintext digest in the signed manifest;
- distinguish ciphertext verification by the server from plaintext verification by an
  authorized restore target;
- prove key escrow recovery from material stored separately from the datastore.

Do not call the leakage acceptable on behalf of the operator before this choice is
explicitly accepted.

### BBS-R3 — the host capture surface cannot remain read-only for every planned level

**Classification:** internal architecture contradiction  
**Severity:** high

D1 says that the server reads through a narrow read-only capture surface. That can work
for an already stopped B1 fixture. Database dumps, volume snapshots, application
quiescing and memory checkpoints create files or alter runtime state. They are not
read-only observations.

Required correction:

- separate **capture preparation** from **backup transfer**;
- let an authorized Maker prepare and seal a local immutable recovery point under a
  typed operation;
- let the Backup Server pull only the sealed manifest and immutable chunks;
- give the Backup Server no general Podman, signal, database or filesystem mutation
  authority on a host;
- retain an explicit abort/thaw path if preparation fails after quiescing;
- prove that a compromised Backup Server credential cannot command arbitrary host
  operations.

The pull principle and absence of an inbound listener on the Backup Server can remain.
The host-side surface and its authority must be described accurately.

### BBS-R4 — restore is an authority-bearing operation and currently lacks the PodMesh exclusion contract

**Classification:** missing safety contract  
**Severity:** critical

Restoring a universe with its UUID, IP, manager facts or memory can create a second
active copy. A different-host restore is not safe merely because the bytes and database
answer correctly.

Required correction:

- restore first into a quarantined, non-announcing state;
- require a new operation UUID and an explicit requested identity mode: original
  identity recovery or new cloned identity;
- refuse original UUID/IP activation until the current external epoch gate accepts it
  and the previous placement is excluded;
- never import a backed-up manager SQLite store directly as current authority;
- reconcile recovered manager facts with the surviving history and keep restored
  authority inactive until validated;
- keep DNS and routes unpublished until external observation proves the accepted
  placement;
- test stale backup restoration, live-source restoration, tombstoned identity,
  concurrent restore, old-manager return and repeated requests.

Backup proves recoverable bytes. It must never mint current authority.

### BBS-R5 — "a newer verified recovery point" is too weak to authorize retention removal

**Classification:** unsafe retention predicate  
**Severity:** high

A newer point may contain fewer Rule 16 levels, a weaker consistency class, a broken
key, a different namespace, an incomplete off-site copy, or only a shallow functional
test. Its existence does not justify removing an older useful point.

Required correction:

- require the replacement point to cover every protected level of the candidate with
  the same or stronger declared consistency and policy class;
- require successful ciphertext verification, key-recovery proof and the declared
  restore verification before it may replace an older verified point;
- preserve minimum counts and time buckets from policy after replacement proof;
- apply `evidence_hold` and `investigation_hold` with the scopes already decided for
  the collector;
- use a generation/lease barrier so mark-and-sweep cannot remove chunks referenced by
  an upload, verification, restore or concurrently published manifest;
- retain the removal decision, manifest and chunk-set digest permanently.

Age may select a candidate. A proof of strictly adequate replacement authorizes its
removal.

### BBS-R6 — the proposed transport conflicts with the current canonical Rule 12 wording

**Classification:** canon/design conflict  
**Severity:** high

Rule 12 currently requires archive transfers to use HTTP Basic Auth and end-to-end TLS
through Cloudflare Tunnel. The design instead relies on a private pull surface and
mentions WireGuard. WireGuard may be a sounder internal transport, but Rule 13 does not
silently replace Rule 12.

Required decision:

- either comply with the current Rule 12 transport requirements for every transfer to
  which they apply; or
- amend the canon explicitly so that authenticated encrypted private mesh transfer is
  an accepted archive transport, with Cloudflare Tunnel retained for its intended
  distribution case.

The Backup Server document must then state which rule applies to host capture,
datastore sync and operator download. Do not implement around the contradiction.

## Important non-blocking findings

### BBS-R7 — the manifest is the root of trust but has no complete contract

Define an immutable, versioned and authenticated manifest before the chunk store. It
must bind universe identity, recovery-point UUID, parent/generation, source, capture
operation, pieces, chunk order and sizes, ciphertext and plaintext commitments, key
IDs, consistency evidence, completeness states, software versions, exclusions,
verification history and hold state.

The datastore must rebuild its index from manifests and chunks after losing its own
database. Index loss must not make otherwise valid backups unreachable.

### BBS-R8 — Rule 16 level mapping is inaccurate

Image identity/content is shown as level `4 / 5`. Level 4 is Git code and architecture;
level 5 is the off-site copy of protected backup levels. Neither level is an image
registry. Record image digest/content as a separate recovery dependency, then declare
how it is protected. A stopped mount-free Podman filesystem export in B1 is a narrow
transport fixture; it is not Rule 16 level 2 and must not be counted as full Rule 16
coverage.

### BBS-R9 — namespaces are organization, not tenant isolation

Namespaces alone do not enforce confidentiality or deletion authority. Define
authentication, authorization, encryption domains, quotas, enumeration boundaries and
restore permissions independently. A compromised tenant host must not test or infer
chunk membership outside its allowed encryption domain.

### BBS-R10 — "incremental forever" needs bounded operational proof

Content-addressed manifests can avoid dependency chains, but re-chunking, verification,
index rebuilding and garbage collection still grow. Measure memory, CPU, disk, manifest
size, restore time and sweep time at realistic chunk counts. Do not freeze "no periodic
fulls" as a product promise before these measurements.

### BBS-R11 — bootstrap and disaster recovery of the Backup Server itself are missing

The server must recover without the live manager or manager-served DNS. Define pinned
bootstrap configuration, reconstruction of its index, recovery of signing/encryption
metadata, and operation from an off-site datastore copy. The Backup Server need not be
always available, but its loss must not make its backups unusable.

### BBS-R12 — partial success needs typed per-piece and overall outcomes

The document correctly says partial capture remains failed. Define exact states such as
`preparing`, `sealed`, `transfer_incomplete`, `stored_unverified`, `verified`,
`restore_verified`, `held`, and `retired`. A recovery point becomes eligible for normal
restore only after its required pieces are sealed and its manifest is committed. Never
promote a partial set because some chunks survived.

## Recommended corrected B1 boundary

B1 should remain intentionally small:

1. one stopped, mount-free, network-disabled Alpine universe;
2. one authorized Maker operation seals its configuration and filesystem fixture;
3. one versioned signed manifest and content-addressed encrypted chunk set;
4. Backup Server pulls through the selected canonical authenticated transport;
5. the source universe is removed only after the backup is safely stored;
6. restore creates a quarantined new-identity copy on another host;
7. external observation verifies files, configuration and application behavior;
8. the datastore index is deleted and rebuilt from immutable manifests/chunks;
9. the encryption key is recovered from separate operator material and the restore is
   repeated;
10. interrupted capture, transfer, manifest publication and restore are retried without
    duplicate identities or false success.

B1 must explicitly state that it does not prove persistent volumes, databases,
original-identity activation, DNS, memory continuity, off-site recovery, retention,
arbitrary Linux distributions, ShaperOS integration or Backup Server HA.

## Acceptance of the existing decisions

| Decision | Review result |
| --- | --- |
| D1 pull/no inbound Backup Server listener | Accept direction after BBS-R3 and BBS-R6 clarify host surface and transport |
| D2 content-addressed deduplication | Accept direction after BBS-R2 defines encryption domain and leakage |
| D3 host-side encryption | Open pending BBS-R2 and key recovery proof |
| D4 proof-based retention | Accept principle; replacement predicate must be strengthened by BBS-R5 |
| D5 periodic externally verified restore | Accept |
| D6 explicit Rule 16 completeness | Accept after correcting level mapping and consistency semantics |
| D7 narrow first workload | Accept after adopting the corrected B1 boundary |

## Verdict and next action

The document is a strong design proposal and does not make false implementation
claims. It is not ready to become the B1 implementation contract.

Claude should revise the design to resolve BBS-R1 through BBS-R6, incorporate BBS-R7
through BBS-R12 into the manifest and test contract, and return the exact decisions
that still require Xavier. A fresh independent review should then decide whether B1
may begin. Implementation, packaging, deployment and runtime qualification all remain
open.

## Operator model — recovery is agent-first

The Backup Server and its recovery bootstrap are operated by a human-agent tandem.
The human is not expected to execute a procedural recovery manual.

The recovery agent must be able to:

- inventory available hosts, datastores, manifests, keys by opaque identifier, and
  compatible restore targets;
- diagnose the failure without depending on the unavailable manager, Backup Server,
  or manager-served DNS;
- produce a typed recovery plan with expected effects, risks, rollback boundaries,
  and required authority;
- verify datastore integrity and rebuild the catalogue from immutable manifests;
- prepare and execute the restore through idempotent operations;
- keep the restored Backup Server quarantined until identity and activation are
  explicitly accepted;
- prove the final service, catalogue, key recovery, and sample restore from outside;
- retain a complete evidence trail for the human and supervising agent.

The human supplies intent and arbitrates exceptional authority: selecting the recovery
source when histories conflict, releasing separately held key material, accepting
original-identity takeover, or authorizing a destructive cleanup. Routine commands,
checks, retries, and evidence collection belong to the agent.

The bootstrap therefore needs a typed machine-readable API and CLI, a declarative
recovery intent, dry-run planning, stable operation IDs, explicit refusal reasons, and
structured evidence. An interactive shell recipe may document diagnostics, but it is
not the primary product interface.
