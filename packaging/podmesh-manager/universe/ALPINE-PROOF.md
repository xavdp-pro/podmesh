# Alpine first: the attempt, and the exact dependency that failed

The operator's image hierarchy makes Alpine the default inside a universe; a Debian image is
allowed only on a recorded failed Alpine proof naming the dependency. This is that record, for
the frozen manager candidate `0.1.0~manager2+gff77b1f946e8` (binary `cbd5020a…3660128`).

Two images were built on lab-a on 2026-09-14 from `alpine-attempt/`:

1. **`alpine:3.22` with the binary as it is** — the binary cannot be loaded at all:
   `exec container process (missing dynamic library?) ... No such file or directory` — the
   candidate is a dynamically linked glibc PIE (interpreter `/lib64/ld-linux-x86-64.so.2`,
   `binary.glibc-max` 2.34), and musl provides no such loader.
2. **`alpine:3.22` with `gcompat` and `libgcc`** — the loader is found and relocation fails:
   `Error relocating /usr/lib/podmesh-manager/podmesh-managerd: fcntl64: symbol not found`.
   `fcntl64` is a glibc ≥ 2.28 symbol the compatibility shim does not provide. The universe
   contract held even so: the entrypoint refused to run (exit 2, boot fact not observed).

**The exact dependency:** a dynamically linked glibc ≥ 2.34 executable. It cannot be met on
Alpine with this binary. An Alpine manager universe therefore needs a musl build of the
resident (`x86_64-unknown-linux-musl`, statically linked), which changes the candidate's binary
digest and is a new candidate, not this one. Until then the Debian 13 image is the exception
the hierarchy allows, with this file as the reason beside it. Both images, when a musl
candidate exists, must implement the same universe contract and pass the same smoke, stop and
store proof.

## Addendum, the same night: Alpine has a viable path

A musl build of the resident from the same frozen source commit (`MUSL-BUILD.md`) runs on
`alpine:3.22` with nothing but `python3` for the entrypoint: smoke exit 0 with the boot fact
observed and the typed shutdown acknowledged; `--fault boot` exit 2; `--fault shutdown` exit 3;
and the full three-host proof PASS with that binary attested as both resident and inspector.
The limitation above is therefore the frozen glibc binary's, not the resident's: an Alpine root
image is the default from here, and the Debian image is the compatibility branch for the frozen
candidate.
