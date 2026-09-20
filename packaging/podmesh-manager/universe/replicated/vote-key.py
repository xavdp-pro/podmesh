#!/usr/bin/env python3
"""Make a manager replica's signing key on its own host, and print only its public half (V3-4, V3-5).

Run as root on the host that runs the replica, into the replica's vote directory -- the host state
PodMesh mounts into the universe (`create` with `manager_host_state: <name>`; the directory is
`<node state>/manager-host/<name>/votes`) or, for a resident run as a host service, the service's
vote directory:

    vote-key.py --vote-dir <dir> --key-id <key_id>

It writes `<key_id>.key` (the 32-byte seed as 64 lowercase hex characters, 0600, refused if it exists)
from the kernel's randomness and prints `{"key_id", "public_key"}`: the public half is what the nodes'
policies and the replicas' configurations name (`add-votes.py`). The seed never leaves the host and is
never printed. A new key signs nothing until its ledger is created (`vote_ledger_init`) and readmitted
by the operator. Replacing a host's key is a change of the authority set (serial + 1).

The public half is derived here in pure Python (RFC 8032, section 5.1.5), so the host needs nothing
but python3; `--self-test` checks the derivation against the known vectors and exits."""
import argparse, hashlib, json, os, sys

P = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
D = -121665 * pow(121666, P - 2, P) % P
I = pow(2, (P - 1) // 4, P)


def _x(y, sign):
    x2 = (y * y - 1) * pow(D * y * y + 1, P - 2, P)
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P:
        x = x * I % P
    if x & 1 != sign:
        x = P - x
    return x


_BY = 4 * pow(5, P - 2, P) % P
BASE = (_x(_BY, 0), _BY, 1, _x(_BY, 0) * _BY % P)


def _add(a, b):
    x1, y1, z1, t1 = a
    x2, y2, z2, t2 = b
    A = (y1 - x1) * (y2 - x2) % P
    B = (y1 + x1) * (y2 + x2) % P
    C = t1 * 2 * D * t2 % P
    Dd = z1 * 2 * z2 % P
    E, F, G, H = B - A, Dd - C, Dd + C, B + A
    return (E * F % P, G * H % P, F * G % P, E * H % P)


def _mul(s, point):
    q = (0, 1, 1, 0)
    while s:
        if s & 1:
            q = _add(q, point)
        point = _add(point, point)
        s >>= 1
    return q


def public_key(seed: bytes) -> str:
    """The Ed25519 public key of a 32-byte seed, lowercase hex."""
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    x, y, z, _ = _mul(a, BASE)
    zi = pow(z, P - 2, P)
    x, y = x * zi % P, y * zi % P
    return (y | ((x & 1) << 255)).to_bytes(32, "little").hex()


# The test keys of the node's and the replicas' test suites (seeds of one repeated byte).
VECTORS = {1: "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
           2: "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394",
           3: "ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1"}


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--vote-dir")
    p.add_argument("--key-id")
    p.add_argument("--self-test", action="store_true")
    a = p.parse_args()
    for seed, expected in VECTORS.items():
        if public_key(bytes([seed]) * 32) != expected:
            sys.exit("vote-key: the public-key derivation does not match its test vectors; nothing written")
    if a.self_test:
        print("vote-key: derivation matches the test vectors")
        return
    if not a.vote_dir or not a.key_id:
        sys.exit("vote-key: --vote-dir and --key-id are required")
    key_id = a.key_id
    if not (1 <= len(key_id) <= 32 and key_id[0].isalnum() and all(c.isalnum() or c in "_.:-" for c in key_id)):
        sys.exit("vote-key: the key_id must be 1-32 characters of [A-Za-z0-9_.:-], starting alphanumeric")
    st = os.lstat(a.vote_dir)
    if not os.path.isdir(a.vote_dir) or os.path.islink(a.vote_dir) or st.st_mode & 0o077 or st.st_uid != os.geteuid():
        sys.exit("vote-key: the vote directory must be a private directory (0700) of this user")
    seed = os.urandom(32)
    path = os.path.join(a.vote_dir, f"{key_id}.key")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(seed.hex() + "\n")
        f.flush()
        os.fsync(f.fileno())
    dfd = os.open(a.vote_dir, os.O_RDONLY)
    os.fsync(dfd)
    os.close(dfd)
    print(json.dumps({"key_id": key_id, "public_key": public_key(seed)}))


if __name__ == "__main__":
    main()
