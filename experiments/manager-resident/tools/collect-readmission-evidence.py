#!/usr/bin/env python3
"""The operator's readmission evidence collector (V3-4's leftover, delivered with V3-5).

A replica's signing ledger that may have gone backwards signs nothing until the operator readmits it
(`vote_ledger_readmit`), and readmission fails closed: it reads, from the operator's evidence directory,
every other replica's store, every other key's ledger and every node's epoch screen, each file only if
its SHA-256 is the one the operator states in the request. This tool gathers those inputs and writes
them in the form readmission reads, then prints the digests to state.

    collect-readmission-evidence.py collect --plan PLAN.json
    collect-readmission-evidence.py screen --socket /run/podmesh/api.sock --resource UUID [--resource UUID ...]

`collect` reads a plan, a JSON object:

    {"replica_config": "/abs/config.json",       the resident configuration of the replica being readmitted
     "evidence_dir": "/abs/dir",                  the operator's evidence directory (it must exist)
     "resident_socket": "/abs/control.sock",      required: the replica's control socket, to read the mark
     "stores":  {"<replica_id>": [argv...]},      prints `podmesh-managerd --inspect-store --facts-only` output
     "ledgers": {"<key_id>": [argv...]},          prints that key's ledger file
     "screens": {"<node host UUID>": [argv...]}}  prints `screen` output, run on that node's host

Each command runs here, as given (an `ssh` to a peer host, or a local command); its standard output is the
input's content. The inputs the plan must name are computed from the replica's configuration exactly as
readmission computes them: every other replica's store, every other key of the policy, every node of
`votes.nodes`. The ledger is marked first: the tool reads the mark on the replica's control socket
(`resident_socket` is required) and refuses a ledger that is not unadmitted. Each command runs twice,
`--settle-seconds` apart (3 by default), and an input whose content changed between the two reads is
refused: it was not at rest. Each screen must carry the node's own clock (`observed_at`, which the
`screen` subcommand prints) no older than the mark. It fails closed: a missing or extra input, a command
that fails, times out or prints something that is not the input it claims to be, an input that changed
between its reads, a screen older than the mark or without its clock, or a ledger not marked, writes
nothing and exits 3. Otherwise every file is written read-only (0444) through a temporary file and a
rename, each stamped `collected_at` now -- after the mark, which readmission checks -- and the tool prints
the `evidence_sha256` map and the request to send.

THE DIGESTS ARE NOT PROOF THAT THE CLUSTER WAS LIVE. They say which bytes the operator vouches for, and
readmission refuses any other. A command that returns a consistent older copy -- a store or a ledger read
from a backup, an `ssh` to the wrong host -- passes every check here except the screens' clock, and
readmission would then set a floor that missed what the copy did not hold. The operator vouches that each
command reads the live input of the host it names (review of V3-5, finding 4).

`screen` runs on a node's host: it asks the node's socket `activation_status` for each resource and
prints `{"activation_status": [...], "observed_at": <the node's clock>}`, failing on any refusal.
"""
import argparse, hashlib, json, os, socket, subprocess, sys, time

EVIDENCE_FORM = "podmesh-manager-readmission-evidence/1"
LEDGER_FORM = "podmesh-manager-vote-ledger/1"
COMMAND_TIMEOUT = 120


class Refused(Exception):
    pass


def fail(msg, code=3):
    print(f"collect-readmission-evidence: {msg}", file=sys.stderr)
    sys.exit(code)


def local_request(path, request, line):
    """One request on a local Unix socket: the node's protocol (one JSON line) or the resident's (one
    JSON document to the end of the stream)."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(30)
    try:
        s.connect(path)
        s.sendall(json.dumps(request).encode() + (b"\n" if line else b""))
        if not line:
            s.shutdown(socket.SHUT_WR)
        data = b""
        while True:
            chunk = s.recv(65536)
            if not chunk:
                break
            data += chunk
        return json.loads(data)
    except (OSError, ValueError) as e:
        raise Refused(f"{path}: {e}") from e
    finally:
        s.close()


def screen(args):
    answers, observed = [], []
    for resource in args.resource:
        try:
            answer = local_request(args.socket, {"operation": "activation_status", "universe_uuid": resource}, True)
        except Refused as e:
            fail(str(e))
        if not answer.get("ok"):
            fail(f"activation_status {resource} refused: {answer.get('error')}")
        answers.append(answer["data"])
        observed.append(answer.get("observed_at"))
    if not all(isinstance(o, int) for o in observed):
        fail("the node's answers carry no observed_at clock")
    print(json.dumps({"activation_status": answers, "observed_at": min(observed)}, sort_keys=True))


def required(config):
    """What readmission reads, from the replica's configuration: the other replicas' stores, the other
    keys' ledgers, every node's screen."""
    votes = config.get("votes")
    if not isinstance(votes, dict):
        raise Refused("the replica's configuration has no votes section")
    me = config["network"]["replica_id"]
    replicas = [r["replica_id"] for r in config["network"]["manager"]["replicas"]]
    keys = [k["key_id"] for k in votes["authority_quorum"]["keys"]]
    return ({r for r in replicas if r != me}, {k for k in keys if k != votes["key_id"]}, set(votes["nodes"]))


def run(argv):
    if not isinstance(argv, list) or not argv or not all(isinstance(a, str) for a in argv):
        raise Refused(f"a command must be a non-empty list of strings: {argv!r}")
    try:
        p = subprocess.run(argv, capture_output=True, timeout=COMMAND_TIMEOUT)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise Refused(f"{argv[0]}: {e}") from e
    if p.returncode:
        raise Refused(f"{' '.join(argv)[:200]} exited {p.returncode}: {p.stderr.decode(errors='replace')[-400:]}")
    try:
        return json.loads(p.stdout)
    except ValueError as e:
        raise Refused(f"{' '.join(argv)[:200]} printed no JSON") from e


def at_rest(input_, content):
    """What must not change between two reads of a live input: a store's facts, a ledger whole, a
    screen's epochs and serials (its answers also carry clocks and remaining times, which move)."""
    if input_ == "store":
        return (content.get("history_count"), content.get("logical_history_sha256"))
    if input_ == "ledger":
        return json.dumps(content, sort_keys=True)
    return sorted((a.get("universe_uuid"), a.get("highest_epoch_seen"), a.get("authority_serial")) for a in content["activation_status"])


def check(input_, source, content):
    """The input is what it claims to be, as far as can be told here; readmission checks it again
    (a store's digest against its facts, a ledger's checksum)."""
    if input_ == "store":
        if not isinstance(content.get("ordered_facts"), list) or content.get("history_count") != len(content["ordered_facts"]) \
                or not isinstance(content.get("logical_history_sha256"), str):
            raise Refused(f"store {source}: not an --inspect-store --facts-only output")
    elif input_ == "ledger":
        if content.get("form") != LEDGER_FORM or content.get("key_id") != source:
            raise Refused(f"ledger {source}: not the ledger of key {source}")
    else:
        answers = content.get("activation_status") if isinstance(content, dict) else None
        if not isinstance(answers, list) or not all(isinstance(a, dict) and a.get("this_host_uuid") == source for a in answers):
            raise Refused(f"screen {source}: not the activation_status answers of node {source}")
        if not isinstance(content.get("observed_at"), int) or isinstance(content.get("observed_at"), bool):
            raise Refused(f"screen {source}: carries no observed_at, the node's clock")


def collect(args):
    try:
        with open(args.plan, encoding="utf-8") as f:
            plan = json.load(f)
        with open(plan["replica_config"], encoding="utf-8") as f:
            config = json.load(f)
        stores, ledgers, screens = required(config)
        for name, want in (("stores", stores), ("ledgers", ledgers), ("screens", screens)):
            got = set(plan.get(name, {}))
            if got != want:
                raise Refused(f"the plan's {name} must be exactly {sorted(want)}; it names {sorted(got)}")
        evidence_dir = plan["evidence_dir"]
        if not os.path.isabs(evidence_dir) or not os.path.isdir(evidence_dir) or os.path.islink(evidence_dir):
            raise Refused(f"the evidence directory {evidence_dir} must be an existing absolute directory")
        if not plan.get("resident_socket"):
            raise Refused("the plan names no resident_socket: the ledger is marked first, and the mark is read before anything is collected")
        status = local_request(plan["resident_socket"], {"operation": "status"}, False)
        votes = status.get("votes") or {}
        if votes.get("ledger_state") != "unadmitted":
            raise Refused(f"the replica's ledger is {votes.get('ledger_state')!r}, not unadmitted: mark it first (vote_ledger_mark_unadmitted)")
        marked_at = votes.get("unadmitted_since")
        if not isinstance(marked_at, int) or isinstance(marked_at, bool):
            raise Refused("the replica's status names no unadmitted_since: the mark cannot be read")
        if int(time.time()) < marked_at:
            raise Refused("this clock is before the mark: the evidence would count as collected before it")
        table = [(i, source, argv) for i, t in (("store", plan["stores"]), ("ledger", plan["ledgers"]), ("screen", plan["screens"]))
                 for source, argv in sorted(t.items())]
        first = {}
        for input_, source, argv in table:
            content = run(argv)
            check(input_, source, content)
            first[(input_, source)] = content
        time.sleep(args.settle_seconds)
        files = {}
        for input_, source, argv in table:
            content = run(argv)
            check(input_, source, content)
            if at_rest(input_, content) != at_rest(input_, first[(input_, source)]):
                raise Refused(f"{input_} {source} changed between two reads {args.settle_seconds} s apart: it is not at rest; collect again once it is")
            if input_ == "screen" and content["observed_at"] < marked_at:
                raise Refused(f"screen {source} was observed at {content['observed_at']}, before the mark at {marked_at}: not a screen of now")
            envelope = {"form": EVIDENCE_FORM, "input": input_, "source": source, "collected_at": int(time.time()), "content": content}
            files[f"{input_}.{source}.json"] = json.dumps(envelope, sort_keys=True).encode()
    except (Refused, KeyError, TypeError, ValueError, OSError) as e:
        fail(f"{e}; nothing was written")
    digests = {}
    for name, data in sorted(files.items()):
        path = os.path.join(evidence_dir, name)
        temporary = os.path.join(evidence_dir, f".{name}.{os.getpid()}.tmp")
        with open(temporary, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.chmod(temporary, 0o444)
        os.replace(temporary, path)
        digests[name] = hashlib.sha256(data).hexdigest()
    fd = os.open(evidence_dir, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)
    print(json.dumps({"evidence_dir": evidence_dir, "evidence_sha256": digests,
                      "request": {"operation": "vote_ledger_readmit", "operation_id": f"readmit-{int(time.time())}", "evidence_sha256": digests}},
                     indent=1, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    c = sub.add_parser("collect")
    c.add_argument("--plan", required=True)
    c.add_argument("--settle-seconds", type=float, default=3.0, help="the delay between the two reads of each input")
    s = sub.add_parser("screen")
    s.add_argument("--socket", required=True)
    s.add_argument("--resource", action="append", required=True)
    args = parser.parse_args()
    collect(args) if args.command == "collect" else screen(args)


if __name__ == "__main__":
    main()
