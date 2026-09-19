#!/usr/bin/env python3
"""The replica set's vote tools (V3-5 packaging): `vote-key.py` derives the public halves of the test
vectors, writes a new key private and never over an existing one, and prints only the public half;
`add-votes.py` gives every replica of a generated set the scopes it votes and proposes in and a `votes`
section whose policy digest is the node's own pinned vector for the test keys, refuses a set that already
votes and a key missing; with PODMESH_MANAGERD naming a resident binary, each configuration passes that
resident's own offline validation (`--validate-config`, networking disabled), its votes section included.
Test-only keys; no host. Run: python3 -B test_votes_tools.py"""
import json, os, pathlib, shutil, stat, subprocess, sys, tempfile

HERE = pathlib.Path(__file__).resolve().parent
PINNED = "965bd61aaad93361c01a54635b3f3ee6c40953e0680589f064cf56ca06c19d86"
PUBLIC = {"replica-a": "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
          "replica-b": "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394",
          "replica-c": "ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1"}
R = "91eeb6bf-5489-405b-b77a-53105b0aff7a"
HOSTS = ["0a0a0a0a-0000-4000-8000-000000000000", "1b1b1b1b-1111-4111-8111-111111111111", "2c2c2c2c-2222-4222-8222-222222222222"]
checks = []


def run(*argv, expect=0):
    p = subprocess.run(["python3", "-B", *map(str, argv)], capture_output=True, text=True)
    assert p.returncode == expect, (argv, p.returncode, p.stdout, p.stderr)
    return p


def main():
    td = pathlib.Path(tempfile.mkdtemp(prefix="votes-tools-", dir=os.environ.get("TMPDIR")))
    try:
        run(HERE / "vote-key.py", "--self-test")
        votes = td / "votes"
        votes.mkdir(mode=0o700)
        out = json.loads(run(HERE / "vote-key.py", "--vote-dir", votes, "--key-id", "replica-lab-a").stdout)
        key = votes / "replica-lab-a.key"
        seed = key.read_text().strip()
        assert stat.S_IMODE(key.stat().st_mode) == 0o600 and len(seed) == 64 and seed not in json.dumps(out), out
        spec = __import__("importlib.util").util.spec_from_file_location("vote_key", HERE / "vote-key.py")
        module = __import__("importlib.util").util.module_from_spec(spec)
        spec.loader.exec_module(module)
        assert module.public_key(bytes.fromhex(seed)) == out["public_key"]
        run(HERE / "vote-key.py", "--vote-dir", votes, "--key-id", "replica-lab-a", expect=1)
        assert key.read_text().strip() == seed
        loose = td / "loose"
        loose.mkdir(mode=0o755)
        run(HERE / "vote-key.py", "--vote-dir", loose, "--key-id", "k", expect=1)
        checks.append("vote-key derives the test vectors' public halves, writes a new seed 0600 and prints only its public half, never overwrites a key, and refuses a directory open to others")

        rs = td / "rs"
        run(HERE / "generate-replica-set.py", "--out", rs, *[a for i, alias in enumerate(("lab-a", "lab-b", "lab-c")) for a in ("--replica", f"{alias}:{HOSTS[i]}:10.86.{i + 1}.10")])
        keys = [f"--key={alias}:{k}:{PUBLIC[k]}" for alias, k in (("lab-a", "replica-a"), ("lab-b", "replica-b"), ("lab-c", "replica-c"))]
        run(HERE / "add-votes.py", "--dir", rs, *keys[:2], f"--resource={R}:300:30:0", expect=1)
        out = json.loads(run(HERE / "add-votes.py", "--dir", rs, *keys, f"--resource={R}:300:30:0", f"--baseline={R}:157:{HOSTS[0]}:1789681724:1789685324").stdout)
        assert out["policy_digest"] == PINNED, out
        manifest = json.loads((rs / "replica-set.json").read_text())
        for r in manifest["replicas"]:
            config = json.loads((rs / r["alias"] / "config.json").read_text())
            scopes = {g["scope"] for g in config["network"]["manager"]["grants"]}
            assert {f"votes/{o['replica_id']}" for o in manifest["replicas"]} <= scopes and {f"proposals/{o['replica_id']}" for o in manifest["replicas"]} <= scopes
            v = config["votes"]
            assert v["nodes"] == HOSTS and v["decisions"]["resources"][0]["baseline"] == {"epoch": 157, "holder": HOSTS[0], "eligible_after": 1789681724, "expires_at": 1789685324} and v["evidence_dir"] == "/run/podmesh-host/evidence"
        run(HERE / "add-votes.py", "--dir", rs, *keys, f"--resource={R}:300:30:0", expect=1)
        checks.append("add-votes gives every replica its vote and proposal scopes and a votes section under the node's pinned policy digest for the test keys, with the resource's rules and baseline; refuses a key missing and a set that already votes")

        managerd = os.environ.get("PODMESH_MANAGERD")
        if managerd:
            for r in manifest["replicas"]:
                config = json.loads((rs / r["alias"] / "config.json").read_text())
                state, runtime = td / f"state-{r['alias']}", td / f"run-{r['alias']}"
                state.mkdir(mode=0o750)
                runtime.mkdir(mode=0o700)
                config["network"]["database_path"] = str(state / "manager.sqlite")
                config["control_socket"] = str(runtime / "control.sock")
                config["observation_writer_uid"] = os.geteuid()
                path = td / f"{r['alias']}.json"
                path.write_text(json.dumps(config))
                path.chmod(0o600)
                p = subprocess.run([managerd, "--config", str(path), "--state-dir", str(state), "--runtime-dir", str(runtime), "--validate-config"],
                                   capture_output=True, text=True, env={**os.environ, "PODMESH_MANAGER_NETWORK_MODE": "disabled"})
                assert p.returncode == 0 and '"configuration_valid":true' in p.stdout.replace(" ", ""), (p.stdout, p.stderr)
            checks.append("each configuration passes the resident's own offline validation, its votes and decisions included")
        else:
            print("SKIPPED the resident's validation: PODMESH_MANAGERD is not set")
    finally:
        shutil.rmtree(td, ignore_errors=True)
    for c in checks:
        print("PASS", c)
    print(f"{len(checks)} checks passed")


if __name__ == "__main__":
    main()
