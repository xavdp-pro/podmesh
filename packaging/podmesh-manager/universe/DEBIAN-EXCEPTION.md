# Debian exception for the manager universe root container (policy of 2026-09-14)

The image policy makes Alpine the default base of a universe root container and allows a Debian
image only for a concrete component that cannot run on Alpine, recorded as a local exception in
this exact form.

| | |
| --- | --- |
| **Component** | `podmesh-managerd`, the frozen manager resident candidate `0.1.0~manager2+gff77b1f946e8`, binary `cbd5020a37d2…3660128` |
| **Alpine limitation** | The binary is a dynamically linked glibc PIE (interpreter `/lib64/ld-linux-x86-64.so.2`, `binary.glibc-max` 2.34). On `alpine:3.22` it cannot be loaded (`missing dynamic library ... No such file or directory`); with `gcompat` and `libgcc` it fails relocation on `fcntl64: symbol not found`, a glibc ≥ 2.28 symbol the shim does not provide. Measured on lab-a on 2026-09-14; both Containerfiles are under `alpine-attempt/` and the outputs are in `ALPINE-PROOF.md`. |
| **Debian dependency** | glibc ≥ 2.34 with its dynamic loader: `debian:13-slim` (glibc 2.41), plus `python3-minimal` for the entrypoint's typed control requests. |
| **Smoke test** | `podman run -d --network=none <image>` then `podman stop --time 15`: the log shows `boot fact observed`, `shutdown acknowledged: {"shutdown_requested":true}`, `resident exited rc=0`, and the container exits 0 within the timeout; the same with `--fault boot` exits 2 and with `--fault shutdown` exits 3. The full proof is `tests/check-manager-universe-ha.py` in the main tree: attested binary, typed stop, durable store through capture, restore, promotion and restart on three hosts. |

## Why the exception is the root container, and how it goes away

A PodMesh universe is today **one** container: `create` takes one image and one command, with no
network, no mounts and no exec, and Podman inside a universe is a research subject
(`docs/EXPERIMENTAL-SCOPE.md`). The policy's preferred shape — an Alpine root with a small
Debian internal container for the component — is not expressible in that contract yet, so the
exception has to be the root image of this one universe. It changes nothing for any other
universe, whose root stays Alpine by default.

**Status after the same day's musl build:** the Alpine root image exists (`Containerfile.alpine`,
`MUSL-BUILD.md`) and passed the same proof, so the exception now covers only the **frozen glibc
candidate** `cbd5020a…` — the Debian image is the compatibility branch that runs that exact
binary until Codex freezes the musl build as a candidate. The two things that remove it entirely: a **musl build
of the resident** (`x86_64-unknown-linux-musl`, statically linked), which is a new candidate
with a new binary digest and needs Codex's build and review; or **internal containers** in the
universe contract, after which the resident moves into a Debian internal container under an
Alpine root. When either exists, the Alpine image must pass the same smoke, typed-stop and
durable-store proof before it replaces this one.
