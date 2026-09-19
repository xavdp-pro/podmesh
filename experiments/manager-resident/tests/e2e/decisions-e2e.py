#!/usr/bin/env python3
"""V3-5, the manager decides, end to end on one machine, with no laboratory host and no workstation tool
in the path of a decision.

Three compiled residents on loopback, each with a test-only key and its signing ledger in its own
directory, its machine-id and the operator's evidence directory mounted read-only (a helper in its own
user and mount namespace); three real PodMesh nodes (podmeshd, run unprivileged on their own state
directories and sockets: activation needs no Podman), each declaring the resource under the replicas'
2-of-3 quorum; the host agent (packaging/podmesh-decision-follow of the node tree) run once per host as
its timer would, reading its own replica's control socket and handing certificates to its own node.

What it shows, in order:
 1. new ledgers sign nothing, and a proposal made while every ledger is unadmitted is decided by nobody;
 2. the operator's readmission of two ledgers through the evidence collector (evidence from the other
    replicas' stores, the other keys' ledgers and the three nodes' screens, digests stated), after the
    wait; the third ledger stays unadmitted and never votes;
 3. epoch 1 (first) decided by the two admitted replicas; the agents deliver it: the named host acquires,
    the two others are superseded, and every node's screen moves to 1;
 4. a same-holder re-issue (epoch 2) decided and delivered the same way;
 5. a rotation to another holder (epoch 3, lease_barrier) proposed with a barrier the view does not allow:
    refused by the voters, never decided, nothing delivered; proposed with the barrier the view requires:
    decided, the new holder's node refuses it until the barrier and accepts it at the next tick after,
    the previous holder is superseded;
 6. a replayed and a late certificate (epochs 1 and 2) delivered by hand: every node refuses them, and no
    screen moves;
 7. one replica stopped: the one vote left decides nothing, the agents deliver nothing, and a hand-made
    1-of-3 certificate from that vote is refused by the node (below_threshold), with its screen unmoved;
 8. the agents run again with nothing new: nothing is delivered twice.

Run (paths from the environment, nothing durable written outside TMPDIR):
  PODMESH_MANAGERD=<resident binary> PODMESHD=<podmeshd> PODMESH_DECISION_FOLLOW=<agent script> \\
  TMPDIR=<scratch> python3 -B tests/e2e/decisions-e2e.py [--keep]
Exit 0 with every check printed PASS; the summary is printed as JSON on the last line."""
import hashlib, json, os, pathlib, shutil, socket, subprocess, sys, tempfile, time

MANAGERD = os.environ["PODMESH_MANAGERD"]
PODMESHD = os.environ["PODMESHD"]
AGENT = os.environ["PODMESH_DECISION_FOLLOW"]
COLLECTOR = pathlib.Path(__file__).resolve().parent.parent.parent / "tools" / "collect-readmission-evidence.py"
R = "91eeb6bf-5489-405b-b77a-53105b0aff7a"
KEYS = [("replica-a", 1), ("replica-b", 2), ("replica-c", 3)]
# The public halves of the test keys whose 32-byte seeds are the byte 1, 2 and 3 repeated (the V3-2 and
# V3-4 test vectors: their policy digest at serial 0 is 965bd61a...).
PUBLIC = {"replica-a": "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
          "replica-b": "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394",
          "replica-c": "ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1"}
LIFE, LEASE, MARGIN = 20, 5, 5
QUORUM = {"threshold": 2, "keys": [{"key_id": k, "public_key": PUBLIC[k]} for k, _ in KEYS]}
POLICY_DIGEST = "965bd61aaad93361c01a54635b3f3ee6c40953e0680589f064cf56ca06c19d86"
BOOT = open("/proc/sys/kernel/random/boot_id").read().strip()
checks, summary = [], {}


def check(ok, what, detail=None):
    if not ok:
        raise AssertionError(f"{what}\n  detail: {json.dumps(detail, default=str)[:3000]}")
    checks.append(what)
    print("PASS", what, flush=True)


def now():
    return int(time.time())


def until(what, timeout, predicate):
    deadline = time.time() + timeout
    while True:
        value = predicate()
        if value:
            return value
        if time.time() > deadline:
            raise AssertionError(f"timed out: {what}")
        time.sleep(0.2)


def unix(path, request, line, timeout=10):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(str(path))
    s.sendall(json.dumps(request).encode() + (b"\n" if line else b""))
    if not line:
        s.shutdown(socket.SHUT_WR)
    data = b""
    while True:
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
    s.close()
    return json.loads(data)


class Lab:
    def __init__(self, root):
        self.root = root
        self.procs = []
        self.nodes, self.residents = [], []

    # ----- read-only mounts -----
    def mounts(self, paths):
        script = "".join(f"mount --bind '{p}' '{p}' && mount -o remount,bind,ro '{p}' && " for p in paths) + "echo ready && exec sleep 3600"
        helper = subprocess.Popen(["unshare", "-rm", "sh", "-c", script], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.procs.append(helper)
        if helper.stdout.readline().strip() != b"ready":
            raise SystemExit("SKIPPED: read-only mounts need unprivileged user namespaces (unshare -rm)")
        return lambda p: pathlib.Path(f"/proc/{helper.pid}/root{p}")

    # ----- nodes -----
    def start_node(self, k):
        d = self.root / f"node{k}"
        d.mkdir(mode=0o700)
        sock = d / "api.sock"
        log = open(d / "podmeshd.log", "w")
        self.procs.append(subprocess.Popen([PODMESHD], env={**os.environ, "PODMESH_STATE_DIR": str(d / "state"), "PODMESH_SOCKET": str(sock)},
                                           stdout=log, stderr=log))
        until(f"node {k} answers", 20, lambda: sock.exists())
        host = self.node_call(sock, {"operation": "identity"})["data"]["host_uuid"]
        self.nodes.append({"socket": sock, "host": host})

    @staticmethod
    def node_call(sock, request):
        return unix(sock, request, True, timeout=30)

    def node(self, k, operation, **fields):
        return self.node_call(self.nodes[k]["socket"], {"operation": operation, **fields})

    def status(self, k):
        answer = self.node(k, "activation_status", universe_uuid=R)
        assert answer["ok"], answer
        return answer["data"]

    # ----- residents -----
    def configure_residents(self):
        view = self.mounts([str(self.root / f"machine-id-{i}") for i in range(3)] + [str(self.root / f"evidence-{i}") for i in range(3)])
        ports = []
        for _ in range(3):
            s = socket.socket()
            s.bind(("127.0.0.1", 0))
            ports.append(s.getsockname()[1])
            s.close()
        uid = os.geteuid()
        manager = {"logical_manager_id": "e2e-decisions",
                   "replicas": [{"replica_id": f"r{i}", "host_id": f"h{i}"} for i in range(3)],
                   "grants": [{"scope": f"{s}/r{i}", "owner_replica_id": f"r{i}"} for i in range(3) for s in ("votes", "proposals")]}
        pair = lambda i, j: f"{0x40 + min(i, j) * 3 + max(i, j):02x}" * 32
        for i in range(3):
            d = self.root / f"r{i}"
            d.mkdir(mode=0o700)
            votes = self.root / f"votes-{i}"
            votes.mkdir(mode=0o700)
            key = votes / f"{KEYS[i][0]}.key"
            key.write_text(f"{KEYS[i][1]:02x}" * 32 + "\n")
            key.chmod(0o600)
            config = {
                "network": {"replica_id": f"r{i}", "database_path": str(d / "manager.sqlite"), "manager": manager,
                            "bind": f"127.0.0.1:{ports[i]}",
                            "peers": [{"replica_id": f"r{j}", "endpoint": f"127.0.0.1:{ports[j]}", "shared_key_hex": pair(i, j)} for j in range(3) if j != i]},
                "control_socket": str(d / "control.sock"), "observation_writer_uid": uid, "interval_ms": 200, "max_backoff_ms": 1000,
                "incoming_workers": 2,
                "votes": {"key_id": KEYS[i][0], "authority_id": "replicas", "authority_quorum": QUORUM, "authority_serial": 0,
                          "replica_keys": {f"r{j}": KEYS[j][0] for j in range(3)}, "nodes": [n["host"] for n in self.nodes],
                          "evidence_dir": str(view(str(self.root / f"evidence-{i}"))), "operator_uid": uid,
                          "max_certificate_life_seconds": LIFE,
                          "decisions": {"voter_interval_ms": 300, "resources": [
                              {"resource": R, "lease_seconds": LEASE, "takeover_margin_seconds": MARGIN, "renewal_not_after": 0}]}},
            }
            path = d / "config.json"
            path.write_text(json.dumps(config))
            path.chmod(0o600)
            self.residents.append({"dir": d, "config": path, "socket": d / "control.sock", "votes": votes,
                                   "host_id": view(str(self.root / f"machine-id-{i}")), "proc": None})

    def start_resident(self, i):
        r = self.residents[i]
        log = open(r["dir"] / "resident.log", "a")
        r["proc"] = subprocess.Popen([MANAGERD, str(r["config"])], stdout=log, stderr=log,
                                     env={**os.environ, "PODMESH_MANAGER_NETWORK_MODE": "authenticated-static-peers",
                                          "PODMESH_MANAGER_VOTE_DIR": str(r["votes"]), "PODMESH_MANAGER_HOST_ID_FILE": str(r["host_id"])})
        self.procs.append(r["proc"])
        until(f"r{i} answers", 20, lambda: r["socket"].exists())

    def stop_resident(self, i):
        r = self.residents[i]
        try:
            unix(r["socket"], {"operation": "shutdown"}, False)
        except OSError:
            pass
        r["proc"].wait(timeout=20)

    def control(self, i, request):
        for _ in range(100):
            answer = unix(self.residents[i]["socket"], request, False, timeout=10)
            if answer.get("error") not in ("vote_operation_uncertain", "vote_busy", "vote_catching_up", "proposal_catching_up"):
                return answer
            time.sleep(0.2)
        raise AssertionError(f"no certain answer from r{i} to {request}")

    def read(self, i):
        answer = self.control(i, {"operation": "decision_read", "resource": R})
        assert "decision" in answer, answer
        return answer["decision"]

    def propose(self, i, payload, label):
        answer = self.control(i, {"operation": "decision_propose", "operation_id": f"propose-{label}", "payload": payload})
        assert "proposal" in answer, answer
        return answer

    # ----- the agents -----
    def agent(self, k):
        mandate = self.root / f"mandate-{k}"
        mandate.write_text(f"authorization_ref=e2e-decisions\nresident_socket={self.residents[k]['socket']}\nresource={R}\n")
        p = subprocess.run(["python3", "-B", AGENT], capture_output=True, text=True, timeout=120,
                           env={**os.environ, "PODMESH_DECISION_FOLLOW_MANDATE": str(mandate), "PODMESH_SOCKET": str(self.nodes[k]["socket"])})
        reports = [json.loads(line) for line in p.stdout.splitlines()]
        return p.returncode, reports[0] if reports else None, p.stderr

    def agents(self):
        return [self.agent(k) for k in range(3)]

    def close(self):
        for p in reversed(self.procs):
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()


def payload(epoch, previous, holder, method, eligible, issued=None):
    at = now() if issued is None else issued
    return {"kind": "podmesh-takeover-proof/quorum-ed25519", "authority_id": "replicas", "policy_digest": POLICY_DIGEST, "resource": R,
            "new_epoch": epoch, "previous_epoch": epoch - 1, "new_holder": holder, "previous_holder": previous,
            "holder_boot_id": BOOT, "grant_id": f"g{epoch}-{at}", "method": method, "eligible_after": eligible,
            "issued_at": at, "expires_at": at + LIFE}


def digest(p):
    return hashlib.sha256(json.dumps(p, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def pending(lab, i, p):
    return next((x for x in lab.read(i)["pending"] if x["payload_digest"] == digest(p)), None)


def decided(lab, i, epoch, timeout=30):
    return until(f"r{i} reads epoch {epoch} decided", timeout,
                 lambda: (lambda d: d["current"]["certificate"] if d["current"] and d["current"]["epoch"] == epoch else None)(lab.read(i)))


def everywhere(lab, epoch, replicas=(0, 1, 2)):
    """Waits until every running replica reads `epoch` decided, as each host's agent reads its own."""
    return [decided(lab, i, epoch) for i in replicas][0]


def screens(lab):
    return [lab.status(k)["highest_epoch_seen"] for k in range(3)]


def main():
    keep = "--keep" in sys.argv
    root = pathlib.Path(tempfile.mkdtemp(prefix="v3-5-e2e-"))
    root.chmod(0o700)
    lab = Lab(root)
    try:
        run(lab)
    finally:
        lab.close()
        if not keep:
            shutil.rmtree(root, ignore_errors=True)
    print(json.dumps({"checks": len(checks), "summary": summary}, sort_keys=True))


def run(lab):
    for i in range(3):
        (lab.root / f"machine-id-{i}").write_text(f"{'0' * 30}{i}{i}\n")
        (lab.root / f"evidence-{i}").mkdir(mode=0o700)
    for k in range(3):
        lab.start_node(k)
        answer = lab.node(k, "activation_require", universe_uuid=R, operation_id="e2e-require", authorization_ref="e2e-decisions",
                          lease_seconds=LEASE, takeover_margin_seconds=MARGIN, authority_id="replicas", authority_quorum=QUORUM)
        assert answer["ok"], answer
        assert answer["data"]["authority_policy_digest"] == POLICY_DIGEST, answer
    hosts = [n["host"] for n in lab.nodes]
    summary["hosts"] = hosts
    check(screens(lab) == [None, None, None], "three real nodes declare the resource under the replicas' 2-of-3 quorum, their screens empty")
    lab.configure_residents()
    for i in range(3):
        lab.start_resident(i)
    for i in range(3):
        until(f"r{i} caught up", 30, lambda: lab.control(i, {"operation": "status"})["catch_up"]["caught_up"])

    # 1. New ledgers: nothing is signed, nothing decided.
    for i in range(3):
        created = lab.control(i, {"operation": "vote_ledger_init", "operation_id": "init-1"})
        assert created.get("vote_ledger") == "created", created
    marked_at = now()
    first = payload(1, None, hosts[0], "first", now())
    lab.propose(0, first, "1-early")
    for i in range(3):
        until(f"r{i} refuses with an unadmitted ledger", 15,
              lambda: (lambda p: p and p["votes"] == 0 and p["here"].get("code") == "ledger_unadmitted")(pending(lab, i, first)))
    code, report, err = lab.agent(0)
    check(code == 0 and report["action"].startswith("none: nothing decided"),
          "new ledgers sign nothing: a proposal made while every ledger is unadmitted gets no vote, and the agent finds nothing to deliver")

    # 2. Readmission of r0 and r1 through the collector, after the wait; r2 stays unadmitted.
    for i in (0, 1):
        others = [j for j in range(3) if j != i]
        plan = {
            "replica_config": str(lab.residents[i]["config"]), "evidence_dir": str(lab.root / f"evidence-{i}"),
            "resident_socket": str(lab.residents[i]["socket"]),
            "stores": {f"r{j}": [MANAGERD, "--inspect-store", "--facts-only", "--config", str(lab.residents[j]["config"]),
                                 "--state-dir", str(lab.residents[j]["dir"])] for j in others},
            "ledgers": {KEYS[j][0]: ["cat", str(lab.residents[j]["votes"] / f"{KEYS[j][0]}.ledger")] for j in others},
            "screens": {hosts[k]: ["python3", "-B", str(COLLECTOR), "screen", "--socket", str(lab.nodes[k]["socket"]), "--resource", R] for k in range(3)},
        }
        plan_path = lab.root / f"plan-{i}.json"
        plan_path.write_text(json.dumps(plan))
        p = subprocess.run(["python3", "-B", str(COLLECTOR), "collect", "--plan", str(plan_path)], capture_output=True, text=True, timeout=300,
                           env={**os.environ, "PODMESH_MANAGER_NETWORK_MODE": "disabled"})
        assert p.returncode == 0, (p.stdout, p.stderr)
        collected = json.loads(p.stdout)
        lab.residents[i]["request"] = collected["request"]
        assert len(collected["evidence_sha256"]) == 2 + 2 + 3, collected
    early = lab.control(0, lab.residents[0]["request"])
    check(early.get("code") == "readmission_too_early", "the collector gathers two stores, two ledgers and three screens per replica into the operator's read-only evidence directory and prints the digests; readmission then waits out the certificate life and the skew bound")
    until("the readmission wait", LIFE + 120, lambda: now() >= early["retry_at"])
    for i in (0, 1):
        admitted = lab.control(i, lab.residents[i]["request"])
        assert admitted.get("vote_ledger") == "admitted", admitted
        assert len(admitted["admission"]["inputs_read"]) == 8, admitted
    summary["readmission_wait_seconds"] = early["retry_at"] - marked_at
    check(all(lab.control(i, {"operation": "status"})["votes"]["ledger_state"] == s for i, s in ((0, "admitted"), (1, "admitted"), (2, "unadmitted"))),
          "readmission admits r0 and r1 from the collected evidence, each reading its own store and the seven files whose digests were stated; r2 stays unadmitted")

    # 3. Epoch 1 decided by r0 and r1 (the first proposal has expired meanwhile: proposed again).
    first = payload(1, None, hosts[0], "first", now())
    lab.propose(1, first, "1")
    c1 = everywhere(lab, 1)
    signers = sorted(s["key_id"] for s in c1["signatures"])
    check(signers == ["replica-a", "replica-b"], "epoch 1 is decided by the two admitted replicas; the unadmitted one does not vote, and it reads the certificate all the same")
    results = lab.agents()
    actions = [r[1]["action"] for r in results]
    assert [r[0] for r in results] == [0, 0, 0], results
    s0 = lab.status(0)
    check(actions == ["activation_acquire", "activation_supersede", "activation_supersede"] and s0["epoch"] == 1 and s0["live"] and s0["holder_host_uuid"] == hosts[0]
          and screens(lab) == [1, 1, 1],
          "the agents deliver it through the node's own socket: the named host acquires epoch 1, the two others are superseded, every screen moves to 1")
    summary["epoch_1"] = {"signers": signers, "actions": actions}

    # 4. A same-holder re-issue, epoch 2.
    reissue = payload(2, hosts[0], hosts[0], "same_holder", c1["eligible_after"])
    lab.propose(0, reissue, "2")
    c2 = everywhere(lab, 2)
    results = lab.agents()
    s0 = lab.status(0)
    check([r[1]["action"] for r in results] == ["activation_acquire", "activation_supersede", "activation_supersede"] and s0["epoch"] == 2
          and s0["grant_id"] == c2["grant_id"] and screens(lab) == [2, 2, 2],
          "a same-holder re-issue (epoch 2) is decided by two replicas of three and delivered: the holder takes the new grant, every screen moves to 2",
          {"results": results, "status": s0, "screens": screens(lab)})

    # 5. A rotation to host 1: first with a barrier the view does not allow, then with the one it requires.
    rotation_early = payload(3, hosts[0], hosts[1], "lease_barrier", now() + LEASE + MARGIN)
    lab.propose(1, rotation_early, "3-early")
    for i in (0, 1):
        until(f"r{i} refuses the early rotation", 15,
              lambda: (lambda p: p and p["here"].get("code") == "barrier_too_early" and p["votes"] == 0)(pending(lab, i, rotation_early)))
    before = screens(lab)
    results = lab.agents()
    check(lab.read(0)["current"]["epoch"] == 2 and screens(lab) == before and all(r[1]["action"].startswith("none") for r in results),
          "a rotation whose barrier does not cover the current certificate's expiry plus the lease and the margin is refused by every voter (barrier_too_early); nothing is decided or delivered")
    required = max(c2["eligible_after"], c2["expires_at"] + LEASE + MARGIN, now() + LEASE + MARGIN)
    # Issued as late as the issue's own term allows (issued + lease + margin <= barrier), so that the
    # certificate lives as long as possible past its barrier.
    issued = required - LEASE - MARGIN
    rotation = payload(3, hosts[0], hosts[1], "lease_barrier", required, issued=issued)
    lab.propose(0, rotation, "3")
    c3 = everywhere(lab, 3)
    first_try = lab.agent(1)
    early_refusal = first_try[1]
    summary["rotation_barrier"] = {"eligible_after": required, "first_try": early_refusal}
    held_at_barrier = now() < required and early_refusal.get("result") == "refused" and "barrier" in (early_refusal.get("error") or "")
    until("the rotation's barrier", 120, lambda: now() >= required)
    results = lab.agents()
    s1 = lab.status(1)
    s0 = lab.status(0)
    check(s1["epoch"] == 3 and s1["holder_host_uuid"] == hosts[1] and s1["live"] and s0["superseded"] is True and screens(lab) == [3, 3, 3]
          and (held_at_barrier or early_refusal.get("result") == "delivered"),
          "the rotation with the barrier the view requires is decided; the new holder's node refuses it before the barrier and takes it after, the previous holder is superseded, every screen moves to 3")
    summary["epoch_3"] = {"held_at_barrier": held_at_barrier, "actions": [r[1]["action"] for r in results]}

    # 6. Replayed and late certificates delivered by hand: refused by every node's screen.
    refused = []
    for k in range(3):
        for c, op in ((c1, "activation_supersede"), (c2, "activation_supersede"), (c1, "activation_acquire"), (c2, "activation_acquire")):
            answer = lab.node(k, op, universe_uuid=R, certificate=c, operation_id=f"late-{op.replace('_', '-')}-{k}-{c['new_epoch']}",
                              authorization_ref="e2e-decisions")
            why = answer.get("error") or ""
            # Refused for the screen, the certificate's life or its holder: never for its form.
            refused.append(not answer["ok"] and any(w in why for w in ("does not supersede", "superseded", "expired", "another host")))
            summary.setdefault("late", []).append(why[:100])
    check(all(refused) and screens(lab) == [3, 3, 3],
          "a replayed or late certificate (epochs 1 and 2, as supersession and as acquisition) is refused by every node, and no screen moves")

    # 7. r1 stopped: one vote decides nothing; a hand-made 1-of-3 certificate is refused.
    lab.stop_resident(1)
    fourth = payload(4, hosts[1], hosts[1], "same_holder", c3["eligible_after"], issued=max(now(), c3["eligible_after"] + 1 - LIFE))
    lab.propose(0, fourth, "4")
    until("r0 votes for epoch 4", 15, lambda: (lambda p: p and p["votes"] == 1 and p["missing"] == 1)(pending(lab, 0, fourth)))
    time.sleep(2)
    results = [lab.agent(k) for k in (0, 2)]
    check(lab.read(0)["current"]["epoch"] == 3 and screens(lab) == [3, 3, 3] and all(r[1]["action"].startswith("none") for r in results),
          "with one replica stopped and one unadmitted, the one vote left decides nothing: the read says one is missing, and the agents deliver nothing")
    vote = lab.control(0, {"operation": "vote_sign", "operation_id": "retry-4", "payload": fourth})
    assert vote.get("replayed") is True, vote
    hand_made = dict(fourth, signatures=[{"key_id": "replica-a", "signature": vote["vote"]["signature"]}])
    answer = lab.node(1, "activation_acquire", universe_uuid=R, certificate=hand_made, operation_id="hand-made-1-of-3", authorization_ref="e2e-decisions")
    duplicated = dict(fourth, signatures=hand_made["signatures"] * 2)
    answer2 = lab.node(1, "activation_acquire", universe_uuid=R, certificate=duplicated, operation_id="hand-made-duplicate", authorization_ref="e2e-decisions")
    check(not answer["ok"] and "below_threshold" in answer["error"] and not answer2["ok"] and "duplicate_key" in answer2["error"] and screens(lab) == [3, 3, 3],
          "the node refuses a hand-made 1-of-3 certificate from that vote (below_threshold), and the same signature listed twice (duplicate_key); no screen moves")
    summary["minority"] = {"one_of_three": answer["error"][:120], "duplicate": answer2["error"][:120]}

    # 8. r1 back: the same decision with a fresh life is decided and delivered; run again, nothing twice.
    lab.start_resident(1)
    until("r1 caught up again", 30, lambda: lab.control(1, {"operation": "status"})["catch_up"]["caught_up"])
    relived = dict(fourth, issued_at=now(), expires_at=now() + LIFE)
    lab.propose(1, relived, "4-relived")
    c4 = everywhere(lab, 4)
    results = lab.agents()
    check([r[1]["action"] for r in results] == ["activation_supersede", "activation_acquire", "activation_supersede"] and screens(lab) == [4, 4, 4]
          and c4["grant_id"] == fourth["grant_id"],
          "the stopped replica back, the same decision re-issued with a fresh life is decided (r0 signs it again, one promise) and delivered: every screen moves to 4")
    results = lab.agents()
    check(all(r[0] == 0 and r[1]["action"].startswith("none") for r in results) and screens(lab) == [4, 4, 4],
          "the agents run again with no new decision: nothing is delivered twice")
    summary["final"] = {"screens": screens(lab), "decided": lab.read(0)["current"]["epoch"]}


if __name__ == "__main__":
    main()
