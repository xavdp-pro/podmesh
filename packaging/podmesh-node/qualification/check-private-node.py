#!/usr/bin/env python3
"""Product API / private SQL qualification driver; external lifecycle belongs to runtime owner.
No Podman, service, signal, reboot, database restore or provider-policy mutation here.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import time
import uuid

MAX_REPLY = 8 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def token(value):
    require(isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9-]{1,80}", value), "invalid operation identity")
    return value


def validate_plan(plan):
    require(plan.get("protocol") == "podmesh-private-node-proof/1", "unknown proof plan")
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", plan["image"]), "immutable local image required")
    require(plan["profile"] in ("flat", "nested"), "unknown universe profile")
    require(isinstance(plan["mandate"], str) and plan["mandate"], "explicit mandate required")
    for field in ("universe_uuid", "clone_uuid"):
        require(str(uuid.UUID(plan[field])) == plan[field], "canonical universe identity required")
    require(plan["universe_uuid"] != plan["clone_uuid"], "distinct clone identity required")
    for value in plan["operation_ids"].values():
        token(value)
    require(len(set(plan["operation_ids"].values())) == len(plan["operation_ids"]), "distinct operation IDs required")
    return plan


def new_plan(image, profile, mandate):
    names = ["create", "start", "resources", "pause", "resume", "stop", "clone", "delete", "arm-boot", "boot-pass", "cleanup-stop", "cleanup-delete"]
    return validate_plan({"protocol": "podmesh-private-node-proof/1", "image": image, "profile": profile,
                          "mandate": mandate, "universe_uuid": str(uuid.uuid4()), "clone_uuid": str(uuid.uuid4()),
                          "operation_ids": {name: str(uuid.uuid4()) for name in names}})


def requests(plan):
    target = plan["universe_uuid"]
    child = plan["clone_uuid"]

    def request(operation, key=None, universe=None, **fields):
        return {"operation": operation, "operation_id": plan["operation_ids"][key or operation],
                "universe_uuid": universe or target, "authorization_ref": plan["mandate"], **fields}

    return [request("create", image=plan["image"], command=["sleep", "3600"], network_profile="isolated", universe_profile=plan["profile"]),
            request("start", observe_seconds=0), request("resources", memory_bytes=33554432, cpus=0.25),
            request("pause"), request("resume"), request("stop", timeout_seconds=5, on_timeout="kill"),
            request("clone", universe=child, source_uuid=target), request("delete", universe=child)]


class Driver:
    def __init__(self, args, plan):
        self.args, self.plan = args, validate_plan(plan)
        require(os.geteuid() == args.application_uid == 1102, "driver must use actual application UID1102")
        # Reuse the reviewed profile/password/socket parser. Credentials stay in child env.
        helper = Path(__file__).resolve().parents[2] / "podmesh-manager/qualification/activation/dump-store.py"
        spec = importlib.util.spec_from_file_location("private_node_dump_parameters", helper)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.parameters = module.client_parameters
        args.evidence.mkdir(mode=0o700, parents=False, exist_ok=True)
        metadata = args.evidence.stat()
        require(metadata.st_uid == os.geteuid() and metadata.st_mode & 0o077 == 0, "evidence directory must be privately owned")
        self.log = args.evidence / (args.phase + ".jsonl")
        self.file = self.log.open("x", encoding="utf8")
        os.chmod(self.log, 0o600)

    def record(self, kind, data):
        self.file.write(canonical({"kind": kind, "time_ns": time.time_ns(), "data": data}) + "\n")
        self.file.flush()
        os.fsync(self.file.fileno())

    def api(self, request):
        with socket.socket(socket.AF_UNIX) as stream:
            stream.settimeout(120)
            stream.connect(str(self.args.socket))
            stream.sendall(canonical(request).encode() + b"\n")
            with stream.makefile("rb") as reader:
                reply = reader.readline(MAX_REPLY + 1)
        require(len(reply) <= MAX_REPLY and reply.endswith(b"\n"), "incomplete or oversized API reply")
        result = json.loads(reply)
        self.record("api", {"request": request, "response": result})
        return result

    def ok(self, request):
        response = self.api(request)
        require(response.get("ok") is True, "product API refused; inspect private evidence")
        return response["data"]

    def sql(self, statement):
        options, environment = self.parameters(self.args.profile)
        arguments = ["mariadb", "--no-defaults", "--batch", "--raw", "--skip-column-names"]
        arguments += [item for item in options[1:] if item not in ("--no-defaults", "--single-transaction", "--hex-blob")]
        result = subprocess.run(arguments, input=statement.encode(), stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=30, env=environment, check=False)
        require(result.returncode == 0, "private SQL oracle refused; client diagnostics suppressed")
        return [line.split("\t") for line in result.stdout.decode().splitlines()]

    def preflight(self):
        capabilities = self.ok({"operation": "capabilities"})
        require(capabilities.get("store_engine") == "mariadb", "app must use MariaDB")
        require(capabilities.get("host_adapter_protocol") == "podmesh-host-capability/1", "typed provider required")
        identity = self.ok({"operation": "identity"})["host_uuid"]
        rows = self.sql("SELECT value FROM metadata WHERE `key`='host_uuid';")
        require(rows == [[identity]], "API and SQL must address the same node journal")
        self.record("identity", {"uid": os.geteuid(), "host_uuid": identity, "capabilities": capabilities})
        self.no_sqlite()

    def no_sqlite(self):
        metadata = self.args.state_dir.stat()
        require(self.args.state_dir.is_dir() and metadata.st_uid == os.geteuid(), "actual application state directory must be accessible and owned by UID1102")
        for name in ("state.sqlite", "state.sqlite-wal", "state.sqlite-shm"):
            require(not (self.args.state_dir / name).exists(), "unexpected SQLite side journal")

    def journal(self, request, expected="verified"):
        operation_id = token(request["operation_id"])
        rows = self.sql("SELECT HEX(request),status,HEX(result) FROM operations WHERE id='" + operation_id + "';")
        require(len(rows) == 1 and rows[0][1] == expected, "one terminal operation required")
        require(json.loads(bytes.fromhex(rows[0][0])) == request, "SQL operation bound to another request")
        result = json.loads(bytes.fromhex(rows[0][2]))
        attempts = self.sql("SELECT id,outcome,finished_at FROM operation_attempts WHERE operation_id='" + operation_id + "' ORDER BY id;")
        require(attempts and attempts[-1][1] == expected and attempts[-1][2] != "NULL", "terminal attempt required")
        self.record("sql_operation", {"request": request, "status": expected, "result": result, "attempts": attempts})
        self.no_sqlite()
        return {"result": result, "attempts": attempts}

    def exercise(self, request):
        first = self.ok(request)
        stored = self.journal(request)
        operation = request["operation"]
        if operation in ("start", "resume"):
            require(first.get("running") is True, "running effect must actually be observed")
        elif operation == "pause":
            require(first.get("state") == "paused", "paused effect must actually be observed")
        elif operation == "stop":
            require(first.get("running") is False, "stopped effect must actually be observed")
        elif operation == "resources":
            require(first.get("verification") == "kernel", "resources require real running cgroup verification")
        elif operation == "delete":
            require(first.get("absent") is True, "delete must observe absence")
        elif operation in ("create", "clone"):
            require(first.get("container_id"), "creation/clone must observe container identity")
        require(first.get("replayed") is not True, "first operation must be new")
        replay = self.ok(request)
        require(replay.get("replayed") is True and replay.get("historical") is True, "explicit historical replay required")
        require(self.journal(request) == stored, "replay must not rewrite outcome or add attempts")
        return first

    def phase(self):
        require(self.sql("SELECT @@hostname,DATABASE();") == [[self.args.expected_server_hostname, self.args.expected_database]], "SQL must reach the declared private server/database, including restored endpoint")
        if self.args.phase != "pending-probe":
            self.preflight()
        planned = requests(self.plan)
        if self.args.phase == "lifecycle":
            count = int(self.sql("SELECT COUNT(*) FROM operations;")[0][0])
            if self.args.resume_ordering:
                require(count == 1, "ordering leg must leave exactly its one CREATE")
                self.journal(planned[0])
                require(self.ok(planned[0]).get("replayed") is True, "ordering CREATE must replay")
                remaining = planned[1:]
            else:
                require(count == 0, "lifecycle requires a fresh private node journal")
                remaining = planned
            for request in remaining:
                self.exercise(request)
            changed = dict(planned[0], command=["false"])
            before = self.journal(planned[0])
            require(self.api(changed).get("ok") is False, "changed operation identity must refuse")
            require(self.journal(planned[0]) == before, "refusal must preserve original journal")
            self.record("coverage", {"operations": [r["operation"] for r in planned], "nested_inner_workload": "not tested", "restart_boot_restore": "pending"})
        elif self.args.phase in ("after-restart", "after-restore"):
            # Runtime owner has already changed the process/DB endpoint as declared.
            # No action here performs restart, import or restoration.
            require(self.args.baseline, "pre-recovery SQL evidence file required")
            baseline = {}
            for line in self.args.baseline.read_text().splitlines():
                item = json.loads(line)
                if item["kind"] == "sql_operation":
                    entry = item["data"]
                    baseline[entry["request"]["operation_id"]] = {"result": entry["result"], "attempts": entry["attempts"]}
            before = {r["operation_id"]: self.journal(r) for r in planned}
            require(all(before[r["operation_id"]] == baseline.get(r["operation_id"]) for r in planned), "recovered journal must match saved source SQL evidence before replay")
            for request in planned:
                require(self.ok(request).get("replayed") is True, "same ID must replay after external recovery")
                require(self.journal(request) == before[request["operation_id"]], "recovery replay must preserve SQL evidence")
            inventory = self.ok({"operation": "inventory"})
            original_id = baseline[planned[0]["operation_id"]]["result"]["container_id"]
            require(any(row.get("Id") == original_id for row in inventory["containers"]), "recovered provider must observe original bound universe identity")
            self.record("recovery_observation", inventory)
        elif self.args.phase == "arm-boot":
            request = {"operation": "start", "operation_id": self.plan["operation_ids"]["arm-boot"], "universe_uuid": self.plan["universe_uuid"], "authorization_ref": self.plan["mandate"], "observe_seconds": 0}
            self.exercise(request)
            self.record("before_external_boot", self.ok({"operation": "boot_restore_status"}))
        elif self.args.phase == "after-boot":
            require(self.args.previous_boot_id, "previous actual host boot ID required")
            status = self.ok({"operation": "boot_restore_status"})
            require(status["boot_id"] != self.args.previous_boot_id, "host boot ID must actually change")
            target = self.plan["universe_uuid"]
            child_id = "boot-" + status["boot_id"].replace("-", "") + "-" + target.replace("-", "")
            rows = self.sql("SELECT request,status FROM operations WHERE id='" + token(child_id) + "';")
            require(len(rows) == 1 and rows[0][1] == "verified", "automatic boot child must already be verified before probe")
            child = json.loads(rows[0][0])
            baseline = self.journal(child)
            request = {"operation": "boot_restore", "operation_id": self.plan["operation_ids"]["boot-pass"], "authorization_ref": self.plan["mandate"], "observe_seconds": 0}
            self.ok(request)
            self.ok(request)
            require(self.journal(child) == baseline, "boot retry must not add another start attempt")
            self.record("after_external_boot", self.ok({"operation": "boot_restore_status"}))
        elif self.args.phase == "ordering-create":
            require(self.sql("SELECT COUNT(*) FROM operations;") == [["0"]], "ordering requires fresh journal")
            self.exercise(planned[0])
        elif self.args.phase == "pending-probe":
            # Runtime externally holds only its provider before issuing CREATE in another process.
            request = planned[0]
            rows = self.sql("SELECT status,result FROM operations WHERE id='" + token(request["operation_id"]) + "';")
            attempts = self.sql("SELECT outcome,finished_at FROM operation_attempts WHERE operation_id='" + token(request["operation_id"]) + "';")
            require(rows == [["pending", "NULL"]] and attempts == [["NULL", "NULL"]], "committed pending operation/attempt required while provider cannot execute")
            self.record("pending_before_external_effect", {"request": request, "operations": rows, "attempts": attempts})
        elif self.args.phase == "cleanup":
            for operation, key, fields in [("stop", "cleanup-stop", {"timeout_seconds": 5, "on_timeout": "kill"}), ("delete", "cleanup-delete", {})]:
                self.exercise({"operation": operation, "operation_id": self.plan["operation_ids"][key], "universe_uuid": self.plan["universe_uuid"], "authorization_ref": self.plan["mandate"], **fields})
        self.record("phase_complete", {"phase": self.args.phase, "scope": "this phase only; no combined production PASS", "source_revision": self.args.source_revision})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    plan = sub.add_parser("plan")
    plan.add_argument("--image", required=True)
    plan.add_argument("--universe-profile", choices=["flat", "nested"], required=True)
    plan.add_argument("--mandate", required=True)
    plan.add_argument("--output", required=True, type=Path)
    run = sub.add_parser("run")
    run.add_argument("--plan", required=True, type=Path)
    run.add_argument("--phase", required=True, choices=["lifecycle", "after-restart", "arm-boot", "after-boot", "after-restore", "ordering-create", "pending-probe", "cleanup"])
    run.add_argument("--socket", required=True, type=Path)
    run.add_argument("--profile", required=True, type=Path)
    run.add_argument("--state-dir", required=True, type=Path)
    run.add_argument("--evidence", required=True, type=Path)
    run.add_argument("--source-revision", required=True)
    run.add_argument("--application-uid", type=int, default=1102)
    run.add_argument("--previous-boot-id")
    run.add_argument("--expected-server-hostname", required=True)
    run.add_argument("--expected-database", required=True)
    run.add_argument("--baseline", type=Path)
    run.add_argument("--resume-ordering", action="store_true")
    args = parser.parse_args()
    if args.command == "plan":
        document = new_plan(args.image, args.universe_profile, args.mandate)
        with args.output.open("x", encoding="utf8") as output:
            os.chmod(args.output, 0o600)
            output.write(canonical(document) + "\n")
        print("Proof identities written; host grants and runtime preparation remain external.")
        return
    require(re.fullmatch(r"[0-9a-f]{40}", args.source_revision), "full immutable source revision required")
    driver = Driver(args, json.loads(args.plan.read_text()))
    driver.record("inputs", {"plan_sha256": hashlib.sha256(args.plan.read_bytes()).hexdigest(), "source_revision": args.source_revision, "phase": args.phase})
    try:
        driver.phase()
    except Exception as problem:
        # No command stderr, DSN, password or profile contents in evidence or stdout.
        driver.record("phase_failed", {"error_type": type(problem).__name__})
        raise RuntimeError("phase failed; private evidence retained") from None
    finally:
        driver.file.close()
    print("Phase verified; full runtime/product qualification remains external.")


if __name__ == "__main__":
    main()
