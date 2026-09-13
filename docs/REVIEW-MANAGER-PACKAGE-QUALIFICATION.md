# Manager package-only qualification review

Date: 2026-09-12
Source commit: `5319ab150fc34abb0b1648be0c3a68350975cebb`
Package: `podmesh-manager` `0.1.0~manager1+g5319ab150fc3`
Scope: signed publication and disabled-service installation on three existing disposable laboratory hosts

## Result

The exact manager package candidate was published through the signed
`trixie-experimental` APT suite and installed through APT on three existing Debian
13 laboratory hosts. The package-only comparison passed independently for the
three sanitized host aliases and for the combined three-host set.

This result qualifies package publication and installation with the manager
disabled. It does not qualify a configured resident process, replica exchange,
manager identity recovery, DNS, external effect exclusion, host failover or HA.

## Candidate binding

- Package SHA-256:
  `f932f5291684cfe92ba686e095cfe052d4503f6d6564e75f1ff7cf7c8e2448b6`
- Installed manager binary SHA-256:
  `e72c324b09c24ea3ae4b14af148680604d85e6edcbb249b49a8cc23fec41e741`
- Source commit:
  `5319ab150fc34abb0b1648be0c3a68350975cebb`
- APT signing fingerprint:
  `870B13865A81810E19109668A7DE52F62814551B`

The offline verifier checked the signed `InRelease`, the covered `Packages`
index, the exact package stanza, package hash, embedded binary hash, regular
payload file set and hashes, and Debian maintainer-script set and hashes. The
same verified package candidate was then bound to the installed files on every
host. `dpkg --verify` was clean on all three.

## External observations

Before installation, each host recorded the installed lifecycle and observer
package versions, service states and PIDs, Unix socket metadata, and salted
commitments of the complete rootful Podman container, image, volume, network and
pod projections. The manager package, account, configuration, state directory,
runtime directory and process were absent.

After installation, every host showed:

- the exact verified manager package version;
- a loaded, disabled and inactive `podmesh-manager.service`;
- zero running processes with the packaged manager executable;
- no manager configuration and no runtime directory;
- an empty `/var/lib/podmesh-manager` owned by the dedicated non-login account
  with mode `0750`;
- unchanged lifecycle and observer PIDs, unit states and socket metadata;
- unchanged salted rootful Podman commitments.

The comparison harness returned `PASS` with no failure for `lab-a`, `lab-b`,
`lab-c` and the combined three-host comparison. Separate private evidence binds
the aliases to three distinct operating-system instances without publishing
laboratory topology.

## Recovery preparation

A fresh off-host snapshot backup of the repository container was completed
before publication. The package, candidate contract, signed metadata, trusted
keyring, verification report, pre/post host evidence, SHA-256 sidecars and
comparison reports are retained outside the public repository.

## Remaining limits

- These were existing disposable laboratory hosts, not three clean full-host
  installations.
- Installing the package creates no logical manager identity or replica
  configuration.
- The default-disabled runtime refusal has not yet been exercised against an
  operator configuration.
- No manager service was activated and no network listener was opened.
- Removal, purge, reinstall, upgrade and rollback remain separate qualification
  phases.
- Gate G4 remains open because its identity, lifecycle and clean-host conditions
  exceed this package-only slice.
- No HA claim follows from this result.

## Review state

The qualification harness and candidate source received independent Claude Code
Opus counter-reviews before publication. The installed-host evidence then received
a separate read-only Opus review that recomputed both artifact manifests, verified
the retained OpenPGP signature, replayed all four comparisons and inspected the
complete public diff. Its final verdict was `NO BLOCKER OR IMPORTANT FINDING`.

## Three-pass closeout

- **Governance:** the laboratory mandate covered signed experimental publication
  and disposable-host installation. No customer service, stable promotion,
  irreversible data loss or secret movement occurred. Public evidence uses only
  sanitized aliases; private topology remains outside this repository.
- **Human and organizational:** the package keeps Manager, Governor, Maker and
  lifecycle responsibilities separate. Its disabled default requires an explicit
  later activation decision, and the documentation distinguishes package delivery
  from operational availability.
- **Technical and adversarial:** the exact signed candidate, installed payload,
  maintainer scripts and four comparison results were independently recomputed.
  Existing services and Podman commitments were preserved. Configuration,
  activation, recovery and HA remain open rather than inheriting this `PASS`.

The package-only perimeter is reconciled against the central delivery checklist.
The larger G4 and product perimeters remain `PARTIAL / OPEN` with their next steps
recorded in the manager deployment and HA acceptance documents.
