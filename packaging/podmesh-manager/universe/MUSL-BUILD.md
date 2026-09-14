# The musl build of the resident, for the Alpine root image

Built on 2026-09-14 on lab-a, from the three experiment crates at the frozen source commit
`ff77b1f946e82af421de2ccdbe06d8cd45b70c33` (verified identical to the web tree's HEAD for
`experiments/manager-ha`, `manager-network`, `manager-resident`), in the container
`docker.io/library/rust:1-alpine` (cargo 1.98.1, rustc 1.98.1) with `apk add musl-dev`, by
`cargo build --release --locked` in `experiments/manager-resident`, with network access for the
crates the lock file pins. Output `target/release/podmesh-manager-resident-lab`, installed as
`/usr/lib/podmesh-manager/podmesh-managerd` in the image:

    static-pie linked, x86-64, 4 549 384 bytes
    sha256 4111e4873f4e6810b2cc8c01af15329902a911e7d0935220d18406c93087ce31

It is **not** the frozen candidate `cbd5020a…3660128`: same source, different toolchain and
libc, different bytes. It runs on the Debian workstation and on Alpine alike, which is what
lets the check use one binary as both the universe's resident and the workstation's inspector.
Freezing it as a candidate — reproducing the build, pinning the toolchain, reviewing it — is
Codex's, and it has not been done. Byte-for-byte reproducibility of this build is not claimed.
