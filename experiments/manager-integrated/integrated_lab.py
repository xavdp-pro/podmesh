"""Disposable process composition, never an installed manager activation path."""

from __future__ import annotations

import dataclasses
from contextlib import closing
import hashlib
import json
import multiprocessing as mp
from multiprocessing.connection import wait
import os
from pathlib import Path
import select
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "experiments/manager-fencing"))
from fencing_lab import Authority, Maker, Permit, Refused  # noqa: E402

HA = ROOT / "experiments/manager-ha/target/debug/podmesh-manager-ha-lab"
RESIDENT = ROOT / "experiments/manager-resident/target/debug/podmesh-manager-resident-lab"
CONTEXT = mp.get_context("spawn")
RESOURCE = "counter:integrated"


def receive(channel, timeout=10):
    if not channel.poll(timeout):
        raise TimeoutError("fixture IPC outcome unknown; do not invent a refusal")
    return json.loads(channel.recv_bytes(65536))


def send(channel, value):
    wire = json.dumps(value, sort_keys=True).encode()
    if len(wire) > 65536:
        raise ValueError("fixture message bound")
    channel.send_bytes(wire)


def gate_worker(path, admin, actor_channels, actors):
    """Actor identity comes from fixed inherited channels, never request JSON."""
    gate = Authority(Path(path))
    send(admin, {"ready": True, "pid": os.getpid()})
    try:
        while True:
            for channel in wait([admin, *actor_channels]):
                request = receive(channel)
                binding = {}
                if channel != admin:
                    supplied = request.get("permit", {})
                    supplied = supplied if isinstance(supplied, dict) else {}
                    binding = {"operation_id": request.get("operation_id"),
                               "authority_id": gate.authority_id,
                               "epoch": supplied.get("epoch"), "resource": supplied.get("resource")}
                try:
                    if channel == admin:
                        if request["operation"] == "stop":
                            send(admin, {"stopped": True})
                            return
                        if request["operation"] != "transfer":
                            raise Refused("unknown fixture control")
                        permit = gate.transfer(RESOURCE, request["expected_epoch"],
                                               *actors[request["owner"]])
                        result = {"permit": dataclasses.asdict(permit)}
                    else:
                        if set(request) != {"permit", "operation_id", "delta"}:
                            raise Refused("effect-only fixture channel")
                        actor = actors[actor_channels.index(channel)]
                        permit = Permit.decode(json.dumps(request["permit"]).encode())
                        result = {"result": gate.effect(permit, actor,
                                                       request["operation_id"], request["delta"])}
                    send(channel, {"ok": True, **binding, **result})
                except Refused as error:
                    send(channel, {"ok": False, **binding, "reason": str(error), "code": error.code})
    finally:
        gate.close()


@dataclasses.dataclass
class ChannelState:
    uncertain: object
    lock: object

    @classmethod
    def new(cls):
        return cls(CONTEXT.Event(), CONTEXT.Lock())


class GateClient:
    def __init__(self, channel, state, timeout=10):
        self.channel = channel
        self.state = state
        self.timeout = timeout
        self.committed = False

    def effect(self, permit, actor, operation_id, delta):
        # The gate ignores local actor assertions: its IPC endpoint pins identity.
        if not self.state.lock.acquire(block=False):
            raise Refused("gate channel busy or abandoned; no request sent", code="channel_unusable")
        try:
            self.committed = None
            if self.state.uncertain.is_set():
                raise Refused("gate channel has an unresolved request", code="channel_unknown")
            # Shared across Maker processes. A timeout, invalid reply or killed
            # holder leaves this set: no later caller can consume a stale reply.
            self.state.uncertain.set()
            try:
                send(self.channel, {"permit": dataclasses.asdict(permit),
                                    "operation_id": operation_id, "delta": delta})
                result = receive(self.channel, self.timeout)
                expected = {"operation_id": operation_id, "authority_id": permit.authority_id,
                            "epoch": permit.epoch, "resource": permit.resource}
                if not isinstance(result, dict) or type(result.get("ok")) is not bool:
                    raise ValueError("invalid gate response")
                if any(type(result.get(key)) is not type(value) or result[key] != value
                       for key, value in expected.items()):
                    raise ValueError("uncorrelated gate response")
                fields = {"result"} if result["ok"] else {"reason", "code"}
                if set(result) != {"ok", *expected, *fields}:
                    raise ValueError("invalid gate response fields")
                if result["ok"] and type(result["result"]) is not int:
                    raise ValueError("invalid gate result")
                if not result["ok"] and any(type(result[key]) is not str for key in fields):
                    raise ValueError("invalid gate refusal")
            except Exception as error:
                raise Refused("gate IPC outcome unknown; channel disabled", code="channel_unknown") from error
            self.state.uncertain.clear()
            self.committed = result["ok"]
            if not result["ok"]:
                raise Refused(result["reason"], code=result["code"])
            return result["result"]
        finally:
            self.state.lock.release()


def control(path, operation="status"):
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(3)
        stream.connect(str(path))
        stream.sendall(json.dumps({"operation": operation}).encode())
        stream.shutdown(socket.SHUT_WR)
        chunks = bytearray()
        while block := stream.recv(8192):
            chunks.extend(block)
            if len(chunks) > 524288:
                raise ValueError("resident status exceeds bound")
        return json.loads(chunks)


def ha_request(directory, replica, request):
    result = subprocess.run([str(HA), str(directory / f"r{replica}.sqlite"),
                             str(directory / "manager.json"), f"r{replica}"],
                            input=json.dumps(request), capture_output=True, text=True, timeout=10)
    reply = json.loads(result.stdout)
    if result.returncode:
        raise Refused(reply.get("error", "local observation failed"))
    return reply


def maker_attempt(directory, replica, actor, authority_id, gate_channel, channel_state, reply, request):
    """One short-lived maker fixture; no authority is added to the resident."""
    directory = Path(directory)
    maker = None
    gate = None
    gate_committed = False
    try:
        status = control(directory / f"r{replica}.sock")
        if status["replica_id"] != actor[0] or status["activation_authority"] is not False:
            raise Refused("unexpected resident identity or authority")
        permit = Permit.decode(json.dumps(request["permit"]).encode())
        if permit.resource in status["inspection"]["blocked_exclusive_resources"]:
            raise Refused("resident conflict blocks this resource", code="conflict")
        maker = Maker(directory / f"maker{replica}.sqlite", authority_id, *actor)
        gate = GateClient(gate_channel, channel_state) if request.get("gate_reachable", True) else None
        value = maker.effect(gate, permit, request["operation_id"], request["delta"])
        gate_committed = True
        # This deliberate kill point models a missing local result after gate commit.
        if request.get("crash_after_gate"):
            os._exit(73)
        outcome = {"authority_id": authority_id, "resource": permit.resource,
                   "epoch": permit.epoch, "operation_id": request["operation_id"],
                   "result": value, "evidence": "external-gate-receipt"}
        ha_request(directory, replica, {
            "operation": "observe", "operation_id": "effect:" + request["operation_id"],
            "scope": f"s{replica}", "subject": "effect:" + request["operation_id"],
            "exclusive_resource": None, "active_claim": False,
            "value": json.dumps(outcome, sort_keys=True),
        })
        send(reply, {"ok": True, "result": value, "published": True})
    except Refused as error:
        gate_committed = gate.committed if gate is not None else gate_committed
        send(reply, {"ok": False, "reason": str(error), "code": error.code,
                     "gate_committed": gate_committed, "unknown": gate_committed is None})
    except Exception as error:
        gate_committed = gate.committed if gate is not None else gate_committed
        send(reply, {"ok": False, "unknown": True, "type": type(error).__name__,
                     "gate_committed": gate_committed})
    finally:
        if maker is not None:
            maker.close()


class Proxy:
    """Fixed destination TCP relay; a cut also closes already accepted streams."""
    def __init__(self, target):
        self.target = target
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(8)
        self.address = self.listener.getsockname()
        self.blocked = threading.Event()
        self.stopping = threading.Event()
        self.active = set()
        self.workers = []
        self.lock = threading.Lock()
        self.thread = threading.Thread(target=self.run)
        self.thread.start()

    def cut(self, blocked):
        with self.lock:
            if blocked:
                self.blocked.set()
                for stream in self.active:
                    try:
                        stream.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
            else:
                self.blocked.clear()

    def relay(self, source):
        destination = None
        try:
            destination = socket.create_connection(self.target, timeout=1)
            with self.lock:
                if self.blocked.is_set():
                    return
                self.active.update((source, destination))
            deadline = time.monotonic() + 5
            while not self.stopping.is_set() and time.monotonic() < deadline:
                readable, _, _ = select.select([source, destination], [], [], 0.1)
                for current in readable:
                    data = current.recv(8192)
                    if not data:
                        return
                    other = destination if current is source else source
                    other.sendall(data)
        except OSError:
            pass
        finally:
            with self.lock:
                self.active.discard(source)
                self.active.discard(destination)
            source.close()
            if destination is not None:
                destination.close()

    def run(self):
        self.listener.settimeout(0.1)
        while not self.stopping.is_set():
            try:
                source, _ = self.listener.accept()
            except socket.timeout:
                continue
            if self.blocked.is_set():
                source.close()
                continue
            source.settimeout(1)
            self.workers = [worker for worker in self.workers if worker.is_alive()]
            # Trusted three-resident fixture; still bound admission under a fault.
            if len(self.workers) >= 8:
                source.close()
                continue
            worker = threading.Thread(target=self.relay, args=(source,))
            self.workers.append(worker)
            worker.start()

    def close(self):
        self.stopping.set()
        self.cut(True)
        self.thread.join(2)
        self.listener.close()
        for worker in self.workers:
            worker.join(6)
        if self.thread.is_alive() or any(worker.is_alive() for worker in self.workers):
            raise RuntimeError("proxy cleanup did not finish")


def until(predicate, timeout=20):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise AssertionError("bounded fixture condition timed out")
        time.sleep(0.05)


class Lab:
    def __init__(self, directory):
        self.directory = Path(directory).resolve()
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=False)
        self.run_id = str(uuid.uuid4())
        self.children = {}
        self.logs = []
        self.proxies = []
        self.gate = None
        self.admin = None
        self.admin_child = None
        self.channels = []
        self.events = []
        try:
            self.initialize()
        except BaseException:
            self.abort()
            raise

    def abort(self):
        """Best-effort bounded cleanup for a failed fixture setup or shutdown."""
        for process in self.children.values():
            if process.poll() is None:
                process.kill()
                process.wait(5)
        if self.gate is not None:
            if self.gate.is_alive():
                self.gate.kill()
            self.gate.join(5)
        for _, _, proxy in self.proxies:
            proxy.close()
        self.proxies.clear()
        for log in self.logs:
            log.close()
        for pair in self.channels:
            for channel in pair:
                channel.close()
        for channel in (self.admin, self.admin_child):
            if channel is not None:
                channel.close()

    def initialize(self):
        for binary in (HA, RESIDENT):
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise FileNotFoundError(f"Build the laboratory binary first: {binary}")
        if len(str(self.directory / "r0.sock")) > 100:
            raise ValueError("use a shorter evidence path for private resident sockets")
        self.actors = [(f"r{i}", str(uuid.uuid4())) for i in range(3)]
        self.manager = {"logical_manager_id": str(uuid.uuid4()),
                        "replicas": [{"replica_id": f"r{i}", "host_id": str(uuid.uuid4())}
                                     for i in range(3)],
                        "grants": [{"scope": f"s{i}", "owner_replica_id": f"r{i}"}
                                   for i in range(3)]}
        self.write_json("manager.json", self.manager)
        listeners = [socket.socket() for _ in range(3)]
        for listener in listeners:
            listener.bind(("127.0.0.1", 0))
        addresses = [listener.getsockname() for listener in listeners]
        keys = {(i, j): os.urandom(32).hex() for i in range(3) for j in range(i + 1, 3)}
        for i in range(3):
            peers = []
            for j in range(3):
                if i == j:
                    continue
                proxy = Proxy(addresses[j])
                self.proxies.append((i, j, proxy))
                peers.append({"replica_id": f"r{j}", "endpoint": "%s:%s" % proxy.address,
                              "shared_key_hex": keys[min(i, j), max(i, j)]})
            self.write_json(f"r{i}.json", {
                "network": {"replica_id": f"r{i}", "database_path": str(self.directory / f"r{i}.sqlite"),
                            "manager": self.manager, "bind": "%s:%s" % addresses[i], "peers": peers},
                "control_socket": str(self.directory / f"r{i}.sock"),
                "interval_ms": 100, "max_backoff_ms": 400, "incoming_workers": 2,
            })
            self.observe(i, "initial")
        for listener in listeners:
            listener.close()
        gate = Authority(self.directory / "gate.sqlite", create=True)
        gate.declare(RESOURCE)
        self.authority_id = gate.authority_id
        gate.close()
        for i in range(3):
            Maker(self.directory / f"maker{i}.sqlite", self.authority_id,
                  *self.actors[i], create=True).close()
        self.admin, self.admin_child = CONTEXT.Pipe()
        self.channels = [CONTEXT.Pipe() for _ in range(3)]
        self.channel_states = [ChannelState.new() for _ in range(3)]
        self.start_gate()
        for i in range(3):
            self.start(i)
        self.converged(3)

    def record(self, action, **values):
        event = {"run_id": self.run_id, "wall_ns": time.time_ns(),
                 "monotonic_ns": time.monotonic_ns(), "action": action, **values}
        self.events.append(event)
        with (self.directory / "events.jsonl").open("a") as output:
            output.write(json.dumps(event, sort_keys=True) + "\n")

    def write_json(self, name, value):
        (self.directory / name).write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")

    def start_gate(self):
        self.gate = CONTEXT.Process(target=gate_worker, args=(str(self.directory / "gate.sqlite"),
            self.admin_child, [pair[1] for pair in self.channels], self.actors))
        self.gate.start()
        self.record("gate_started", **receive(self.admin))

    def stop_gate(self):
        send(self.admin, {"operation": "stop"})
        assert receive(self.admin)["stopped"]
        self.gate.join(10)
        assert self.gate.exitcode == 0
        self.record("gate_stopped", pid=self.gate.pid, exit_code=self.gate.exitcode)
        self.gate = None

    def start(self, i):
        log = (self.directory / f"resident{i}.log").open("ab")
        self.logs.append(log)
        environment = dict(os.environ)
        environment["PODMESH_MANAGER_NETWORK_MODE"] = "authenticated-static-peers"
        self.children[i] = subprocess.Popen([str(RESIDENT), str(self.directory / f"r{i}.json")],
                                             stdout=log, stderr=log, env=environment)
        def ready():
            try:
                return control(self.directory / f"r{i}.sock")["activation_authority"] is False
            except (OSError, ValueError):
                assert self.children[i].poll() is None, "resident exited before ready"
                return False
        until(ready, 8)
        self.record("resident_started", replica=i, pid=self.children[i].pid)

    def observe(self, i, name, resource=None):
        result = ha_request(self.directory, i, {"operation": "observe", "operation_id": name,
            "scope": f"s{i}", "subject": name, "exclusive_resource": resource,
            "active_claim": resource is not None, "value": "fixture-fact"})
        self.record("local_fact", replica=i, event=result["fact"])
        return result

    def histories(self):
        results = []
        for i in range(3):
            with closing(sqlite3.connect(f"file:{self.directory}/r{i}.sqlite?mode=ro", uri=True)) as database:
                assert database.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
                rows = database.execute("SELECT event_id,fact_json,sha256 FROM facts ORDER BY event_id").fetchall()
                assert all(hashlib.sha256(row[1].encode()).hexdigest() == row[2] for row in rows)
                results.append(rows)
        return results

    def converged(self, count):
        def ready():
            histories = self.histories()
            return len(histories[0]) == count and histories[0] == histories[1] == histories[2]
        until(ready)
        self.record("external_convergence", history_count=count)

    def partition(self, blocked):
        for source, destination, proxy in self.proxies:
            if source == 2 or destination == 2:
                proxy.cut(blocked)
        self.record("proxy_partition", isolated_replica=2, blocked=blocked)

    def transfer(self, expected_epoch, owner):
        send(self.admin, {"operation": "transfer", "expected_epoch": expected_epoch, "owner": owner})
        result = receive(self.admin)
        self.record("explicit_fixture_transfer", expected_epoch=expected_epoch, owner=owner, outcome=result)
        assert result["ok"]
        return result["permit"]

    def attempt(self, i, permit, operation_id=None, **options):
        request = {"permit": permit, "operation_id": operation_id or str(uuid.uuid4()), "delta": 1, **options}
        parent, child = CONTEXT.Pipe()
        worker = CONTEXT.Process(target=maker_attempt, args=(str(self.directory), i, self.actors[i],
            self.authority_id, self.channels[i][0], self.channel_states[i], child, request))
        worker.start()
        try:
            worker.join(15)
            if worker.is_alive():
                raise TimeoutError("maker outcome unknown")
            if request.get("crash_after_gate"):
                assert worker.exitcode == 73
                result = {"ok": False, "unknown": True, "injected_exit": 73}
            else:
                assert worker.exitcode == 0
                result = receive(parent)
            self.record("maker_attempt", replica=i, pid=worker.pid, exit_code=worker.exitcode,
                        request=request, outcome=result)
            return result
        finally:
            if worker.is_alive():
                worker.kill()
                worker.join(5)
            parent.close()
            child.close()

    def effects(self):
        with closing(sqlite3.connect(f"file:{self.directory}/gate.sqlite?mode=ro", uri=True)) as database:
            database.row_factory = sqlite3.Row
            assert database.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
            return [dict(row) for row in database.execute(
                "SELECT effects.*,metadata.authority_id FROM effects CROSS JOIN metadata ORDER BY sequence")]

    def close(self):
        try:
            self.finish()
        finally:
            self.abort()

    def finish(self):
        for i, process in self.children.items():
            if process.poll() is None:
                try:
                    control(self.directory / f"r{i}.sock", "shutdown")
                    process.wait(12)
                except (OSError, ValueError, subprocess.TimeoutExpired):
                    process.kill()
                    process.wait(5)
            self.record("resident_reaped", replica=i, pid=process.pid, exit_code=process.returncode)
        if self.gate is not None:
            if self.gate.is_alive():
                self.stop_gate()
            else:
                self.gate.join(5)
                self.record("gate_unexpected_exit", exit_code=self.gate.exitcode)
        self.abort()
        self.write_json("external-effects.json", self.effects())
        stores = {}
        for name in [*(f"r{i}.sqlite" for i in range(3)), "gate.sqlite"]:
            with closing(sqlite3.connect(f"file:{self.directory}/{name}?mode=ro", uri=True)) as database:
                tables = [row[0] for row in database.execute(
                    "SELECT name FROM sqlite_master WHERE type='table'")]
                # These names come only from disposable locally created fixtures.
                stores[name] = {"schema": database.execute("PRAGMA user_version").fetchone()[0],
                                "integrity": database.execute("PRAGMA integrity_check").fetchone()[0],
                                "counts": {name: database.execute(f'SELECT COUNT(*) FROM "{name}"').fetchone()[0]
                                           for name in tables}}
        self.write_json("external-stores.json", stores)
        self.write_json("run.json", {"run_id": self.run_id, "actors": self.actors,
                                    "authority_id": self.authority_id,
                                    "source_commit": subprocess.check_output(
                                        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                                    "source_status": subprocess.check_output(
                                        ["git", "status", "--short"], cwd=ROOT, text=True),
                                    "python_version": sys.version,
                                    "platform": os.uname().sysname + " " + os.uname().release,
                                    "limitation": "Local fixture; no host, DNS, Podman, HA or zero-loss qualification"})
        # Hash closed SQLite databases and all run artifacts, including private configs.
        hashes = {str(path.relative_to(self.directory)): hashlib.sha256(path.read_bytes()).hexdigest()
                  for path in self.directory.iterdir() if path.is_file()}
        for path in (HA, RESIDENT, Path(__file__), ROOT / "experiments/manager-fencing/fencing_lab.py"):
            hashes[str(path.relative_to(ROOT))] = hashlib.sha256(path.read_bytes()).hexdigest()
        for experiment in ("manager-ha", "manager-network", "manager-resident", "manager-integrated"):
            for pattern in ("src/*.rs", "tests/*.rs", "Cargo.*", "*.py"):
                for path in (ROOT / "experiments" / experiment).glob(pattern):
                    hashes[str(path.relative_to(ROOT))] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.write_json("sha256.json", hashes)
