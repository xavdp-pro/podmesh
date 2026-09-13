# PodMesh Backup Server

Status: design direction, revised after independent counter-review; not implemented,
not validated. Nothing here is qualified.

Revision 2, 2026-09-13. The first revision was counter-reviewed by OpenAI Codex
(`/tmp/podmesh-claude/REVIEW-BACKUP-SERVER-CODEX-2026-09-13.md`, verdict OPEN with six
blocking findings). This revision answers BBS-R1 to BBS-R6 and folds BBS-R7 to BBS-R12
into the manifest, state and test contract. Three of the blocking findings were errors
of mine and are named as such where they are corrected. **Four decisions remain the
operator's and are listed at the end; B1's public contract must not be frozen until
they are taken.**

## Purpose

Store versioned universe recovery points independently of live replicas. Support
configuration, image identity or content, persistent volumes, database state and
optional memory checkpoints.

**Replication is not backup.** It propagates deletion and corruption faithfully and at
speed. The manager's replicated history, the migration chain and this service answer
three different questions: where a universe *is*, how it *moves*, and what it *was*.

**Backup proves recoverable bytes. It never mints current authority.** That sentence is
the reviewer's, and it is the one this revision is built around.

## What the canon already decides

Two rules of `SHAPER-OS-V1.14/software/RULES.md` bind this service. They are not
re-decided here; several were written after real incidents.

**Rule 16 — the five levels.** Container, volumes, database, git, off-site. "This set
is enough. Missing a level is a hole."

- Level 2 is the **volumes, not the overlay**. `nosav/`, caches, image layers and
  `node_modules` are excluded. `app/` is code in git and is *not* a substitute for a
  volume backup.
- Level 3 is a **real dump**, not a live volume tar.
- Level 4 is git, and **git is never treated as a data backup**.
- Level 5 is the off-site copy of levels 2 and 3, and optionally 1.

**Rule 12 — archive hygiene.** Every clause is a failure mode this service must make
impossible rather than merely avoid: the key never travels with the coffer; the
backup's key is its own key, refused if it equals the vault master key, and reaches the
tool through the environment and never as an argument; a dump that was not taken is
announced rather than written empty; the archive command's failure is the backup's
failure; a failure after the archive completes keeps the archive; a failed dump leaves
nothing behind (`.part` then rename); `.env` is excluded in every spelling; and how the
tool is called is proven with a recorder on a PATH built from scratch.

Rule 12 **also governs transport**, and that is where this design collided with the
canon — see D6 and Decision X3.

Rule 20's closed-loop quality gate applies: the delivered interaction is exercised, not
simulated.

**Rule 10 — no duration, anywhere.** `RULES.md:555-559`: *"No document, pitch, README,
doctrine page, or client-facing sentence in this repository states a restore or
cold-boot duration — **not even to dismiss it**. A number quoted in order to be refuted
still gets lifted out of its paragraph and quoted back as a promise."* The sanctioned
formulation is qualitative: **restore is fast and structured**. The single exception is
a measured observation inside an operational log or deployment note, which must state
its exact conditions — cache state, data volume, what is excluded — and must be
**explicitly labelled an observation, never an engagement**.

This binds this service's own output. D5 and the proof list below require *measuring*
recovery time; that measurement lives in an operational record under the exception
above, and no figure derived from it may enter this document, a README, a status
message or anything a client reads. We promise the method, never the stopwatch.

**Rule 11 — images are never backed up**, and the universe is the backup unit. See the
pieces table below; this one changed the design.

**Rule 31 — the universe declares its own data lifecycle**, and erasure is fractal. See
its own section; this one is missing from the design entirely and is Decision X5.

**Rule 30 — the Backup Server has a named caller.** Before any data-bearing change, a
**full snapshot taken immediately before it**, never a nightly backup "close enough";
that snapshot is **restored into an ephemeral sandbox and proven loadable** before the
change proceeds; and *rollback is restoring the snapshot*, not running a reverse
script. A fleet schema change across N per-universe databases is **N migrations, each
with its own snapshot** — one shared transaction is forbidden. This service must make
that workflow cheap enough that nobody is tempted to skip it.

## What we take from Proxmox Backup Server, and what we do not

**Taken.** A content-addressed, deduplicating chunk store — universes are
near-identical by construction, so dedup is the entire economy of the thing. Verify
jobs that re-read and re-checksum independently of any backup or restore. Datastore-to-
datastore sync as the mechanism for Rule 16's level 5. Incremental transfer by content
addressing, with the caveat in BBS-R10 below.

**Not taken.** PBS's push model (D1). Retention purely by schedule (D4). Tape and
live-restore-from-the-datastore, both out of scope.

**Qualified, not promised (BBS-R10).** "Incremental forever" is a design intention, not
a product claim. Re-chunking, verification, index rebuilding and sweeping all grow with
the store. Memory, CPU, disk, manifest size, restore time and sweep time must be
measured at realistic chunk counts before "no periodic fulls" is stated anywhere a
reader could rely on it. PodMesh has no dirty-bitmap equivalent, so incrementality
costs a full re-read of the source at every capture.

## A recovery point is not a disk

A PBS backup is a disk image and a config file. A PodMesh recovery point is a set of
pieces that are only meaningful together:

| Piece | Rule 16 level | Notes |
| --- | --- | --- |
| Universe identity and its PodMesh journal facts | — | UUID, creation operation, ownership |
| Container configuration | — | command, labels, network, mounts, resources |
| Persistent volumes | 2 | volumes only, `nosav/` and `.env*` excluded |
| Database dumps | 3 | per Rule 12's dump laws |
| Memory checkpoint | — | optional, only where the runtime permits |
| Image **identity** — the digest and the lock, never the layers | **none** | a *recovery dependency*; the canon forbids carrying the content — see below |

**Correction (BBS-R8), and a second correction the canon forced.** Revision 1 mapped
image identity to "level 4 / 5". That was wrong: level 4 is git code and architecture,
level 5 is the off-site copy of protected levels, and neither is an image registry.

Revision 2 then said the manifest could "declare explicitly whether this recovery point
carries the content itself". **That is forbidden.** Rule 11 at `RULES.md:677-683` is
explicit:

> A universe's restorable identity is its `manifest.json`, its `cfg-image-lock.json`
> and its volumes. **Images are never backed up**: they are rebuilt from source at the
> recorded commit, or pulled by the digest the lock names. A backup containing images
> is backing up a derivative, and hiding that the original may no longer be
> reproducible.

So a recovery point carries the **manifest, the image lock and the volumes**, and never
an image layer. The reason matters more than the rule: hoarding layers would let a
universe stay restorable while its source became unbuildable, and nobody would notice
until the layers were lost too. An image that can no longer be rebuilt at its recorded
commit or pulled by its locked digest is a **reproducibility defect that must surface**,
and the manifest's job is to make it visible — not to paper over it with a copy.

The same rule fixes the unit: the **universe** is what can be snapshotted, exported and
restored as one thing (`RULES.md:569-571`). A brick is an application container built
from an immutable image, and is not a backup unit.

And restoration is not finished when the bytes are back: Rule 11 ends it with the
universe's own `deploy/proof.sh` — *"a restore nobody proved is a claim"*.

## Consistency: what a capture may honestly claim

**Correction (BBS-R1), and this was the worst error in revision 1.** Revision 1 called
an uncoordinated live capture "crash-consistent". It is not. Pieces read sequentially
over minutes may never have existed together at any instant, and a manifest asserting
crash consistency for such a set would be a false guarantee written into evidence.

Four classes, and a capture claims the weakest one it can prove:

- **`incoherent`** — pieces gathered from a running universe with no coordination.
  This is what a naive live capture produces. It is stored and it may be useful, but it
  is **not qualified as restorable** until a restore of that specific point has been
  proven. It must never be presented as a recovery guarantee.
- **`crash-consistent`** — reserved for an **atomic or coordinated storage snapshot**
  equivalent to a single power-loss instant. Requires a snapshot mechanism (LVM, ZFS,
  Btrfs, or a runtime freeze) that gives one boundary across every piece.
- **`application-consistent`** — the application declares and executes its own
  protocol: pre-freeze, dump, volume snapshot, post-thaw, with a defined failure
  recovery. A database dump with volumes "captured around it" does **not** qualify by
  itself; the ordering must be enforced, not observed.
- **`memory-coherent`** — a memory checkpoint bound to the exact disk state it was
  taken against.

Every capture therefore records a **quiesce boundary**: the mechanism used, the instant
it opened, the instant it closed, and a **maximum freeze duration** after which the
capture aborts and thaws rather than holding an application down. Every piece is bound
to one recovery-point UUID, one capture generation, its own start and end time, the
source identity and the snapshot boundary. **A piece that changed during capture makes
the recovery point a failure, not a recovery point of a lower class.** A restore never
silently upgrades a class.

## Decisions

**D1 — the backup server pulls, and has no inbound listener.** It holds every
universe's data, which makes it the highest-value target in the constellation, and the
canon's security direction is that what has power has no front door. It opens outbound
connections, never accepts one. A compromised host cannot reach it, cannot enumerate
other universes' backups and cannot delete anything. *Accepted by the review, subject
to D3 and D6.*

**D2 — content-addressed chunks; the deduplication domain is an operator decision.**
*Corrected (BBS-R2).* Revision 1 said "plaintext digest, encrypted chunk, dedup within
a key domain, that is PBS's trade and it is acceptable here". Two faults: those
properties do not compose automatically, and I accepted a confidentiality trade on the
operator's behalf. With randomized authenticated encryption, identical plaintext yields
different ciphertext and storage dedup cannot simply keep one copy; convergent
encryption enables dedup but leaks plaintext equality and permits confirmation attacks;
a shared domain key widens dedup and widens the blast radius of one compromised host.
The three viable shapes and their exact costs are Decision X1 below.

**D3 — encryption happens on the host, before transfer, under envelope keys.** The
server stores opaque chunks it cannot read. A per-backup or per-generation
**data-encryption key** is wrapped by a **key-encryption key**; the datastore holds
ciphertext, manifests, proofs and wrapped DEKs, and never the unwrapped KEK or the root
recovery material. At least two independently recoverable copies of the root material
exist, at least one of them offline or in a different provider or security domain, and
its recovery must not depend on the manager or on the Backup Server being restored.
Rule 12 governs the key itself: its own key, never the vault's, never in the archive,
never on a command line. Key rotation must keep retained historical backups
decryptable: re-wrapping a DEK publishes a new signed envelope and never rewrites a
manifest or a ciphertext.

**D4 — retention: a schedule selects candidates; adequate replacement authorises
removal.** *Strengthened (BBS-R5).* Revision 1 said "a newer verified recovery point".
That is too weak — a newer point may carry fewer Rule 16 levels, a weaker consistency
class, a broken key, a different namespace or only a shallow test. A replacement may
authorise removal only when it **covers every protected level of the candidate with the
same or stronger declared consistency and policy class**, and has passed ciphertext
verification, key-recovery proof and its declared restore verification. Policy minimum
counts and time buckets survive replacement proof. `evidence_hold` and
`investigation_hold` apply with exactly the scopes already decided for the migration
collector. A **generation and lease barrier** prevents mark-and-sweep from removing a
chunk referenced by an in-flight upload, verification, restore or concurrently
published manifest. The removal decision, the manifest and the chunk-set digest are
retained permanently. Age selects; proof removes.

**D5 — a restore is proven periodically, from outside.** A backup job that reported
success is not evidence. The service restores to a scratch target on a schedule and
verifies from outside the restorer. Measured recovery time and measured data-loss
bounds are outputs of that test, not estimates.

**D6 — the host surface is a sealed local recovery point, not a read-only peephole.**
*Corrected (BBS-R3).* Revision 1 said the server "reads through a narrow read-only
capture surface". That contradicted the rest of the design: a database dump, a volume
snapshot, an application quiesce and a memory checkpoint all create files or alter
runtime state. The corrected shape separates two operations:

1. **Capture preparation.** An authorized Maker, under a typed PodMesh operation on the
   host, quiesces what must be quiesced, produces the pieces, and **seals** them as a
   local immutable recovery point with its manifest. If preparation fails after
   quiescing, an explicit abort-and-thaw path runs and the failure is recorded.
2. **Backup transfer.** The Backup Server pulls **only** the sealed manifest and the
   immutable chunks it names.

The Backup Server therefore holds **no** general Podman, signal, database or filesystem
mutation authority on any host. A compromised Backup Server credential must be provably
unable to command an arbitrary host operation, and that is a test, not an assertion.
D1's pull direction and absence of an inbound listener are unchanged.

**D7 — restoring is an authority-bearing operation with its own exclusion contract.**
*New (BBS-R4), and its absence was the second serious error in revision 1.* Restoring a
universe with its UUID, its IP, its manager facts or its memory can create a second
active copy; a different-host restore is not safe merely because the bytes and the
database answer correctly. Therefore:

- a restore lands first in a **quarantined, non-announcing state**;
- it requires a new operation UUID and an **explicit identity mode**: original-identity
  recovery, or new cloned identity;
- **original UUID or IP activation is refused** until the current external epoch gate
  accepts it *and* the previous placement is excluded;
- a backed-up manager store is **never imported as current authority**; recovered
  manager facts are reconciled against the surviving history and the restored authority
  stays inactive until validated;
- DNS and routes stay unpublished until external observation proves the accepted
  placement;
- the tests are stale-backup restore, live-source restore, tombstoned identity,
  concurrent restore, old-manager return, and a repeated identical request.

**D8 — a recovery point declares its completeness against Rule 16.** Per level:
present, deliberately absent, or **failed**. "Missing a level is a hole", so the hole is
on the manifest where a human sees it.

**D9 — namespaces organise; they do not isolate.** *(BBS-R9.)* Authentication,
authorization, encryption domains, quotas, enumeration boundaries and restore
permissions are defined independently of namespaces. A compromised tenant host must not
be able to test or infer chunk membership outside its own encryption domain — which is
the same question as D2's equality leakage, seen from the other side.

**D10 — the first lot mirrors the workload shape migration already qualifies.** Alpine,
musl, network-disabled, mount-free, on the existing Debian 13 hosts.

## The manifest is the root of trust

*(BBS-R7.)* The manifest is defined **before** the chunk store, because everything else
derives from it. It is immutable, versioned and authenticated, and it binds:

universe identity; recovery-point UUID; parent and generation; source host identity;
the capture operation ID; each piece with its chunk order, sizes and offsets; the
plaintext digest and the ciphertext digest of every chunk; encryption algorithm, format
version, nonce, DEK envelope identifier, KEK identifier and any compression parameters;
the consistency class with its quiesce evidence and boundary times; per-level
completeness states; the producing software versions and the minimum restore-tool
version; declared exclusions; verification history; and hold state.

Two consequences that are requirements, not remarks:

- **The datastore must rebuild its index from manifests and chunks alone.** Losing its
  own database must never make otherwise valid backups unreachable. This is a B1 test,
  not a later hardening.
- **Verification is split by who can do it.** The server verifies **ciphertext**
  digests, because that is all it can see. Only an authorized restore target verifies
  **plaintext** digests. A report must never present the first as the second.

Never overwrite a manifest or a proof in place: publish a new signed object and
reference it from a later catalogue checkpoint.

## Typed states

*(BBS-R12.)* Per piece and for the recovery point as a whole:

`preparing` → `sealed` → `transfer_incomplete` → `stored_unverified` → `verified` →
`restore_verified`, with `held` and `retired` as orthogonal terminal conditions.

A recovery point becomes eligible for an ordinary restore only once its required pieces
are `sealed` and its manifest is committed. **A partial set is never promoted because
some chunks survived.** Rule 12's clause applies in full: a failure after the archive is
complete keeps the archive, and a housekeeping step that cannot run is a reported
failure over a surviving archive.

## Data lifecycle and erasure — and the conflict it creates

*Missing from revisions 1 and 2 entirely; found by a canon sweep.* Rule 31
(`RULES.md:1037-1043`) binds this service directly:

- **Every universe declares its `dataLifecycle` in `manifest.json`**: `personalData`
  true or false; `retention` as a duration or `unlimited`, **per data class** (GED
  documents, JSONL logs, vector collections, database rows); and `onTermination` —
  what is destroyed, what is exported to the client, and in what format.
- **Silence is not a declaration.** A missing `personalData` is an error, never an
  implied `false`.
- **Erasure is fractal**: deleting a client's data means deleting it in every store of
  that universe — database rows, `sav/` volumes, GED files, **and its Qdrant
  collection**. *"A vector left behind is a leak."*

Three consequences for this service. It **reads the universe's declared
`dataLifecycle`** and treats retention per data class as an input to scheduling and
purge, rather than applying one datastore-wide policy. It **refuses to capture a
universe whose declaration is missing**, because silence is an error. And a recovery
point **records the declaration in force when it was taken**, since a retention rule
that changed afterwards cannot retroactively describe what was captured.

### The conflict, stated plainly

Fractal erasure and this design's storage model are in direct tension, and the tension
is structural rather than a matter of effort:

- chunks are **immutable** and content-addressed;
- chunks are **deduplicated across universes** within a key domain (D2), so one chunk
  may be referenced by several universes' manifests;
- off-site objects may be under an **R2 bucket lock** that refuses deletion for its
  whole duration, by design (that is what makes them ransomware-resistant).

So an erasure request for one universe can be impossible to honour by deletion: the
bytes are shared, or locked, or both. There are only three honest answers.

**Crypto-erasure.** Destroy the data-encryption key for that universe's data; the
ciphertext survives, unreadable and unrecoverable. This composes with immutability and
with bucket locks, and it is the only answer that works under both. **But it requires
the key domain to be no wider than the erasure unit** — and that is exactly what
Decision X1 chooses. *Cross-universe deduplication and per-universe erasability are
mutually exclusive.* Choosing a wide dedup domain in X1 forfeits crypto-erasure at
universe granularity; choosing per-universe keys forfeits the dedup economy that
motivated the chunk store in the first place.

**Per-universe keys, no cross-universe dedup.** Erasure works cleanly. Storage cost
rises by roughly the factor the dedup was saving, which for near-identical universes is
the whole point of the design.

**Declare backups exempt for their retention window.** Legitimate only if it is written
into the universe's `onTermination` *before* any data is captured, and stated to the
client. It is not something this service may decide by omission.

**This is Decision X5, and it is not separable from X1.** They must be answered
together, and before B5 puts anything off-site under a lock.

## Off-site (Rule 16 level 5)

Codex's environment inspection
(`/tmp/podmesh-claude/R2-ENCRYPTED-KEY-BACKUP-2026-09-13.md`) establishes both the shape
and the current blocker. Cloudflare R2 through its S3-compatible API is a suitable
off-site target. **It is not provisionable today**: the general Cloudflare token
verifies and lists zones and tunnels, but R2 bucket listing returns HTTP 403 and no R2
S3 access-key pair exists in the inspected environment.

Three separate credentials, never one: a **backup writer** with object read and write on
the single bucket, present only on authorized producers; a **restore reader** with
object read only, used by the external recovery tool; and a **retention administrator**
used only to configure bucket-lock and lifecycle rules, present on no ordinary host and
inside no Backup Server universe. R2 exposes no bucket-scoped write-only permission, so
deletion resistance comes from bucket locks and credential isolation, never from an
invented policy. Values live in the external secret store; git carries only names,
purpose, fingerprints and rotation metadata.

Objects are content-addressed and immutable, under a flat prefix convention:
`podmesh/v1/chunks/<sha256>`, `podmesh/v1/manifests/<universe>/<backup>.json`,
`podmesh/v1/envelopes/<backup>/<version>.json`, `podmesh/v1/proofs/<backup>/<kind>.json`,
`podmesh/v1/catalog/checkpoints/<generation>.json`.

Bucket locks are configured only after the whole workflow is proven on a disposable
prefix, because a wrong retention rule makes test data undeletable for its duration.
**R2 lifecycle automation must never bypass D4**: age alone authorises no deletion.

## Bootstrap and disaster recovery of the Backup Server itself

*(BBS-R11.)* The Backup Server must not depend on itself, on the live manager, or on
manager-served DNS in order to be recovered. A minimal, independently installable
`podmesh-recovery` package targets a clean Debian host, works without ShaperOS, and
carries pinned bootstrap configuration.

The sequence: install the pinned package; inject the read-only off-site credential from
the external secret store; fetch signed catalogue checkpoints, manifests, envelopes,
chunks and proofs; verify signatures, hashes, format compatibility and completeness
**before** decryption; obtain the separately held root recovery material through the
authorized tandem workflow; restore into network and identity quarantine; rebuild and
verify the catalogue from the immutable objects; demonstrate a byte-exact restore of a
known backup with external evidence; prove exclusion of the old active identity and
obtain explicit activation authority; only then expose the service and rotate runtime
credentials.

The Backup Server need not be always available. **Its loss must never make its backups
unusable.**

## The operator model: recovery is agent-first

The Backup Server and its recovery bootstrap are operated by a human-agent tandem, and
the human is not expected to execute a procedural manual under pressure.

The recovery agent inventories hosts, datastores, manifests, keys by opaque identifier
and compatible targets; diagnoses without the unavailable manager, Backup Server or
manager-served DNS; produces a typed recovery plan with expected effects, risks,
rollback boundaries and required authority; verifies datastore integrity and rebuilds
the catalogue; executes through idempotent operations; keeps the result quarantined
until identity and activation are explicitly accepted; proves the final service,
catalogue, key recovery and a sample restore from outside; and retains a complete
evidence trail.

The human supplies intent and arbitrates exceptional authority: choosing the recovery
source when histories conflict, releasing separately held key material, accepting
original-identity takeover, authorising a destructive cleanup.

The bootstrap therefore needs a typed machine-readable API and CLI, a declarative
recovery intent, dry-run planning, stable operation IDs, explicit refusal reasons and
structured evidence. An interactive shell recipe may document diagnostics; it is not the
product interface.

## Deployment portability

Standalone operation without ShaperOS is **mandatory** across supported host types.
ShaperOS deployment is the operator's preference for internal use and must never become
a hard dependency. The same backup and restore contracts apply in every mode, and each
mode is validated separately: host installation does not prove container operation.

Targets: a physical server, a virtual machine or VPS, a container, a ShaperOS universe,
and a PodMesh node. **The storage server does not require Podman or CRIU locally** —
receiving and retaining sealed chunks needs neither. Host-side capture preparation does
require runtime access, and memory-coherent capture has further kernel and runtime
constraints; ordinary backup storage must not inherit them.

The normal standalone version is built first. Everything else is an integration of it.

## B1 — the corrected first lot

Deliberately small, and shaped by the reviewer:

1. one stopped, mount-free, network-disabled Alpine universe;
2. one authorized Maker operation seals its configuration and filesystem fixture;
3. one versioned signed manifest and a content-addressed encrypted chunk set;
4. the Backup Server pulls through the transport chosen in Decision X3;
5. the source universe is removed only after the backup is safely stored;
6. restore creates a **quarantined new-identity** copy on another host;
7. external observation verifies files, configuration and application behaviour;
8. the datastore index is **deleted and rebuilt** from the immutable manifests and
   chunks;
9. the encryption key is recovered from separate operator material and the restore is
   repeated;
10. interrupted capture, transfer, manifest publication and restore are each retried
    without duplicate identity and without false success.

Plus Rule 12's hygiene guards, implemented and proven with a recorder.

**B1 explicitly does not prove:** persistent volumes, databases, original-identity
activation, DNS, memory continuity, off-site recovery, retention, arbitrary Linux
distributions, ShaperOS integration, or Backup Server high availability. A stopped
mount-free filesystem export is a **transport fixture**; it is not Rule 16 level 2 and
must not be counted as Rule 16 coverage.

## Delivery order after B1

B2 volumes (level 2, with the `nosav/` and `.env*` exclusions). B3 databases (level 3
under Rule 12's dump laws). B4 retention, holds and sweep with the lease barrier. B5
off-site (level 5), once Decision X4's credentials exist. B6 memory-coherent points,
where the runtime permits, reusing the migration chain's checkpoint machinery and its
proven limits. Later: scheduling policy, the human interface, and the level 1
container/VM snapshot where a hypervisor provides it.

## Decisions that remain the operator's

B1's public contract must not be frozen until these four are taken. Each is stated with
what it costs, because none of them has a free answer.

**X1 — the deduplication and encryption domain.** Three shapes:

| Shape | Dedup reach | Leakage | Blast radius of one compromised host |
| --- | --- | --- | --- |
| Randomized AEAD, one key per universe | none across universes | none | that universe only |
| Randomized AEAD, one key per declared domain | full within the domain | index reveals which universes share a chunk | the whole domain |
| Convergent encryption within a domain | full within the domain | plaintext **equality** leaks; confirmation-of-content attacks become possible for anyone holding the index and a candidate file | the whole domain |

Storage economics push toward the second or third; confidentiality pushes toward the
first. I will not choose this on your behalf, and revision 1 was wrong to.

**X1 also decides X5.** A key domain wider than one universe forfeits crypto-erasure at
universe granularity, which is the only erasure mechanism that survives immutable
chunks and bucket locks. Answer them together.

**X2 — who owns and can recover each domain key**, and where the two independent copies
of the root recovery material live. One of them must be outside the provider that holds
the ciphertext, and neither may depend on PodMesh being restored.

**X3 — Rule 12's transport clause.** Rule 12 currently requires archive transfers to use
HTTP Basic Auth and end-to-end TLS through a Cloudflare Tunnel. This design proposes an
authenticated private pull over the Rule 13 WireGuard mesh. **Rule 13 does not silently
replace Rule 12.** Either the design complies with Rule 12 wherever it applies, or the
canon is amended explicitly to accept authenticated encrypted private-mesh transfer as
a valid archive transport, with the Cloudflare Tunnel retained for its intended
distribution case. Once chosen, this document must state which rule governs host
capture, which governs datastore sync, and which governs an operator download. Nothing
is implemented around this contradiction.

**X4 — the off-site credentials.** Provision the three bucket-scoped R2 credentials, or
name a different off-site target. Until then level 5 is unreachable and B5 cannot start.

**X5 — erasure granularity, which X1 decides.** Whether a client erasure request can be
honoured on backups, and by what mechanism: crypto-erasure at a key domain no wider
than the erasure unit, per-universe keys without cross-universe deduplication, or a
declared exemption written into `onTermination` before any capture. See the data
lifecycle section. This must be settled before B5 places anything under a bucket lock,
because a lock cannot be lifted for a mistake.

## Smaller canon obligations, recorded so they are not rediscovered

Each is a verbatim clause of the canon that binds this service without changing its
shape. They belong in the B1 checklist, not in a later hardening pass.

- **Private keys never cross a level** (Rule 36, `:1117`, `:1126`): the parent's Ed25519
  authority key never leaves the parent, and no private key climbs into a ledger or
  descends into a child. No archive may contain one.
- **`lastBackup` in `status.json` is canonical** (Rule 37, `:1174-1176`): every board or
  cockpit tile is a rendering of it, never a rival. This service writes it, and
  maintains no competing authoritative state file.
- **The test universe is destroyed after it passes** (Rule 10, `:542`; LAW.md `:13`,
  `:19`): a validation run rebuilds from empty and destroys the vehicle, which is what
  makes it a cold-recovery proof rather than a warm one.
- **Never wipe what you did not provision** (BOOT-CONTRACT §2, `:27-30`): no volume, no
  database, no universe. If it is unclear whether a machine carries production, it does.
- **Halt on a missing secret** (Rule 0J, `:232-233`; BOOT-CONTRACT §12, `:173`): a
  required key that is absent, empty or still a placeholder stops the run before
  anything is built or launched, and names what is missing.
- **Off-site encryption is AES-256-GCM to a cold bucket** (Rule 16, `:844`), copying the
  archives and dumps — never a git clone pretending to be a backup.
- **Namespace isolation on restore** (Rule 22, `:948-949`): a universe's vector
  collection is its own, and a restore never cross-mounts one into another.
- **Multi-threaded compression** for archive creation (Rule 12, `:754`).
- **Proof is read from outside the producer** (BOOT-CONTRACT §9, `:102-106`): a
  `COMPLETED` status is not proof that a file is correct, and a health endpoint
  answering 200 is not proof that a job ran.

## What must be proven, not claimed

A restore onto a **different** host, verified from outside. That a **destroyed**
universe comes back, not that a copy exists. That the key is **recoverable** by the
operator from their own material, the coffer alone proving nothing. That a **partially
failed** capture is reported as failed and keeps whatever it completed. That the
datastore's index can be **destroyed and rebuilt** from immutable objects. That a
compromised Backup Server credential **cannot command a host**. Each deployment mode
separately. Measured recovery time and measured data-loss bounds, per level and per
consistency class.
