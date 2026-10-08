#!/usr/bin/env python3
"""Preserve a configured MariaDB store without exposing its password in argv.

The client writes a private partial file. Only a successful nonempty dump is
published at the requested path. This is capture, not restoration proof.
"""
import argparse
import json
import os
import stat
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit


class DumpFailure(ValueError):
    """A diagnostic constructed locally without credential-derived text."""


def client_parameters(profile_path):
    document = json.loads(profile_path.read_text())
    if not isinstance(document, dict):
        raise ValueError("profile must be an object")
    store = document.get("store", document)
    if not isinstance(store, dict):
        raise ValueError("store must be an object")
    if store.get("engine") not in ("mariadb", "mysql"):
        raise ValueError("profile must name MariaDB")
    config = store.get("mariadb", {})
    if not isinstance(config, dict):
        raise ValueError("MariaDB configuration must be an object")
    if "password" in config:
        raise ValueError("inline passwords are not supported; use password_file")
    if config.get("dsn"):
        dsn = urlsplit(config["dsn"])
        if dsn.scheme != "mysql":
            raise ValueError("DSN must use mysql://")
        user, password = unquote(dsn.username or ""), unquote(dsn.password or "")
        database = unquote(dsn.path.removeprefix("/"))
        host, port = dsn.hostname or "127.0.0.1", dsn.port or 3306
        socket = parse_qs(dsn.query).get("socket", [None])[0]
    else:
        user, database = config.get("user"), config.get("database")
        host, port = config.get("host", "127.0.0.1"), config.get("port", 3306)
        socket = config.get("socket")
        password_path = Path(config["password_file"])
        if not stat.S_ISREG(password_path.stat().st_mode) or password_path.stat().st_mode & 0o077:
            raise ValueError("password_file must be regular and private to its owner")
        password = password_path.read_text().rstrip("\r\n")
    if not all(isinstance(value, str) and value for value in (user, database, password)):
        raise ValueError("profile requires user, database and a nonempty credential")
    if not isinstance(port, int) or not 1 <= port <= 65535:
        raise ValueError("invalid MariaDB port")
    args = ["mariadb-dump", "--no-defaults", "--single-transaction", "--hex-blob", "--user=" + user]
    if socket:
        if not isinstance(socket, str) or not Path(socket).is_absolute():
            raise ValueError("socket must be an absolute path")
        args += ["--protocol=SOCKET", "--socket=" + socket]
    else:
        if not isinstance(host, str) or not host:
            raise ValueError("profile requires a host or socket")
        args += ["--protocol=TCP", "--host=" + host, "--port=" + str(port)]
    args += ["--", database]
    environment = os.environ.copy()
    environment["MYSQL_PWD"] = password
    return args, environment


def dump(profile, output, timeout):
    args, environment = client_parameters(profile)
    partial = None
    try:
        with tempfile.NamedTemporaryFile(prefix="." + output.name + ".", suffix=".part",
                                         dir=output.parent, delete=False) as stream:
            partial = Path(stream.name)
            # Client stderr can contain values derived from connection credentials.
            # Report its exit status without copying that private text into logs.
            result = subprocess.run(args, env=environment, stdout=stream, stderr=subprocess.PIPE,
                                    timeout=timeout, check=False)
            if result.returncode != 0:
                raise DumpFailure(f"dump client failed (exit {result.returncode}); final dump unchanged")
            stream.flush()
            os.fsync(stream.fileno())
            if stream.tell() == 0:
                raise DumpFailure("dump client produced an empty file; final dump unchanged")
        os.replace(partial, output)
        partial = None
        directory = os.open(output.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if partial is not None:
            partial.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    try:
        dump(args.profile, args.output, args.timeout)
    except DumpFailure as error:
        parser.exit(1, f"MariaDB {error}\n")
    except (ValueError, KeyError, TypeError, OSError, subprocess.TimeoutExpired):
        # Do not print exception strings: malformed URLs and client errors may
        # include credentials. The final artifact's absence is fail-closed.
        parser.exit(1, "MariaDB dump refused or failed; no partial dump was published\n")


if __name__ == "__main__":
    main()
