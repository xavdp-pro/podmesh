#!/usr/bin/env python3
"""The readmission evidence collector fails closed, and otherwise writes what readmission reads.

With stubbed commands (a `python3 -c` that prints a given JSON document, or fails): a plan that misses an
input, names one too many, or whose command fails, times out, prints no JSON or prints another input than
it claims (a store without its digest, another key's ledger, another node's answers) writes nothing and
exits 3; a replica whose ledger is not marked unadmitted (read on its control socket) writes nothing; a
complete plan writes one read-only file per input, each an evidence envelope stamped after the mark, and
prints the digests of exactly those bytes and the readmission request. The same tool against real
residents and nodes, followed by the resident's readmission, is the end-to-end test's
(tests/e2e/decisions-e2e.py). No daemon, no host. Run: python3 -B tools/test_collect_readmission_evidence.py"""
import hashlib, json, os, pathlib, socket, stat, subprocess, sys, tempfile, threading, time

TOOL = pathlib.Path(__file__).resolve().parent / "collect-readmission-evidence.py"
NODES = ["0a0a0a0a-0000-4000-8000-000000000000", "1b1b1b1b-1111-4111-8111-111111111111"]
checks = []


def printing(document):
    return ["python3", "-c", f"import sys; sys.stdout.write({json.dumps(json.dumps(document))})"]


def config(td):
    c = {"network": {"replica_id": "r0", "manager": {"replicas": [{"replica_id": f"r{i}", "host_id": f"h{i}"} for i in range(3)]}},
         "votes": {"key_id": "replica-a", "authority_quorum": {"threshold": 2, "keys": [{"key_id": k} for k in ("replica-a", "replica-b", "replica-c")]},
                   "nodes": NODES}}
    path = td / "config.json"
    path.write_text(json.dumps(c))
    return path


STORE = {"history_count": 0, "ordered_facts": [], "logical_history_sha256": "0" * 64}


def ledger(key):
    return {"form": "podmesh-manager-vote-ledger/1", "key_id": key}


def screen(node):
    return {"activation_status": [{"this_host_uuid": node, "universe_uuid": "r", "highest_epoch_seen": None}]}


def plan(td, evidence, **overrides):
    p = {"replica_config": str(config(td)), "evidence_dir": str(evidence),
         "stores": {"r1": printing(STORE), "r2": printing(STORE)},
         "ledgers": {"replica-b": printing(ledger("replica-b")), "replica-c": printing(ledger("replica-c"))},
         "screens": {n: printing(screen(n)) for n in NODES}}
    for k, v in overrides.items():
        if v is None:
            p.pop(k, None)
        else:
            p[k] = v
    path = td / "plan.json"
    path.write_text(json.dumps(p))
    return path


def collect(plan_path, expect):
    p = subprocess.run(["python3", "-B", str(TOOL), "collect", "--plan", str(plan_path)], capture_output=True, text=True)
    assert p.returncode == expect, (p.returncode, p.stdout, p.stderr)
    return p


class Resident:
    """A control socket that answers `status` with a ledger state."""

    def __init__(self, path, state, since):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.bind(str(path))
        self.sock.listen(4)
        self.answer = {"votes": {"ledger_state": state, "unadmitted_since": since}}
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            with conn:
                while conn.recv(4096):
                    pass
                conn.sendall(json.dumps(self.answer).encode())


def main():
    td = pathlib.Path(tempfile.mkdtemp(prefix="collect-evidence-", dir=os.environ.get("TMPDIR")))
    evidence = td / "evidence"
    evidence.mkdir()
    empty = lambda: not any(evidence.iterdir())
    base = json.loads(plan(td, evidence).read_text())

    for why, overrides, needle in [
        ("a store missing", {"stores": {"r1": base["stores"]["r1"]}}, "stores must be exactly"),
        ("one ledger too many", {"ledgers": {**base["ledgers"], "replica-a": printing(ledger("replica-a"))}}, "ledgers must be exactly"),
        ("a screen missing", {"screens": {NODES[0]: base["screens"][NODES[0]]}}, "screens must be exactly"),
        ("a command that fails", {"stores": {**base["stores"], "r2": ["python3", "-c", "import sys; sys.exit(4)"]}}, "exited 4"),
        ("a command that prints no JSON", {"stores": {**base["stores"], "r2": ["python3", "-c", "print('x')"]}}, "no JSON"),
        ("a store without its digest", {"stores": {**base["stores"], "r2": printing({"history_count": 0, "ordered_facts": []})}}, "not an --inspect-store"),
        ("another key's ledger", {"ledgers": {**base["ledgers"], "replica-c": printing(ledger("replica-b"))}}, "not the ledger of key"),
        ("another node's answers", {"screens": {**base["screens"], NODES[1]: printing(screen(NODES[0]))}}, "not the activation_status answers"),
        ("an evidence directory that does not exist", {"evidence_dir": str(td / "absent")}, "must be an existing"),
    ]:
        p = collect(plan(td, evidence, **overrides), 3)
        assert needle in p.stderr and "nothing was written" in p.stderr and empty(), (why, p.stderr)
    checks.append("a plan that misses an input or names one too many, a command that fails or prints no JSON, a store, a ledger or a screen that is not what it claims, or a missing evidence directory: nothing written, exit 3")

    admitted = Resident(td / "admitted.sock", "admitted", None)
    p = collect(plan(td, evidence, resident_socket=str(td / "admitted.sock")), 3)
    assert "not unadmitted" in p.stderr and empty(), p.stderr
    admitted.sock.close()
    checks.append("a replica whose ledger is not marked unadmitted: nothing written, exit 3")

    marked = int(time.time())
    Resident(td / "marked.sock", "unadmitted", marked)
    p = collect(plan(td, evidence, resident_socket=str(td / "marked.sock")), 0)
    out = json.loads(p.stdout)
    names = sorted(os.listdir(evidence))
    assert names == sorted(["store.r1.json", "store.r2.json", "ledger.replica-b.json", "ledger.replica-c.json"] + [f"screen.{n}.json" for n in NODES]), names
    for name in names:
        data = (evidence / name).read_bytes()
        assert out["evidence_sha256"][name] == hashlib.sha256(data).hexdigest(), name
        assert stat.S_IMODE((evidence / name).stat().st_mode) == 0o444, name
        envelope = json.loads(data)
        input_, source = name[:-5].split(".", 1)
        assert envelope["form"] == "podmesh-manager-readmission-evidence/1" and envelope["input"] == input_ and envelope["source"] == source
        assert envelope["collected_at"] >= marked
    assert out["request"]["operation"] == "vote_ledger_readmit" and out["request"]["evidence_sha256"] == out["evidence_sha256"]
    checks.append("a complete plan writes one read-only envelope per input, collected after the mark, and prints the digests of exactly those bytes and the readmission request")

    for c in checks:
        print("PASS", c)
    print(f"{len(checks)} checks passed")


if __name__ == "__main__":
    main()
