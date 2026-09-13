# Manager2 packaging preparation review

Date: 2026-09-13  
Scope: source and package-qualification tooling only  
Base: Stage R commit `3a036fb6ab9735c28bf18c390fd685e334b22363`

## Result

The manager2 package preparation is ready to become an exact build candidate.
This result does not claim that a manager2 package exists in signed APT, that any
host was upgraded, or that manager networking, replication, takeover, DNS,
fencing or HA works on installed hosts.

The preparation adds the required `observation_writer_uid: 0` example while
retaining a no-write-authority `grants: []` default. Package installation remains
inactive: it does not enable, start or restart the manager, change network policy,
or migrate protected configuration.

## Inactive-upgrade evidence

The existing laboratory hosts already carry manager1. A dedicated evidence path
therefore compares an inactive manager1 installation with an inactive manager2
installation instead of reusing the first-install proof. It binds both installed
payloads to distinct reviewed candidate reports and contracts and requires a
strict Debian version increase.

The comparison requires:

- one unchanged host boot;
- no manager process or runtime directory;
- a disabled and inactive manager unit with no retained invocation, start or
  restart record;
- unchanged manager account, protected configuration and durable state;
- unchanged lifecycle and observer packages, services, PIDs, invocation data and
  sockets;
- unchanged rootful Podman commitments.

The historical default-refusal attempts remain valid evidence. Their later
`systemctl reset-failed` cleanup means the upgrade captures can prove only that
no invocation record is retained; they cannot prove that a unit was never run.

The protected configuration commitment covers `/etc/podmesh-manager` directory
metadata, `config.json` metadata and file bytes. The state commitment covers a
canonical manifest of the complete manager state tree. Both use private salts.
The collector computes them twice between two full host captures and refuses
instability.

## Configuration transition

Manager1 configurations do not contain `observation_writer_uid` and therefore
fail closed under manager2. The package-only upgrade must first prove that those
files remained unchanged. A separate operator step then adds UID 0 through a
same-directory atomic replacement, preserves root ownership, group, mode and
single-link status, and executes manager2's real offline parser as the service
account. No maintainer script performs this transition.

Default-refusal evidence records the writer UID directly and requires integer UID
0 on all three hosts. Its canonical topology commitment now covers both sorted
replicas and sorted exact scope grants. A change to one host's grant ownership is
therefore visible and fails the cross-host comparison.

## Verification

Codex GPT-6 Astra reran:

- resident process tests: 22 passed;
- authenticated transport tests: 29 passed;
- durable manager tests: 79 passed, with one documented crash helper ignored;
- strict Clippy and Rust formatting for all three crates;
- complete package qualification, signed-candidate fixture, refusal and inactive
  upgrade suites;
- deterministic package assembly tests and `git diff --check`.

Claude Code Opus, high effort, independently inspected the complete preparation
and reran the package and qualification suites. Its first review found one blocker
and four important issues: inaccurate never-invoked wording, excessive upgrade
scope wording, insufficient collector negative coverage, a missing exact atomic
configuration procedure and missing configuration-directory evidence. All were
corrected. Its closure review reported exactly: **no blocker or important finding
remains**.

`shellcheck` was unavailable, so its optional checks were skipped. Bash syntax,
Python compilation, schema validation and the executable negative suites passed.

## Next gate

Build the release binary and byte-reproducible Debian package from the exact clean
commit containing this preparation. Verify and review the candidate contract,
back up the signed APT repository, publish only to the experimental suite, then
run the inactive three-host upgrade procedure. Configuration transition and the
new default-refusal campaign are separate gates after package-only upgrade.
Network activation remains closed until those results are reviewed.
