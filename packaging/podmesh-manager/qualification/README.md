# Three-host manager package qualification harness

This directory prepares evidence for an exact `podmesh-manager` package candidate.
The package-only collector does not connect to a host, install a package, create
configuration, enable or start a unit, or activate manager networking. An operator
runs it separately on each declared disposable host. The bounded
`refusal/run-default-refusal.sh` harness is a later, explicit phase: it validates
an already protected configuration, attempts one start with the packaged network
gate still disabled, records the expected refusal and does not reset or mutate
any other service.

The harness covers the package-only, disabled-service slice of G4. It does not by
itself qualify the full G4 gate, preserved manager identity, or G2,
G3, G5, DNS, replication, routing, WireGuard, workload activation, restart,
upgrade, removal, purge, or rollback. Those phases require their own approved
procedure and evidence under `docs/MANAGER-HA-ACCEPTANCE.md`.

## Configured default-refusal stage

After package-only qualification and protected configuration, use one common
private root-owned `0600` salt of at least 32 random bytes on the three hosts.
Keep it outside every evidence bundle. With the manager inactive, disabled and
without any systemd drop-in, run as root:

```sh
refusal/run-default-refusal.sh \
  --host-alias lab-a \
  --salt-file /private/path/refusal-salt \
  --output /private/evidence/lab-a.json
```

Repeat independently for the other two aliases. The harness:

- commits the complete configuration, topology, local identities, peer identities,
  endpoints and pair keys with the private salt, without emitting their values;
- proves the validation command's effective service-account UID and the private
  runtime directory's create, empty, validate, empty and remove lifecycle;
- binds the failed start to its exact systemd InvocationID and unit fragment;
- requires a disabled unit with no drop-in, `ExecMainStatus=1`, no restart and
  the expected default-network refusal; the `systemctl start` client status is
  recorded but is not the process outcome for this `Type=simple` unit;
- compares lifecycle and observer process identity, stable rootful Podman
  container identity/state/start/PID/restart fields, routes and firewall state;
- records firewall or route collection failures as `unknown` and never turns them
  into an empty-input success hash;
- refuses a detected IPv4 route or nftables ruleset change; `unknown` remains
  explicit and cannot support an unchanged-infrastructure claim;
- refuses any retained manager process, state entry, control socket or configured
  TCP/UDP listener.

Recompute the cross-host result with the checked-in comparator and the campaign
summary that binds the expected package, source and aliases:

```sh
refusal/compare-three-hosts.sh \
  --summary /private/evidence/campaign-summary.json \
  /private/evidence/lab-a.json \
  /private/evidence/lab-b.json \
  /private/evidence/lab-c.json
```

The cross-host comparison requires one logical identity and topology, three
distinct local replica and host commitments, two reciprocal peers per host,
matching endpoint-to-bind commitments and three symmetric distinct pair-key
commitments. Passing this stage does not activate networking or qualify exchange,
replication, takeover, DNS or HA.

## Evidence stages

Use stable aliases such as `lab-a`, `lab-b`, and `lab-c`; never put an address,
credential, token, peer key, configuration body, private hostname, process
command line, Podman raw output, network definition, mount, or `podman info`
in an evidence bundle.

1. On each host, create a private root-owned `0600` salt file of at least 32 random bytes outside the evidence bundle, for example with `head -c 32 /dev/urandom > <private-salt>` followed by `chmod 0600 <private-salt>`. Then run `collect-host.sh --host-alias <alias> --stage pre-install --salt-file <private-salt> --output <file>` as root. The same salt must be retained privately for that host's post-install collection. The collector records only salted commitments of allow-listed Podman projections; the container projection includes identity, state, start time, PID and restart count so a restart changes the commitment. It never writes raw Podman inventory or `podman info` to evidence.
2. On the verification workstation, run `verify-candidate.sh` with the local `.deb`, the reviewed candidate contract, the signed local APT metadata and its trusted keyring. It extracts the verified `Release` payload, pins its primary signing fingerprint from the contract, verifies the exact `Packages` index SHA-256, byte size and path covered by that Release, and checks the matching package stanza's identity, version, architecture and SHA-256. It also checks the `.deb` SHA-256, embedded manager binary SHA-256, exact file list, every regular payload file hash and the exact maintainer-script set and hashes. The current verifier does not check the package stanza's `Filename` or `Size` fields. This command does not fetch metadata or install anything.
3. The operator performs the separately authorized installation phase outside this harness. No script here contains an install command.
4. On each host, run the collector with `--stage post-install --salt-file <same-private-salt> --candidate-verification <report.json>`. It binds the installed version, binary, regular payload and maintainer scripts to the offline verification report and requires `dpkg --verify` to be clean. Then run `compare-evidence.py --phase install --pre <pre.json> --post <post.json> --candidate-verification <report.json> --contract <contract.json>` for that alias. The comparison requires the existing lifecycle and observer packages, active units and sockets to have been present before installation and to remain unchanged, together with their PIDs and rootful Podman commitments. It requires the manager package to be present while its unit remains disabled and inactive, with no retained systemd invocation, start or restart evidence; it also requires no packaged binary process and an empty new manager state directory with the declared identity and permissions.
5. Run `compare-evidence.py --phase three-host --pre lab-a-pre.json lab-b-pre.json lab-c-pre.json --post lab-a-post.json lab-b-post.json lab-c-post.json --candidate-verification <report.json> --contract <contract.json>` to reject duplicate aliases, pair each pre/post capture, require the same verified candidate on every host and ensure the intended pre-install manager absence on every host. This is a package-only installation comparison, not proof that the three machines are distinct or reachable.

Each collector canonicalizes JSON with `jq -S` and writes a SHA-256 sidecar. Preserve the JSON, sidecar, candidate-verification JSON, reviewed contract, signed metadata and keyring fingerprint as the evidence bundle. Do not preserve the private salt with it. A commitment detects a change only when the same private salt is used for both captures; it cannot establish that two hosts are distinct.

## Candidate contract

`candidate-contract.example.json` is a shape-only template. Make a reviewed,
host-independent contract from it for the exact package candidate. The contract
contains no endpoint or secret. It pins the primary OpenPGP signing fingerprint.
`expected_files`, `expected_regular_payload_files` and
`expected_maintainer_scripts` are deliberately exact so a package cannot silently
add or change a lifecycle/observer path, payload file or Debian lifecycle script.

## Local checks

Run `tests/run-tests.sh`. It validates the schema and exercises passing and
negative comparison cases, the collector through controlled command stubs, and
the actual candidate verifier with a throwaway GPG key, signed metadata and a
fixture package. If `shellcheck` is available it is run against the shell scripts.
The tests do not run real Podman or systemd services and do not install a package.

Required observations are never recorded as `unknown` or inferred as zero,
stopped or unchanged. Missing evidence or a collector command that fails aborts
evidence acquisition; it cannot produce a passing host state.
