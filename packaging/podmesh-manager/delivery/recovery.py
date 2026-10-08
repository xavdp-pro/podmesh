"""Native same-host manager recovery; no authority, adoption or raw libpod edits."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import tarfile
import time
import uuid

MAX_BYTES = 8 * 1024 ** 3
MAX_FILES = 200000
MAX_SQL = 128 * 1024 ** 2
TABLES = ["exchange_audit_events", "facts", "identity", "receipts", "store_schema"]
DUMP = ("export MYSQL_PWD=\"$(cat /run/app-passwd)\"; exec /usr/bin/mariadb-dump "
        "--no-defaults --protocol=TCP --host=127.0.0.1 --port=3306 "
        "--user=podmesh-manager --single-transaction --no-tablespaces "
        "--order-by-primary --skip-extended-insert --skip-comments --skip-dump-date "
        "--hex-blob --routines --events --triggers podmesh-manager")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def require(okay, message):
    if not okay:
        raise ValueError(message)


def exact(obj, fields):
    require(isinstance(obj, dict) and set(obj) == set(fields), "unknown recovery fields")


def canonical_uuid(value):
    require(str(uuid.UUID(value)) == value, "canonical recovery UUID required")
    return value


def private(path):
    item = path.lstat()
    require(stat.S_ISREG(item.st_mode) and item.st_uid == 0
            and stat.S_IMODE(item.st_mode) == 0o600, "private root0600 input required")
    for parent in path.parents:
        s = parent.lstat()
        require(stat.S_ISDIR(s.st_mode) and s.st_uid == 0 and not s.st_mode & 0o022,
                "unsafe recovery input parent")


def document(path):
    private(path)
    require(path.stat().st_size <= MAX_SQL, "input exceeds recovery bound")
    return json.loads(path.read_text())


def extents(path, size):
    if not size:
        return []
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        rows, offset = [], 0
        while offset < size:
            try:
                start = os.lseek(fd, offset, os.SEEK_DATA)
            except OSError as error:
                if error.errno == 6:  # ENXIO: the remainder is a hole.
                    break
                raise ValueError("filesystem cannot prove sparse extents") from None
            end = min(os.lseek(fd, start, os.SEEK_HOLE), size)
            require(offset <= start < end, "invalid sparse extent")
            rows.append([start, end])
            offset = end
        return rows
    finally:
        os.close(fd)


def tree(root):
    """Physical tree, never follow symlinks; reject special files/external hardlinks."""
    require(root.is_dir() and not root.is_symlink(), "tree must be a real directory")
    rows, links, total = {}, {}, 0
    paths = [root]
    for directory, dirs, files in os.walk(root, followlinks=False):
        paths.extend(Path(directory) / name for name in sorted(dirs + files))
    require(len(paths) <= MAX_FILES, "tree member bound exceeded")
    for path in sorted(set(paths)):
        s = path.lstat()
        name = str(path.relative_to(root))
        row = {"mode": stat.S_IMODE(s.st_mode), "uid": s.st_uid, "gid": s.st_gid,
               "mtime_ns": s.st_mtime_ns,
               "xattrs": {key: base64.b64encode(os.getxattr(path, key, follow_symlinks=False)).decode()
                          for key in sorted(os.listxattr(path, follow_symlinks=False))}}
        if stat.S_ISLNK(s.st_mode):
            row.update(kind="symlink", link=os.readlink(path))
        elif stat.S_ISDIR(s.st_mode):
            row.update(kind="directory")
        else:
            require(stat.S_ISREG(s.st_mode), "unmapped special file in source tree")
            total += s.st_size
            require(total <= MAX_BYTES, "source tree byte bound exceeded")
            row.update(kind="file", size=s.st_size, sha256=digest(path), extents=extents(path, s.st_size))
            links.setdefault((s.st_dev, s.st_ino), []).append((name, s.st_nlink))
        rows[name] = row
    for members in links.values():
        require(len(members) == members[0][1], "hardlink leaves declared tree")
        anchor = min(name for name, _ in members)
        for name, _ in members:
            rows[name]["hardlink"] = anchor
    return rows


def validate_tree(rows):
    require(isinstance(rows, dict) and "." in rows and len(rows) <= MAX_FILES,
            "invalid tree manifest")
    total = 0
    anchors = {}
    for name, row in rows.items():
        if row.get("kind") == "file":
            group = row["hardlink"]
            anchors[group] = min(anchors.get(group, name), name)
    for name, row in rows.items():
        path = Path(name)
        require(not path.is_absolute() and ".." not in path.parts and str(path) == name,
                "unsafe manifest path")
        require(row["kind"] in ("directory", "file", "symlink"), "unknown tree member kind")
        for parent in path.parents:
            key = str(parent)
            require(key in rows and rows[key]["kind"] == "directory", "manifest parent missing or traverses a symlink/file")
        if row["kind"] == "file":
            total += row["size"]
            require(0 <= row["size"] <= MAX_BYTES and total <= MAX_BYTES, "manifest byte bound exceeded")
            anchor = rows.get(row["hardlink"])
            require(anchor is not None and anchor["kind"] == "file" and anchor == row
                    and row["hardlink"] == anchors[row["hardlink"]],
                    "invalid hardlink identity")
            end = 0
            for start, stop in row["extents"]:
                require(end <= start < stop <= row["size"], "invalid extent manifest")
                end = stop
    return total


def metadata(path, row):
    os.chown(path, row["uid"], row["gid"], follow_symlinks=False)
    if row["kind"] != "symlink":
        os.chmod(path, row["mode"])
    for key in os.listxattr(path, follow_symlinks=False):
        if key not in row["xattrs"]:
            os.removexattr(path, key, follow_symlinks=False)
    for key, value in row["xattrs"].items():
        os.setxattr(path, key, base64.b64decode(value, validate=True), follow_symlinks=False)
    os.utime(path, ns=(row["mtime_ns"], row["mtime_ns"]), follow_symlinks=False)


def materialize(rows, destination, content):
    """Restore manifest-defined members without archive-controlled extraction."""
    validate_tree(rows)
    require(destination.is_dir() and not destination.is_symlink() and not any(destination.iterdir()),
            "restoration destination must be an empty directory")
    for name, row in sorted(rows.items(), key=lambda pair: (len(Path(pair[0]).parts), pair[0])):
        if name == ".":
            continue
        path = destination / name
        if row["kind"] == "directory":
            path.mkdir(mode=0o700)
        elif row["kind"] == "symlink":
            path.symlink_to(row["link"])
        elif row["hardlink"] == name:
            with content(name) as stream:
                fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(fd, "wb") as output:
                    for start, end in row["extents"]:
                        stream.seek(start)
                        output.seek(start)
                        remaining = end - start
                        while remaining:
                            block = stream.read(min(1024 ** 2, remaining))
                            require(block, "truncated restored content")
                            output.write(block)
                            remaining -= len(block)
                    output.truncate(row["size"])
                    output.flush()
                    os.fsync(output.fileno())
            require(digest(path) == row["sha256"], "restored file content differs")
    for name, row in sorted(rows.items()):
        if row["kind"] == "file" and row["hardlink"] != name:
            os.link(destination / row["hardlink"], destination / name, follow_symlinks=False)
    for name, row in sorted(rows.items(), key=lambda pair: len(Path(pair[0]).parts), reverse=True):
        metadata(destination / name, row)
        if row["kind"] != "symlink":
            fd = os.open(destination / name, os.O_RDONLY | os.O_NOFOLLOW)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
    require(tree(destination) == rows, "restored tree metadata/content differs")


def restore_archive(archive_path, rows, source_name, destination):
    validate_tree(rows)
    with tarfile.open(archive_path, "r:") as archive:
        members = {}
        for member in archive:
            path = Path(member.name)
            require(not path.is_absolute() and ".." not in path.parts and path.parts[0] == source_name,
                    "archive leaves declared source root")
            name = str(Path(*path.parts[1:]))
            require(name not in members and name in rows, "extra/duplicate archive member")
            row = rows[name]
            require((member.isdir() and row["kind"] == "directory")
                    or (member.issym() and row["kind"] == "symlink" and member.linkname == row["link"])
                    or ((member.isfile() or member.islnk()) and row["kind"] == "file"),
                    "archive member kind differs")
            if member.isfile():
                require(member.size == row["size"], "archive file size differs")
            if member.islnk():
                link = Path(member.linkname)
                require(not link.is_absolute() and ".." not in link.parts and link.parts[0] == source_name,
                        "archive hardlink leaves source root")
                target = rows.get(str(Path(*link.parts[1:])))
                require(target is not None and target["kind"] == "file"
                        and target["hardlink"] == row["hardlink"], "archive hardlink group differs")
            members[name] = member
        require(set(members) == set(rows), "incomplete full archive")
        materialize(rows, destination, lambda name: archive.extractfile(members[name]))


def subtree(rows, prefix):
    prefix = prefix.rstrip("/")
    result = {}
    for name, row in rows.items():
        if name == prefix or name.startswith(prefix + "/"):
            key = "." if name == prefix else name[len(prefix) + 1:]
            value = dict(row)
            if value["kind"] == "file":
                anchor = value["hardlink"]
                require(anchor == prefix or anchor.startswith(prefix + "/"),
                        "durable volume hardlink crosses component boundary")
                value["hardlink"] = "." if anchor == prefix else anchor[len(prefix) + 1:]
            if value["kind"] == "symlink":
                target = Path(value["link"])
                require(not target.is_absolute(), "durable volume has external absolute symlink")
                parts = list(Path(key).parent.parts)
                for part in target.parts:
                    if part == "..":
                        require(parts and parts[-1] != ".", "durable volume symlink escapes")
                        parts.pop()
                    elif part != ".":
                        parts.append(part)
            result[key] = value
    validate_tree(result)
    return result


def query(instance, statement):
    # Statements are fixed product SQL or use canonical UUID literals only.
    import shlex
    script = ('export MYSQL_PWD="$(cat /run/app-passwd)"; exec /usr/bin/mariadb '
              '--no-defaults --protocol=TCP --host=127.0.0.1 --port=3306 '
              '--user=podmesh-manager --database=podmesh-manager --batch --skip-column-names '
              '--execute=' + shlex.quote(statement))
    response = instance.podman("exec", "--user=1103:1103", instance.r["db"],
                               "/bin/sh", "-eu", "-c", script, timeout=120)
    require(len(response.stdout) <= MAX_SQL, "SQL response exceeds bound")
    return response.stdout.decode()


def oracle(instance, empty=False):
    identity = query(instance, "SELECT CURRENT_USER(),DATABASE(),@@hostname;").strip().split("\t")
    require(identity == ["podmesh-manager@%", "podmesh-manager", instance.prefix + "-db"],
            "private SQL identity differs")
    tables = query(instance, "SHOW TABLES;").splitlines()
    triggers = int(query(instance, "SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=DATABASE();"))
    require(tables == ([] if empty else TABLES) and triggers == (0 if empty else 8),
            "unexpected or incomplete manager schema")
    require(query(instance, "SELECT COUNT(*) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=DATABASE();"
                  "SELECT COUNT(*) FROM information_schema.EVENTS WHERE EVENT_SCHEMA=DATABASE();").splitlines() == ["0", "0"],
            "unmapped routine/event state")
    if not empty:
        # DurableStore migration1 implements legacy manager history schema3.
        require(query(instance, "SELECT version FROM store_schema WHERE name='manager';").strip() == "1",
                "manager DurableStore migration version differs")
        require(query(instance, "SELECT replica_id FROM identity WHERE singleton=1;").strip() == instance.r["replica_id"],
                "SQL replica identity differs")
    result = {"identity": identity, "tables": tables, "triggers": triggers,
              "grants": query(instance, "SHOW GRANTS FOR CURRENT_USER;").splitlines()}
    if not empty:
        result["historical_uncertainty"] = uncertainty_inventory(inspect_quiescent_store(instance))
    return result


def inspect_quiescent_store(instance):
    """Reuse DurableStore's full read-only validation, never a second phase machine."""
    state = instance.check_container("app")["State"]
    require(not state["Running"] and state["Pid"] == 0 and state["ExitCode"] == 0
            and not state.get("OOMKilled", False) and not (instance.root / "api/control.sock").exists(),
            "canonical recovery inspection requires quiescent clean APP")
    name = instance.prefix + "-inspection"
    require(instance.inspect("container", name) is None, "inspection resource already exists; retain it")
    result = instance.podman("run", "--rm", "--name", name,
        "--label", "io.podmesh.private-instance=" + instance.scope,
        "--label", "io.podmesh.bundle=" + instance.r["bundle"],
        "--pod", instance.r["pod"], "--pull=never", "--image-volume=ignore",
        "--user=1103:1103", "--cap-drop=ALL", "--security-opt=no-new-privileges",
        "--pid=private", "--ipc=private", "--uts=private", "--read-only", "--read-only-tmpfs=false",
        "--memory=512m", "--memory-swap=512m", "--cpus=1",
        "--volume", instance.prefix + "-app:/var/lib/podmesh-manager:ro",
        "--volume", str(instance.root / "app-config") + ":/etc/podmesh-manager:ro",
        "--env=PODMESH_STORE_PROFILE=/etc/podmesh-manager/store.json",
        "--entrypoint=/usr/lib/podmesh-manager/podmesh-managerd", instance.manifest["application_image"],
        "--config", "/etc/podmesh-manager/config.json", "--state-dir", "/var/lib/podmesh-manager",
        "--inspect-store", timeout=120)
    require(instance.inspect("container", name) is None, "inspection resource not removed; retain state")
    require(0 < len(result.stdout) <= MAX_SQL, "canonical inspection exceeds bound")
    inspection = json.loads(result.stdout)
    require(inspection["replica_id"] == instance.r["replica_id"], "canonical replica identity differs")
    return inspection


def uncertainty_inventory(inspection):
    """The canonical inspector has already validated every row, link and prefix."""
    attempts = inspection["incomplete_attempts"]
    keys = {(item["direction"], item["attempt_id"]) for item in attempts}
    require(len(keys) == len(attempts), "duplicate canonical incomplete attempt")
    events = [row for row in inspection["ordered_audit_events"]
              if (row["event"]["direction"], row["event"]["attempt_id"]) in keys]
    require({(row["event"]["direction"], row["event"]["attempt_id"]) for row in events} == keys,
            "canonical incomplete attempt lacks audit evidence")
    return {"type": "manager-historical-uncertainty/v1",
            "attempts": sorted(attempts, key=lambda item: (item["direction"], item["attempt_id"])),
            "audit_events": sorted(events, key=lambda row: row["event"]["audit_event_id"])}


def same_uncertainty(captured, observed):
    require(captured == observed, "historical uncertainty lost, altered or added")


def immutable_rows(instance):
    columns = {
        "identity": "singleton,replica_id,topology_json",
        "facts": "event_id,fact_json,sha256",
        "receipts": "operation_id,kind,source_replica_id,wire_operation_id,request_json,response_json,sha256",
        "exchange_audit_events": "audit_event_id,attempt_id,wire_nonce,direction,phase,authenticated_peer_id,"
            "peer_claim,operation_id,request_frame_bytes,request_announced_body_bytes,request_sha256,"
            "reply_frame_bytes,reply_announced_body_bytes,reply_sha256,outcome,error_category,reason_code,"
            "local_receipt_operation_id,local_receipt_sha256,remote_receipt_operation_id,"
            "remote_receipt_sha256,replayed,record_json,sha256",
        "store_schema": "name,version,applied_at"}
    return {table: sorted(query(instance, "SELECT HEX(JSON_ARRAY(" + fields + ")) FROM " + table + ";").splitlines())
            for table, fields in columns.items()}


def preserved_rows(before, after):
    require(set(before) == set(after) == set(TABLES), "history component inventory differs")
    for table in TABLES:
        require(len(before[table]) == len(set(before[table])) and set(before[table]) <= set(after[table]),
                "source immutable history lost or rewritten")
    require(before["identity"] == after["identity"] and before["store_schema"] == after["store_schema"],
            "identity/schema provenance changed")


def absent_operation(snapshot, operation):
    require(all(json.loads(bytes.fromhex(row))[0] != operation for row in snapshot["receipts"]),
            "nominal operation is already present in captured history")


def trigger_rows(instance):
    return sorted(query(instance,
        "SELECT HEX(JSON_ARRAY(TRIGGER_NAME,ACTION_TIMING,EVENT_MANIPULATION,EVENT_OBJECT_TABLE,"
        "ACTION_STATEMENT,DEFINER,SQL_MODE,CHARACTER_SET_CLIENT,COLLATION_CONNECTION,DATABASE_COLLATION)) "
        "FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=DATABASE();").splitlines())


def peer_complete(status, peers, history_count=None, prior=None):
    catch = status.get("catch_up", {})
    require(catch.get("caught_up") is True and catch.get("caught_up_by") == "every_peer"
            and set(catch.get("peers_imported", [])) | set(catch.get("peers_matched", [])) == peers
            and not any(catch.get(key) for key in ("peers_missing", "peers_ahead", "peers_not_attempted")),
            "every-peer catchup required; window fallback is not restoration proof")
    require(len(peers) == 2 and set(status.get("peers", {})) == peers, "exact two peers required")
    for name in peers:
        peer = status["peers"][name]
        require(peer["authenticated_successes"] > 0 and peer.get("last_success_age_ms") is not None
                and peer["last_success_age_ms"] <= 30000, "fresh authenticated peer evidence required")
        if history_count is not None:
            require(peer["authenticated_successes"] > prior[name] and peer.get("acknowledged_unchanged") is True
                    and peer.get("outcome") == "authenticated_import_receipt"
                    and peer.get("acknowledged_history_len") == history_count
                    and peer.get("local_history_len_at_attempt") == history_count
                    and peer.get("history_count_delta") == 0, "peer has not acknowledged the fresh complete history")


def audit_events(instance):
    return [json.loads(bytes.fromhex(row)) for row in
            query(instance, "SELECT HEX(record_json) FROM exchange_audit_events;").splitlines()]


def linked_peer_receipts(events, before, peers):
    requests = {event["attempt_id"]: event for event in events
                if event["phase"] == "outbound_request_prepared"}
    matched = {}
    for event in events:
        peer = event.get("authenticated_peer_id")
        if (event["audit_event_id"] in before or event["phase"] != "outbound_exchange_completed"
                or event["outcome"] != "accepted" or peer not in peers):
            continue
        prepared = requests.get(event["attempt_id"])
        require(prepared is not None and prepared["direction"] == event["direction"] == "outbound"
                and prepared["wire_nonce"] == event["wire_nonce"]
                and prepared["request_sha256"] == event["request_sha256"]
                and event.get("remote_receipt_operation_id") and event.get("remote_receipt_sha256"),
                "fresh authenticated completion lacks request/remote-receipt linkage")
        matched[peer] = {key: event[key] for key in ("audit_event_id", "attempt_id", "request_sha256",
                                                   "remote_receipt_operation_id", "remote_receipt_sha256")}
    require(set(matched) == peers, "both peers require new linked authenticated receipts")
    return matched


def sql_dump(instance):
    result = instance.podman("exec", "--user=1103:1103", instance.r["db"],
                             "/bin/sh", "-eu", "-c", DUMP, timeout=120)
    require(0 < len(result.stdout) <= MAX_SQL and b"CREATE TABLE" in result.stdout,
            "bounded complete SQL dump required")
    return result.stdout


def safe_diff(rows, role):
    require(isinstance(rows, list), "writable layer inventory unavailable")
    if role in ("app", "infra"):
        require(not rows, "unmapped durable application/infra writable layer")
    else:
        for row in rows:
            path = row.get("Path")
            require(isinstance(path, str) and Path(path).is_absolute() and ".." not in Path(path).parts,
                    "invalid writable layer path")
            require(any(path == prefix or path.startswith(prefix + "/")
                        for prefix in ("/run", "/tmp", "/var/run", "/var/tmp")),
                    "unmapped database writable layer; no SQL-only fallback")
    return rows


def observation_scope(config, replica, previous):
    scope = previous["request"]["scope"]
    grants = config["network"]["manager"]["grants"]
    require(any(grant["scope"] == scope and grant["owner_replica_id"] == replica for grant in grants),
            "captured observation scope is not owned by original replica")
    return scope


def closed_inputs(root, stopped=False):
    """Configuration is data, not unspecified engine bookkeeping."""
    directories = {"app-config", "api", "db-admin", "r", "tmp", "networks", "graphroot"}
    files = {"storage.conf", "containers.conf", "network-plan.json", "instance.json", "operator.lock",
             "ready-app.json", "shutdown-app.json"}
    require({p.name for p in root.iterdir()} <= directories | files, "unmapped durable instance-root component")
    for path in root.iterdir():
        require((path.name in directories and stat.S_ISDIR(path.lstat().st_mode))
                or (path.name in files and stat.S_ISREG(path.lstat().st_mode)), "instance component type differs")
    for name, members in (("app-config", {"config.json", "store.json", "passwd"}),
                          ("db-admin", {"passwd"})):
        path = root / name
        require(path.is_dir() and not path.is_symlink()
                and {p.name for p in path.iterdir()} == members, "unmapped private configuration component")
        require(all(stat.S_ISREG((path / member).lstat().st_mode) for member in members),
                "configuration component is not a regular file")
    api = root / "api"
    require(api.is_dir() and not api.is_symlink(), "API directory differs")
    require({p.name for p in api.iterdir()} <= ({"control.sock"} if not stopped else set()),
            "unmapped API component or stopped control socket remains")
    if stopped:
        for name in ("r", "tmp"):
            require((root / name).is_dir() and not (root / name).is_symlink(), "engine transient directory differs")


def closed_runtime(root, containers, version="synthetic-test-only"):
    """Closed stopped layouts: empty legacy paths or ID-bound 5.4.2 records."""
    runroot = root / "r"
    require({p.name for p in runroot.iterdir()} <= {"networks", "vfs-containers", "vfs-layers", "vfs-locks"},
            "unclassified stopped engine transient component")
    for name in ("networks", "vfs-locks"):
        path = runroot / name
        if path.exists():
            require(path.is_dir() and not path.is_symlink() and not any(path.iterdir()), "unmapped engine transient content")
    directory = runroot / "vfs-containers"
    if directory.exists():
        require(directory.is_dir() and not directory.is_symlink()
                and {p.name for p in directory.iterdir()} <= containers, "unmapped runroot container")
        for path in directory.iterdir():
            require(path.is_dir() and not path.is_symlink()
                    and {p.name for p in path.iterdir()} <= ({"userdata", "healthcheck.log"} if version == "5.4.2" else {"userdata"}),
                    "unmapped runroot container component")
            health = path / "healthcheck.log"
            if health.exists():
                bounded_engine_file(health)
            data = path / "userdata"
            if data.exists():
                require(data.is_dir() and not data.is_symlink(), "runroot userdata is not a directory")
                members = {".containerenv", "conmon.pid", "hostname", "hosts", "resolv.conf", "oci-log", "pidfile"} if version == "5.4.2" else set()
                require({p.name for p in data.iterdir()} <= members, "unmapped runroot userdata")
                for file in data.iterdir():
                    bounded_engine_file(file)
    directory = runroot / "vfs-layers"
    if directory.exists():
        require(directory.is_dir() and not directory.is_symlink()
                and {p.name for p in directory.iterdir()} <= {"mountpoints.json", "mountpoints.lock"}, "unmapped runtime layers")
        for path in directory.iterdir():
            require(stat.S_ISREG(path.lstat().st_mode), "runtime layer metadata differs")
            if path.name == "mountpoints.json":
                require(json.loads(path.read_text()) == [], "live/unmapped layer mount remains")
    directory = root / "tmp"
    allowed = {"alive", "alive.lck", "exits"} | ({"persist"} if version == "5.4.2" else set())
    require({p.name for p in directory.iterdir()} <= allowed, "unmapped engine tmp content")
    for path in directory.iterdir():
        if path.name in ("exits", "persist"):
            require(path.is_dir() and not path.is_symlink() and not any(path.iterdir()), "unmapped exit records")
        else:
            require(stat.S_ISREG(path.lstat().st_mode) and path.stat().st_size == 0, "engine liveness metadata differs")


def engine_version(instance):
    line = instance.podman("--version").stdout.decode().strip()
    require(line.startswith("podman version ") and len(line.split()) == 3,
            "engine version unavailable")
    version = line.split()[2]
    require(len(version) <= 64 and all(c.isascii() and (c.isalnum() or c in ".-+") for c in version),
            "engine version provenance malformed")
    return version


def engine_backend(instance):
    backend = json.loads(instance.podman("info", "--format=json").stdout)["host"]["databaseBackend"]
    require(backend in ("boltdb", "sqlite"), "unclassified engine database backend")
    return backend


def bounded_engine_file(path):
    item = path.lstat()
    require(stat.S_ISREG(item.st_mode) and item.st_uid == item.st_gid == 0 and item.st_nlink == 1
            and not stat.S_IMODE(item.st_mode) & 0o022 and item.st_size <= 8 * 1024 ** 2,
            "unclassified engine metadata file")


def closed_oci_config(path, root, layer, containers, volumes):
    bounded_engine_file(path)
    config = json.loads(path.read_text())
    exact(config, ("ociVersion", "process", "root", "hostname", "mounts", "annotations", "linux"))
    graph = root / "graphroot"
    require(config["ociVersion"] == "1.2.0" and config["root"]["path"] == str(graph / "vfs/dir" / layer)
            and config["annotations"].get("io.container.manager") == "libpod"
            and config["annotations"].get("io.kubernetes.cri-o.SandboxID", next(iter(containers))) in containers,
            "OCI bookkeeping layer or sandbox identity differs")
    sources = {str(root / name) for name in ("app-config", "app-config/passwd", "db-admin", "api")}
    sources |= {str(graph / "volumes" / name / "_data") for name in volumes}
    sources |= {str(graph / "vfs-containers" / identifier / "userdata/shm") for identifier in containers}
    sources |= {str(root / "r/vfs-containers" / identifier / "userdata" / name)
                for identifier in containers for name in ("hostname", "hosts", "resolv.conf", ".containerenv")}
    require(all(mount.get("source") in sources if mount["type"] == "bind"
                else mount["type"] in ("sysfs", "tmpfs", "proc", "devpts", "mqueue", "cgroup")
                for mount in config["mounts"]), "unmapped OCI bookkeeping mount")


def closed_sqlite_engine(root, images, containers, volumes, pod_id, infra_id):
    """Podman 5.4.2 engine bookkeeping only; never the application store.

    The schema fingerprint covers all twelve CREATE statements and constraints
    from upstream libpod/sqlite_state_internal.go at v5.4.2. No SQL is rewritten.
    """
    path = root / "graphroot/db.sql"
    item = path.lstat()
    require(stat.S_ISREG(item.st_mode) and item.st_uid == item.st_gid == 0
            and stat.S_IMODE(item.st_mode) == 0o644 and item.st_nlink == 1
            and 0 < item.st_size <= MAX_SQL, "engine SQLite file metadata differs")
    before = digest(path)
    require(pod_id and pod_id not in containers, "observed engine pod identity required")
    require(infra_id in containers, "observed engine infra container identity required")
    # The observed rollback-journal layout has no durable sidecars. Never ignore
    # a WAL, SHM or journal, or use immutable=1 to pretend one does not exist.
    require(not any(path.with_name(path.name + suffix).exists() for suffix in ("-wal", "-shm", "-journal")),
            "unclassified engine SQLite sidecar")
    connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=5)
    try:
        connection.execute("PRAGMA query_only=ON")
        connection.execute("PRAGMA trusted_schema=OFF")
        connection.execute("BEGIN")
        require(connection.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
                and not connection.execute("PRAGMA foreign_key_check").fetchall(), "engine SQLite integrity differs")
        objects = connection.execute("SELECT type,name,sql FROM sqlite_master ORDER BY name").fetchall()
        require(all(kind == "table" or (kind == "index" and sql is None and name.startswith("sqlite_autoindex_"))
                    for kind, name, sql in objects), "unmapped engine SQLite schema object")
        schema = {name: " ".join(sql.split()) for kind, name, sql in objects if kind == "table"}
        schema_sha = hashlib.sha256(json.dumps(schema, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        require(schema_sha == "89b38ebd067626a261f22395fa3ca950111642980311989de3016594f53df7c3",
                "unclassified Podman SQLite schema")
        counts = {name: connection.execute('SELECT COUNT(*) FROM "' + name + '"').fetchone()[0] for name in schema}
        require(all(count <= 128 for count in counts.values()) and counts["ContainerExecSession"] == 0,
                "unmapped engine session or bookkeeping count")
        graph = root / "graphroot"
        require(connection.execute("SELECT * FROM DBConfig").fetchall() == [
            (1, 1, "linux", str(graph / "libpod"), str(root / "tmp"), str(graph), str(root / "r"), "vfs", str(graph / "volumes"))],
            "engine SQLite configuration does not bind the private root")
        for table, field, expected in (("IDNamespace", "ID", containers | {pod_id}),
                ("ContainerConfig", "ID", containers), ("ContainerState", "ID", containers),
                ("PodConfig", "ID", {pod_id}), ("PodState", "ID", {pod_id}),
                ("VolumeConfig", "Name", volumes), ("VolumeState", "Name", volumes)):
            require({row[0] for row in connection.execute('SELECT "' + field + '" FROM "' + table + '"')} == expected,
                    "unmapped engine SQLite resource identity")
        infra_containers = set()
        for identifier, name, pod, encoded in connection.execute("SELECT ID,Name,PodID,JSON FROM ContainerConfig"):
            config = json.loads(encoded)
            require(config["id"] == identifier and config["name"] == name and config.get("pod") == pod == pod_id
                    and config.get("rootfsImageID") in images and not config.get("secrets") and not config.get("secret_env"),
                    "engine container configuration or secret dependency differs")
            if config.get("pause") is True:
                infra_containers.add(identifier)
        require(infra_containers == {infra_id}, "engine infra container role differs")
        for identifier, name, encoded in connection.execute("SELECT ID,Name,JSON FROM PodConfig"):
            config = json.loads(encoded)
            require(config["id"] == identifier and config["name"] == name, "engine pod JSON identity differs")
        for name, storage_id, encoded in connection.execute("SELECT Name,StorageID,JSON FROM VolumeConfig"):
            config = json.loads(encoded)
            require(storage_id is None and config["name"] == name
                    and config.get("volumeDriver") in ("", "local")
                    and config.get("mountPoint") == str(graph / "volumes" / name / "_data"),
                    "engine volume JSON binding differs")
        # SavePod in Podman 5.4.2 updates JSON, not the nullable SQL infra
        # column populated at AddPod time. Bind JSON to the observed infra and
        # its unique IsInfra (pause) role; never fill or repair the SQL column.
        for identifier, column_infra, encoded in connection.execute("SELECT ID,InfraContainerID,JSON FROM PodState"):
            state = json.loads(encoded)
            require(identifier == pod_id and state.get("InfraContainerID") == infra_id
                    and column_infra in (None, infra_id), "engine infra container identity differs")
        require(all(a in containers and b in containers for a, b in connection.execute("SELECT ID,DependencyID FROM ContainerDependency"))
                and all(a in containers and b in volumes for a, b in connection.execute("SELECT ContainerID,VolumeName FROM ContainerVolume")),
                "engine dependency/volume binding differs")
        # Podman retains exit diagnostics for removed --rm inspectors until its
        # normal pruning. They are not live resources; preserve them as history.
        exits = connection.execute("SELECT ID,Timestamp,ExitCode FROM ContainerExitCode ORDER BY ID").fetchall()
        require(all(isinstance(identifier, str) and len(identifier) == 64
                    and all(c in "0123456789abcdef" for c in identifier)
                    and isinstance(timestamp, int) and timestamp >= 0 and isinstance(code, int) and -1 <= code <= 255
                    for identifier, timestamp, code in exits), "engine exit diagnostics differ")
    finally:
        connection.close()
    require(digest(path) == before, "engine SQLite changed during read-only classification")
    return {"path": "db.sql", "sha256": before, "bytes": item.st_size, "schema_sha256": schema_sha,
            "row_counts": counts, "infra_container_id": infra_id, "exit_record_ids": [row[0] for row in exits]}


def closed_graphroot(root, images, containers, volumes, version="synthetic-test-only", pod_id=None, database_backend="boltdb", infra_id=None):
    """Bounded containers/storage VFS layout; unknown versions/layouts refuse.

    This classifies every path, including children. JSON metadata must refer only
    to the observed images, three containers, and their complete parent layers.
    No arbitrary file is accepted merely because it lives under graphroot.
    """
    require(database_backend in ("boltdb", "sqlite"), "unclassified engine database backend")
    graph = root / "graphroot"
    require(graph.is_dir() and not graph.is_symlink(), "private VFS graphroot missing")
    allowed = {"vfs", "vfs-images", "vfs-layers", "vfs-containers", "volumes", "libpod",
               "storage.lock", "userns.lock", "defaultNetworkBackend", "mounts", "tmp", "secrets"}
    if database_backend == "sqlite":
        require(version == "5.4.2", "unclassified SQLite engine version")
        allowed.add("db.sql")
    require({p.name for p in graph.iterdir()} <= allowed, "unmapped graphroot component")
    metadata_rows = {}
    volatile_metadata = []
    for kind, expected in (("images", images), ("containers", containers), ("layers", None)):
        directory = graph / ("vfs-" + kind)
        require(directory.is_dir() and not directory.is_symlink(), "VFS metadata directory missing")
        path = directory / (kind + ".json")
        require(stat.S_ISREG(path.lstat().st_mode), "VFS metadata is not regular")
        rows = json.loads(path.read_text())
        require(isinstance(rows, list) and all(isinstance(row, dict) for row in rows), "VFS metadata shape differs")
        fields = {"images": {"id", "layer", "created", "digest", "metadata", "names", "names-history",
                              "big-data-names", "big-data-sizes", "big-data-digests"},
                  "containers": {"id", "image", "layer", "created", "flags", "metadata", "names"},
                  "layers": {"id", "parent", "created", "names", "compressed-diff-digest", "compressed-size",
                             "compression", "diff-digest", "diff-size", "gidset", "uidset"}}[kind]
        require(all(set(row) <= fields for row in rows), "unknown VFS metadata schema field")
        ids = {row["id"] for row in rows}
        require(len(ids) == len(rows) and all(isinstance(value, str) and len(value) == 64
                    and all(c in "0123456789abcdef" for c in value) for value in ids), "VFS metadata identity differs")
        if expected is not None:
            require(ids == expected, "unmapped VFS resource identity")
        known = {kind + ".json", kind + ".lock"}
        if version == "5.4.2" and kind in ("containers", "layers"):
            volatile_name = "volatile-" + kind + ".json"
            known.add(volatile_name)
            volatile = directory / volatile_name
            if volatile.exists() or volatile.is_symlink():
                item = volatile.lstat()
                require(stat.S_ISREG(item.st_mode) and item.st_uid == item.st_gid == 0
                        and stat.S_IMODE(item.st_mode) == 0o600 and item.st_nlink == 1
                        and item.st_size == 2 and volatile.read_bytes() == b"[]",
                        "unclassified volatile VFS state")
                volatile_metadata.append({"path": str(volatile.relative_to(graph)), "bytes": 2,
                    "sha256": digest(volatile), "state": "empty-array"})
        if kind in ("images", "containers"):
            known |= ids
        else:
            known |= {row["id"] + ".tar-split.gz" for row in rows if row.get("diff-digest")}
        require({p.name for p in directory.iterdir()} <= known, "unmapped VFS metadata file")
        metadata_rows[kind] = {row["id"]: row for row in rows}
    bigdata = []
    for image, row in metadata_rows["images"].items():
        directory = graph / "vfs-images" / image
        names = row.get("big-data-names", [])
        sizes, digests = row.get("big-data-sizes", {}), row.get("big-data-digests", {})
        if row.get("metadata"):
            signature_metadata = json.loads(row["metadata"])
            unsigned_native = (version == "5.4.2" and signature_metadata == {}
                and set(names) == {"manifest", "manifest-" + row["digest"], "sha256:" + image})
            require(unsigned_native or signature_metadata == {"signatures-sizes": {row["digest"]: []}},
                    "unmapped image metadata/signature state")
        require(set(names) == set(sizes) == set(digests), "image big-data metadata incomplete")
        require(all(name == "manifest" or name == "sha256:" + image
                    or name == "manifest-" + row.get("digest", "")
                    or name == "signature-" + row.get("digest", "").removeprefix("sha256:")
                    for name in names), "unknown durable image big-data")
        expected_files = {name if name == "manifest" else "=" + base64.b64encode(name.encode()).decode() for name in names}
        require(directory.is_dir() and not directory.is_symlink()
                and {p.name for p in directory.iterdir()} == expected_files, "image big-data inventory differs")
        for name in names:
            file = directory / (name if name == "manifest" else "=" + base64.b64encode(name.encode()).decode())
            require(stat.S_ISREG(file.lstat().st_mode) and file.stat().st_size == sizes[name]
                    and "sha256:" + digest(file) == digests[name], "image big-data content differs")
            if name.startswith("signature-"):
                require(sizes[name] == 0, "nonempty signature state needs explicit preservation contract")
            bigdata.append({"image_id": image, "name": name, "sha256": digest(file), "bytes": sizes[name]})
    for container, row in metadata_rows["containers"].items():
        require(row.get("image") in images, "container refers to unknown image")
        if row.get("metadata"):
            metadata_fields = json.loads(row["metadata"])
            exact(metadata_fields, ("image-name", "image-id", "name", "created-at"))
            require(metadata_fields["image-id"] == row["image"] and metadata_fields["name"] in row["names"],
                    "container engine metadata identity differs")
        require(set(row.get("flags", {})) <= {"MountLabel", "ProcessLabel"}, "unknown container metadata flags")
        directory = graph / "vfs-containers" / container
        require(directory.is_dir() and not directory.is_symlink()
                and {p.name for p in directory.iterdir()} == {"userdata"}, "unmapped container metadata component")
        userdata = directory / "userdata"
        files = {"config.json"} if version == "5.4.2" else set()
        require(userdata.is_dir() and not userdata.is_symlink()
                and {p.name for p in userdata.iterdir()} <= {"artifacts", "secrets", "shm"} | files,
                "unmapped container userdata")
        for path in userdata.iterdir():
            if path.name == "config.json":
                closed_oci_config(path, root, row["layer"], containers, volumes)
                continue
            require(path.is_dir() and not path.is_symlink() and not any(path.iterdir()),
                    "unmapped durable userdata contents")
    layers = metadata_rows["layers"]
    for row in metadata_rows["containers"].values():
        require(row["layer"] in layers and layers[row["layer"]].get("parent")
                == metadata_rows["images"][row["image"]]["layer"], "container writable layer parent differs from image")
    reachable = set()
    for row in list(metadata_rows["images"].values()) + list(metadata_rows["containers"].values()):
        layer = row.get("layer")
        chain = set()
        while layer:
            require(layer in layers and layer not in chain, "unknown/cyclic VFS layer")
            chain.add(layer)
            reachable.add(layer)
            layer = layers[layer].get("parent")
    require(reachable == set(layers), "unmapped durable VFS layer")
    directory = graph / "vfs"
    require(directory.is_dir() and not directory.is_symlink()
            and {p.name for p in directory.iterdir()} <= {"dir"}, "unmapped VFS payload component")
    payload = directory / "dir"
    require(payload.is_dir() and not payload.is_symlink()
            and {p.name for p in payload.iterdir()} == reachable, "VFS layer payload inventory differs")
    require(all(p.is_dir() and not p.is_symlink() for p in payload.iterdir()), "VFS layer is not a directory")
    directory = graph / "volumes"
    require(directory.is_dir() and not directory.is_symlink()
            and {p.name for p in directory.iterdir()} == volumes, "unmapped VFS volume component")
    for volume in directory.iterdir():
        require(volume.is_dir() and not volume.is_symlink()
                and {p.name for p in volume.iterdir()} == {"_data"}
                and (volume / "_data").is_dir() and not (volume / "_data").is_symlink(),
                "unmapped volume bookkeeping component")
    directory = graph / "libpod"
    engine_files = set() if database_backend == "sqlite" else {"bolt_state.db"}
    require(directory.is_dir() and not directory.is_symlink()
            and {p.name for p in directory.iterdir()} <= engine_files, "unmapped libpod bookkeeping component")
    sqlite_inventory = closed_sqlite_engine(root, images, containers, volumes, pod_id, infra_id) if database_backend == "sqlite" else None
    secrets = graph / "secrets"
    require(database_backend != "sqlite" or secrets.exists(), "observed SQLite engine secret-lock store missing")
    if secrets.exists() or secrets.is_symlink():
        require(secrets.is_dir() and not secrets.is_symlink() and secrets.stat().st_uid == secrets.stat().st_gid == 0
                and stat.S_IMODE(secrets.stat().st_mode) == 0o700
                and {p.name for p in secrets.iterdir()} == {"secrets.lock"}, "unmapped engine secret store")
        lock = secrets / "secrets.lock"
        item = lock.lstat()
        require(stat.S_ISREG(item.st_mode) and item.st_uid == item.st_gid == 0
                and stat.S_IMODE(item.st_mode) == 0o644 and item.st_size == 0 and item.st_nlink == 1,
                "engine secret lock metadata differs")
    for name in ("mounts", "tmp"):
        path = graph / name
        if path.exists():
            require(path.is_dir() and not path.is_symlink() and not any(path.iterdir()), "unmapped graphroot transient content")
    backend = graph / "defaultNetworkBackend"
    if backend.exists():
        require(stat.S_ISREG(backend.lstat().st_mode) and backend.read_text().strip() == "netavark",
                "unobserved network backend")
    for path in graph.iterdir():
        if path.name not in {"vfs", "vfs-images", "vfs-layers", "vfs-containers", "volumes", "libpod", "mounts", "tmp", "secrets"}:
            require(stat.S_ISREG(path.lstat().st_mode), "engine lock is not regular")
    for directory in (graph / "libpod", graph / "vfs-images", graph / "vfs-layers", graph / "vfs-containers"):
        require(all(stat.S_ISREG(p.lstat().st_mode) for p in directory.iterdir() if p.name not in images | containers),
                "engine metadata is not regular")
    return {"layout": "bounded-private-vfs/v2", "engine_version": version, "engine_database_backend": database_backend,
            "engine_sqlite": sqlite_inventory, "engine_secrets": "empty-lock-only" if secrets.exists() else "absent",
            "vfs_volatile": volatile_metadata,
            "image_ids": sorted(images),
            "container_ids": sorted(containers), "layer_ids": sorted(reachable), "volume_names": sorted(volumes),
            "image_bigdata": bigdata}


def capture(instance, a, api):
    instance.read()
    require(not instance.r.get("recovery"), "recursive recovery capture requires a separately defined contract")
    require(instance.r["phase"] == "started", "capture requires a started source with its DB available")
    version = engine_version(instance)
    backend = engine_backend(instance)
    capture_id = canonical_uuid(a.capture_id)
    parent = api.BASE / "captures"
    if parent.exists():
        api.protected(parent, 0o700)
    output = parent / capture_id
    require(not output.exists() and not output.is_symlink(), "capture ID already exists")
    closed_inputs(instance.root)
    pod_before = instance.owned("pod", instance.r["pod"])
    before = [instance.check_container(role) for role in ("app", "db")]
    before.append(instance.inspect("container", pod_before["InfraContainerID"]))
    closed_graphroot(instance.root, {api.image_id(item["Image"]) for item in before},
                     {item["Id"] for item in before}, {instance.prefix + "-app", instance.prefix + "-db"}, version,
                     pod_before["Id"], backend, pod_before["InfraContainerID"])
    instance.unitcheck()
    for name in instance.r["units"]:
        require(not api.run(["/usr/bin/systemctl", "show", "--property=DropInPaths", "--value", name]).stdout.strip(),
                "undeclared unit drop-in prevents complete capture")
    instance.r["capture_intent"] = {"type": "manager-capture-intent/v1", "capture_id": capture_id}
    instance.r["phase"] = "capture-in-progress"
    instance.save()
    # Stop only APP first. DB stays available for final immutable SQL capture.
    instance.shutdown_app()
    require(instance.check_container("db")["State"]["Running"], "private DB must remain available for capture")
    sql_identity = oracle(instance)
    sql_identity["immutable_rows"] = immutable_rows(instance)
    sql_identity["trigger_rows"] = trigger_rows(instance)
    require(json.loads((instance.root / "shutdown-app.json").read_text())["typed_request_acknowledged"],
            "actual typed source shutdown acknowledgement required")
    sql_identity["fact_count"] = int(query(instance, "SELECT COUNT(*) FROM facts;"))
    sql_identity["replay"] = None
    configuration = json.loads((instance.root / "app-config/config.json").read_text())
    owned_scopes = {grant["scope"] for grant in configuration["network"]["manager"]["grants"]
                    if grant["owner_replica_id"] == instance.r["replica_id"]}
    receipts = query(instance, "SELECT HEX(request_json),HEX(response_json),HEX(sha256) FROM receipts ORDER BY operation_id;")
    for row in receipts.splitlines():
        fields = row.split("\t")
        require(len(fields) == 3, "receipt capture row malformed")
        request = json.loads(bytes.fromhex(fields[0]))
        if (request.get("operation") == "observe" and request.get("exclusive_resource") is None
                and request.get("active_claim") is False and request.get("scope") in owned_scopes):
            sql_identity["replay"] = {"request": request, "response": json.loads(bytes.fromhex(fields[1])),
                                       "sha256": bytes.fromhex(fields[2]).decode()}
            break
    require(sql_identity["replay"] is not None, "capture needs an original nominal observation for replay proof")
    sql = sql_dump(instance)
    instance.stop_units()
    instance.podman("pod", "stop", instance.r["pod"])
    instance.no_foreign_containers()
    inspect = {role: instance.check_container(role) for role in ("app", "db")}
    pod = instance.owned("pod", instance.r["pod"])
    inspect["infra"] = instance.inspect("container", pod["InfraContainerID"])
    for item in inspect.values():
        require(not item["State"]["Running"] and item["State"]["Pid"] == 0, "source process remains")
    closed_inputs(instance.root, stopped=True)
    closed_runtime(instance.root, {item["Id"] for item in inspect.values()}, version)
    diffs = {role: safe_diff(json.loads(instance.podman("diff", "--format=json", item["Id"]).stdout), role)
             for role, item in inspect.items()}
    allowed_images = {api.image_id(item["Image"]) for item in inspect.values()}
    images = json.loads(instance.podman("images", "--all", "--no-trunc", "--format=json").stdout)
    require({api.image_id(item.get("Id", item.get("ID"))) for item in images} == allowed_images,
            "unmapped image content in manager graphroot")
    volumes = {role: instance.owned("volume", instance.prefix + "-" + role) for role in ("app", "db")}
    require(set(instance.podman("volume", "ls", "--quiet").stdout.decode().split())
            == {instance.prefix + "-app", instance.prefix + "-db"}, "unmapped durable volume")
    layout = closed_graphroot(instance.root, allowed_images, {item["Id"] for item in inspect.values()},
                              {instance.prefix + "-app", instance.prefix + "-db"}, version, pod["Id"], backend, pod["InfraContainerID"])
    network_files = {p.name for p in (instance.root / "networks").iterdir()}
    require(instance.prefix + "-net.json" in network_files
            and network_files <= {instance.prefix + "-net.json", "cni.lock", "netavark.lock"},
            "unmapped private network configuration")
    for name in network_files - {instance.prefix + "-net.json"}:
        path = instance.root / "networks" / name
        require(stat.S_ISREG(path.lstat().st_mode) and path.stat().st_size == 0,
                "unmapped network lock contents")
    volume_paths = {}
    for role, item in volumes.items():
        path = Path(item["Mountpoint"])
        require(path.is_relative_to(instance.root / "graphroot"), "source volume outside graphroot")
        volume_paths[role] = str(path.relative_to(instance.root))
    allowed = {"app-config", "api", "db-admin", "r", "tmp", "networks", "graphroot",
               "storage.conf", "containers.conf", "network-plan.json", "instance.json", "operator.lock",
               "ready-app.json", "shutdown-app.json"}
    require({p.name for p in instance.root.iterdir()} <= allowed, "unmapped durable instance-root component")
    if not parent.exists():
        parent.mkdir(mode=0o700)
    api.protected(parent, 0o700)
    require(not output.exists(), "capture ID already exists")
    output.mkdir(mode=0o700)
    api.jsonwrite(output / "capture-intent.json", {"type": "manager-capture-intent/v1", "capture_id": capture_id,
                                                  "source_scope": instance.scope, "source_receipt_sha256": digest(instance.receipt)})
    api.write(output / "store.sql", sql)
    image_files = {}
    for image in sorted(allowed_images):
        file = output / (image + ".oci.tar")
        instance.podman("save", "--format=oci-archive", "--output", file, image, timeout=300)
        file.chmod(0o600)
        with file.open("rb") as stream:
            os.fsync(stream.fileno())
        image_files[file.name] = digest(file)
    # Closed metadata is preserved before the full root becomes immutable capture input.
    api.jsonwrite(instance.root / "recovery-components.json", {
        "containers": inspect, "pod": pod, "volumes": volumes, "diffs": diffs, "storage_layout": layout,
        "network": instance.owned("network", instance.prefix + "-net"),
        "units": {name: (api.UNITS / name).read_text() for name in instance.r["units"]}})
    instance.r["phase"] = "capture-stopped"
    instance.save()
    rows = tree(instance.root)
    require(shutil.disk_usage(output).free >= 2 * validate_tree(rows) + 256 * 1024 ** 2,
            "capture capacity insufficient")
    for prefix in volume_paths.values():
        subtree(rows, prefix)
    archive = output / "source-full.tar"
    api.run(["/usr/bin/tar", "--format=pax", "--xattrs", "--acls", "--sparse", "--numeric-owner",
             "-cpf", str(archive), "-C", str(instance.root.parent), instance.scope], timeout=600)
    archive.chmod(0o600)
    with archive.open("rb") as stream:
        os.fsync(stream.fileno())
    require(tree(instance.root) == rows, "source changed during complete capture")
    manifest = {"type": "manager-full-capture/v2", "capture_id": capture_id,
                "scope": instance.scope, "bundle": instance.r["bundle"],
                "machine_id": instance.r["machine_id"], "replica_id": instance.r["replica_id"],
                "receipt": instance.r, "receipt_sha256": digest(instance.receipt),
                "archive_sha256": digest(archive), "sql_sha256": digest(output / "store.sql"),
                "tree": rows, "volume_paths": volume_paths, "image_files": image_files,
                "sql_identity": sql_identity, "writable_layers": diffs}
    api.jsonwrite(output / "capture.json", manifest)
    return {"state": "capture-stopped", "capture": str(output), "manifest_sha256": digest(output / "capture.json")}


def load_capture(path, api):
    api.protected(path, 0o700)
    manifest = document(path / "capture.json")
    exact(manifest, ("type", "capture_id", "scope", "bundle", "machine_id", "replica_id", "receipt",
                     "receipt_sha256", "archive_sha256", "sql_sha256", "tree", "volume_paths", "image_files",
                     "sql_identity", "writable_layers"))
    require(manifest["type"] == "manager-full-capture/v2", "unknown capture contract; exact uncertainty inventory required")
    exact(manifest["sql_identity"]["historical_uncertainty"], ("type", "attempts", "audit_events"))
    require(manifest["sql_identity"]["historical_uncertainty"]["type"] == "manager-historical-uncertainty/v1",
            "unknown historical uncertainty contract")
    require(manifest["scope"] == manifest["receipt"]["scope"]
            and manifest["bundle"] == manifest["receipt"]["bundle"]
            and manifest["replica_id"] == manifest["receipt"]["replica_id"]
            and manifest["machine_id"] == manifest["receipt"]["machine_id"], "capture identity fields disagree")
    canonical_uuid(manifest["capture_id"])
    validate_tree(manifest["tree"])
    require(manifest["tree"]["instance.json"]["sha256"] == manifest["receipt_sha256"],
            "captured receipt hash differs from full tree")
    for role, rows in manifest["writable_layers"].items():
        safe_diff(rows, role)
    require(set(manifest["writable_layers"]) == {"app", "db", "infra"}, "writable layer inventory incomplete")
    for name, value in {"source-full.tar": manifest["archive_sha256"], "store.sql": manifest["sql_sha256"],
                        **manifest["image_files"]}.items():
        require(Path(name).name == name, "unsafe capture member")
        private(path / name)
        require(digest(path / name) == value, "capture content hash differs")
    return manifest


def release(instance, a, api):
    manifest = load_capture(a.capture, api)
    checkpoint = document(a.transfer)
    exact(checkpoint, ("verified_offguest", "manifest_sha256", "archive_sha256", "sql_sha256"))
    require(checkpoint == {"verified_offguest": True, "manifest_sha256": digest(a.capture / "capture.json"),
                           "archive_sha256": manifest["archive_sha256"], "sql_sha256": manifest["sql_sha256"]},
            "operator verified offguest transfer required")
    instance.read()
    require(instance.scope == manifest["scope"] and instance.r["phase"] == "capture-stopped"
            and instance.r == manifest["receipt"] and digest(instance.receipt) == manifest["receipt_sha256"]
            and tree(instance.root) == manifest["tree"], "source changed since capture")
    require(engine_version(instance) == json.loads((instance.root / "recovery-components.json").read_text())["storage_layout"]["engine_version"],
            "source engine changed before release")
    require(not (a.capture / "release.json").exists(), "source release already recorded")
    api.jsonwrite(a.capture / "release-intent.json", {"type": "manager-source-release-intent/v1",
                  "captured_receipt_sha256": manifest["receipt_sha256"],
                  "transfer_checkpoint_sha256": digest(a.transfer)})
    instance.rollback()
    for name in manifest["receipt"]["units"]:
        require(not (api.UNITS / name).exists(), "source unit remains after release")
    for record in manifest["receipt"]["resources"]:
        if record["kind"] != "volume":
            require(instance.inspect(record["kind"], record["id"]) is None, "source resource remains after release")
    links = json.loads(api.run(["/usr/sbin/ip", "-j", "link", "show"]).stdout)
    require(all(link["ifname"] != manifest["receipt"]["bridge_interface"] for link in links),
            "source bridge interface remains after release")
    plan = manifest["receipt"]["network_plan"]
    if plan["peer_publish"]:
        listeners = api.run(["/usr/bin/ss", "-H", "-ltn"]).stdout.decode().splitlines()
        require(not any(line.split()[3].rsplit(":", 1)[-1] == str(plan["peer_container_port"])
                        for line in listeners if len(line.split()) >= 4), "source peer endpoint remains occupied")
    proof = {"type": "manager-source-release/v1", "source_scope": instance.scope,
             "capture_manifest_sha256": digest(a.capture / "capture.json"),
             "captured_receipt_sha256": manifest["receipt_sha256"],
             "rolled_back_receipt_sha256": digest(instance.receipt),
             "transfer_checkpoint_sha256": digest(a.transfer), "source_phase": "rolled-back"}
    api.jsonwrite(a.capture / "release.json", proof)
    return proof


def restore(instance, a, api):
    manifest = load_capture(a.capture, api)
    released = document(a.capture / "release.json")
    exact(released, ("type", "source_scope", "capture_manifest_sha256", "captured_receipt_sha256",
                     "rolled_back_receipt_sha256", "transfer_checkpoint_sha256", "source_phase"))
    require(released["type"] == "manager-source-release/v1" and released["source_phase"] == "rolled-back"
            and released["capture_manifest_sha256"] == digest(a.capture / "capture.json")
            and released["captured_receipt_sha256"] == manifest["receipt_sha256"]
            and released["source_scope"] == manifest["scope"], "release lineage differs")
    source = api.Instance(manifest["scope"])
    source.read()
    require(source.r["phase"] == "rolled-back" and digest(source.receipt) == released["rolled_back_receipt_sha256"],
            "live source must remain released")
    require(instance.scope != source.scope and manifest["bundle"] == instance.manifest["bundle"]
            and manifest["machine_id"] == Path("/etc/machine-id").read_text().strip(),
            "only same-package same-real-host rebind is supported")
    recovery_id = canonical_uuid(a.recovery_id)
    plan = document(a.network_plan)
    require(plan["peer_publish"] == manifest["receipt"]["network_plan"]["peer_publish"]
            and plan["peer_container_port"] == manifest["receipt"]["network_plan"]["peer_container_port"],
            "peer endpoint/config changes are outside recovery scope")
    inputs = api.BASE / "recovery-inputs"
    if not inputs.exists():
        inputs.mkdir(mode=0o700)
    api.protected(inputs, 0o700)
    stage = inputs / recovery_id
    require(not stage.exists() and not instance.root.exists(), "new recovery identity and absent target required")
    stage.mkdir(mode=0o700)
    original = stage / "source"
    original.mkdir(mode=0o700)
    require(shutil.disk_usage(stage).free >= 3 * validate_tree(manifest["tree"]) + 256 * 1024 ** 2,
            "recovery capacity insufficient")
    restore_archive(a.capture / "source-full.tar", manifest["tree"], source.scope, original)
    source_layout = json.loads((original / "recovery-components.json").read_text())["storage_layout"]
    require(engine_version(instance) == source_layout["engine_version"],
            "engine changed since source capture; explicit layout qualification required")
    config = json.loads((original / "app-config/config.json").read_text())
    api.Instance.validate_inputs(config, plan, manifest["machine_id"])
    for key, file in (("configuration", "app-config/config.json"),
                      ("application_password", "app-config/passwd"), ("database_root_password", "db-admin/passwd")):
        dest = stage / key
        api.write(dest, (original / file).read_bytes())
        setattr(a, key, dest)
    origin = {"type": "manager-recovery-rebind/v1", "recovery_id": recovery_id,
              "engine_version": engine_version(instance),
              "source_capture": str(a.capture), "manifest_sha256": digest(a.capture / "capture.json"),
              "source_receipt_sha256": manifest["receipt_sha256"], "release_sha256": digest(a.capture / "release.json"),
              "original_replica_id": manifest["replica_id"], "full_source_copy": str(original),
              "stage": "intent", "mapping": [], "db_physical_exception": "new final DB volume; exact logical SQL import",
              "image_inputs": [{"path": str(a.capture / name), "sha256": value,
                                "image_id": name.removesuffix(".oci.tar")} for name, value in manifest["image_files"].items()],
              "infra_image": api.image_id(json.loads((original / "recovery-components.json").read_text())["containers"]["infra"]["Image"])}
    instance.prepare(a, recovery=origin)
    require(instance.r["phase"] == "recovery-incomplete", "target start gate unavailable")
    origin = instance.r["recovery"]
    require(engine_backend(instance) == source_layout["engine_database_backend"], "target engine database backend differs")
    target_pod = instance.owned("pod", instance.r["pod"])
    target_containers = [instance.check_container(role) for role in ("app", "db")]
    target_containers.append(instance.inspect("container", target_pod["InfraContainerID"]))
    target_layout = closed_graphroot(instance.root, {api.image_id(item["Image"]) for item in target_containers},
        {item["Id"] for item in target_containers}, {instance.prefix + "-app", instance.prefix + "-db"},
        source_layout["engine_version"], target_pod["Id"], source_layout["engine_database_backend"], target_pod["InfraContainerID"])
    origin["mapping"].append({"component": "engine-state-database", "source": source_layout,
        "target": target_layout, "behavior": "source-bytes-sealed-in-full-copy; target-regenerated-for-new-observed-resources"})
    instance.save()
    for component, names in (("app-config", ("config.json", "store.json", "passwd")),
                             ("db-admin", ("passwd",))):
        for name in names:
            source_key = component + "/" + name
            target_file = instance.root / source_key
            require(digest(target_file) == manifest["tree"][source_key]["sha256"],
                    "private configuration bytes changed during rebind")
            metadata(target_file, manifest["tree"][source_key])
            with target_file.open("rb") as stream:
                os.fsync(stream.fileno())
        metadata(instance.root / component, manifest["tree"][component])
        fd = os.open(instance.root / component, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
        require(tree(instance.root / component) == subtree(manifest["tree"], component),
                "private configuration metadata changed during rebind")
        origin["mapping"].append({"component": component, "source": str(source.root / component),
                                  "target": str(instance.root / component), "behavior": "full-tree-restored"})
    origin["stage"] = "app-volume-copy-intent"
    instance.save()
    app = Path(instance.owned("volume", instance.prefix + "-app")["Mountpoint"])
    rows = subtree(manifest["tree"], manifest["volume_paths"]["app"])
    materialize(rows, app, lambda name: (original / manifest["volume_paths"]["app"] / name).open("rb"))
    for role in ("app", "db"):
        old = next(r for r in manifest["receipt"]["resources"] if r["kind"] == "volume" and r["name"].endswith("-" + role))
        origin["mapping"].append({"component": role + "-volume", "source": old["id"],
                                   "target": instance.prefix + "-" + role,
                                   "behavior": "full-tree-restored" if role == "app" else "fresh-empty-logical-import"})
    for record in instance.r["resources"]:
        if record["kind"] != "volume":
            old_name = record["name"].replace(instance.prefix, source.prefix, 1)
            old = next(r for r in manifest["receipt"]["resources"] if r["kind"] == record["kind"] and r["name"] == old_name)
            origin["mapping"].append({"component": record["kind"], "source": old["id"], "target": record["id"],
                                      "source_observed_id": manifest["receipt"]["network_id"] if record["kind"] == "network" else old["id"],
                                      "target_observed_id": instance.r["network_id"] if record["kind"] == "network" else record["id"],
                                      "behavior": "new-controller-observed-identity"})
    for name, value in instance.r["units"].items():
        old_name = name.replace(instance.prefix, source.prefix, 1)
        origin["mapping"].append({"component": "unit", "source": old_name, "target": name,
                                  "source_sha256": manifest["receipt"]["units"][old_name], "target_sha256": value,
                                  "behavior": "new-controller-generated-unit"})
    origin["mapping"].append({"component": "infra-container",
                              "source": json.loads((original / "recovery-components.json").read_text())["pod"]["InfraContainerID"],
                              "target": instance.owned("pod", instance.r["pod"])["InfraContainerID"],
                              "behavior": "new-controller-observed-identity"})
    origin["mapping"].append({"component": "complete-source-graphroot", "source": str(source.root / "graphroot"),
                              "target": str(original / "graphroot"), "behavior": "complete-immutable-input",
                              "active_target": str(instance.root / "graphroot"), "active_behavior": "explicit-resource-and-data-rebind"})
    for name in ("config.json", "store.json", "passwd"):
        require(digest(instance.root / "app-config" / name) == manifest["tree"]["app-config/" + name]["sha256"],
                "application configuration or private credential changed during rebind")
        origin["mapping"].append({"component": "application-input", "source": str(source.root / "app-config" / name),
                                  "target": str(instance.root / "app-config" / name), "behavior": "bytes-and-1103-ownership-preserved"})
    origin["mapping"].append({"component": "db-writable-layer", "source_changes": manifest["writable_layers"]["db"],
                              "behavior": "only-declared-runtime-temporary-paths-regenerated"})
    origin["mapping"].append({"component": "classified-engine-bookkeeping",
                              "inventory": json.loads((original / "recovery-components.json").read_text())["storage_layout"],
                              "source": str(original / "graphroot"), "target": str(instance.root / "graphroot"),
                              "behavior": "only-closed-layout-metadata-regenerated-for-observed-new-identities"})
    origin["mapping"].append({"component": "controller-runtime-files",
                              "files": [key for key in manifest["tree"] if "/" not in key and key != "."
                                        and key not in ("app-config", "db-admin", "graphroot")],
                              "behavior": "closed-controller-paths-network-plan-receipt-locks-readiness-regenerated; source-input-retained"})
    for name in manifest["image_files"]:
        image = name.removesuffix(".oci.tar")
        observed = json.loads(instance.podman("image", "inspect", image).stdout)[0]
        require(api.image_id(observed["Id"]) == image, "restored image content differs")
        origin["mapping"].append({"component": "image", "source": image, "target": image, "behavior": "content-preserved"})
    origin["stage"] = "db-initialize-intent"
    instance.save()
    api.run(["/usr/bin/systemctl", "start", instance.prefix + "-db.service"], timeout=120)
    empty = oracle(instance, empty=True)
    origin["empty_db"] = empty
    origin["stage"] = "sql-import-intent"
    instance.save()
    script = ('export MYSQL_PWD="$(cat /run/app-passwd)"; exec /usr/bin/mariadb '
              '--no-defaults --protocol=TCP --host=127.0.0.1 --port=3306 '
              '--user=podmesh-manager --database=podmesh-manager')
    private(a.capture / "store.sql")
    sql = (a.capture / "store.sql").read_bytes()
    require(len(sql) <= MAX_SQL and hashlib.sha256(sql).hexdigest() == manifest["sql_sha256"],
            "SQL import input changed")
    instance.podman("exec", "-i", "--user=1103:1103", instance.r["db"], "/bin/sh", "-eu", "-c", script,
                    input_data=sql, timeout=120)
    imported = oracle(instance)
    same_uncertainty(manifest["sql_identity"]["historical_uncertainty"], imported["historical_uncertainty"])
    require(imported["grants"] == manifest["sql_identity"]["grants"], "SQL privileges/DEFINER compatibility changed")
    require(hashlib.sha256(sql_dump(instance)).hexdigest() == manifest["sql_sha256"],
            "full SQL schema/rows/triggers differ before application start")
    require(immutable_rows(instance) == manifest["sql_identity"]["immutable_rows"]
            and trigger_rows(instance) == manifest["sql_identity"]["trigger_rows"], "full pre-APP history/trigger equality failed")
    require(tree(app) == rows, "restored app-volume changed before application start")
    require(instance.r["replica_id"] == manifest["replica_id"], "original replica not preserved")
    api.run(["/usr/bin/systemctl", "stop", instance.prefix + "-db.service"], timeout=90)
    instance.podman("pod", "stop", instance.r["pod"])
    state = instance.check_container("db")["State"]
    require(not state["Running"] and state["Pid"] == 0 and state["ExitCode"] == 0 and not state.get("OOMKilled", False),
            "final imported DB did not stop cleanly")
    require(tree(original) == manifest["tree"], "immutable full source copy changed")
    origin["stage"] = "state-verified"
    origin["pre_app_sql_sha256"] = manifest["sql_sha256"]
    origin["source_copy_verified"] = True
    instance.r["phase"] = "restored-stopped"
    instance.save()
    return {"state": "restored-stopped", "recovery_id": recovery_id, "functional_verification": "required"}


def verify(instance, a, api):
    instance.read()
    origin = instance.r.get("recovery")
    require(origin and origin["stage"] == "state-verified" and instance.r["phase"] == "started",
            "verified restore must be started for nominal proof")
    require(engine_version(instance) == origin["engine_version"], "engine changed before functional verification")
    manifest = load_capture(Path(origin["source_capture"]), api)
    require(digest(Path(origin["source_capture"]) / "capture.json") == origin["manifest_sha256"]
            and tree(Path(origin["full_source_copy"])) == manifest["tree"], "capture lineage changed")
    instance.ready_app()
    live_pid = instance.check_container("app")["State"]["Pid"]
    status = instance.control_api("status")
    config = json.loads((instance.root / "app-config/config.json").read_text())
    peers = {peer["replica_id"] for peer in config["network"]["peers"]}
    peer_complete(status, peers)
    preserved_rows(manifest["sql_identity"]["immutable_rows"], immutable_rows(instance))
    require(trigger_rows(instance) == manifest["sql_identity"]["trigger_rows"], "immutable-history trigger protection changed")
    op = canonical_uuid(a.operation_id)
    absent_operation(manifest["sql_identity"]["immutable_rows"], op)
    previous = manifest["sql_identity"]["replay"]
    request = previous["request"]
    replay = instance.control_request({"operation": "append_observation", "operation_id": request["operation_id"],
                                      "scope": request["scope"], "subject": request["subject"], "value": request["value"]})
    require(replay.get("replayed") is True and replay.get("response") == previous["response"]
            and replay.get("receipt", {}).get("sha256") == previous["sha256"], "original receipt replay differs")
    nominal = {"operation": "append_observation", "operation_id": op,
               "scope": observation_scope(config, instance.r["replica_id"], previous),
               "subject": "recovery-" + origin["recovery_id"], "value": origin["recovery_id"]}
    if "nominal_intent" not in origin:
        require(query(instance, "SELECT COUNT(*) FROM receipts WHERE operation_id='" + op + "';").strip() == "0",
                "nominal operation must be new after restore")
        origin["nominal_intent"] = nominal
        origin["peer_success_before"] = {peer: status["peers"][peer]["authenticated_successes"] for peer in peers}
        origin["audits_before_nominal"] = [event["audit_event_id"] for event in audit_events(instance)]
        origin["nominal_pid"] = live_pid
        instance.save()
    require(origin["nominal_intent"] == nominal, "different nominal operation; retained intent refuses")
    require(origin["nominal_pid"] == live_pid, "APP restarted during retained nominal intent")
    response = instance.control_request(nominal)
    require(response.get("response", {}).get("result") == "observed" and response.get("receipt") is not None,
            "nominal append did not return a durable observation")
    require(query(instance, "SELECT COUNT(*) FROM receipts WHERE operation_id='" + op + "';").strip() == "1"
            and int(query(instance, "SELECT COUNT(*) FROM facts;")) > manifest["sql_identity"]["fact_count"],
            "nominal append has no durable SQL effect")
    actual = query(instance, "SELECT HEX(request_json),HEX(response_json),sha256 FROM receipts WHERE operation_id='" + op + "';").strip().split("\t")
    expected = {**nominal, "operation": "observe", "exclusive_resource": None, "active_claim": False}
    require(len(actual) == 3 and json.loads(bytes.fromhex(actual[0])) == expected
            and json.loads(bytes.fromhex(actual[1])) == response["response"]
            and actual[2] == response["receipt"]["sha256"], "nominal request/result/receipt SQL binding differs")
    fact = response["response"]["fact"]
    event_id = fact["event_id"]
    require(isinstance(event_id, str) and event_id.startswith(instance.r["replica_id"] + ":")
            and len(event_id) == 57 and event_id[37:].isascii() and event_id[37:].isdigit(),
            "fresh fact event identity differs")
    fact_row = query(instance, "SELECT HEX(fact_json),sha256 FROM facts WHERE event_id='" + event_id + "';").strip().split("\t")
    require(len(fact_row) == 2 and json.loads(bytes.fromhex(fact_row[0])) == fact
            and hashlib.sha256(bytes.fromhex(fact_row[0])).hexdigest() == fact_row[1]
            and fact["scope"] == nominal["scope"] and fact["subject"] == nominal["subject"]
            and fact["value"] == nominal["value"] and fact["origin_replica_id"] == instance.r["replica_id"]
            and fact["logical_manager_id"] == config["network"]["manager"]["logical_manager_id"]
            and fact["origin_host_id"] == next(replica["host_id"] for replica in config["network"]["manager"]["replicas"]
                                               if replica["replica_id"] == instance.r["replica_id"]),
            "fresh nominal fact SQL binding differs")
    deadline = time.monotonic() + 90
    matched = None
    while time.monotonic() < deadline:
        require(instance.check_container("app")["State"]["Pid"] == live_pid, "APP identity changed during nominal proof")
        status = instance.control_api("status")
        count = int(query(instance, "SELECT COUNT(*) FROM facts;"))
        try:
            peer_complete(status, peers, count, origin["peer_success_before"])
            matched = linked_peer_receipts(audit_events(instance), set(origin["audits_before_nominal"]), peers)
            break
        except ValueError:
            time.sleep(0.2)
    require(matched is not None, "fresh nominal effect not acknowledged by both authenticated peers")
    # A live valid prefix may still be in flight. Join all workers through the
    # typed shutdown before classifying new uncertainty as historical.
    instance.shutdown_app()
    preserved_rows(manifest["sql_identity"]["immutable_rows"], immutable_rows(instance))
    identity = oracle(instance)
    same_uncertainty(manifest["sql_identity"]["historical_uncertainty"], identity["historical_uncertainty"])
    identity.pop("grants")
    instance.stop_units()
    instance.r["phase"] = "restored-stopped"
    instance.save()
    proof = {"type": "manager-restored-functional/v1", "recovery_id": origin["recovery_id"],
             "capture_manifest_sha256": origin["manifest_sha256"], "operation_id": op,
             "status": status, "sql_identity": identity, "replay": replay, "nominal": response,
             "peer_receipt_links": matched,
             "application_after_proof": "typed-shutdown-clean-stopped",
             "database_after_proof": "clean-stopped",
             "peer_convergence": "local fresh-history acknowledgements proved; independent peer SQL verification required",
             "scope": "restored replica proof; not complete fleet restoration or HA"}
    file = instance.root / "restored-functional.json"
    if file.exists():
        private(file)
    api.jsonwrite(file, proof, replace=file.exists())
    return proof
