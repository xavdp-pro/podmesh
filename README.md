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

## Current scope

An experimental Rust service and CLI provide local inventory, persistent host identity, an observation journal, and initial managed-container creation, deletion and cloning. These operations have been exercised on three laboratory hosts. Cloning is limited to network-disabled copies of stopped, PodMesh-created containers without volumes or bind mounts, taken through a committed snapshot image. Start/stop, migration integration, networking, general volume handling and high availability remain incomplete.

The service currently exposes a root-only Unix socket and uses the default rootful Podman store. A request's authorization reference records provenance; it is not an implemented remote authorization system. The local administrator controls access.

See [the intent](INTENT.md), [delivery checklist](docs/DELIVERY-CHECKLIST.md), and [acceptance test plan](docs/ACCEPTANCE-TEST-PLAN.md) for requirements and qualified status.

## Debian delivery

[deb.xavdp.pro](https://deb.xavdp.pro) hosts the signed experimental APT repository. The initial PodMesh package is available for Debian 13 amd64; stable and other architectures are not yet validated.

The delivery objective is an installable dependency chain for the complete supported system. Publish project-specific components and required patched runtime packages with explicit versions, dependencies, conflicts and rollback instructions. Use Debian's existing packages for ordinary dependencies rather than unnecessarily republishing them. The patched migration runtime and future backup service are not yet delivered as a complete APT installation chain.

## Collaboration

Created by Xavier de Poorter, in collaboration with OpenAI Codex — GPT-6 Astra. Human direction, implementation, experiments and critical review form a shared development process. Public claims of reliability must identify the tested scope and remaining limits.
