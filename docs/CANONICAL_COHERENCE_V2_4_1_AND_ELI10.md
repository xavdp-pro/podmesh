# CANONICAL COHERENCE & ELI10 OVERVIEW (`PodMesh v2.4.1` × `Shaper OS`)

## 1. Explaining `PodMesh` to a 10-Year-Old: "The Spaceship for the LEGO House"

Imagine your company (or your school) is a **big LEGO house** built inside a computer:
- Inside this house, there is a room for the **telephone** (`VOX`), a library to store all your **papers and drawings** (`GED`), an office for **accounting and work** (`Work`), a **storefront** (`Shop`), and a **master key registry** (`Directory`).

With ordinary computers, there are two big problems:
1. If the computer breaks down or catches fire, **the entire LEGO house is destroyed**.
2. If 100 people save the exact same big book in the library, it takes up **100 times the space** in your room!

**That is where `PodMesh` comes in: it is a magic armored spaceship that carries your LEGO house.**

Here are its **4 superpowers**:

1. **Teleportation in Under 2 Seconds (`PodMesh`)**:
   If Computer #1 starts getting tired or needs to be turned off, `PodMesh` picks up your entire LEGO house all at once — including the people talking on the phone inside it — and **teleports it to Computer #2 in 1.6 seconds**, without even dropping the phone call!
2. **Magic Bricks That Never Take Up Space Twice (`MeshFS` & `BLAKE3`)**:
   Whenever you save a large file or video, `MeshFS` slices it into small 4-megabyte cubes and gives each cube a **unique digital fingerprint** (`BLAKE3`).
   If 50 people save the same large file — whether on Linux or on a Windows `Z:` drive — `MeshFS` recognizes the fingerprint and **stores only one copy of that cube**. You save a massive amount of disk space!
3. **The Instant Time-Vault (`PMBS`)**:
   When you want to make a safety backup, `PMBS` does not even need to copy all your cubes again: it simply looks at the cubes already sitting in `MeshFS` and writes down their fingerprint list in a tamper-proof ledger (*Zero-Transit Backup*). Even if a hacker tries to erase your files, your cubes remain locked safe inside the vault.
4. **Invisible Secret Tunnels (`WireGuard`)**:
   All `PodMesh` computers talk to each other through secret underground tunnels. From the outside on the Internet, nobody can see a single open door or window (`GEM-001` Zero Open Ports): thieves do not even know where to knock.

> **In one sentence:**
> **`PodMesh` is the armored flying carpet that lets an entire company live, store its belongings without ever wasting space, and jump from one computer to another in the blink of an eye without ever stopping.**

---

## 2. The 4 Architectural Golden Rules (`v2.4.1`)

1. **1 Component = 1 Pure Rust Engine (`*-engine`) on 1 Unix Socket (`/run/shaper/<unit>.sock`) + 1 Optional Web UI (`*-web`)**.
2. **AI Works Without Bureaucracy Inside Its Mandate (`00-TANDEM.md`)**: Humans and AI agents act through the exact same declared UDS contract; routine work within mandate executes immediately in $\mathcal{O}(1)$; only material changes of scope escalate to a single-click human approval.
3. **Disk = `BLAKE3` (`b3sum`) · Memory / Protocols = `SHA-256`** + per-document `doc_dek` in `funit-vault` for GDPR Art. 17 crypto-shredding on Btrfs Copy-on-Write storage.
4. **Atomic Space Teleportation in `1.6 s`** driven by `space.manifest.json`.
