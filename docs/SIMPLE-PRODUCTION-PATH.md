# Simple production path

Order matters.

## A — Container life (first)

On each host, for universes it owns:

1. Discover  
2. Create (local image; no silent pull)  
3. Start (report what is observed)  
4. Stop (timeout and escalation)  
5. Pause / resume  
6. Resources  
7. Clone when stopped  
8. Delete only what this host’s journal owns  
9. Storage status; declare / grow a volume within bounds  
10. Come back after reboot for what must return  

Results: accepted, in progress, failed, verified.

MariaDB for `podmesh-node` belongs here.

## B — Move and replicate (second)

1. **Move** — relocate the outer Podman universe (nested Podman follows);
   identity kept; memory when that is the case.  
2. **Replicate** — recovery point / live copy; planned switchover when needed.

Prove each once under PodMesh’s contract. Do not retest the world for every gesture.

## C — Later

Managed network, secrets **API**, publisher, automatic HA, vote keys,
load-placement agent. After A and B are boring. Gate 4 stays HELD.

Store plumbing that already lands secrets fields on `DurableStore` (assembly
branch) is scaffolding for A, not a C claim.

## Roles

- Human: which service may exist.  
- Agent: typed operations.  
- PodMesh: execute and report.  
- Replicated manager: coordinates across hosts — not this workstation.

## Done

A and B without this laptop. Site and wiki say the same thing.
SQLite on the node is scaffolding until `podmesh-node` MariaDB; manager MariaDB follows.
