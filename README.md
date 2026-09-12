# PodMesh

Portable Podman operations for human-agent tandems.

## Why we started

PodMesh grew out of Xavier de Poorter's extensive hands-on experience with Proxmox: a platform he values and has used for many successful experiments, alongside difficult recovery situations and constraints around joining populated hosts, identities and cluster reconfiguration. Those experiences inspired a desire for more freedom to connect, separate, recover and move workloads between autonomous hosts.

Xavier and OpenAI Codex (GPT-6 Astra) began by experimenting with memory-preserving migration of nested Podman containers. The work built on CRIU and the existing nested-process support in OpenVZ/Virtuozzo's vzcriu, with experimental adaptations for the Linux environment used in the laboratory. The original projects retain credit for their underlying implementations.

- [Experimental vzcriu fork and patches](https://github.com/xavdp-pro/vzcriu/tree/experiment/debian13-nested-podman)
- [Nested Podman reproduction kit](https://github.com/xavdp-pro/vzcriu/tree/experiment/debian13-nested-podman/contrib/nested-podman)
- [Recorded replication experiment and limitations](https://github.com/xavdp-pro/vzcriu/blob/experiment/debian13-nested-podman/contrib/nested-podman/REPLICATION.md)

These experiments demonstrated selected workload recovery and migration cases; they do not establish general fault tolerance or uninterrupted application availability. The analogy with live VM migration motivated the work, but this is not an implementation of VMware vMotion.

PodMesh is an independent Podman project for Linux hosts, not a Proxmox extension or compatibility layer. Physical servers, virtual machines and VPS are deployment targets. ShaperOS is our preferred integration environment, while standalone operation remains a requirement.

See [the experimental scope and review contract](docs/EXPERIMENTAL-SCOPE.md) for
what we are exploring, what the evidence establishes, and how this relates to the
current SHAPER runtime standard.

## Current scope

An experimental Rust service and CLI provide local inventory, persistent host identity, an observation journal, and initial managed-container creation, explicit start and stop, deletion and cloning. These operations have been exercised on three laboratory hosts. They act only on network-disabled containers whose creation is recorded in the host's PodMesh journal. Start reports the observed outcome; stop requires a declared graceful timeout and escalation behavior. Cloning is limited to stopped containers without volumes or bind mounts, taken through a committed snapshot image. Migration integration, networking, general volume handling and high availability remain incomplete.

The service currently exposes a root-only Unix socket and uses the default rootful Podman store. A request's authorization reference records provenance; it is not an implemented remote authorization system. The local administrator controls access.

See [the intent](INTENT.md), [delivery checklist](docs/DELIVERY-CHECKLIST.md), and [acceptance test plan](docs/ACCEPTANCE-TEST-PLAN.md) for requirements and qualified status.

## Debian delivery

[deb.xavdp.pro](https://deb.xavdp.pro) hosts the signed experimental APT repository. The published package is `0.1.0~experimental6` for Debian 13 amd64. The repository keeps one version per suite, so retain a verified copy of the previous package before upgrading; an older version may no longer be available from this suite. Stable and other architectures are not validated.

The delivery objective is an installable dependency chain for the complete supported system. Publish project-specific components and required patched runtime packages with explicit versions, dependencies, conflicts and rollback instructions. Use Debian's existing packages for ordinary dependencies rather than unnecessarily republishing them. The patched `podmesh-vzcriu` runtime and separate helper packages have been published and tested through experimental APT. Dependency installation was checked in a clean Debian container using the pinned package artifacts; complete clean-host runtime qualification remains open; the future backup service is not implemented.

## Collaboration

Created by Xavier de Poorter, in collaboration with OpenAI Codex — GPT-6 Astra and Claude Code. Codex contributed the initial migration research, implementation and review; Claude Code has contributed implementation, packaging, tests and review. Individual commits and evidence records identify the contributing engines and versions. Human direction, implementation, experiments and critical review form a shared development process. Public claims of reliability must identify the tested scope and remaining limits.
