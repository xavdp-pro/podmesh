#!/usr/bin/env python3
"""Explicit root administration of one NEW private node instance, never legacy state."""
import argparse
import fcntl
import grp
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import socket
import stat
import struct
import subprocess
import sys
import time

BASE = Path("/var/lib/podmesh-node-private")
UNITS = Path("/etc/systemd/system")
BUNDLE = Path(__file__).resolve().parent
LABEL = "io.podmesh.private-instance"


class Refusal(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise Refusal(message)


def sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def image_id(value):
    require(isinstance(value,str), "image ID unavailable")
    value = value.removeprefix("sha256:")
    require(re.fullmatch(r"[0-9a-f]{64}",value), "invalid image ID")
    return value


def protected(path, mode=None, uid=0):
    s = path.lstat()
    require(not stat.S_ISLNK(s.st_mode) and s.st_uid == uid, "path ownership or type changed")
    if mode is not None:
        require(stat.S_IMODE(s.st_mode) == mode, "path permissions changed")
    for parent in path.parents:
        p = parent.lstat()
        require(stat.S_ISDIR(p.st_mode) and p.st_uid == 0 and not p.st_mode & 0o022,
                "parent must be a protected root directory")
    return s


def write(path, value, mode=0o600, uid=0, gid=0, replace=False):
    data = value if isinstance(value, bytes) else value.encode()
    target = path.with_name(path.name + ".next") if replace else path
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, "wb") as f:
        os.fchmod(f.fileno(), mode)
        os.fchown(f.fileno(), uid, gid)
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    if replace:
        os.replace(target, path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def jsonwrite(path, value, replace=False):
    write(path, json.dumps(value, indent=2, sort_keys=True) + "\n", replace=replace)


def run(args, env=None, okay=(0,), timeout=90):
    result = subprocess.run(args, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=timeout, check=False)
    require(result.returncode in okay, "administration command failed; private state preserved")
    return result


class Instance:
    def __init__(self, scope):
        require(re.fullmatch(r"[a-z][a-z0-9-]{0,31}", scope), "invalid instance scope")
        self.scope = scope
        self.root = BASE / scope
        self.prefix = "podmesh-node-private-" + scope
        self.manifest = json.loads((BUNDLE / "bundle.json").read_text())
        for name, digest in self.manifest["payload_sha256"].items():
            require(re.fullmatch(r"[a-zA-Z0-9._-]+", name), "invalid bundle member")
            protected(BUNDLE / name)
            require(sha(BUNDLE / name) == digest, "bundle payload hash differs")
        self.receipt = self.root / "instance.json"
        # Do not inherit ambient DSNs, Podman remotes or another host storage profile.
        self.env = {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "HOME": "/root", "LANG": "C.UTF-8",
                    "CONTAINERS_STORAGE_CONF": str(self.root / "storage.conf"),
                    "CONTAINERS_CONF": str(self.root / "containers.conf")}

    def podman(self, *args, okay=(0,), timeout=90):
        return run(["/usr/bin/podman", *map(str, args)], self.env, okay, timeout)

    def read(self):
        protected(self.root, 0o700)
        protected(self.receipt, 0o600)
        self.r = json.loads(self.receipt.read_text())
        require(self.r["scope"] == self.scope and self.r["bundle"] == self.manifest["bundle"],
                "different instance or bundle; no adoption")
        require(self.r["host_machine_id"] == Path("/etc/machine-id").read_text().strip(),
                "different host identity; explicit recovery required")
        for name, digest in self.r["configuration_sha256"].items():
            path = self.root / name
            protected(path, 0o600)
            require(sha(path) == digest, "root configuration changed")
        for name, digest in self.r["application_configuration_sha256"].items():
            path = self.root / "app-config" / name
            metadata = path.lstat()
            require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == 1102
                    and metadata.st_gid == 1102 and stat.S_IMODE(metadata.st_mode) == 0o600,
                    "application configuration ownership changed")
            require(sha(path) == digest, "application configuration changed")
        return self.r

    def save(self):
        jsonwrite(self.receipt, self.r, replace=True)

    def inspect(self, kind, identity):
        result = self.podman(kind, "exists", identity, okay=(0, 1))
        if result.returncode == 1:
            return None
        return json.loads(self.podman(kind, "inspect", identity).stdout)[0]

    def owned(self, kind, identity):
        item = self.inspect(kind, identity)
        if item is None:
            return None
        labels = item.get("Labels", {}) if kind != "container" else item["Config"]["Labels"]
        require(labels.get(LABEL) == self.scope and labels.get("io.podmesh.bundle") == self.r["bundle"],
                "resource ownership differs; preserve it")
        if kind != "volume":
            require(item["Id"] == identity, "resource identity changed")
        return item

    def check_container(self, role):
        item = self.owned("container", self.r[role])
        require(item is not None, "unit container is absent")
        require(image_id(item["Image"]) == image_id(self.manifest["application_image" if role == "app" else "database_image"]),
                "unit image differs")
        expected = ({"/var/lib/podmesh-node","/etc/podmesh-node","/run/podmesh-node","/run/podmesh-host"}
                    if role == "app" else {"/var/lib/mysql","/run/db-admin","/run/app-config"})
        mounts = item["Mounts"]
        require(len(mounts) == len(expected) and {m["Destination"] for m in mounts} == expected,
                "unexpected unit mount; inherited anonymous volumes are forbidden")
        sources = {"/etc/podmesh-node":self.root/"app-config", "/run/podmesh-node":self.root/"api",
                   "/run/podmesh-host":self.root/"provider-socket", "/run/db-admin":self.root/"db-admin",
                   "/run/app-config":self.root/"app-config"}
        for mount in mounts:
            destination = mount["Destination"]
            if destination in sources:
                require(Path(mount["Source"]) == sources[destination], "unit bind source differs")
                require(bool(mount["RW"]) == (destination == "/run/podmesh-node"), "unit bind access differs")
            else:
                volume_name = self.prefix + ("-app" if role == "app" else "-db")
                volume = self.owned("volume",volume_name)
                require(volume is not None and mount["Source"] == volume["Mountpoint"] and mount["RW"],
                        "unit persistent volume differs")
        host = item["HostConfig"]
        require(not host["Privileged"] and not host.get("PortBindings"), "unit privilege or published port differs")
        pod = self.owned("pod",self.r["pod"])
        require(pod is not None and item["Pod"] == self.r["pod"], "unit pod identity differs")
        infra_id = pod["InfraContainerID"]
        infra = self.inspect("container",infra_id)
        require(infra is not None and infra["Id"] == infra_id and infra["Pod"] == self.r["pod"],
                "pod infra identity differs")
        require(infra["HostConfig"]["NetworkMode"] == "none" and not infra["HostConfig"].get("PortBindings")
                and not any(infra.get("NetworkSettings",{}).get("Ports",{}).values())
                and not pod.get("InfraConfig",{}).get("PortBindings"),
                "pod infra network or published ports differ")
        require(host["NetworkMode"] == "container:"+pod["InfraContainerID"], "unit network namespace differs")
        for field in ("PidMode","IpcMode","UTSMode"):
            require(host[field] == "private", "unit namespace differs")
        if role == "app":
            require(item["Config"]["User"] == "1102:1102", "application account differs")
            require(host["ReadonlyRootfs"], "application root filesystem must be immutable")
            require(not item.get("EffectiveCaps",[]) and "no-new-privileges" in host["SecurityOpt"],
                    "application capabilities differ")
        return item

    def unitcheck(self):
        for name, digest in self.r["units"].items():
            path = UNITS / name
            protected(path, 0o644)
            require(sha(path) == digest, "unit modified externally; preserve it")

    def stop_units(self):
        # An inactive/failed target can still have active dependencies. Request
        # explicit stops for each recorded own service, ordered by systemd.
        if self.r["units"]:
            run(["/usr/bin/systemctl", "stop", *self.r["units"]], okay=(0,5), timeout=120)

    def prepare(self, a):
        for database in (pwd.getpwuid, grp.getgrgid):
            try:
                account = database(1102)
            except KeyError:
                continue
            require(account[0] == "podmesh-node", "host UID/GID1102 belongs to another identity")
        require(run(["/usr/bin/systemctl", "is-active", "podmesh.service"], okay=(0, 3, 4)).returncode != 0,
                "legacy node active; separate cutover required")
        require(run(["/usr/bin/systemctl", "is-enabled", "podmesh.service"], okay=(0, 1, 3, 4)).returncode != 0,
                "legacy node enabled; separate cutover required")
        if not BASE.exists():
            BASE.mkdir(mode=0o700)
        protected(BASE, 0o700)
        require(not self.root.exists() and not self.root.is_symlink(), "instance already exists; never overwrite")
        # One per-host node identity, including prepared/stopped instances; rollback
        # records do not disappear and require a separate migration mandate.
        for existing in BASE.iterdir():
            if existing.is_dir() and (existing / "instance.json").exists():
                document = json.loads((existing / "instance.json").read_text())
                require(document.get("phase") == "rolled-back", "another private node instance remains")
        for name in (self.prefix+"-provider.service", self.prefix+"-db.service",
                     self.prefix+"-app.service", self.prefix+".target"):
            require(not (UNITS / name).exists() and not (UNITS / name).is_symlink(), "unit name already present")
        secrets = []
        for path in (a.application_password, a.database_root_password):
            protected(path, 0o600)
            require(stat.S_ISREG(path.lstat().st_mode), "credential must be a regular root0600 file")
            secret = path.read_bytes().rstrip(b"\r\n")
            require(0 < len(secret) <= 4096 and b"\n" not in secret and b"\r" not in secret and b"\0" not in secret,
                    "credential must be a nonempty single line")
            secrets.append(secret)
        require(secrets[0] != secrets[1], "application and DB administrator credentials must differ")
        protected(a.policy, 0o600)
        policy = json.loads(a.policy.read_text())
        require(policy["scope"] == self.scope and policy["application_uid"] == 1102
                and policy["application_gid"] == 1102 and policy["state_dir"] == str(self.root / "provider-state"),
                "policy must bind the new exact scope, identity and provider state path")
        self.root.mkdir(mode=0o700)
        for name, uid, gid, mode in (("app-config",1102,1102,0o700), ("api",1102,1102,0o700),
                                    ("provider-socket",0,1102,0o750), ("provider-state",0,0,0o700),
                                    ("db-admin",0,0,0o700), ("runroot",0,0,0o700), ("tmp",0,0,0o700)):
            path = self.root / name
            path.mkdir(mode=mode)
            os.chown(path, uid, gid)
            path.chmod(mode)
        write(self.root / "app-config/passwd", secrets[0], uid=1102, gid=1102)
        profile = {"engine":"mariadb", "mariadb":{"host":"127.0.0.1", "port":3306,
                   "user":"podmesh-node", "database":"podmesh-node", "password_file":"/etc/podmesh-node/passwd"}}
        write(self.root / "app-config/store.json", json.dumps(profile) + "\n", uid=1102, gid=1102)
        write(self.root / "db-admin/passwd", secrets[1])
        write(self.root / "policy.json", json.dumps(policy, indent=2) + "\n")
        write(self.root / "storage.conf", f'[storage]\ndriver="vfs"\ngraphroot="{self.root}/graphroot"\nrunroot="{self.root}/runroot"\n')
        write(self.root / "containers.conf", f'[engine]\ntmp_dir="{self.root}/tmp"\n')
        self.r = {"scope":self.scope, "bundle":self.manifest["bundle"], "phase":"preparing",
                  "host_machine_id":Path("/etc/machine-id").read_text().strip(), "units":{}, "resources":[],
                  "configuration_sha256":{n:sha(self.root / n) for n in ("policy.json","storage.conf","containers.conf","db-admin/passwd")},
                  "application_configuration_sha256":{n:sha(self.root / "app-config" / n) for n in ("store.json","passwd")}}
        jsonwrite(self.receipt, self.r)  # Durable intent before every host resource.
        for archive, image in (("application.oci.tar", self.manifest["application_image"]),
                               ("database.oci.tar", self.manifest["database_image"])):
            self.podman("load", "--input", BUNDLE / archive, timeout=300)
            observed = json.loads(self.podman("image", "inspect", image).stdout)[0]
            require(image_id(observed["Id"]) == image_id(image), "loaded image ID differs")
        self.r["images"] = [self.manifest["application_image"], self.manifest["database_image"]]
        self.save()
        labels = ["--label", LABEL + "=" + self.scope, "--label", "io.podmesh.bundle=" + self.r["bundle"]]
        def create(kind, name, options):
            require(self.inspect(kind, name) is None, "new resource name already exists")
            record = {"kind":kind, "name":name, "id":None}
            self.r["resources"].append(record)
            self.save()
            naming = [name] if kind == "volume" else ["--name", name]
            output = self.podman(kind, "create", *options, *labels, *naming).stdout.decode().strip()
            record["id"] = name if kind == "volume" else output
            self.save()
            self.owned(kind, record["id"])
            return record["id"]
        appvol = create("volume", self.prefix + "-app", [])
        dbvol = create("volume", self.prefix + "-db", [])
        mount = Path(self.owned("volume", appvol)["Mountpoint"])
        require(mount.is_relative_to(self.root / "graphroot"), "application volume outside owned storage")
        protected(mount)
        os.chown(mount, 1102, 1102)
        mount.chmod(0o700)
        pod = create("pod", self.prefix, ["--network=none", "--share=net", "--userns=host"])
        self.r["pod"] = pod
        self.save()
        # Container create has a different CLI ordering (image after options).
        def container(role, image, options, command=()):
            name = self.prefix + "-" + role
            require(self.inspect("container", name) is None, "new container name already exists")
            record = {"kind":"container", "name":name, "id":None}
            self.r["resources"].append(record)
            self.save()
            result = self.podman("create", "--name", name, *labels, "--pod", pod, "--pull=never",
                                 "--image-volume=ignore", "--hostname", name, "--pid=private", "--ipc=private",
                                 "--uts=private", *options, image, *command)
            record["id"] = result.stdout.decode().strip()
            self.r[role] = record["id"]
            self.save()
            self.check_container(role)
        container("db", self.manifest["database_image"], ["--memory=512m", "--memory-swap=512m", "--cpus=1",
                  "--volume", dbvol+":/var/lib/mysql", "--volume", str(self.root / "db-admin")+":/run/db-admin:ro",
                  "--volume", str(self.root / "app-config")+":/run/app-config:ro",
                  "--env=MARIADB_ROOT_PASSWORD_FILE=/run/db-admin/passwd", "--env=MARIADB_PASSWORD_FILE=/run/app-config/passwd",
                  "--env=MARIADB_USER=podmesh-node", "--env=MARIADB_DATABASE=podmesh-node"],
                  ["mariadbd", "--bind-address=127.0.0.1"])
        container("app", self.manifest["application_image"], ["--user=1102:1102",
                  "--read-only", "--read-only-tmpfs=false",
                  "--cap-drop=ALL", "--security-opt=no-new-privileges", "--memory=512m", "--memory-swap=512m", "--cpus=1",
                  "--volume", appvol+":/var/lib/podmesh-node", "--volume", str(self.root / "app-config")+":/etc/podmesh-node:ro",
                  "--volume", str(self.root / "api")+":/run/podmesh-node",
                  "--volume", str(self.root / "provider-socket")+":/run/podmesh-host:ro"])
        self.generate_units()
        self.r["phase"] = "prepared"
        self.save()
        run(["/usr/bin/systemctl", "daemon-reload"])

    def generate_units(self):
        target = self.prefix + ".target"
        provider = self.prefix + "-provider.service"
        database = self.prefix + "-db.service"
        application = self.prefix + "-app.service"
        common = (f'Environment="CONTAINERS_STORAGE_CONF={self.root}/storage.conf" "CONTAINERS_CONF={self.root}/containers.conf"\n'
                  'Environment="PATH=/usr/sbin:/usr/bin:/sbin:/bin" "HOME=/root"\n'
                  'UnsetEnvironment=PODMESH_HOST_ADAPTER_SOCKET CONTAINER_HOST CONTAINER_CONNECTION PODMESH_MARIADB_DSN\n')
        helper = f"/usr/bin/python3 {BUNDLE}/instance.py --scope {self.scope}"
        texts = {
            target:f"[Unit]\nDescription=Private PodMesh node {self.scope}\nRequires={provider} {database} {application}\nAfter={application}\n",
            provider:f"[Unit]\nDescription=Scoped PodMesh root provider\nPartOf={target}\nBefore={database} {application}\n"
                     f"[Service]\nType=simple\nUser=root\n{common}"
                     f"ExecStart={BUNDLE}/podmesh-host-adapter --policy {self.root}/policy.json --socket {self.root}/provider-socket/capability.sock\n"
                     f"ExecStartPost={helper} hook --role provider --event ready\nExecStopPost={helper} hook --role provider --event stopped\n"
                     "TimeoutStartSec=90\nTimeoutStopSec=30\nKillMode=control-group\nRestart=no\n",
        }
        for role, name, after in (("db",database,provider), ("app",application,database)):
            texts[name] = (f"[Unit]\nDescription=Private PodMesh {role}\nPartOf={target}\nRequires={after}\nAfter={after}\n"
                           f"[Service]\nType=simple\nUser=root\n{common}"
                           f"ExecStart=/usr/bin/podman start --attach {self.r[role]}\n"
                           f"ExecStartPost={helper} hook --role {role} --event ready\n"
                           f"ExecStop=/usr/bin/podman stop --time 20 {self.r[role]}\n"
                           f"ExecStopPost={helper} hook --role {role} --event stopped\n"
                           "TimeoutStartSec=90\nTimeoutStopSec=30\nKillMode=control-group\nRestart=no\n")
        for name, text in texts.items():
            digest = hashlib.sha256(text.encode()).hexdigest()
            self.r["units"][name] = digest
            self.save()  # Intent before unit creation; x creation never overwrites.
            write(UNITS / name, text, mode=0o644)

    def hook(self, role, event):
        self.read()
        self.unitcheck()
        if role == "db":
            if event == "ready":
                self.check_container("db")
                self.wait_db()
            return
        endpoint = self.root / ("api/api.sock" if role == "app" else "provider-socket/capability.sock")
        marker = self.root / (role + "-socket.json")
        expected_uid = 1102 if role == "app" else 0
        if event == "ready":
            require(not marker.exists(), "old socket receipt remains; preserve endpoint")
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                if endpoint.exists():
                    s = endpoint.lstat()
                    require(stat.S_ISSOCK(s.st_mode) and s.st_uid == expected_uid and s.st_gid == 1102,
                            "socket identity differs")
                    with socket.socket(socket.AF_UNIX) as peer:
                        peer.settimeout(2)
                        peer.connect(str(endpoint))
                        pid, uid, gid = struct.unpack("3i", peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                    require(uid == expected_uid and gid == (1102 if role == "app" else 0), "peer identity differs")
                    name = "podmeshd" if role == "app" else "podmesh-host-adapter"
                    require(sha(Path(f"/proc/{pid}/exe")) == self.manifest["payload_sha256"][name], "running executable differs")
                    if role == "app":
                        item = self.check_container("app")
                        require(item["State"]["Pid"] == pid, "app socket belongs to a different process")
                    else:
                        main_pid = int(run(["/usr/bin/systemctl", "show", self.prefix+"-provider.service", "--property=MainPID", "--value"]).stdout)
                        require(main_pid == pid, "provider socket belongs to another process")
                    jsonwrite(marker, {"device":s.st_dev, "inode":s.st_ino, "pid":pid, "uid":uid})
                    return
                time.sleep(0.2)
            raise Refusal("socket readiness timed out")
        if not endpoint.exists():
            if marker.exists():
                protected(marker, 0o600)
                marker.unlink()
            return
        require(marker.exists(), "unrecorded socket remains; explicit ownership review required")
        protected(marker, 0o600)
        record = json.loads(marker.read_text())
        s = endpoint.lstat()
        require(stat.S_ISSOCK(s.st_mode) and (s.st_dev,s.st_ino,s.st_uid) ==
                (record["device"],record["inode"],record["uid"]), "socket replaced; preserve it")
        if role == "app":
            item = self.owned("container", self.r["app"])
            require(item is None or not item["State"]["Running"], "application still running")
        else:
            require(not Path(f'/proc/{record["pid"]}').exists(), "recorded provider PID still present")
        endpoint.unlink()
        marker.unlink()

    def wait_db(self):
        # Credential only in the child environment INSIDE its private namespace.
        script = 'export MYSQL_PWD="$(cat /run/app-config/passwd)"; exec /usr/bin/mariadb --no-defaults --protocol=TCP --host=127.0.0.1 --port=3306 --user=podmesh-node --database=podmesh-node --batch --skip-column-names --execute="SELECT 1"'
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            self.owned("container", self.r["db"])
            result = self.podman("exec", "--user=1102:1102", self.r["db"], "/bin/sh", "-eu", "-c", script,
                                 okay=(0,1), timeout=15)
            if result.returncode == 0 and result.stdout.strip() == b"1":
                return
            time.sleep(1)
        raise Refusal("private DB application access failed")

    def control(self, action):
        self.read()
        require(self.r["phase"] in ("prepared","started","stopped"), "instance incomplete or rolled back")
        self.unitcheck()
        for record in self.r["resources"]:
            require(record["id"] and self.owned(record["kind"],record["id"]), "owned resource missing")
        self.check_container("db")
        self.check_container("app")
        target = self.prefix + ".target"
        if action == "start":
            try:
                run(["/usr/bin/systemctl", "start", target], timeout=240)
            except (Refusal, subprocess.TimeoutExpired):
                self.stop_units()
                raise Refusal("start failed; own control plane stopped, state retained")
            self.r["phase"] = "started"
        else:
            self.stop_units()
            self.r["stop_observations"] = {}
            for role in ("app","db"):
                item = self.owned("container",self.r[role])
                if item is not None:
                    self.r["stop_observations"][role] = {key:item["State"].get(key) for key in
                                                         ("Running","Pid","ExitCode","OOMKilled","FinishedAt")}
                    self.save()
                require(item is not None and not item["State"]["Running"] and item["State"]["Pid"] == 0,
                        "unit process remains after stop")
                require(not item["State"].get("OOMKilled",False) and item["State"]["ExitCode"] != 137,
                        "unit was force-killed; preserve state and inspect its journal")
            self.r["phase"] = "stopped"
        self.save()

    def rollback(self):
        self.read()
        if self.r["phase"] == "rolled-back":
            return
        # Validate EVERYTHING before stopping/removing. A refused rollback leaves
        # the running app available to perform journalled workload cleanup.
        for name, digest in self.r["units"].items():
            path = UNITS / name
            if path.exists():
                protected(path, 0o644)
                require(sha(path) == digest, "unit changed; rollback refused")
        self.assert_no_workloads()
        for record in self.r["resources"]:
            if record["id"] is None:
                require(self.inspect(record["kind"],record["name"]) is None,
                        "interrupted creation has no observed ID; explicit ownership review required")
            else:
                self.owned(record["kind"], record["id"])
        self.stop_units()
        self.assert_no_workloads()  # Close the API/create race before removal.
        for name in self.r["units"]:
            run(["/usr/bin/systemctl", "disable", name], okay=(0,1))
        for record in reversed(self.r["resources"]):
            if record["kind"] == "volume" or not record["id"]:
                continue
            item = self.owned(record["kind"],record["id"])
            if item:
                if record["kind"] == "container":
                    require(not item["State"]["Running"], "container remains running; rollback refused")
                if record["kind"] == "pod":
                    self.podman("pod", "stop", record["id"])
                self.podman(record["kind"], "rm", record["id"])
        for name in self.r["units"]:
            path = UNITS / name
            if path.exists():
                require(sha(path) == self.r["units"][name], "unit changed during rollback")
                path.unlink()
        run(["/usr/bin/systemctl", "daemon-reload"])
        self.r["phase"] = "rolled-back"
        self.save()

    def assert_no_workloads(self):
        items = json.loads(self.podman("ps", "--all", "--format=json").stdout)
        own = {r["id"] for r in self.r["resources"] if r["kind"] == "container"}
        # The pod's infra container is owned by the recorded pod, never adopt others.
        if self.r.get("pod"):
            pod = self.owned("pod",self.r["pod"])
            if pod:
                own.add(pod.get("InfraContainerID"))
        require(all(i.get("Id",i.get("ID")) in own for i in items),
                "workload or foreign container remains; use journalled API cleanup first")
        for file in (self.root / "provider-state").glob("*.json"):
            if file.name.startswith("intent-"):
                continue
            protected(file,0o600)
            record = json.loads(file.read_text())
            identity = record.get("container_id")
            if identity:
                require(self.inspect("container",identity) is None, "recorded universe remains")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--scope",required=True)
    commands = p.add_subparsers(dest="action",required=True)
    prep = commands.add_parser("prepare")
    prep.add_argument("--policy",required=True,type=Path)
    prep.add_argument("--application-password",required=True,type=Path)
    prep.add_argument("--database-root-password",required=True,type=Path)
    for name in ("start","stop","rollback","status"):
        commands.add_parser(name)
    hook = commands.add_parser("hook",help=argparse.SUPPRESS)
    hook.add_argument("--role",choices=("provider","db","app"),required=True)
    hook.add_argument("--event",choices=("ready","stopped"),required=True)
    a = p.parse_args()
    require(os.geteuid() == 0, "separate root administrator required")
    os.umask(0o077)
    instance = Instance(a.scope)
    if a.action == "prepare":
        if not BASE.exists():
            BASE.mkdir(mode=0o700)
        protected(BASE,0o700)
        fd = os.open(BASE / "prepare.lock",os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW,0o600)
        with os.fdopen(fd,"r+") as lock:
            protected(BASE / "prepare.lock",0o600)
            fcntl.flock(lock,fcntl.LOCK_EX | fcntl.LOCK_NB)
            instance.prepare(a)
        return
    if a.action == "hook":  # Unit callbacks must not wait on operator's lock.
        instance.hook(a.role,a.event)
        return
    instance.read()
    lock_path = instance.root / "operator.lock"
    fd = os.open(lock_path,os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,"r+") as lock:
        protected(lock_path,0o600)
        fcntl.flock(lock,fcntl.LOCK_EX | fcntl.LOCK_NB)
        if a.action in ("start","stop"):
            instance.control(a.action)
        elif a.action == "rollback":
            instance.rollback()
        else:
            print(json.dumps({"scope":instance.scope,"bundle":instance.r["bundle"],"phase":instance.r["phase"],
                              "resources":instance.r["resources"],"data_retained":True},indent=2))


if __name__ == "__main__":
    try:
        main()
    except Refusal as error:
        sys.exit(str(error))
    except Exception:
        # Malformed inputs and child stderr may contain credentials; do not echo.
        sys.exit("private instance administration failed; preserve its state and inspect privately")
