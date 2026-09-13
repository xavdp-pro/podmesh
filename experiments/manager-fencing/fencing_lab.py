"""Isolated effect-gate model. No network, Podman, DNS, or production authority."""

from __future__ import annotations

import contextlib
import dataclasses
import functools
import json
import os
from pathlib import Path
import re
import sqlite3
import uuid

MAX_RESOURCES = 64
MAX_EFFECTS = 2048
MAX_EPOCH = 2**31 - 1
MAX_COUNTER = 10**9
MAX_WIRE = 4096
GATE_APPLICATION_ID = 0x504D4647
MAKER_APPLICATION_ID = 0x504D464D
GATE_SCHEMA = 2
MAKER_SCHEMA = 3
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,95}\Z")


class Refused(Exception):
    """A typed laboratory refusal, with no implicit retry or takeover."""

    def __init__(self, message, *, code="refused"):
        super().__init__(message)
        self.code = code


def storage_boundary(function):
    """Public storage operations return bounded categories, never retry implicitly."""
    @functools.wraps(function)
    def guarded(*args, **kwargs):
        try:
            return function(*args, **kwargs)
        except (Refused, sqlite3.Error, OSError) as error:
            if function.__name__ == "__init__" and hasattr(args[0], "db"):
                with contextlib.suppress(sqlite3.Error):
                    args[0].db.close()
            if isinstance(error, Refused):
                raise
            number = getattr(error, "sqlite_errorcode", 0) & 255
            if number in (sqlite3.SQLITE_BUSY, sqlite3.SQLITE_LOCKED):
                code = "storage_busy"
            elif number in (sqlite3.SQLITE_ERROR, sqlite3.SQLITE_SCHEMA,
                            sqlite3.SQLITE_CORRUPT, sqlite3.SQLITE_NOTADB):
                code = "storage_schema"
            elif isinstance(error, OSError) or number in (
                sqlite3.SQLITE_IOERR, sqlite3.SQLITE_CANTOPEN, sqlite3.SQLITE_FULL, sqlite3.SQLITE_READONLY
            ):
                code = "storage_io"
            else:
                code = "storage_fault"
            raise Refused(code, code=code) from error
    return guarded


def identifier(value: str) -> str:
    if not isinstance(value, str) or not IDENTIFIER.fullmatch(value):
        raise Refused("invalid identifier")
    return value


def epoch(value: int) -> int:
    if type(value) is not int or not 0 <= value <= MAX_EPOCH:
        raise Refused("invalid epoch")
    return value


def no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Refused("duplicate field")
        result[key] = value
    return result


@dataclasses.dataclass(frozen=True)
class Permit:
    authority_id: str
    resource: str
    epoch: int
    replica_id: str
    instance_id: str
    grant_id: str

    def validate(self):
        for name in ("authority_id", "resource", "replica_id", "instance_id", "grant_id"):
            identifier(getattr(self, name))
        if epoch(self.epoch) == 0:
            raise Refused("zero activation epoch")
        return self

    def encode(self) -> bytes:
        self.validate()
        return json.dumps(dataclasses.asdict(self), sort_keys=True).encode()

    @classmethod
    def decode(cls, wire: bytes):
        if not isinstance(wire, bytes) or len(wire) > MAX_WIRE:
            raise Refused("invalid or oversized permit")
        try:
            data = json.loads(wire, object_pairs_hook=no_duplicates)
            if not isinstance(data, dict) or set(data) != {f.name for f in dataclasses.fields(cls)}:
                raise Refused("unknown or missing permit fields")
            return cls(**data).validate()
        except (ValueError, TypeError, UnicodeError, RecursionError) as error:
            raise Refused("malformed permit") from error


def connect(path: Path, *, create: bool, application_id: int, schema: int):
    path = Path(path).resolve()
    if create:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.close(descriptor)
    elif not path.is_file():
        raise Refused("missing durable store; explicit recovery required")
    connection = sqlite3.connect(path.as_uri() + "?mode=rw", uri=True, timeout=2)
    try:
        connection.row_factory = sqlite3.Row
        if not create and (
            connection.execute("PRAGMA application_id").fetchone()[0] != application_id or
            connection.execute("PRAGMA user_version").fetchone()[0] != schema
        ):
            raise Refused("unsupported store role or schema", code="storage_schema")
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute("PRAGMA synchronous=FULL")
        connection.execute("PRAGMA foreign_keys=ON")
    except BaseException:
        connection.close()
        raise
    return connection


@contextlib.contextmanager
def transaction(connection):
    connection.execute("BEGIN IMMEDIATE")
    try:
        yield
        connection.commit()
    except BaseException:
        connection.rollback()
        raise


class Authority:
    """Trusted single gate fixture, explicitly external to manager replica memory.

    Enrollment/transfer/revoke are privileged operator-fixture methods. This is
    not an authentication implementation or a distributed authority election.
    """

    @storage_boundary
    def __init__(self, path: Path, *, create=False):
        self.db = connect(path, create=create, application_id=GATE_APPLICATION_ID, schema=GATE_SCHEMA)
        if create:
            with self.db:
                self.db.executescript("""
                    CREATE TABLE metadata (authority_id TEXT NOT NULL);
                    CREATE TABLE resources (
                        resource TEXT PRIMARY KEY, epoch INTEGER NOT NULL,
                        replica_id TEXT, instance_id TEXT, grant_id TEXT,
                        counter INTEGER NOT NULL DEFAULT 0);
                    CREATE TABLE effects (
                        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                        resource TEXT NOT NULL REFERENCES resources(resource),
                        operation_id TEXT NOT NULL, epoch INTEGER NOT NULL,
                        replica_id TEXT NOT NULL, instance_id TEXT NOT NULL,
                        delta INTEGER NOT NULL, result INTEGER NOT NULL,
                        UNIQUE(resource, operation_id));
                """)
                self.db.execute(f"PRAGMA application_id={GATE_APPLICATION_ID}")
                self.db.execute(f"PRAGMA user_version={GATE_SCHEMA}")
                self.db.execute("INSERT INTO metadata VALUES (?)", (str(uuid.uuid4()),))
        rows = self.db.execute("SELECT authority_id FROM metadata").fetchall()
        if len(rows) != 1:
            raise Refused("invalid gate identity")
        self.authority_id = identifier(rows[0][0])

    @storage_boundary
    def close(self):
        self.db.close()

    @storage_boundary
    def declare(self, resource: str):
        identifier(resource)
        with transaction(self.db):
            if self.db.execute("SELECT 1 FROM resources WHERE resource=?", (resource,)).fetchone():
                raise Refused("resource already declared")
            if self.db.execute("SELECT COUNT(*) FROM resources").fetchone()[0] >= MAX_RESOURCES:
                raise Refused("resource quota")
            self.db.execute("INSERT INTO resources(resource,epoch) VALUES (?,0)", (resource,))

    def _row(self, resource):
        row = self.db.execute("SELECT * FROM resources WHERE resource=?", (resource,)).fetchone()
        if row is None:
            raise Refused("undeclared resource")
        return row

    @storage_boundary
    def transfer(self, resource: str, expected_epoch: int, replica_id: str, instance_id: str) -> Permit:
        """CAS rotation by the trusted fixture controller, never inferred from reachability."""
        for value in (resource, replica_id, instance_id):
            identifier(value)
        epoch(expected_epoch)
        with transaction(self.db):
            row = self._row(resource)
            if row["epoch"] != expected_epoch:
                raise Refused("stale transfer precondition")
            if expected_epoch == MAX_EPOCH:
                raise Refused("epoch exhausted")
            permit = Permit(self.authority_id, resource, expected_epoch + 1,
                            replica_id, instance_id, str(uuid.uuid4()))
            self.db.execute("UPDATE resources SET epoch=?,replica_id=?,instance_id=?,grant_id=? WHERE resource=?",
                            (permit.epoch, replica_id, instance_id, permit.grant_id, resource))
        return permit

    @storage_boundary
    def revoke(self, resource: str, expected_epoch: int):
        identifier(resource)
        epoch(expected_epoch)
        with transaction(self.db):
            row = self._row(resource)
            if row["epoch"] != expected_epoch or expected_epoch == MAX_EPOCH:
                raise Refused("invalid revoke precondition")
            self.db.execute("UPDATE resources SET epoch=?,replica_id=NULL,instance_id=NULL,grant_id=NULL WHERE resource=?",
                            (expected_epoch + 1, resource))

    @storage_boundary
    def effect(self, permit: Permit, actor: tuple[str, str], operation_id: str, delta: int) -> int:
        """The entire modeled effect is inside this transaction, not a later shell call.

        actor represents an authenticated channel principal supplied by the test
        fixture. A real endpoint must derive it from authentication, not JSON.
        """
        permit.validate()
        identifier(operation_id)
        if type(delta) is not int or not -1000 <= delta <= 1000:
            raise Refused("invalid delta")
        if actor != (permit.replica_id, permit.instance_id):
            raise Refused("actor does not match permit")
        with transaction(self.db):
            row = self._row(permit.resource)
            if (permit.authority_id != self.authority_id or
                (row["epoch"], row["replica_id"], row["instance_id"], row["grant_id"]) !=
                (permit.epoch, permit.replica_id, permit.instance_id, permit.grant_id)):
                raise Refused("revoked, stale, foreign, or fabricated permit")
            previous = self.db.execute("SELECT * FROM effects WHERE resource=? AND operation_id=?",
                                       (permit.resource, operation_id)).fetchone()
            if previous:
                if (previous["epoch"], previous["replica_id"], previous["instance_id"], previous["delta"]) != (
                    permit.epoch, *actor, delta):
                    raise Refused("incompatible operation reuse")
                return previous["result"]
            if self.db.execute("SELECT COUNT(*) FROM effects").fetchone()[0] >= MAX_EFFECTS:
                raise Refused("effect quota")
            result = row["counter"] + delta
            if abs(result) > MAX_COUNTER:
                raise Refused("counter bound")
            self.db.execute("INSERT INTO effects(resource,operation_id,epoch,replica_id,instance_id,delta,result) VALUES (?,?,?,?,?,?,?)",
                            (permit.resource, operation_id, permit.epoch, *actor, delta, result))
            self.db.execute("UPDATE resources SET counter=? WHERE resource=?", (result, permit.resource))
        return result

    @storage_boundary
    def inspect(self, resource: str):
        identifier(resource)
        return dict(self._row(resource))


class Maker:
    """Durable stale-epoch screen plus mandatory online gate on every effect."""

    @storage_boundary
    def __init__(self, path: Path, authority_id: str, replica_id: str, instance_id: str, *, create=False):
        for value in (authority_id, replica_id, instance_id):
            identifier(value)
        self.db = connect(path, create=create, application_id=MAKER_APPLICATION_ID, schema=MAKER_SCHEMA)
        self.authority_id, self.actor = authority_id, (replica_id, instance_id)
        if create:
            self.db.executescript("""
                CREATE TABLE binding(authority_id TEXT,replica_id TEXT,instance_id TEXT);
                CREATE TABLE seen(resource TEXT PRIMARY KEY, epoch INTEGER NOT NULL);
            """)
            self.db.execute(f"PRAGMA application_id={MAKER_APPLICATION_ID}")
            self.db.execute(f"PRAGMA user_version={MAKER_SCHEMA}")
            with self.db:
                self.db.execute("INSERT INTO binding VALUES (?,?,?)", (authority_id, *self.actor))
        rows = self.db.execute("SELECT * FROM binding").fetchall()
        if len(rows) != 1 or tuple(rows[0]) != (authority_id, *self.actor):
            raise Refused("maker identity mismatch")

    @storage_boundary
    def close(self):
        self.db.close()

    @storage_boundary
    def effect(self, gate: Authority | None, permit: Permit, operation_id: str, delta: int):
        permit.validate()
        if permit.authority_id != self.authority_id or (permit.replica_id, permit.instance_id) != self.actor:
            raise Refused("maker permit binding mismatch")
        if gate is None:
            raise Refused("gate unreachable; no new exclusive effect")
        # Hold this local transaction until the guarded result is known. A crash
        # after the gate commit can lose this cache update, but cannot replay the
        # gate effect: the gate's operation receipt survives independently.
        with transaction(self.db):
            seen = self.db.execute("SELECT epoch FROM seen WHERE resource=?", (permit.resource,)).fetchone()
            if seen and seen[0] > permit.epoch:
                raise Refused("maker rejects previously superseded epoch")
            if not seen and self.db.execute("SELECT COUNT(*) FROM seen").fetchone()[0] >= MAX_RESOURCES:
                raise Refused("maker resource quota")
            result = gate.effect(permit, self.actor, operation_id, delta)
            self.db.execute("INSERT INTO seen VALUES (?,?) ON CONFLICT(resource) DO UPDATE SET epoch=MAX(epoch,excluded.epoch)",
                            (permit.resource, permit.epoch))
            return result
