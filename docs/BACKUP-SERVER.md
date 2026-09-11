# PodMesh Backup Server

Status: requested design direction; not implemented or validated.

## Purpose

Store versioned universe recovery points independently of live replicas. Support configuration, required image references or content, persistent volumes and optional memory checkpoints. Replication alone is not backup: it can propagate deletion or corruption.

## Deployment portability

The operator prefers deployment inside ShaperOS for our own use. Standalone operation without ShaperOS remains mandatory across supported host types; this preference must not introduce a hard ShaperOS dependency.

The backup server must be installable independently on a supported Linux host, initially targeting Debian. Deployment targets include physical servers, virtual machines, VPS or virtual hosts, containers, and ShaperOS universes. ShaperOS and any particular hypervisor are not prerequisites.

Use the same backup and restore contracts in standalone and integrated modes. ShaperOS integration may reuse its logging, supervision and authority services without imposing them on standalone users. Document and test persistent storage, permissions, networking and resource requirements for each supported mode. These are target requirements, not a claim of compatibility with every operating system or container environment.

Separate the storage server from host-side capture: receiving and retaining backups does not inherently require running Podman or CRIU locally. Host-side capture of filesystem, application and optional process state requires appropriate runtime access and compatibility. Recovery with memory has additional kernel/runtime constraints; ordinary backup storage must not inherit those unnecessarily.

## Planned capabilities

- Agent-accessible API and CLI, with a later human interface.
- Scheduled capture and explicit retention policies.
- Integrity verification and encryption with documented key recovery.
- Consistent recovery manifests linking configuration, image identity, volumes and optional memory state.
- Restore to another compatible host, with explicit identity and activation rules.
- Independently verified restore tests, measured recovery time and documented data-loss bounds.

## Delivery order

Continue the current PodMesh lifecycle tests before implementing this component. Design the backup server as a separately deployable service. Validate each supported deployment mode rather than assuming host installation proves container or ShaperOS operation.
