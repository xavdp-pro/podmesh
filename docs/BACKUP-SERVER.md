# PodMesh Backup Server

> **Intent Classification**: GENERIC INTENT (Universal / Parameterized Blueprint)
>
> Rule 0B mandates that exact string (`RULES.md:98`); revisions 1 to 5 paraphrased it.
> This service is a reusable blueprint, not one client's universe.
> **Perimeter**: P1 (Rule 0A — it holds encryption keys, signing authority and every
> universe's data, so it is classified before design, not after)

Status: design direction, revised four times after independent counter-review; not
implemented, not validated. Nothing here is qualified.

**Revision 5**, 2026-09-14. The ledger, because three agents have edited this file and
the numbering has already confused one reviewer: R1 = `7e4e294` (mine); R2 = `0cc7dd5`
(Xavier with Codex, adding B0 and the capture adapters); R3 = `1cd1736` (mine, which
deleted R2 by accident); R4 = `2aea3bc` (restoring R2 and crossing the design against
PodMesh's own artifacts); R5 = this. Where the text below says "revision 3", it means
`1cd1736`, and "revision 4" means `2aea3bc`.

**Revision 8**, 2026-09-14. A propagation review of revision 7 found three more stranded
edits and one command that could not run:

- **The step 0 marker command was invalid JSON.** A backslash and a newline inside a JSON
  string is not a valid escape, so the creation request would have been rejected before
  the daemon parsed it — and the obvious one-line repair leaves an escaped space that
  becomes a command word, killing the TERM handler and restoring the exact `forced: true`
  failure the paragraph exists to prevent. Three fixture defects in three revisions, each
  introduced while fixing the last, all in one command. It is now one line, and the
  marker value is pinned to hexadecimal because it is interpolated into a shell word and
  a path.
- **B1 produced no catalogue checkpoint** while revision 7 required step 8 to rebuild
  from one. Step 3 now produces it.
- **Step 5's normative sentence still said `verified`** while a note three lines below
  said the gate is `restore_verified`. An acknowledgement is not a correction.
- **The two-signer rule left four things unstated**: the datastore has no `host_uuid` to
  be keyed by, "one key per host" is false when one host holds two signers, the "Backup
  Server package" does not exist, and the restore target verifies signatures with no
  stated copy of `producers.json`. All four are answered.

**Revision 7**, 2026-09-14. A regression review of revision 6 found that three of its nine
edits were not carried into the sections that quote them — this document's own recurring
failure mode, committed again in the revision that named it — and that one new mechanism
could not work:

- **Step 1 still downgraded the class on a forced stop**, the exact sentence revision 6
  existed to abolish, surviving in the one place an implementer reads to build the run.
- **The marker command referenced a shell variable PodMesh cannot set.** The creation
  request has no environment field, so `$MARKER` would have expanded to nothing and every
  run would have failed step 7 on a zero-byte marker. The value is now interpolated as a
  literal, into both the file name and the contents.
- **Step 4 still keyed the outbox by `authorization_id`**, which the same revision
  established a backup cannot obtain.
- **The `producers.json` paragraph was refuted by its own closing sentence**, and the
  signing-key bullet still named one state directory where there are two signers.
- **Holds left the manifest for the catalogue and the index-rebuild contract did not
  follow them**, so a rebuilt index would have come back with no holds while D4 gates
  removal on them.
- Four citations corrected, and the partial-order diagram replaced by an edge list.

**Revision 6**, 2026-09-14, is `ca5293d` plus the fifth review's corrections. That review
returned **GO** — the first — and verified about fifty-five citations without finding a
wrong line number. It also found that revision 5's own new text violated B1's
forbidden-claims list, which is why revision 6 exists before anyone starts B1:

- **A forced stop had no honest class.** Revision 5 said such a capture is
  "`crash-consistent` at best", which the adapter section forbids and forbidden claim 4
  forbids outright. A forced stop is now a **failed run**, not a weaker label.
- **The puller's credential could not work as described.** The outbox and the state
  directory are both mode 0700 and root-owned, so "an account restricted to the outbox
  tree" was unimplementable; the restriction is now enforced by a forced command on the
  channel. And "adds no new mechanism" was false: the two mechanisms it does add are now
  named, including who issues the sealed point's identifier.
- **The manifest bound fields its producer cannot know**, verification history and hold
  state, while claiming immutability. Both move to the catalogue. The **image ID** it
  needed and did not carry is added.
- **The consistency classes were printed as a chain and described as a lattice.** They
  are a partial order, drawn, with the rule D4 needs for the incomparable pair.
- Step 0's marker had no write mechanism and would have forced every stop; D5 still
  required the Maker; step 5 destroyed the last source on a ciphertext-only check; the
  Rule 12 clause accounting omitted one clause and counted another twice. All corrected,
  with the two `/tmp` citations moved into the repository.

**What revision 5 changed**, from a fourth review that returned NO-GO narrowly with four
blocking findings:

- **The fixture could return a false pass**, which is the one failure this lot must not
  be able to have. A never-started container's export equals its image, and the restore
  host is pre-seeded with that image, so a round trip moving zero bytes would have
  reported success. B1 now marks the fixture before capture and verifies the marker
  after restore: **step 0** and step 7.
- **The far end of the pull was unnamed** and its authorization was a signature over a
  document, which authenticates bytes and admits no reader. D1 now uses PodMesh's
  existing outbox and transport-controller pattern, states the puller's own credential
  separately, and drops the Rule 13 mesh from B1 entirely.
- **The producer identity was `authorization_ref`**, which PodMesh declares is not a
  verified credential. It is now `host_uuid`, with canonicalization pinned to RFC 8785,
  pure Ed25519 named against Ed25519ph, and `producers.json` moved out of a
  `/etc/podmesh/` that does not exist.
- **Step 9 recovered a key from material no decision had produced.** X2's interim is now
  stated, and stated as a lab interim that does not answer X2.
- Four contradictions in normative text are reconciled, B1's forbidden claims are a
  closed list rather than prose, and two shifted citations are corrected.

Revision 1 was counter-reviewed by OpenAI Codex
([docs/reference/review-backup-server-codex-2026-09-13.md](reference/review-backup-server-codex-2026-09-13.md), verdict OPEN with six
blocking findings). This revision answers BBS-R1 to BBS-R6 and folds BBS-R7 to BBS-R12
into the manifest, state and test contract. Three of the blocking findings were errors
of mine and are named as such where they are corrected. A second, fresh review then
returned NO-GO on that revision with six more; those are answered here too, and one of
them was serious enough to be recorded at the end of this document rather than quietly
fixed. **Five decisions remain the operator's and are listed at the end. None of them
blocks B1: the interim it uses meanwhile is stated with them.**

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

Seven rules of `SHAPER-OS-V1.14/software/RULES.md` bind this service — 16, 12, 20, 10,
11, 31 and 30 — besides the classification rules in the header. They are not re-decided
here; several were written after real incidents.

**Rule 16 — the five levels.** Container, volumes, database, git, off-site. "This set
is enough. Missing a level is a hole."

- Level 2 is the **volumes, not the overlay**. `nosav/`, caches, image layers and
  `node_modules` are excluded. `app/` is code in git and is *not* a substitute for a
  volume backup (that clause is Rule 4, `RULES.md:397`, not Rule 16).
- Level 3 is a **real dump**, not a live volume tar — and it is the relational dump
  **plus the Qdrant snapshot** (`RULES.md:842`). Omitting the vector collection is a
  hole by this rule's own words, and it also breaks erasure: a service that never
  captures the collection cannot honour an erasure over it.
- Level 4 is git, and **git is never treated as a data backup**.
- Level 5 is the off-site copy of levels 2 and 3, and optionally 1 — and if level 1 is
  ever copied off-site it carries the image store with it, which is Rule 16's choice
  and not this service's. This service still never builds a recovery point containing
  an image layer.
- Level 1 is a **host snapshot of the enclosing LXC**, and it therefore necessarily
  contains the Podman image store. That does not contradict "images are never backed
  up": level 1 is a machine-recovery net taken by the hypervisor or the host's storage,
  not a universe recovery point produced by this service. This service never places an
  image layer in a recovery point it builds.

**Rule 12 — archive hygiene.** Every clause is a failure mode this service must make
impossible rather than merely avoid: the key never travels with the coffer; the
backup's key is its own key, refused if it equals the vault master key, and reaches the
tool through the environment and never as an argument; a dump that was not taken is
announced rather than written empty; the archive command's failure is the backup's
failure; a failure after the archive completes keeps the archive; a failed dump leaves
nothing behind (`.part` then rename); `.env` is excluded in every spelling; and how the
tool is called is proven with a recorder on a PATH built from scratch.

Rule 12 **also governs transport**, but only within a scope this service mostly falls
outside: `RULES.md:753` binds "all archive transfers (`PROJECT.tar.bz2`,
`REMOTE.tar.bz2`)" and `RULES.md:848` widens that to "any `tar.bz2` that leaves the
host". A content-addressed chunk pull is neither. Where a `tar.bz2` does leave a host —
B2's volume archives — the clause applies in full. See Decision X3, which revision 3
mis-read as a collision when it is a gap.

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

**Rule 31 — the universe declares its own data lifecycle**, and erasure is fractal. It
was absent from revisions 1 and 2 entirely; see its own section, and Decision X5.

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
| Persistent volumes | 2 | volumes only; `nosav/` excluded, and `.env` in **every** spelling — `.env*`, `*.env`, **and `deploy/env`**, whose basename matches neither glob |
| Database dumps **and the Qdrant snapshot** | 3 | per Rule 12's dump laws; the vector collection is part of level 3 |
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
an image layer.

**What PodMesh can actually record today, which is less.** Rule 11's restorable identity
assumes a `cfg-image-lock.json` and a source commit. PodMesh has neither: a creation
request carries a **full local image ID** (`LOCAL-API.md:31`, `:85`), and PodMesh
**never pulls** (`LOCAL-API.md:151`). A local image ID is neither a pullable registry
digest nor a recorded commit, so Rule 11's "rebuilt from source, or pulled by the digest
the lock names" is **unreachable in PodMesh as it stands**. B1 therefore records the
local image ID and nothing more, its restore target must already hold that image, and
B1 **does not prove Rule 11 restorable identity** — it proves transport. Closing that
gap means PodMesh recording a registry digest and a source commit, which is its own lot
and belongs to PodMesh rather than to this service. The reason matters more than the rule: hoarding layers would let a
universe stay restorable while its source became unbuildable, and nobody would notice
until the layers were lost too. An image that can no longer be rebuilt at its recorded
commit or pulled by its locked digest is a **reproducibility defect that must surface**,
and the manifest's job is to make it visible — not to paper over it with a copy.

The same rule fixes the unit: the **universe** is what can be snapshotted, exported and
restored as one thing (`RULES.md:568-570`). A brick is an application container built
from an immutable image, and is not a backup unit.

**The word means two things, and this document must not equivocate.** In the Shaper OS
canon a *universe* is the **LXC system container** — its own init, its own package set,
its own nftables. In PodMesh today a *universe* is a **Podman container** with a UUID
(`DELIVERY-CHECKLIST.md:42`, `:44` — no single line states it; the two together do). Read naively, the sentence above would say PodMesh has
no backup units at all, which is not what Rule 11 means and not what this service is for.

The reconciliation: **Rule 11's unit is the unit of a *complete* recovery point.** A
PodMesh universe is a *piece* — the largest piece PodMesh itself can currently produce,
and the right subject for B1's transport fixture, but not a whole machine. Rule 16's
level 1 exists precisely to cover the LXC that PodMesh cannot. So:

- **B1 captures a PodMesh universe**, i.e. a Podman container: its configuration, its
  **image lock** and its filesystem export. It is a transport fixture and is explicitly
  not Rule 16 coverage.

  **The image lock is a recorded reference, never captured bytes**, and revision 4 left
  that ambiguous enough to read as a contradiction of Rule 11 in the same section. Rule
  11 is that images are never backed up; what the manifest records is the **local image
  ID** the universe was created from, as a string, so a restore can refuse to proceed
  against a different one. The image's contents never enter a chunk. The consequence is
  stated rather than hidden: **a restore host must already hold that image**, PodMesh
  records a local image ID and never pulls, and so B1's restore target is pre-seeded —
  which is exactly why B1 does not prove Rule 11 restorable identity, and why step 0's
  marker is what makes the round trip mean anything at all.
- **A complete recovery point in the canon's sense** additionally requires the level 1
  snapshot of the enclosing LXC, which is a hypervisor or host-storage operation and is
  scheduled last for that reason.

Wherever this document says "universe" without qualification below, it means the PodMesh
one.

And restoration is not finished when the bytes are back: Rule 11 ends it with the
universe's own `deploy/proof.sh` — *"a restore nobody proved is a claim"*.

## Consistency: what a capture may honestly claim

**Correction (BBS-R1), and this was the worst error in revision 1.** Revision 1 called
an uncoordinated live capture "crash-consistent". It is not. Pieces read sequentially
over minutes may never have existed together at any instant, and a manifest asserting
crash consistency for such a set would be a false guarantee written into evidence.

**Five classes** — revision 4 added `quiescent` and left the count at four, which is the
kind of stale number an implementer builds an enum from. The rule is that **a capture
never claims more than it can prove.** *Revision 5 wrote that as "claims the weakest
class it can prove", which an implementer can satisfy by always returning `incoherent`
— provable for every capture, and flatly contradicting this document's statement that
B1's fixture is `quiescent`. The intent is a ceiling, not a floor.*

**They form a partial order, not a chain.** Revision 5 printed a single chain with a `≤`
in it and then said in the next paragraph that two of the classes are not ordered against
each other. Both cannot be true, and a chain is the one an implementer would encode. The
order is given below as an edge list rather than a drawing, because a picture added to
remove ambiguity is worth nothing if the reader has to decode the box characters:

```
incoherent              <  crash-consistent
crash-consistent        <  application-consistent
crash-consistent        <  quiescent
application-consistent  <  memory-coherent
quiescent               <  memory-coherent

application-consistent  and  quiescent   — incomparable, neither satisfies the other
```

`application-consistent` and `quiescent` are **incomparable**: a stopped source has no
in-flight state to lose, while a running application that executes its own protocol keeps
serving. Neither dominates. Where both are available the application's own protocol is
preferred, because it does not cost the stop — that is a preference, not a rank.

**What incomparability means for D4's retention rule**, which requires a candidate to be
covered "with the same or stronger declared consistency". Stronger means *strictly above
in this partial order*. So `quiescent` does not satisfy a requirement for
`application-consistent`, and `application-consistent` does not satisfy one for
`quiescent`; only the identical class, or `memory-coherent`, satisfies either. A rule
phrased on a total order would have silently accepted each in place of the other. The
classes:

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
- **`quiescent`** — the source was **not running for the entire capture**, so every
  piece comes from one instant by construction, without needing a snapshot mechanism to
  produce that instant. This is stronger than `crash-consistent`, not weaker: there was
  never any in-flight state to lose. It costs the stop, which is why it is the fallback
  adapter's class and not the general answer. **B1's fixture is this class.**

  **The stop must be unforced, and this is a precondition, not a detail.** PodMesh's stop
  sends the container's stop signal and then **SIGKILL** if it is still running after the
  timeout, reporting the escalation as `forced: true` (`LOCAL-API.md:62`). A SIGKILLed
  process never ran its shutdown path, so the application's own files may be mid-write —
  the stop bought coherence across pieces but not a settled application. A capture may
  therefore claim `quiescent` only when the stop operation reported **`forced: false`**.

  **Where it reported `forced: true`, the run has failed its precondition and is
  reported as a failed run.** It does not fall back to a weaker label. *Revision 5 said
  "the capture is `crash-consistent` at best", and that was wrong twice over in one
  clause: the adapter section below states that only the three snapshot adapters can
  yield `crash-consistent` and that the ordinary-filesystem adapter never does, and B1's
  own forbidden-claims list forbids the label outright. `incoherent` does not fit either
  — it is defined for pieces gathered from a **running** universe. There is no class for
  this branch because there should not be one: a forced stop means the fixture was not
  in the state the run assumes, and inventing a label for it would be exactly the silent
  downgrade this section exists to prevent.* The adapter records the escalation in the
  quiesce evidence, and the operator restarts the run with a longer timeout or a
  container that handles its stop signal. A capture must never *upgrade* a class because
  a stop was requested, and must never invent one because a stop went badly.
- **`memory-coherent`** — a memory checkpoint bound to the exact disk state it was
  taken against.

Every capture therefore records a **quiesce boundary**: the mechanism used, the instant
it opened, the instant it closed, and a **maximum freeze duration** after which the
capture aborts and thaws rather than holding an application down. Every piece is bound
to one recovery-point UUID, one capture generation, its own start and end time, the
source identity and the snapshot boundary. **A piece that changed during capture makes
the recovery point a failure, not a recovery point of a lower class.** A restore never
silently upgrades a class.

## Capture adapters and consistency

*Restored from commit `0cc7dd5`, which revision 3 deleted by accident — see the note at
the end of this document. This section is what actually produces the classes above: the
class a capture may claim is a property of the adapter that took it.*

PodMesh Backup Server selects a capture adapter from observed storage capabilities; it
does not require LVM2, ZFS or Btrfs to install or operate.

- **LVM thin:** quiesce, take the logical-volume snapshot, release the live universe,
  and read the snapshot for transfer.
- **ZFS:** quiesce, take a dataset snapshot, release the universe, and use the immutable
  snapshot as the source for full or qualified incremental send.
- **Btrfs:** quiesce, take read-only subvolume snapshots, release the universe, and use
  them for full or qualified incremental send. Nested subvolumes require explicit
  enumeration because the parent snapshot is not recursively complete.
- **Ordinary filesystem:** stop the generic universe for the duration of the archive.
  A shorter freeze is allowed only under a declared application-specific quiesce or
  database-dump contract. **Podman process pause alone is not evidence of a coherent
  application and volume recovery point.**

In all cases, the local snapshot or archive staging area is **disposable capture
material**. Only a transferred, authenticated, catalogued and independently restorable
recovery point is a backup.

The three snapshot adapters are the only mechanisms that can yield `crash-consistent` on
a **running** universe, which is why B0 matters. The ordinary-filesystem adapter yields
`quiescent` instead — a different and stronger class, bought with a stop rather than
with a snapshot. Revision 3 said the fallback "yields `crash-consistent` too", which was
wrong by the definition three paragraphs above it: no snapshot mechanism, no
crash-consistency.

**The price of the pull model, stated.** Every host materialises a complete sealed local
recovery point on every capture — storage for the staging area, and a full re-read of
the source, since content addressing gives incrementality only in what is *transferred*,
never in what is read. That cost is why the adapter matters and why B0 measures capacity
behaviour rather than only correctness.

## Decisions

**D1 — the backup server pulls, and has no inbound listener.** It holds every
universe's data, which makes it the highest-value target in the constellation, and the
canon's security direction is that what has power has no front door. It opens outbound
connections and **accepts none at the application layer**: no listening service, no API,
no port a host can address.

*Corrected in revision 5, where the previous wording was a security claim resting on
nothing.* It said the pull "is authenticated by the signature on the sealed manifest".
A signature over a document authenticates **the document**; it says nothing about who is
entitled to read it. Anyone able to reach the source host's sealed output would obtain a
perfectly valid signature with it. Authorization of the reader and authenticity of the
bytes are two different properties, and conflating them is how an exfiltration path gets
written into a design as a safeguard. That sentence is withdrawn.

**How the far end actually works, in PodMesh's existing pattern.** PodMesh already moves
documents it does not want inside a 4 KiB request: the service writes them into
`outbox/<authorization_id>/` under its state directory, which **only the service writes**,
and an **external transport controller running as root** moves them; the reverse
direction lands in `inbox/<authorization_id>/` (`LOCAL-API.md:99`). The Backup Server
reuses that pattern rather than inventing a transport. *Revision 5 said it "adds no new
mechanism", which was false and worth correcting because it discourages an implementer
from noticing the two things that genuinely have to be built.* It adds exactly two, both
named here:

- **A sealing operation on the local socket.** No such typed operation exists today. It
  is new work on the daemon, and B1 step 2 depends on it.
- **An identifier for the sealed point.** The existing `outbox/<authorization_id>/`
  identifier is issued **only** by `migration_authorize_transfer`, against a
  `checkpointed` reservation and a named destination host (`src/transfer.rs:172-183`).
  A backup has neither. The sealing operation therefore issues its own identifier, of the
  same shape and from the same durable-before-any-artifact discipline, and the outbox
  directory is keyed by it. Leaving this unsaid would have been the same defect as
  naming key material that no decision produces, one level down.

With those two built, the flow is:

- **The source host** seals a recovery point and writes it to its outbox. It opens no
  connection and needs no knowledge of the Backup Server.
- **The transport controller** is the puller, and its authorization is **its own**,
  unrelated to the manifest signature.

  **What that credential has to be, given PodMesh's real permissions.** Both the state
  directory and the outbox are created mode **0700 and owned by root**
  (`src/bin/podmeshd.rs:11`, `src/transfer.rs:29-37`). No unprivileged account can even
  traverse them, so revision 5's "a single account whose access is restricted to the
  outbox tree" was not implementable as written: any account that can read the outbox is
  root, and calling it restricted would have been a safeguard that does not exist.

  The restriction has to be enforced by the channel, not by the filesystem. In B1 the
  Backup Server's public key is installed on each source host under a **forced command**
  with `restrict`, so the key can invoke exactly one root-run program that streams a
  named outbox directory and can do nothing else — no shell, no port forwarding, no
  arbitrary path. Without that clause the obvious reading of this design is unrestricted
  root SSH from the Backup Server onto every source host, which would make D6's own
  property false: a compromised Backup Server credential must be provably unable to
  command an arbitrary host operation, and B1 lists proving that among its exit
  conditions. A design whose plainest implementation contradicts one of its decisions
  has to say so in the decision.
- **The manifest signature** does one job downstream of that: it proves the bytes that
  arrived are the bytes the producer sealed. A verifier checks it after transport, and it
  would still be checked if the bytes had arrived on a USB stick.

**The mesh is not required for B1, and B1 does not use it.** Rule 13's WireGuard mesh is
optional by PodMesh's own contract, its authentication is an open design question
elsewhere (`CONTROL-SERVICES-UNIVERSE.md`), and it has never been run end to end
(`docs/README.md:49` records both transports as owing tests). B1 therefore pulls over ordinary
existing IP connectivity, which is the same transport the three lab hosts already use.
Where a deployment does put the Backup Server on the mesh, it is a peer like any other:
an interface exists and is addressable, **nothing behind it answers**, and Rule 13's
mandatory named-peer comment applies to its `[Peer]` block like every other
(`RULES.md:812-819`).

A compromised host therefore cannot reach the Backup Server, cannot enumerate other
universes' backups and cannot delete anything. *Accepted by the review, subject to D3
and D6.*

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
counts and time buckets survive replacement proof. `evidence_hold` and `investigation_hold` are
adopted **by name and by principle** from the migration collector, and their scopes are
re-derived here rather than transposed: the collector's scopes are defined over its own
three effects, and there is no backup analogue of its history-preserving reservation
transition — which is precisely the effect `evidence_hold` does *not* block. B4 states
the mapping over this service's effects: chunk sweep, manifest retirement and off-site
deletion. Two of the collector's clauses carry over unchanged: **an unknown hold is a
hold**, and an implementation that does not implement holds says so and claims neither
behaviour. A **generation and lease barrier** prevents mark-and-sweep from removing a
chunk referenced by an in-flight upload, verification, restore or concurrently
published manifest. The removal decision, the manifest and the chunk-set digest are
retained permanently. Age selects; proof removes.

**D5 — a restore is proven periodically, from outside.** A backup job that reported
success is not evidence. *Corrected: revision 3 said "the service restores", which D6
and the portability section forbid — the Backup Server has neither the runtime nor the
authority to restore anything on a host.* What actually happens: the Backup Server
**requests** a restore verification through the same typed, authorized operation on the
target host's own root-only local socket that performs any restore — the Maker is not
that operation and is not required to exist, for the reason the signing section gives —
on a scratch target, and then **verifies the evidence from outside the restorer**. It commands nothing and it trusts nothing it did not check. Measured
recovery time and measured data-loss bounds are outputs of that test, recorded as
operational observations under Rule 10's exception, never as figures in this document.

**Who triggers a capture** is the same question and has the same answer: not the Backup
Server, which holds no authority on a host. A scheduler on the protected side requests a
typed capture-preparation operation, **the host's authorized caller of that operation —
its Maker where ShaperOS is present, and an operator or the scheduler itself where it is
not** — executes or refuses it, and the Backup Server discovers a new sealed point when
it next pulls. *The unqualified "the host's Maker" stood here through revision 5 while
three other sections said the Maker is not implemented and cannot be a dependency.* The scheduler's placement
and authority are part of the lot that introduces scheduling, not B1 — B1's captures are
requested by hand.

**D6 — the host surface is a sealed local recovery point, not a read-only peephole.**
*Corrected (BBS-R3).* Revision 1 said the server "reads through a narrow read-only
capture surface". That contradicted the rest of the design: a database dump, a volume
snapshot, an application quiesce and a memory checkpoint all create files or alter
runtime state. The corrected shape separates two operations:

1. **Capture preparation.** An authorized caller of a typed PodMesh operation on the
   host — a ShaperOS Maker where one exists, and equally an operator or a scheduler
   where one does not, since standalone operation is mandatory — quiesces what must be
   quiesced, produces the pieces, and **seals** them as a local immutable recovery point
   with its manifest, written to the host's outbox. If preparation fails after
   quiescing, an explicit abort-and-thaw path runs and the failure is recorded.
2. **Backup transfer.** The transport controller pulls **only** the sealed manifest and
   the immutable chunks it names, from that outbox, under its own authorization.

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
derives from it.

**Three different things are called a manifest in this ecosystem** and they must not be
confused: the canon's universe `manifest.json` (Rule 11's restorable identity, and where
Rule 31's `dataLifecycle` lives); PodMesh's existing **migration checkpoint manifest**,
hashed into a transfer handoff (`LOCAL-API.md:88`, re-hashed at `:103`); and this service's **recovery-point
manifest**, described here. Where this document says "the manifest" unqualified, it
means the third. It is immutable, versioned and authenticated, and it binds:

universe identity; recovery-point UUID; parent and generation; source host identity as
the producer's `host_uuid`; the `authorization_ref` recorded as provenance only; the
universe's declared **`data_lifecycle`** — copied from its `manifest.json` at capture
time, because Rule 31 binds retention and erasure to that declaration and a recovery
point that does not carry it cannot be swept, held or erased correctly later, and the
erasure section below is unenforceable without it; **the local image ID the universe was
created from**, recorded as a string so a restore can refuse to proceed against a
different one — the image's bytes are never captured, per Rule 11, and step 6's restore
has nothing to refuse against if this field is missing; the capture operation ID; the
stop outcome including whether it was `forced`; each piece with its chunk order, sizes
and offsets; the plaintext digest and the ciphertext digest of every chunk; encryption
algorithm, format version, nonce, DEK envelope identifier, KEK identifier and any
compression parameters; the consistency class with its quiesce evidence and boundary
times; per-level completeness states; the producing software versions and the minimum
restore-tool version; and declared exclusions.

**What the manifest does *not* carry, and why the previous list was self-contradictory.**
Revisions 4 and 5 ended that list with "verification history; and hold state". Neither
can be in a signed immutable manifest. The next section binds a signature over the
canonical serialization of every other field, and this one forbids overwriting a manifest
in place. But verification happens on the **datastore, after transfer**, and a hold is
applied later still. A producer cannot sign a fact that does not exist when it signs.
Carrying those two fields would have meant either a mutable manifest or a signature over
fields nobody can fill, and an implementer would have discovered that only when writing
the signer.

So **verification history and hold state live in the catalogue**, as separate signed
objects referenced from a later catalogue checkpoint, exactly as the last paragraph of
this section already requires for proofs. The manifest stays immutable and signable at
the instant it is produced.

### Who signs it, and what a verifier checks

*Missing from revision 3, which called the manifest "authenticated" and "signed" without
ever naming a signer — a root of trust anchored in nothing.*

Rule 36 (`RULES.md:1117`, `:1126`) settles the shape: a private key never leaves its
level, so there is **no fleet-wide signing key** and the Backup Server never signs what
it did not produce. The manifest binds a **producer identity commitment** and a
signature over the canonical serialization of every other field.

*Corrected: an earlier draft named the signer as "the Maker, with a key in that host's
vault". Neither object exists in PodMesh — the Maker is a ShaperOS organ above PodMesh
and is recorded as not implemented, and the word "vault" appears in no other PodMesh
document. Anchoring the root of trust in something the implementer cannot find is the
same defect as anchoring it in nothing.* In PodMesh's own terms:

- **The producer identity is the host's `host_uuid`**, and nothing else. It is PodMesh's
  own durable per-host identifier, minted once into the journal's metadata table
  (`src/lib.rs:37`) and readable through the existing `podmesh identity` operation
  (`src/lib.rs:148`). It already exists, it is stable across restarts, and it is the
  only host identity PodMesh has. The manifest carries it as the **producer identity
  commitment**, and `producers.json` is keyed by it.

  **The datastore is a producer too, and it has no `host_uuid`.** It signs catalogue
  checkpoints and proofs, and this document requires it to appear in `producers.json`
  like any other producer — while also saying the Backup Server need not be a PodMesh
  host, in which case there is no journal, no metadata table and no `host_uuid` to be
  keyed by. Revision 7 sharpened that from a latent ambiguity into a flat contradiction
  and did not notice. So: **a producer identity is a UUID**, and where PodMesh is present
  it is PodMesh's `host_uuid`, reused rather than duplicated. Where it is not, the Backup
  Server mints its own **once, on first use, into its own state directory**, by the same
  rule PodMesh applies — generated once, never regenerated, never transmitted. One
  identity per signer, whatever produced it.

  **`authorization_ref` is not that key and must never be used as one.** PodMesh states
  it plainly: *"`authorization_ref` is audit provenance, not a remotely verified
  credential"* (`PREPARE-A-HOST.md:38`). Revision 4 made it the signer's identity, which
  would have built the root of trust on a string the source host chooses for itself and
  nobody checks. It is recorded verbatim in the manifest as **provenance** — it answers
  *under what authority was this sealing requested*, which is worth keeping — and it is
  **never a lookup key, never resolved against `producers.json`, and never part of a
  verification decision**. The eventual holder of that authority is a ShaperOS Maker
  where ShaperOS is present; that is not a B1 dependency, because standalone operation
  is mandatory.
- **The key** is an Ed25519 signing key at `backup/signing.ed25519` **inside the signer's
  own state directory**, mode 0600 root, generated on first use and never transmitted.
  **One key per signer** — revision 7 wrote "one key per host", which its own two-signer
  rule makes false: in B1 the Backup Server is one of the three lab hosts, so that host
  carries two signing keys in two directories, PodMesh's and the datastore's. The level
  Rule 36 protects is the signer, and a host may hold more than one.

  **There are two signers and therefore two state directories, and revision 6 collapsed
  them.** On a **source host** the signer is PodMesh, and its state directory is
  `PODMESH_STATE_DIR` when set and `/var/lib/podmesh` otherwise
  (`src/bin/podmeshd.rs:8`). On the **Backup Server** the signer is the datastore, which
  signs catalogue checkpoints and proofs, and whose directory is **not** `/var/lib/podmesh`
  — no package creates that on a host that is not a PodMesh host, and this document
  elsewhere says the Backup Server needs neither Podman nor CRIU.

  **Where that directory is, said plainly, because revision 7 named a package that does
  not exist.** It said "the one the Backup Server package creates". There is no Backup
  Server package: nothing in this repository ships one, and the only package this document
  names is `podmesh-recovery`, which is the bootstrap for a clean host and not the
  datastore. Naming a configuration root that no lot ships is exactly the defect that
  `/etc/podmesh/` was, and revision 7 deleted the sentence saying so and then did it. So
  the datastore's state directory is **an explicit parameter of the service, with no
  default** — supplied at startup, created and owned by the service, and **the service
  halts and names it when it is absent or unwritable**, per Rule 0J. The lot that packages
  the Backup Server chooses a default; until that lot exists, B1 passes the path. A
  literal path was wrong in the first place: PodMesh's own test hosts relocate the state
  directory, and a hard-coded path puts the signing key outside the tree an operator backs
  up, snapshots and destroys.
- **The algorithm is pure Ed25519 (RFC 8032 §5.1), over the canonical serialization
  itself — not Ed25519ph, and not over a digest.** Revision 4 said "Ed25519 over the
  SHA-256 of a canonical serialization", which names neither of the two real schemes:
  pure Ed25519 hashes the message internally, so pre-hashing it produces a signature over
  a 32-byte string that a compliant verifier of the document will not reproduce, and
  Ed25519ph is a *different* algorithm with a different domain-separation prefix. Two
  implementers reading that sentence would have built two incompatible verifiers. The
  manifest's SHA-256 digest is still computed and published, as its **identifier** in the
  catalogue — it is not the signing input.
- **Canonicalization is RFC 8785 (JSON Canonicalization Scheme)**, cited by name and
  version rather than described. Revision 4 described it in prose — sorted keys, no
  insignificant whitespace, integers only — which is most of JCS and not all of it, and
  leaves string escaping and number formatting undefined; those are precisely where
  independent implementations diverge. Numbers are additionally constrained to integers
  representable in 64 bits, and no floating-point value appears anywhere in a manifest.
- **The map from producer to public key** is a pinned local file at
  `backup/producers.json`, listing each producer's identity UUID and its Ed25519 public
  key. It lives **in the state directory of every component that verifies**, by the same
  per-signer rule as the key above.

  **There are two such components, not one, and revision 7 named only the first.** The
  **Backup Server** verifies manifests it did not produce, and its bootstrap restores the
  file with the rest of its directory. But a **restore target** verifies too: this
  document makes it the only party that checks **plaintext** digests, which it reads out
  of a manifest whose signature it must therefore check first. Under revision 7's
  role-relative rule that host had no stated copy at all — a gap revision 6's single fixed
  path had accidentally covered. So **B1 installs `producers.json` on all three lab
  hosts**, with all three producers in it, rather than only on the datastore; "B1
  populates it by hand for three hosts" below means three installations, not three
  entries in one file.

  *Two wrong homes preceded this one, and the second was the first moved sideways.*
  Revision 4 put it under `/etc/podmesh/`, **a directory that does not exist**: PodMesh
  has no `/etc/podmesh`, no packaging creates one, and no other document mentions one.
  Revision 5 moved it to `$PODMESH_STATE_DIR`, which is unset on a Backup Server that is
  not a PodMesh host, where `/var/lib/podmesh` is created by no package — a configuration
  root no lot ships, again. In B1 the Backup Server happens to be one of the three lab
  hosts, so neither mistake would have been caught by running B1.

  A verifier resolves against that file and
  **never** against a key learned from the manifest. **If it is absent or unreadable, a
  verifier halts and reports** — it never falls back to trusting the manifest, and it
  never treats an unverifiable manifest as verified; Rule 0G's "no fake, no fallback"
  is the doctrinal form of the same requirement. B1 populates it by hand for three
  hosts; a later lot may derive it from a topology once one exists that carries per-host
  keys — neither `topology.json` nor `fleet.yml` does today.
- **Catalogue checkpoints and proofs** are signed by the datastore's own key under the
  same rules, and the datastore appears in `producers.json` like any other producer.
- **Retired producers are retained** in that file, because a manifest signed years ago
  must stay verifiable after its signer is gone. **Revocation** marks a producer
  `revoked_from` a stated instant: manifests it signed before that instant stay valid,
  and anything after is refused. Rotation publishes a new public key and **re-signs
  nothing** — re-signing would rewrite history, which immutability forbids.

Key generation, custody, rotation and revocation for **signing** join key escrow for
**encryption** under Decision X2.

Two consequences that are requirements, not remarks:

- **The datastore must rebuild its index from the immutable objects alone** — manifests,
  chunks **and signed catalogue checkpoints**. Losing its own database must never make
  otherwise valid backups unreachable. This is a B1 test, not a later hardening.

  *The checkpoints were added to this sentence in revision 7, and leaving them out was a
  hole revision 6 opened without noticing.* Until revision 6 a hold travelled inside the
  signed manifest and so survived any rebuild by construction. Revision 6 moved holds and
  verification history to the catalogue — correctly, because a producer cannot sign
  facts that arise after it signs — and did not extend this contract to follow them. A
  rebuild from manifests and chunks alone would then come back with **no holds at all**,
  while D4 gates removal on holds and says an unknown hold is a hold. The sweep after
  such a rebuild would either have to refuse everything or quietly delete something under
  a hold it no longer knew about. B1 does not exercise holds, so this was not a B1
  defect; it was a stated safety property with a new gap under it.
- **Verification is split by who can do it.** The server verifies **ciphertext**
  digests, because that is all it can see. Only an authorized restore target verifies
  **plaintext** digests. A report must never present the first as the second.

Never overwrite a manifest or a proof in place: publish a new signed object and
reference it from a later catalogue checkpoint.

## Typed states

*(BBS-R12.)* Per piece and for the recovery point as a whole:

`preparing` → `sealed` → `transfer_incomplete` → `stored_unverified` → `verified` →
`restore_verified`, plus **`failed`** as a terminal outcome reachable from any of them,
and `held` and `retired` as orthogonal conditions.

*Two corrections to revision 3, which copied this list from the counter-review and
inherited its errors.* There was **no failure state at all**, although the consistency
section requires that a piece changing during capture makes the point *a failure*, the
capture contract requires an abort-and-thaw path *whose failure is recorded*, and B1
step 10 tests four interrupted operations. A contract that types outcomes must be able
to name the bad one. And `held` was called terminal; a hold is by definition lifted, so
it is orthogonal, like `retired` is not.

**A recovery point becomes eligible for an ordinary restore only once it is `verified`
on the datastore** — not once it is `sealed`. Revision 3 said `sealed`, which is the
pre-transfer state: the point exists only on the source host, the Backup Server has
never seen it, and calling it restorable contradicts the pull model, D5, and this
document's own first principle that only a transferred, authenticated, catalogued and
independently restorable point is a backup. A `sealed` point is capture material.

**A partial set is never promoted because some chunks survived.** Rule 12's clause
applies in full: a failure after the archive is complete keeps the archive, and a
housekeeping step that cannot run is a reported failure over a surviving archive.

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

**When that refusal starts, and why not in B1.** PodMesh emits no `dataLifecycle` today
— the field exists nowhere in PodMesh outside this document — so a refusal enforced in
B1 would refuse B1's own fixture, a stopped Alpine container holding nothing. The
refusal begins with **B2**, the first lot that captures real data, and B2 must first
make PodMesh carry the declaration. B1 records `data_lifecycle: null` with a typed
reason, which is an explicit absence rather than an assumed `personalData: false` —
silence is still not a declaration; it is recorded as silence.

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
([docs/reference/r2-encrypted-key-backup-2026-09-13.md](reference/r2-encrypted-key-backup-2026-09-13.md)) establishes both the shape
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

### Storage preferences, as hypotheses

*Also restored from commit `0cc7dd5`.* For PodMesh Backup Server deployments, prefer
dedicated snapshot-capable storage qualified by PodMesh. **Btrfs** is the provisional
default candidate for general PodMesh hosts, because it is part of Linux and combines
subvolumes, snapshots, reflink clones and incremental send/receive. **ZFS** is the
provisional candidate for a dedicated backup datastore, where end-to-end checksums,
scrubs, hierarchy replication and raw encrypted send matter more than minimal host
integration. **LVM2 thin** remains an important compatibility backend. These preferences
are **hypotheses until the sequential lab comparison of B0 records equivalent restore
evidence**, and nothing may be built as though they were settled.

And the clause both sibling documents carry and this one had dropped
(`PREPARE-A-HOST.md:56-57`, `DELIVERY-CHECKLIST.md:73`): **no disk is ever reformatted
automatically to obtain a preferred backend.** A host that lacks one uses the portable
fallback and says so.

With a qualified snapshot backend, PodMesh quiesces the protected universe, creates an
immutable local capture and **releases the universe before the longer transfer begins**.
Installation on an ordinary root filesystem remains supported, but the generic capture
keeps the protected universe stopped while its filesystem archive is created. A declared
application-specific consistency adapter may shorten that interruption. A process freeze
alone does not prove that buffered application data, databases and external volumes form
one coherent recovery point. **Installation preflight must report the selected capture
class and the expected interruption before a backup schedule may be enabled** — and
"expected interruption" is a class, not a number (Rule 10).

Targets: a physical server, a virtual machine or VPS, a container, a ShaperOS universe,
and a PodMesh node. **The storage server does not require Podman or CRIU locally** —
receiving and retaining sealed chunks needs neither. Host-side capture preparation does
require runtime access, and memory-coherent capture has further kernel and runtime
constraints; ordinary backup storage must not inherit them.

The normal standalone version is built first. Everything else is an integration of it.

## B1 — what is built, as of 2026-09-14

Steps 1 and the capture half of step 2 exist as `recovery_point_prepare` in PodMesh
(`src/recovery_point.rs`, checked by `tests/check-recovery-point.py`). It produces a
recovery point in the **`prepared`** state: stopped-universe export, manifest with every
field below, digests bound to the files, generations chained, idempotent by operation ID,
escalated stops refused with no class. It is **not sealed**: no signing crate is available
to this build and none has been added, because that is a dependency decision for the
operator. The manifest carries `signed: false` and a format string ending in
`unsigned-unencrypted` so that no verifier can mistake it.

Step 6 exists as `recovery_point_restore` (same file, checked by
`tests/check-recovery-point-restore.py`). From a point in this host's inbox it creates a
**quarantined, new-identity** universe: no network, not started, under a UUID the caller
chooses that must differ from the source's — refused even when the source is unknown on this
host, which is the second-host case. Before it imports anything it checks the archive's size
and digest against the manifest, the manifest's canonical form and pinned format, and that
the manifest does **not** claim a signature: a signature this build cannot verify is refused
rather than trusted, because accepting it would be anchoring in the manifest's own say-so.
The container is created through the ordinary `create` operation under a derived operation
ID, so the restored universe is owned the way every universe is owned and no new ownership
rule exists for it. The check proves the round trip by the marker the source wrote while
running, found again in a fresh export of the restored container; and it proves the refusals
by weakening the daemon and watching each one turn the check red for the stated reason.

What step 6 does **not** do: verify the manifest's origin. The archive is bound to the
manifest by digest; nothing binds the manifest to a producer, and the response says so in a
`manifest_verification` field. Steps 3 to 5 and 7 to 10 are not started: there is no
transport controller, no datastore, no catalogue, no signed manifest.

## B1 — the corrected first lot

Deliberately small, and shaped by the reviewer:

0. **the fixture must contain bytes that only transport can have produced, and revision 4's
   did not.** This is the correction that matters most in this revision, because it is the
   one failure this lot must not be able to have. Revision 4's fixture was a
   never-started, mount-free container: its filesystem export is byte-identical to the
   image it was created from, PodMesh records a local image ID and never pulls, and step 6
   restores onto a host **pre-seeded with that same image**. A round trip that transferred
   zero bytes — or transferred them into a black hole and restored from the local image —
   would have satisfied every check and reported success. The lot would have proven that
   two hosts hold the same public image, which was already true before it ran.

   So the fixture is **marked before it is captured**: start the universe, write a
   distinctive marker into its writable layer — a file whose name and contents are a
   random value generated for this run and recorded in the run's evidence, plus a
   timestamp — then stop it unforced, then capture. The marker exists in **no** image, on
   **no** other host, and in no adapter's default output. Step 7 verifies it on the
   restored copy by reading its contents and comparing them to the recorded value. If the
   marker is absent or differs, the run failed, whatever else succeeded. This is what
   makes the round trip a measurement rather than a tautology.

   **How the marker is written, since PodMesh has no `exec`.** Its local API offers
   create, start, stop, delete, clone and the migration operations, and nothing that runs
   a command in a running universe. Two routes work and both are in this project's
   existing practice: fold the write into the container's own `command` at create time,
   which the API already accepts (`LOCAL-API.md:31-32`), or write it with direct Podman,
   which is this project's declared convention for fixtures as opposed to the operations
   under test (`REVIEW-EXPERIMENTAL3.md:7`). B1 uses the first, so the marker is part of
   the universe PodMesh created rather than something reached around it.

   **And the command must handle its stop signal, or step 0 breaks step 1.** These two
   steps were written together and still nearly collided. A container's PID 1 receives no
   default signal action from the kernel: a plain `sleep` or a bare `sh -c` as PID 1
   **ignores SIGTERM**, `podman stop` escalates to SIGKILL, `forced` comes back `true`,
   and step 1's precondition fails — so the fixture designed to make the run meaningful
   would have made every run fail. The command therefore installs an explicit handler.
   `trap` gives PID 1 a handler so the kernel delivers the signal, and `sleep … & wait`
   is the required idiom: a POSIX shell does not run traps while a foreground command is
   executing, but `wait` is interruptible by a trapped signal.

   **The marker value is interpolated as a literal, because PodMesh passes no
   environment.** The creation request accepts exactly `operation_id`, `universe_uuid`,
   `authorization_ref`, `image` and `command` (`LOCAL-API.md:27-33`), and the daemon
   parses only `image` and `command` (`src/lifecycle.rs:358-371`). A command referencing
   a shell variable would expand it to the empty string, write a zero-byte marker, and
   fail step 7 on every run — the same class of defect as the signal one, in the same
   sentence that fixed it. The run generates the value, then builds the array with it
   already inside:

   ```
   ["sh","-c","printf %s '<run-value>' > /marker-<run-value>; trap 'exit 0' TERM; sleep 3600 & wait"]
   ```

   **On one line, and that is not formatting.** Revision 7 wrapped this with a backslash
   and a newline. JSON strings may not contain a raw newline and `\` followed by a newline
   is not a valid escape, so the request would have been rejected before the daemon ever
   parsed a `command` array. The obvious repair is worse: joining the lines while keeping
   the backslash makes `\ ` a literal escaped space, the next command word becomes
   `" trap"`, the shell reports it cannot run that, and PID 1 is left with **no TERM
   handler** — straight back into the `forced: true` failure these two paragraphs exist to
   prevent. Three fixture defects in three revisions, each introduced while fixing the
   last, all in this one command.

   **`<run-value>` is hexadecimal, and that is a constraint, not a suggestion.** It is
   interpolated raw into a single-quoted shell word and into a filesystem path, so a value
   containing a quote or a slash breaks the command or redirects the write. The run
   generates it from a hex-encoded random value and nothing else.

   The value appears in **both the file name and the contents**, and the run records the
   create timestamp alongside it in the evidence, which is what the marker specification
   above asks for. The sleep bound must outlast **create through stop** — step 1 stops the
   container before capturing, so the capture itself runs against a stopped universe — and
   if PID 1 exits on its own before the stop is issued the container is already gone and
   the stop reports nothing useful. The run records the `forced` flag either way; it does
   not assume any of this worked;
1. one **PodMesh universe** — a Podman container with its UUID, on a Debian 13 lab host —
   marked as in step 0, mount-free and network-disabled, then **stopped with an unforced
   stop** and captured through the **ordinary-filesystem adapter** while stopped for the
   entire capture. The stop's `forced` flag is recorded and **must be `false`**; a `true`
   there is a **failed run**, reported as one. *It does not downgrade the class, which is
   what this step said through revision 6 while the consistency section was being
   rewritten to abolish exactly that. There is no weaker class to fall to: the
   ordinary-filesystem adapter cannot yield `crash-consistent`, forbidden claim 4 forbids
   the label, and `incoherent` is defined for a running universe. This was the one place
   an implementer actually reads to build the run.* No
   snapshot backend is required, which is why B1 does not wait for B0. The operator's
   own clause governs the relationship: **B1 must work through the portable archive
   fallback; where B0 has qualified a snapshot adapter, the same round trip is repeated
   from its frozen local capture.** The fallback path is B1's mandatory one and its exit
   condition; the repeat is a conditional extra that applies only if B0 has already
   finished, and B1 closes without it if B0 has not. Its consistency class is
   `quiescent`;
2. one **authorized sealing operation on PodMesh's root-only local socket** seals its
   configuration and filesystem fixture, signed by the host's own key and bound to its
   `host_uuid`. *Revision 4 called this "one authorized Maker operation" while the
   signing section of the same document had already established that the Maker is a
   ShaperOS organ above PodMesh, is recorded as not implemented, and cannot be a B1
   dependency. B1 cannot require an operation no component can perform;*
3. one versioned signed manifest, a content-addressed encrypted chunk set, **and one
   signed catalogue checkpoint** on the datastore. *The checkpoint was missing from this
   step while revision 7 added it to step 8's rebuild and to the rebuild contract — so
   step 8 would have rebuilt from an object class B1 never produced, and either could not
   run or would have run vacuously and still reported a pass. The same failure mode, one
   section over, in the revision written to fix it.* The checkpoint is what carries
   verification history and hold state, which left the manifest because a producer cannot
   sign facts that arise after it signs;
4. the Backup Server **pulls from the source host's outbox**, outbound only, exactly as
   D1 describes: the source writes the sealed point to its outbox, under **the identifier
   the sealing operation issued** and never under an `authorization_id`, which only
   `migration_authorize_transfer` mints and which a backup cannot obtain; the
   transport controller moves it under **its own** authorization, and the manifest
   signature is checked after arrival to prove the bytes, not to admit the reader. Over
   ordinary existing IP connectivity — **the Rule 13 mesh is not required and B1 does not
   use it.** No `tar.bz2` leaves a host in B1, so Rule 12's transport clause is not
   engaged; which other clauses are is stated below rather than claimed wholesale (X3
   records the amendment still owed, with its owner);
5. the source universe is removed only after its recovery point reaches
   **`restore_verified`** — not `stored_unverified`, and not `verified` either, for the
   reason below. *This step said `verified` through revision 7, which named the defect
   three lines further down and left the normative sentence alone. An acknowledgement is
   not a correction, and this line is the one an implementer reads before destroying the
   only remaining copy of the source.*

   **`verified` on the datastore is weaker than it sounds, and the gate is strengthened
   accordingly.** The datastore verifies **ciphertext** digests, because that is all it
   can see; only an authorized restore target verifies **plaintext** digests. Removing
   the last source on a ciphertext-only check would destroy it without anyone having
   confirmed the bytes decrypt to what was captured — the same class of gap this step's
   own parenthetical exists to close. So in B1 the source is removed only after **step 7
   has passed on the restored copy, marker included**, which is the first moment any
   plaintext has been verified. The typed-states list already has the name for that
   moment: the gate is **`restore_verified`**, not `verified`, and this step should have
   said so rather than describing it. B2 inherits this ordering rather than the weaker one,
   because B1's fixture is the template;
6. restore creates a **quarantined new-identity** copy on another host;
7. external observation verifies files, configuration and application behaviour —
   **including step 0's marker, read from the restored copy and compared byte for byte
   against the value recorded before capture.** The marker check is the step's exit
   condition, not one assertion among several: everything else in the export could be
   satisfied by the pre-seeded image, and only the marker could not. Verification is
   performed from outside the restored universe, per Rule 0G;
8. the datastore index is **deleted and rebuilt** from the immutable objects alone —
   manifests, chunks and signed catalogue checkpoints;
9. the encryption key is recovered **from the X2 interim material defined below** — not
   from the source host and not from the datastore — and the restore is repeated on a
   host that never held the key. *Revision 4 left X2 wholly undecided while keeping this
   step, so the step named material that did not exist and could not have been produced;
   an implementer would have reached it and stopped. The interim is now stated, so the
   step has something to recover from;*
10. interrupted capture, transfer, manifest publication and restore are each retried
    without duplicate identity and without false success.

Plus the clauses of Rule 12 that B1 can actually exercise, which are fewer than a blanket
claim suggests — and revision 4 got the list itself wrong in two places, so it is restated
here precisely.

*The two errors: it said B1 has "no `tar`", when B1's mandatory path is the **portable
archive fallback** and the checklist's own wording for it is "stopped-universe archive"
(`DELIVERY-CHECKLIST.md:65`) — B1 does produce an archive, and step 1 says so. And it
put the **archive-failure clause** among the clauses with "no subject yet" and then
listed that same clause, in its own words, among the ones B1 exercises. A document
cannot both test a clause and have no subject for it.*

*Revision 5's list was the third loose attempt in three revisions: it omitted clause 4
entirely, counted clause 5 twice as though it were two, filed volume exclusions under
Rule 12 when they are Rule 16 level 2, and claimed a clause whose testable half has no
subject in PodMesh. Rule 20 makes coverage limits a recorded obligation, so the
accounting is done clause by clause against the canon's own order.* The eight clauses of
"what a backup archive never contains, and what it never lies about" (`RULES.md:759-803`):

| # | Clause | B1 |
| --- | --- | --- |
| 1 | The key that opens the coffer does not travel with the coffer (`:759`) | **exercised** — the KEK's two copies live outside the source host and outside the datastore, per the X2 interim |
| 2 | The backup's encryption key is its own key (`:765`) | **partly** — the half that says the key reaches its tool through the environment and never a command line is exercised; the half that refuses a key equal to `VAULT_MASTER_KEY` has **no subject**, because PodMesh has no vault and no such key exists to compare against |
| 3 | A dump that was not taken is announced, never written empty (`:772`) | **no subject** — no database |
| 4 | The archive command's failure is the backup's failure (`:779`) | **exercised** — B1's mandatory path produces an archive, and the status line is printed only after that archive exists, has a size and has a checksum. Step 10's interrupted capture is this clause's test |
| 5 | A failure after the archive is complete keeps the archive, and a housekeeping step that cannot run is a reported failure over a surviving archive (`:783`) | **exercised** — one clause, tested by step 10's interrupted transfer and publication |
| 6 | A dump that failed leaves nothing behind, written `.part` and renamed only once it has a size (`:788`) | **no subject** — it is written about dumps and B1 has none. The archive analogue is clause 4 and is not double-counted here |
| 7 | `.env` in every spelling (`:793`) | **no subject** — no `.env`, including `deploy/env` |
| 8 | How the script calls the client is proven with a recorder (`:796`) | **no subject** — no dump client to call. B2 and B3 introduce both the client and the recorder |

**Not engaged at all:** Rule 12's transport requirements (`:753-757`; `:758` already
opens the eight clauses above), scoped by their own
opening line to `tar.bz2` archive transfers. B1's archive is chunked and encrypted before
it moves and no `tar.bz2` leaves anything — see X3, and its owner.

**Also not Rule 12:** volume exclusions, which revision 5 filed here. They are Rule 16
level 2 and arrive with B2.

B1's evidence record reproduces this table with its observed results, rather than claiming
all eight.

**B1 explicitly does not prove:** persistent volumes, databases, original-identity
activation, DNS, memory continuity, off-site recovery, retention, arbitrary Linux
distributions, ShaperOS integration, or Backup Server high availability. **Nor does it
prove Rule 11 restorable identity**: PodMesh records a local image ID and never pulls,
so B1's restore target is pre-seeded with the image and the round trip proves transport,
not that the universe could be rebuilt anywhere. **Nor does it prove the deduplication
economy**, which needs two universes and does not exist at n = 1. A stopped
mount-free filesystem export is a **transport fixture**; it is not Rule 16 level 2 and
must not be counted as Rule 16 coverage.

### B1's forbidden claims

*Stated as a closed list, because "does not prove" in prose has twice been read as
hedging and then contradicted three sections away by a sentence that claimed the thing.*
These are the claims B1's evidence record, its commit messages, its summary to the
operator and any document quoting it **may not make**, whatever the run shows:

1. **Rule 16 coverage of any level.** B1 covers no level. Level 2 in particular is
   volumes, and B1 has none.
2. **Rule 11 restorable identity.** The restore target is pre-seeded with the image; see
   the image-lock note above.
3. **Any deduplication figure**, saving, ratio or percentage. At n = 1 across universes
   there is nothing to measure and the interim deduplicates nothing.
4. **Application-consistency or crash-consistency.** B1's class is `quiescent`, and only
   when the stop reported `forced: false`.
5. **Off-site behaviour or retention behaviour.** Neither exists before B4 and B5, and
   X4's credentials do not exist at all.
6. **Hold behaviour.** Holds arrive with B4's lease barrier.
7. **That the datastore learns nothing about content.** No shape in the X1 table achieves
   that; row 1's own leakage note says what it does and does not achieve.

A run that satisfies all ten steps proves that a marked universe survived a seal, a
transfer, an index rebuild, a key recovery and a restore, and was verified from outside.
That is the whole of it, and it is worth having exactly because it is bounded.

## Delivery order: B0 and B1 in parallel, then B2 onward

**B0 — qualify local capture backends.** *(Restored from `0cc7dd5`.)* Run the authorized
sequential 60 GB lab comparison in [LVM-LAB-PLAN.md](LVM-LAB-PLAN.md): LVM2/thin, ZFS,
then Btrfs. For each backend, prove quiesce, snapshot, immediate release of the live
universe, transfer from the snapshot, restore on another host, failure handling and
capacity behaviour. Also prove the ordinary-filesystem fallback with a stopped universe.
This establishes the capture adapters the later lots use; **it does not make a local
snapshot an independent backup.**

B0 is `DELIVERY-CHECKLIST.md` §4 and it comes before §5. B1 does not depend on it — a
stopped, mount-free fixture needs no snapshot backend — so the two may proceed in
parallel, but B2 onward do depend on it.

Then B2 volumes (level 2, with the `nosav/` exclusion and the `.env` exclusion in every
spelling, including `deploy/env`). B3 databases **and the Qdrant snapshot** (level 3
under Rule 12's dump laws). B4 retention, holds and sweep with the lease barrier. B5
off-site (level 5), once Decision X4's credentials exist. B6 memory-coherent points,
where the runtime permits, reusing the migration chain's checkpoint machinery and its
proven limits. Later: scheduling policy, the human interface, and the level 1
container/VM snapshot where a hypervisor provides it.

## Decisions that remain the operator's

**None of these blocks B1** — see the interim below. Each is stated with what it costs,
because none has a free answer.

**X1 — the deduplication and encryption domain.** Four shapes, not three:

| Shape | Dedup reach | Leakage | Per-universe crypto-erasure | Blast radius of one compromised host |
| --- | --- | --- | --- | --- |
| Randomized AEAD, one key per universe | none across universes | metadata only: chunk count, chunk sizes and capture times per universe | yes | that universe only |
| Randomized AEAD, one key per declared domain | full within the domain | the index reveals which universes share a chunk | no | the whole domain |
| Convergent encryption, one key domain | full within the domain | plaintext **equality**; confirmation-of-content for anyone holding the index and a candidate file | no | the whole domain |
| **Convergent chunks, per-universe wrapped chunk keys** | **full** | same equality cost as the row above, nothing new | **yes** | that universe's key only |

*The first row said "Leakage: none", and that was false in a table whose purpose is to
let an operator compare confidentiality costs.* No shape leaks nothing. Even with a
distinct random key per universe, the datastore still learns how many chunks each
universe has, how large each is, when each capture happened and how the sizes move
between generations — which is enough to infer a universe's rough size, its change rate
and its backup schedule. What row 1 avoids is **cross-universe plaintext equality**, not
observation. The claim "the datastore learns nothing about content" is not available to
any shape here and is on this lot's forbidden-claims list.

The fourth shape was missing from revision 3 and it is probably the answer. A chunk is
encrypted under a key derived from its own plaintext and stored once; each universe's
manifest carries that chunk key **wrapped under that universe's own KEK**. Destroying a
universe's KEK crypto-erases it — manifest and every chunk key with it — while its
neighbours keep reading. Revision 3 claimed dedup and per-universe erasability were
mutually exclusive; that conflated the *chunk-encryption* domain with the *wrapping*
domain, and only the second must match the erasure unit. The residue is that a
co-tenant still holds byte-identical content — but a chunk deduplicates only when two
universes genuinely hold the same bytes, which is shared data rather than a failed
erasure.

What is still missing before this can be decided: **a cost figure for the dedup reach**.
PodMesh has no chunk store, so the saving is asserted and not measured — and B1 cannot
supply the number either, because its interim deduplicates nothing across universes and
there is only one. The first real figure needs two universes under one candidate shape,
which is a measurement lot of its own.

**And the canon may already have answered it.** `RULES.md:1214` says R2 buckets are
*"derivable (`r2://<instance-id>`), never enumerated"*, while this design proposes one
bucket with universes enumerated inside it. A per-instance bucket makes cross-universe
deduplication impossible at the storage layer — which would settle X1 as shape 1 by
canon rather than by preference. This document cites Rule 37 as binding for `lastBackup`
and cannot ignore its bucket clause without saying why. **Before X1 reaches Xavier,
somebody must establish whether Rule 37's fleet map reaches PodMesh universes at all**
(defensible either way: a PodMesh universe may not be a ledger instance). If it does,
there is no decision to take.

Storage economics push toward the second, third or fourth; confidentiality pushes toward
the first. I will not choose this on your behalf, and revision 1 was wrong to.

**X1 and X5 are related, not identical.** Revision 3 said X1 decided X5; that was wrong,
and the fourth shape is why. X1 determines whether crypto-erasure at universe
granularity is *available*; it does not determine whether erasure is honoured, and it
has no bearing at all on the declared-exemption route. Answer them together, because the
wrapping domain must match the erasure unit — but they are two questions.

**X2 — who owns and can recover each domain key**, and where the two independent copies
of the root recovery material live. One of them must be outside the provider that holds
the ciphertext, and neither may depend on PodMesh being restored.

**X3 — extending Rule 12's transport clause, which is a gap and not a collision.**
Revision 3 called this a canon conflict and escalated it without reading the scope
first. It is narrower than that. Rule 12's transport requirements are scoped by their
own opening line to *"All archive transfers (`PROJECT.tar.bz2`, `REMOTE.tar.bz2`)"*
(`RULES.md:753`), and Rule 16 widens them to *"any `tar.bz2` that leaves the host"*
(`RULES.md:848`). This service transfers **content-addressed encrypted chunks**, which
are not `tar.bz2` leaving a host. The literal canon therefore does not govern this
transport: it is **silent**.

Silence is not permission, so an amendment is still owed — the canon should say what
governs a chunk pull, a datastore sync and an operator download separately, and the
Cloudflare Tunnel remains right for the distribution case it was written for.

**And the canon says where an owed amendment is filed**: `RULES.md:5-6` and Rule 35
(`RULES.md:1108`) send doctrine-versus-code gaps to `doctrine/CONVERGENCE-STATE.md`.
That file exists and today carries no PodMesh entry at all. Declaring an amendment
"owed" with no owner and no trigger is precisely the shape Rule 35 calls quietly
softening doctrine to match what was built, so **this gap must be filed there, with an
owner, before B1 closes** — not before it starts. But **B1 is not
blocked on it**: B1 names its own transport, below, and the amendment follows the
measurement rather than preceding it.

**The owner, since revision 4 required one and then named none.** The
`CONVERGENCE-STATE.md` entry is written by **whoever closes B1**, and it is a named exit
condition of that lot rather than a standing intention: the entry is filed before B1's
evidence record is signed off, and B1 does not close without it. Its trigger is B1's own
measurement, which is what gives the amendment a subject. The amendment itself is then
Xavier's to accept, because it changes canon and no agent amends canon. Filing the gap
and deciding it are two acts with two different owners, and revision 4 collapsed them.

Where a `tar.bz2` *does* leave a host — B2's volume archives are exactly that — Rule 12
applies in full and unamended.

**X4 — the off-site credentials.** Provision the three bucket-scoped R2 credentials, or
name a different off-site target. Until then level 5 is unreachable and B5 cannot start.

**X5 — erasure granularity.** Whether a client erasure request can be honoured on
backups, and by what mechanism: crypto-erasure at a wrapping domain no wider than the
erasure unit, per-universe keys without cross-universe deduplication, or a declared
exemption written into `onTermination` **before** any capture. See the data lifecycle
section. This must be settled before B5 places anything under a bucket lock, because a
lock cannot be lifted for a mistake.

## What B1 uses in the meantime, so that none of the above blocks it

B1 captures **one** universe. Under every shape in the X1 table, one universe is one key
domain, and nothing in B1 depends on cross-universe deduplication — there is no second
universe to deduplicate against.

- **Encryption:** per-universe randomized AEAD with a per-backup DEK wrapped by a
  per-universe KEK. This is the interim, chosen because at n = 1 there is no
  second universe to deduplicate against, so it forecloses nothing B1 could have
  measured. It is **not** neutral: the convergent shapes deduplicate identical chunks
  *within* one universe and randomized AEAD deduplicates nothing, so adopting X1's
  answer later means re-keying or re-chunking B1's store. That cost is known, bounded,
  and smaller than not starting.
- **Key custody — the X2 interim, which B1 step 9 recovers from.** The universe's KEK is
  generated on the source host and never transmitted. Before the first capture, it is
  exported once, wrapped under a passphrase the operator supplies and holds, into **two
  copies that live nowhere in the system under test**: one on removable media held by the
  operator, one in the operator's existing password manager. Neither is on the source
  host, on the Backup Server, or in the datastore, and recovering either depends on
  nothing PodMesh runs — which is the property step 9 exists to demonstrate. The
  passphrase is not written down beside the material it protects.

  This is a **lab interim for a three-host lot, and it is not the answer to X2**. It does
  not scale, it names the operator as the single point of failure, it has no rotation
  path and no revocation path, and one of the two copies is inside a provider the
  operator does not control. X2 remains open, with everything it asks unanswered: who
  owns each domain key at fleet scale, where the two independent copies live, and which
  of them sits outside the provider holding the ciphertext. The interim exists so that
  step 9 has material to recover, not so that X2 can be skipped.
- **Erasure:** crypto-erasure by destroying that universe's KEK **and both interim
  copies** — which is the first place the interim's cost shows, since an erasure that
  misses the removable copy is not an erasure.
- **Transport:** a pull from the source host's outbox over ordinary existing IP
  connectivity, authorized by the transport controller's own credential, as D1 sets out.
  **Not the Rule 13 mesh**, which B1 does not require and does not use. The scope finding
  in X3 is recorded and the amendment is owed, with its owner named there. No `tar.bz2`
  leaves a host in B1, so Rule 12's transport clause is not engaged; which other clauses
  B1 exercises, and which have no subject, is listed with the B1 steps rather than
  claimed wholesale.
- **Manifest signing:** see below.

When X1 is answered, B1's chunk store is re-keyed or re-chunked as that answer requires.
That is a known and bounded cost of starting, and it is smaller than the cost of not
starting.

## Smaller canon obligations, recorded so they are not rediscovered

Each is a verbatim clause of the canon that binds this service without changing its
shape. They belong in the B1 checklist, not in a later hardening pass.

- **Private keys never cross a level** (Rule 36, `:1117`, `:1126`): the parent's Ed25519
  authority key never leaves the parent, and no private key climbs into a ledger or
  descends into a child. No archive may contain one.
- **`lastBackup` in `status.json` is canonical** (Rule 37, `:1174-1176`): every board or
  cockpit tile is a rendering of it, never a rival.

  *Revision 4 added "this service writes it", which asserts two objects PodMesh does not
  have.* There is no `status.json` anywhere in PodMesh and no ledger instance for a
  PodMesh universe to be; Rule 37's fleet map is a ShaperOS structure, and whether it
  reaches PodMesh universes at all is the open question recorded under X1 above. So the
  honest form is conditional: **where a deployment has a ShaperOS ledger, this service
  updates that ledger's `lastBackup` and maintains no competing authoritative state
  file; where it does not — which is B1 — there is nothing to write and the service
  invents no substitute.** Its own records are the manifests and the catalogue, which
  are evidence, not a rival status surface. Resolving X1's Rule 37 question resolves
  this one with it.
- **The test universe is destroyed after it passes** (Rule 10, `:542`;
  `SHAPER-OS-V1.14/LAW.md:13` and `:19` — the root file, not the eight-line pointer at
  `software/LAW.md`): a validation run rebuilds from empty and destroys the vehicle, which is what
  makes it a cold-recovery proof rather than a warm one.
- **Never wipe what you did not provision** (`SHAPER-OS-V1.14/docs/agent/BOOT-CONTRACT.md`
  §2, `:27-30`): no volume, no
  database, no universe. If it is unclear whether a machine carries production, it does.
- **Halt on a missing secret** (Rule 0J, `:232-233`; BOOT-CONTRACT §12, `:173`): a
  required key that is absent, empty or still a placeholder stops the run before
  anything is built or launched, and names what is missing.
- **Off-site encryption is AES-256-GCM to a cold bucket** (Rule 16, `:844`), copying the
  archives and dumps — never a git clone pretending to be a backup.
- **Namespace isolation on restore** (Rule 22, `:948-949`): a universe's vector
  collection is its own, and a restore never cross-mounts one into another.
- **Multi-threaded compression** for archive creation (Rule 12, `:754`) — this binds
  **B2 onward**, which create `tar.bz2` archives. B1 creates none, and X3's scope
  finding rules the same bullet list out for a chunk pull; keeping it as a B1
  obligation would contradict that finding in the same document.
- **Proof is read from outside the producer** (BOOT-CONTRACT §9, `:102-106`; the
  doctrinal parent is Rule 0G, `RULES.md:164-176`, "no fake, no fallback"): a
  `COMPLETED` status is not proof that a file is correct, and a health endpoint
  answering 200 is not proof that a job ran.
- **A repair loop that can give up** (Rule 27, `LAW.md:24`): B1 step 10 retries four
  interrupted operations, so each needs a declared bound, a backoff and a resting
  terminal state. A retry that never stops is not recovery.
- **The parent repairs the child, never itself** (Rule 23, `LAW.md:17`; the SSH-authority row beneath it at `:18` is Rule 36, not Rule 24 — Rule 24 is the Root Guardian Law, `RULES.md:960`): the
  recovery agent described below sits **outside** the service it restores. A Backup
  Server that repairs its own running instance breaks the external-healing law, which
  is exactly why the bootstrap is a separate `podmesh-recovery` package on a clean host
  rather than a self-repair mode.
- **Every WireGuard peer block carries a human-readable comment** (Rule 13,
  `:812-819`): the canon requires `### Client <hostname> (CT <vmid> on <host>)` above
  every `[Peer]`, and *"anonymous or untagged peer blocks are strictly prohibited"*. B1
  does not use the mesh, so it registers no peer and the clause has no subject there.
  The moment a deployment does put the Backup Server on the mesh — the highest-value
  target in the constellation, per D1 — its peer block is exactly the one that must be
  identifiable at a glance, and an unnamed key on the gateway is the harder failure to
  audit later.
- **Every checklist item records what actually happened** (Rule 20, `:906-909`): target
  and source version, execution date, steps, expected and observed results, the actual
  evidence reference, the execution actor, and coverage limits — with independent review
  supplementing the agent's own run, and human acceptance separate from both. This binds
  B1's ten steps directly and is why B1 produces an **evidence record** rather than a
  passing run: the marker value from step 0, the stop's `forced` flag, which Rule 12
  clauses were tested and which had no subject, and what the lot did not cover all belong
  in it. A green run with no such record does not satisfy Rule 20, and this document's
  own forbidden-claims list is a coverage limit in the rule's sense.
- **Whatever this becomes needs its own intent and topology entry** (Rules 0D and 0E),
  with an immutable tag that production never floats to.

## A deletion this document has to own

Revision 3 of this file silently reverted commit `0cc7dd5` — "Qualify snapshot backends
before Backup Server capture", authored by Xavier with Codex at 22:21 on 2026-09-13,
one hour and forty-three minutes before it. That commit had added lot B0, the "Capture
adapters and consistency" section and the storage recommendation: 54 lines, three
operative clauses, and the only named source of a `crash-consistent` capture in the
whole design. It was deleted because revision 3 was written as a whole-file rewrite by
an agent that had not re-read the file first, and the tooling's warning that the file
had changed was misread.

Three sibling documents kept pointing at the contract this one had dropped:
`PREPARE-A-HOST.md:47-48`, `LVM-LAB-PLAN.md:11`, and `DELIVERY-CHECKLIST.md:64-65`
and `:70`. An implementer starting B1 in that window would have found no named
capture source and would plausibly have invented `podman pause` plus a copy — which is
precisely what the restored section forbids, and precisely the error the first
counter-review's BBS-R1 was written to prevent.

The content is restored above, merged rather than pasted back: the adapters now sit
beside the consistency classes they produce, because that is the relationship the two
sections always had. It is recorded here rather than fixed quietly because a design
document that quietly loses an operator's decision is worth less than one that says
where it has been wrong.

## What must be proven, not claimed

A restore onto a **different** host, verified from outside. That a **destroyed**
universe comes back, not that a copy exists. That the key is **recoverable** by the
operator from their own material, the coffer alone proving nothing. That a **partially
failed** capture is reported as failed and keeps whatever it completed. That the
datastore's index can be **destroyed and rebuilt** from immutable objects. That a
compromised Backup Server credential **cannot command a host**. Each deployment mode
separately. Measured recovery time and measured data-loss bounds, per level and per
consistency class.
