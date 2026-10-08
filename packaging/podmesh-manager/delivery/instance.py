#!/usr/bin/env python3
"""Explicit root administration of one NEW private manager instance, never legacy state."""
import argparse
import fcntl
import grp
import hashlib
import importlib.util
import ipaddress
import uuid
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
from types import SimpleNamespace

BASE = Path("/var/lib/podmesh-manager-private")
UNITS = Path("/etc/systemd/system")
BUNDLE = Path(__file__).resolve().parent
LABEL = "io.podmesh.private-instance"
RUNROOT_MAX_BYTES = 50


class Refusal(RuntimeError):
    pass


def private_runroot(root):
    path = root / "r"
    require(len(os.fsencode(path)) <= RUNROOT_MAX_BYTES,
            "private runroot exceeds observed engine50-byte limit; choose a shorter new scope")
    return path


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
    def __init__(self,scope):
        require(re.fullmatch(r"[a-z][a-z0-9-]{0,14}",scope),"invalid instance scope; maximum15ASCII characters")
        self.scope,self.root = scope,BASE/scope
        private_runroot(self.root)
        self.prefix = "podmesh-manager-private-"+scope
        self.receipt = self.root/"instance.json"
        self.manifest = json.loads((BUNDLE/"bundle.json").read_text())
        for name,digest in self.manifest["payload_sha256"].items():
            require(re.fullmatch(r"[a-zA-Z0-9._-]+",name),"invalid bundle member")
            protected(BUNDLE/name)
            require(sha(BUNDLE/name)==digest,"bundle payload differs")
        self.env = {"PATH":"/usr/sbin:/usr/bin:/sbin:/bin","HOME":"/root","LANG":"C.UTF-8",
                    "CONTAINERS_STORAGE_CONF":str(self.root/"storage.conf"),
                    "CONTAINERS_CONF":str(self.root/"containers.conf")}

    def podman(self,*args,okay=(0,),timeout=90,input_data=None):
        result = subprocess.run(["/usr/bin/podman",*map(str,args)],env=self.env,input=input_data,
                                stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=timeout,check=False)
        require(result.returncode in okay,"private Podman command failed; retain state")
        return result

    def read(self):
        protected(self.root,0o700)
        protected(self.receipt,0o600)
        self.r = json.loads(self.receipt.read_text())
        require(self.r["scope"]==self.scope and self.r["bundle"]==self.manifest["bundle"],"different instance/bundle; no adoption")
        require(self.r["machine_id"]==Path("/etc/machine-id").read_text().strip(),"different host; explicit recovery required")
        for name,digest in self.r["root_hashes"].items():
            protected(self.root/name,0o600)
            require(sha(self.root/name)==digest,"root configuration changed")
        for name,digest in self.r["app_hashes"].items():
            path=self.root/"app-config"/name
            s=path.lstat()
            require(stat.S_ISREG(s.st_mode) and s.st_uid==s.st_gid==1103 and stat.S_IMODE(s.st_mode)==0o600,
                    "application configuration ownership changed")
            require(sha(path)==digest,"private application configuration changed")
        return self.r

    def save(self):
        jsonwrite(self.receipt,self.r,replace=True)

    def inspect(self,kind,identity):
        result=self.podman(kind,"exists",identity,okay=(0,1))
        if result.returncode==1:
            return None
        return json.loads(self.podman(kind,"inspect",identity).stdout)[0]

    def owned(self,kind,identity):
        item=self.inspect(kind,identity)
        if item is None:
            return None
        labels=item.get("labels",{}) if kind=="network" else item.get("Labels",{})
        if kind=="container":
            labels=item["Config"]["Labels"]
        require(labels.get(LABEL)==self.scope and labels.get("io.podmesh.bundle")==self.r["bundle"],"resource ownership differs")
        if kind not in ("volume","network"):
            require(item["Id"]==identity,"resource identity differs")
        if kind=="network":
            require(item["id"]==self.r["network_id"],"network identity differs")
            if "bridge_interface" in self.r:
                require(item["network_interface"]==self.r["bridge_interface"],"network bridge identity differs")
        return item

    def unitcheck(self):
        for name,digest in self.r["units"].items():
            protected(UNITS/name,0o644)
            require(sha(UNITS/name)==digest,"unit changed externally; preserve it")

    @staticmethod
    def validate_inputs(config,plan,machine):
        require(config["observation_writer_uid"]==1103 and config["control_socket"]=="/run/podmesh-manager/control.sock",
                "functional control identity differs")
        require(config.get("votes") is None,"vote authority is outside private delivery mandate")
        network=config["network"]
        require(network["database_path"]=="/var/lib/podmesh-manager/manager.sqlite","profile resolution path differs")
        local=str(uuid.UUID(network["replica_id"]))
        require(local==network["replica_id"],"canonical replica UUID required")
        topology=network["manager"]
        require(str(uuid.UUID(topology["logical_manager_id"]))==topology["logical_manager_id"],"canonical logical manager required")
        replicas={r["replica_id"]:r["host_id"] for r in topology["replicas"]}
        require(len(replicas)==len(topology["replicas"]) and replicas.get(local)==str(uuid.UUID(machine)),"local replica/host binding differs")
        for replica,host in replicas.items():
            require(str(uuid.UUID(replica))==replica and str(uuid.UUID(host))==host,"canonical topology identities required")
        def endpoint(text):
            host,port=text.rsplit(":",1)
            ip=ipaddress.IPv4Address(host)
            require(port.isdecimal() and 0<int(port)<65536,"invalid endpoint port")
            return str(ip),int(port)
        bind=endpoint(network["bind"])
        require(type(plan["peer_container_port"]) is int and 1024<=plan["peer_container_port"]<65536
                and plan["peer_container_port"]!=3306,"peer port must be unprivileged and distinct from DB")
        require(bind[0]=="0.0.0.0" and bind[1]==plan["peer_container_port"],"manager must bind declared pod peer port")
        peers=network["peers"]
        require({p["replica_id"] for p in peers}==set(replicas)-{local} and len(peers)==len(replicas)-1,
                "exact complete nonlocal peer list required")
        keys=set()
        for peer in peers:
            endpoint(peer["endpoint"])
            key=peer["shared_key_hex"]
            require(re.fullmatch(r"[0-9a-f]{64}",key) and key not in keys,"distinct private HMAC pair keys required")
            keys.add(key)
        subnet=ipaddress.IPv4Network(plan["subnet"])
        gateway=ipaddress.IPv4Address(plan["gateway"])
        app_ip=ipaddress.IPv4Address(plan["pod_ip"])
        require(24<=subnet.prefixlen<=28 and gateway in subnet and app_ip in subnet and gateway!=app_ip
                and gateway not in (subnet.network_address,subnet.broadcast_address)
                and app_ip not in (subnet.network_address,subnet.broadcast_address),"invalid bounded bridge addresses")
        publication=plan.get("peer_publish")
        if publication is not None:
            require(set(publication)=={"host_ip","host_port"},"only one declared peer publication allowed")
            host_ip=ipaddress.IPv4Address(publication["host_ip"])
            require(not host_ip.is_unspecified and not host_ip.is_multicast and int(publication["host_port"])==bind[1],
                    "publication requires exact host IP and same peer port")
        require(set(plan)=={"subnet","gateway","pod_ip","peer_container_port","peer_publish"},"unknown network plan fields")
        return subnet

    def prepare(self,a,recovery=None):
        for get in (pwd.getpwuid,grp.getgrgid):
            try:
                account=get(1103)
            except KeyError:
                continue
            require(account[0]=="podmesh-manager","UID/GID1103 belongs to another host identity")
        for query in ("is-active","is-enabled"):
            require(run(["/usr/bin/systemctl",query,"podmesh-manager.service"],okay=(0,1,3,4)).returncode!=0,
                    "legacy manager remains active/enabled; explicit cutover required")
        protected(BASE,0o700)
        require(not self.root.exists() and not self.root.is_symlink(),"instance path already present")
        for sibling in BASE.iterdir():
            if sibling.is_dir() and (sibling/"instance.json").exists():
                require(json.loads((sibling/"instance.json").read_text()).get("phase")=="rolled-back","another private manager instance remains")
        for suffix in ("-app.service","-db.service",".target"):
            path=UNITS/(self.prefix+suffix)
            require(not path.exists() and not path.is_symlink(),"unit already present")
        for path in (a.configuration,a.network_plan,a.application_password,a.database_root_password):
            protected(path,0o600)
            require(stat.S_ISREG(path.lstat().st_mode),"private input must be regular root0600")
        config=json.loads(a.configuration.read_text())
        plan=json.loads(a.network_plan.read_text())
        machine=Path("/etc/machine-id").read_text().strip()
        subnet=self.validate_inputs(config,plan,machine)
        bridge="pm"+hashlib.sha256((self.scope+":"+self.manifest["bundle"]).encode()).hexdigest()[:12]
        links=json.loads(run(["/usr/sbin/ip","-j","link","show"]).stdout)
        require(all(link["ifname"]!=bridge for link in links),"private bridge interface already exists")
        routes=json.loads(run(["/usr/sbin/ip","-j","route","show","table","all"]).stdout)
        addresses=json.loads(run(["/usr/sbin/ip","-j","address","show"]).stdout)
        for route in routes:
            destination=route.get("dst")
            if destination and destination!="default":
                candidate=ipaddress.ip_network(destination,strict=False)
                require(candidate.version!=4 or not subnet.overlaps(candidate),"bridge overlaps actual host route")
        local_ips={a["local"] for interface in addresses for a in interface.get("addr_info",[])}
        if plan["peer_publish"]:
            require(plan["peer_publish"]["host_ip"] in local_ips,"publication IP is not owned by this host")
            listeners=run(["/usr/bin/ss","-H","-ltn"]).stdout.decode().splitlines()
            require(not any(line.split()[3].rsplit(":",1)[-1]==str(plan["peer_container_port"]) for line in listeners if len(line.split())>=4),
                    "peer publication port already used")
        passwords=[]
        for path in (a.application_password,a.database_root_password):
            value=path.read_bytes().rstrip(b"\r\n")
            require(0<len(value)<=4096 and not any(c in value for c in (b"\n",b"\r",b"\0")),"single-line credential required")
            passwords.append(value)
        require(passwords[0]!=passwords[1],"application and administrator credentials must differ")
        self.root.mkdir(mode=0o700)
        for name,uid in (("app-config",1103),("api",1103),("db-admin",0),("r",0),("tmp",0),("networks",0)):
            (self.root/name).mkdir(mode=0o700)
            os.chown(self.root/name,uid,uid)
        write(self.root/"app-config/config.json",json.dumps(config)+"\n",uid=1103,gid=1103)
        profile={"engine":"mariadb","mariadb":{"host":"127.0.0.1","port":3306,"user":"podmesh-manager",
                 "database":"podmesh-manager","password_file":"/etc/podmesh-manager/passwd"}}
        write(self.root/"app-config/store.json",json.dumps(profile)+"\n",uid=1103,gid=1103)
        write(self.root/"app-config/passwd",passwords[0],uid=1103,gid=1103)
        write(self.root/"db-admin/passwd",passwords[1])
        write(self.root/"network-plan.json",json.dumps(plan)+"\n")
        write(self.root/"storage.conf",f'[storage]\ndriver="vfs"\ngraphroot="{self.root}/graphroot"\nrunroot="{private_runroot(self.root)}"\n')
        write(self.root/"containers.conf",f'[engine]\ntmp_dir="{self.root}/tmp"\n[network]\nnetwork_config_dir="{self.root}/networks"\n')
        self.r={"scope":self.scope,"bundle":self.manifest["bundle"],"machine_id":machine,"phase":"preparing",
                "units":{},"resources":[],"network_plan":plan,"bridge_interface":bridge,"replica_id":config["network"]["replica_id"],
                "root_hashes":{n:sha(self.root/n) for n in ("storage.conf","containers.conf","network-plan.json","db-admin/passwd")},
                "app_hashes":{n:sha(self.root/"app-config"/n) for n in ("config.json","store.json","passwd")}}
        if recovery is not None:
            self.r["recovery"]=recovery
        jsonwrite(self.receipt,self.r)
        for archive,image in (("application.oci.tar",self.manifest["application_image"]),("database.oci.tar",self.manifest["database_image"])):
            self.podman("load","--input",BUNDLE/archive,timeout=300)
            require(image_id(json.loads(self.podman("image","inspect",image).stdout)[0]["Id"])==image_id(image),"loaded image differs")
        if recovery is not None:
            for entry in recovery["image_inputs"]:
                path=Path(entry["path"])
                protected(path,0o600)
                require(sha(path)==entry["sha256"],"captured image transport changed")
                self.podman("load","--input",path,timeout=300)
                require(image_id(json.loads(self.podman("image","inspect",entry["image_id"]).stdout)[0]["Id"])
                        ==entry["image_id"],"captured image identity changed")
        labels=["--label",LABEL+"="+self.scope,"--label","io.podmesh.bundle="+self.r["bundle"]]
        def create(kind,name,options,image=None,command=()):
            require(self.inspect(kind,name) is None,"resource already exists")
            record={"kind":kind,"name":name,"id":None}
            self.r["resources"].append(record)
            self.save()
            if kind=="container":
                result=self.podman("create","--name",name,*labels,*options,image,*command)
            else:
                naming=[name] if kind in ("volume","network") else ["--name",name]
                result=self.podman(kind,"create",*options,*labels,*naming)
            record["id"]=name if kind in ("volume","network") else result.stdout.decode().strip()
            if kind=="network":
                self.r["network_id"]=self.inspect("network",name)["id"]
            self.save()
            self.owned(kind,record["id"])
            return record["id"]
        appvol=create("volume",self.prefix+"-app",[])
        dbvol=create("volume",self.prefix+"-db",[])
        path=Path(self.owned("volume",appvol)["Mountpoint"])
        require(path.is_relative_to(self.root/"graphroot"),"volume outside own store")
        protected(path)
        os.chown(path,1103,1103)
        path.chmod(0o700)
        for name in self.podman("network","ls","--quiet").stdout.decode().split():
            network=self.inspect("network",name)
            for existing in network.get("subnets",[]):
                candidate=ipaddress.ip_network(existing["subnet"])
                require(candidate.version!=4 or not subnet.overlaps(candidate),"bridge overlaps configured network")
        network=create("network",self.prefix+"-net",["--subnet",str(subnet),"--gateway",plan["gateway"],"--interface-name",bridge])
        require(self.owned("network",network)["network_interface"]==bridge,"private bridge name differs")
        options=["--network",network,"--ip",plan["pod_ip"],"--share=net","--userns=host"]
        if recovery is not None:
            options += ["--infra-image",recovery["infra_image"]]
        if plan["peer_publish"]:
            publication=plan["peer_publish"]
            options += ["--publish",f'{publication["host_ip"]}:{publication["host_port"]}:{plan["peer_container_port"]}/tcp']
        self.r["pod"]=create("pod",self.prefix,options)
        self.save()
        common=["--pod",self.r["pod"],"--pull=never","--image-volume=ignore","--pid=private","--ipc=private","--uts=private",
                "--memory=512m","--memory-swap=512m","--cpus=1"]
        self.r["db"]=create("container",self.prefix+"-db",[*common,"--hostname",self.prefix+"-db",
                   "--volume",dbvol+":/var/lib/mysql","--volume",str(self.root/"app-config/passwd")+":/run/app-passwd:ro",
                   "--volume",str(self.root/"db-admin")+":/run/db-admin:ro","--env=MARIADB_USER=podmesh-manager",
                   "--env=MARIADB_DATABASE=podmesh-manager","--env=MARIADB_PASSWORD_FILE=/run/app-passwd",
                   "--env=MARIADB_ROOT_PASSWORD_FILE=/run/db-admin/passwd"],self.manifest["database_image"],["mariadbd","--bind-address=127.0.0.1"])
        self.r["app"]=create("container",self.prefix+"-app",[*common,"--hostname",self.prefix+"-app",
                   "--user=1103:1103","--cap-drop=ALL","--security-opt=no-new-privileges",
                   "--read-only","--read-only-tmpfs=false","--volume",appvol+":/var/lib/podmesh-manager",
                   "--volume",str(self.root/"app-config")+":/etc/podmesh-manager:ro","--volume",str(self.root/"api")+":/run/podmesh-manager",
                   "--env=PODMESH_STORE_PROFILE=/etc/podmesh-manager/store.json","--env=PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers"],
                   self.manifest["application_image"])
        self.save()
        self.check_container("db")
        self.check_container("app")
        self.generate_units()
        self.r["phase"]="recovery-incomplete" if recovery is not None else "prepared"
        self.save()
        run(["/usr/bin/systemctl","daemon-reload"])

    def check_container(self,role):
        item=self.owned("container",self.r[role])
        require(item is not None,"unit container absent")
        require(image_id(item["Image"])==image_id(self.manifest["application_image" if role=="app" else "database_image"]),"unit image differs")
        expected={"/var/lib/podmesh-manager","/etc/podmesh-manager","/run/podmesh-manager"} if role=="app" else {"/var/lib/mysql","/run/app-passwd","/run/db-admin"}
        require(len(item["Mounts"])==len(expected) and {m["Destination"] for m in item["Mounts"]}==expected,"unexpected unit mount")
        bindings={"/etc/podmesh-manager":self.root/"app-config","/run/podmesh-manager":self.root/"api",
                  "/run/app-passwd":self.root/"app-config/passwd","/run/db-admin":self.root/"db-admin"}
        for mount in item["Mounts"]:
            destination=mount["Destination"]
            if destination in bindings:
                require(Path(mount["Source"])==bindings[destination] and bool(mount["RW"])==(destination=="/run/podmesh-manager"),"bind source/access differs")
            else:
                volume=self.owned("volume",self.prefix+("-app" if role=="app" else "-db"))
                require(volume is not None and mount["Source"]==volume["Mountpoint"] and mount["RW"],"persistent volume differs")
        host=item["HostConfig"]
        require(not host["Privileged"] and not host.get("PortBindings"),"container privilege or published ports differ")
        for field in ("PidMode","IpcMode","UTSMode"):
            require(host[field] in ("","private"),"container namespace differs")
        pod=self.owned("pod",self.r["pod"])
        require(pod is not None and item["Pod"]==self.r["pod"] and host["NetworkMode"]=="container:"+pod["InfraContainerID"],"private pod namespace differs")
        infra=self.inspect("container",pod["InfraContainerID"])
        require(infra is not None and infra["Id"]==pod["InfraContainerID"] and infra["Pod"]==self.r["pod"],"infra identity differs")
        require(not infra["Mounts"],"infra must not carry inherited application/database volumes")
        infra_image=json.loads(self.podman("image","inspect",infra["Image"]).stdout)[0]
        require(not infra_image["Config"].get("Volumes"),"infra image declares inherited volumes; real pause image required")
        # Infra is the only endpoint publication carrier. Never allow DB3306.
        published=infra["HostConfig"].get("PortBindings") or {}
        grant=self.r["network_plan"]["peer_publish"]
        if grant:
            expected_ports={str(self.r["network_plan"]["peer_container_port"])+"/tcp":[{"HostIp":grant["host_ip"],"HostPort":str(grant["host_port"])}]}
            require(published==expected_ports,"infra publication differs from exact peer grant")
        else:
            require(not published,"infra has undeclared publication")
        net=self.owned("network",self.prefix+"-net")
        require(net is not None and net["name"]==self.prefix+"-net","private bridge identity differs")
        require(net["subnets"]==[{"subnet":self.r["network_plan"]["subnet"],"gateway":self.r["network_plan"]["gateway"]}],"private bridge subnet differs")
        self.check_infra_network(pod,infra)
        if role=="app":
            require(item["Config"]["User"]=="1103:1103" and host["ReadonlyRootfs"] and not item.get("EffectiveCaps",[]) and "no-new-privileges" in host["SecurityOpt"],"application isolation differs")
            env=dict(s.split("=",1) for s in item["Config"]["Env"])
            require(env.get("PODMESH_STORE_PROFILE")=="/etc/podmesh-manager/store.json" and env.get("PODMESH_MANAGER_NETWORK_MODE")=="authenticated-static-peers","application profile/network mode differs")
        return item

    def check_infra_network(self,pod,infra):
        # Creation is not an observation of an assigned network namespace. Verify
        # the immutable planned scope first, and actual addresses once running.
        declared=pod["InfraConfig"]
        require(not declared["HostNetwork"] and declared["Networks"]==[self.prefix+"-net"]
                and declared["StaticIP"]==self.r["network_plan"]["pod_ip"],"planned pod network differs")
        if infra["State"]["Running"]:
            attached=infra["NetworkSettings"]["Networks"]
            require(set(attached)=={self.prefix+"-net"},"infra joined an undeclared network")
            actual=attached[self.prefix+"-net"]
            require(actual["IPAddress"]==self.r["network_plan"]["pod_ip"]
                    and actual["Gateway"]==self.r["network_plan"]["gateway"],"actual infra bridge addresses differ")

    def control_api(self,operation):
        require(operation in ("status","shutdown"),"only product control/readiness operations allowed")
        return self.control_request({"operation":operation})

    def control_request(self,request):
        require(request.get("operation") in ("status","shutdown","append_observation"),"unsupported control operation")
        if request.get("operation")=="append_observation":
            require(self.r.get("recovery",{}).get("stage")=="state-verified", "append proof requires verified recovery")
        self.check_container("app")
        script='id -u; id -g; id -un; exec socat -t 10 -T 10 STDIO UNIX-CONNECT:/run/podmesh-manager/control.sock'
        result=self.podman("exec","-i","--user=1103:1103",self.r["app"],"/bin/sh","-eu","-c",script,
                           input_data=json.dumps(request).encode(),timeout=15)
        require(len(result.stdout)<=65536,"control response exceeds bound")
        lines=result.stdout.decode().splitlines()
        require(len(lines)>=4 and lines[:3]==["1103","1103","podmesh-manager"],"actual control caller differs")
        return json.loads("\n".join(lines[3:]))

    def wait_db(self):
        script='export MYSQL_PWD="$(cat /run/app-passwd)"; exec /usr/bin/mariadb --no-defaults --protocol=TCP --host=127.0.0.1 --port=3306 --user=podmesh-manager --database=podmesh-manager --batch --skip-column-names --execute="SELECT 1"'
        deadline=time.monotonic()+60
        while time.monotonic()<deadline:
            result=self.podman("exec","--user=1103:1103",self.r["db"],"/bin/sh","-eu","-c",script,okay=(0,1),timeout=15)
            if result.returncode==0 and result.stdout.strip()==b"1":
                return
            time.sleep(1)
        raise Refusal("private database application-account access failed")

    def ready_app(self):
        deadline=time.monotonic()+60
        endpoint=self.root/"api/control.sock"
        while time.monotonic()<deadline:
            item=self.check_container("app")
            require(item["State"]["Running"],"application exited before readiness")
            if endpoint.exists():
                s=endpoint.lstat()
                require(stat.S_ISSOCK(s.st_mode) and s.st_uid==s.st_gid==1103 and stat.S_IMODE(s.st_mode)==0o600,"control socket ownership differs")
                with socket.socket(socket.AF_UNIX) as peer:
                    peer.settimeout(2)
                    peer.connect(str(endpoint))
                    pid,uid,gid=struct.unpack("3i",peer.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
                require((uid,gid)==(1103,1103) and pid==item["State"]["Pid"],"control peer PID/account differs")
                require(sha(Path(f"/proc/{pid}/exe"))==self.manifest["payload_sha256"]["podmesh-managerd"],"running binary differs")
                response=self.control_api("status")
                require(response["replica_id"]==self.r["replica_id"] and not response["activation_authority"] and not response["store_closed"],"manager identity/store/authority readiness refused")
                if response["catch_up"]["caught_up"]:
                    marker=self.root/"ready-app.json"
                    if marker.exists():
                        protected(marker,0o600)
                    jsonwrite(marker,{"pid":pid,"socket_device":s.st_dev,"socket_inode":s.st_ino,
                                      "source":self.manifest["binary_source_revision"],"binary_sha256":sha(Path(f"/proc/{pid}/exe")),
                                      "catch_up_by":response["catch_up"]["caught_up_by"]},replace=marker.exists())
                    return
            time.sleep(0.2)
        raise Refusal("manager readiness timed out; peer convergence is not inferred")

    def shutdown_app(self):
        item=self.check_container("app")
        acknowledged=False
        if item["State"]["Running"]:
            require(self.control_api("shutdown")=={"shutdown_requested":True},"typed manager shutdown not acknowledged")
            acknowledged=True
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            item=self.owned("container",self.r["app"])
            require(item is not None,"application disappeared during shutdown")
            state=item["State"]
            if not state["Running"] and state["Pid"]==0:
                require(state["ExitCode"]==0 and not state.get("OOMKilled",False),"application exit was not clean")
                require(not (self.root/"api/control.sock").exists(),"control socket remains; preserve it")
                marker=self.root/"shutdown-app.json"
                if marker.exists():
                    protected(marker,0o600)
                    previous=json.loads(marker.read_text())
                    if isinstance(state.get("FinishedAt"),str) and state["FinishedAt"] and previous.get("finished_at")==state["FinishedAt"]:
                        acknowledged=acknowledged or previous.get("typed_request_acknowledged",False)
                jsonwrite(marker,{"typed_request_acknowledged":acknowledged,"actual_exit":0,"socket_absent":True,
                                  "finished_at":state.get("FinishedAt"),"forced_signal_used":False},replace=marker.exists())
                return
            time.sleep(0.1)
        raise Refusal("typed shutdown timed out; no automatic signal fallback")

    def generate_units(self):
        target=self.prefix+".target"
        db=self.prefix+"-db.service"
        app=self.prefix+"-app.service"
        helper=f"/usr/bin/python3 {BUNDLE}/instance.py --scope {self.scope}"
        common=(f'Environment="CONTAINERS_STORAGE_CONF={self.root}/storage.conf" "CONTAINERS_CONF={self.root}/containers.conf"\n'
                'Environment="PATH=/usr/sbin:/usr/bin:/sbin:/bin" "HOME=/root"\n'
                'UnsetEnvironment=CONTAINER_HOST CONTAINER_CONNECTION PODMESH_MARIADB_DSN PODMESH_HOST_ADAPTER_SOCKET\n')
        texts={target:f"[Unit]\nDescription=Private PodMesh manager {self.scope}\nRequires={db} {app}\nAfter={app}\n"}
        texts[db]=(f"[Unit]\nDescription=Private manager database\nPartOf={target}\nBefore={app}\n[Service]\nType=simple\nUser=root\n{common}"
                   f"ExecStart=/usr/bin/podman start --attach {self.r['db']}\nExecStartPost={helper} hook --role db --event ready\n"
                   f"ExecStop=/usr/bin/podman stop --time 20 {self.r['db']}\nTimeoutStartSec=90\nTimeoutStopSec=30\nRestart=no\n")
        texts[app]=(f"[Unit]\nDescription=Private manager application\nPartOf={target}\nRequires={db}\nAfter={db}\n[Service]\nType=simple\nUser=root\n{common}"
                    f"ExecCondition={helper} hook --role app --event permit\n"
                    f"ExecStart=/usr/bin/podman start --attach --sig-proxy=false {self.r['app']}\nExecStartPost={helper} hook --role app --event ready\n"
                    f"ExecStop={helper} hook --role app --event stop\nTimeoutStartSec=90\nTimeoutStopSec=45\nKillMode=process\nSendSIGKILL=no\nRestart=no\n")
        for name,text in texts.items():
            self.r["units"][name]=hashlib.sha256(text.encode()).hexdigest()
            self.save()
            write(UNITS/name,text,mode=0o644)

    def hook(self,role,event):
        self.read()
        self.unitcheck()
        self.check_container(role)
        if role=="app" and event=="permit":
            require(self.r["phase"] in ("prepared","started","stopped","restored-stopped"),
                    "source capture or incomplete instance cannot start application")
            require(not self.r.get("recovery") or self.r["recovery"].get("stage")=="state-verified",
                    "incomplete recovery cannot start application")
            return
        if role=="db":
            require(event=="ready","unsupported database callback")
            self.wait_db()
        elif event=="ready":
            self.ready_app()
        else:
            self.shutdown_app()

    def stop_units(self):
        if self.r.get("app"):
            item=self.owned("container",self.r["app"])
            if item and item["State"]["Running"]:
                self.shutdown_app()  # Before requesting service stop; failed ack preserves DB/app.
        if self.r["units"]:
            run(["/usr/bin/systemctl","stop",*self.r["units"]],okay=(0,5),timeout=120)
        for role in ("app","db"):
            if self.r.get(role):
                item=self.owned("container",self.r[role])
                require(item is None or (not item["State"]["Running"] and item["State"]["Pid"]==0
                        and item["State"]["ExitCode"]==0 and not item["State"].get("OOMKilled",False)),"unit remains active or exited uncleanly")

    def control(self,action):
        self.read()
        require(self.r["phase"] in ("prepared","started","stopped","restored-stopped"),"instance incomplete or rolled back")
        require(not self.r.get("recovery") or self.r["recovery"].get("stage")=="state-verified",
                "incomplete recovery cannot start application")
        self.unitcheck()
        for record in self.r["resources"]:
            require(record["id"] and self.owned(record["kind"],record["id"]),"recorded resource missing")
        self.check_container("db")
        self.check_container("app")
        if action=="start":
            try:
                run(["/usr/bin/systemctl","start",self.prefix+".target"],timeout=240)
            except (Refusal,subprocess.TimeoutExpired):
                self.stop_units()
                raise Refusal("start failed; inspect retained state and private readiness evidence")
            self.r["phase"]="started"
        else:
            self.stop_units()
            self.r["phase"]="stopped"
        self.save()

    def no_foreign_containers(self):
        items=json.loads(self.podman("ps","--all","--format=json").stdout)
        own={r["id"] for r in self.r["resources"] if r["kind"]=="container" and r["id"]}
        if self.r.get("pod"):
            pod=self.owned("pod",self.r["pod"])
            if pod:
                own.add(pod["InfraContainerID"])
        require(all(i.get("Id",i.get("ID")) in own for i in items),"foreign container remains in store; rollback refused")

    def rollback(self):
        self.read()
        if self.r["phase"]=="rolled-back":
            return
        for name,digest in self.r["units"].items():
            path=UNITS/name
            if path.exists():
                protected(path,0o644)
                require(sha(path)==digest,"unit changed; rollback refused")
        self.no_foreign_containers()
        for record in self.r["resources"]:
            if record["id"]:
                self.owned(record["kind"],record["id"])
            else:
                require(self.inspect(record["kind"],record["name"]) is None,"interrupted creation lacks observed ID; do not adopt it")
        self.stop_units()
        self.no_foreign_containers()
        for name in self.r["units"]:
            run(["/usr/bin/systemctl","disable",name],okay=(0,1))
        for record in reversed(self.r["resources"]):
            if not record["id"] or record["kind"]=="volume":
                continue
            item=self.owned(record["kind"],record["id"])
            if item:
                if record["kind"]=="container":
                    require(not item["State"]["Running"],"container remains running")
                if record["kind"]=="pod":
                    self.podman("pod","stop",record["id"])
                self.podman(record["kind"],"rm",record["id"])
        for name,digest in self.r["units"].items():
            path=UNITS/name
            if path.exists():
                require(sha(path)==digest,"unit changed during rollback")
                path.unlink()
        run(["/usr/bin/systemctl","daemon-reload"])
        self.r["phase"]="rolled-back"
        self.save()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scope",required=True)
    sub=parser.add_subparsers(dest="action",required=True)
    prep=sub.add_parser("prepare")
    prep.add_argument("--configuration",required=True,type=Path)
    prep.add_argument("--network-plan",required=True,type=Path)
    prep.add_argument("--application-password",required=True,type=Path)
    prep.add_argument("--database-root-password",required=True,type=Path)
    for name in ("start","stop","rollback","status"):
        sub.add_parser(name)
    capture=sub.add_parser("capture",help="Typed stop and immutable complete recovery capture")
    capture.add_argument("--capture-id",required=True)
    release=sub.add_parser("release-for-restore",help="Release only own source after verified offguest capture")
    release.add_argument("--capture",required=True,type=Path)
    release.add_argument("--transfer",required=True,type=Path)
    restore=sub.add_parser("restore",help="Same-host original-replica rebind into a NEW target scope")
    restore.add_argument("--capture",required=True,type=Path)
    restore.add_argument("--network-plan",required=True,type=Path)
    restore.add_argument("--recovery-id",required=True)
    verify=sub.add_parser("verify-restored",help="Replay preserved receipt and execute one fresh bounded observation")
    verify.add_argument("--operation-id",required=True)
    hook=sub.add_parser("hook",help=argparse.SUPPRESS)
    hook.add_argument("--role",choices=("app","db"),required=True)
    hook.add_argument("--event",choices=("ready","stop","permit"),required=True)
    a=parser.parse_args()
    require(os.geteuid()==0,"separate root operator required")
    os.umask(0o077)
    candidate=Instance(a.scope)
    if a.action in ("prepare","restore"):
        if not BASE.exists():
            BASE.mkdir(mode=0o700)
        protected(BASE,0o700)
        lock_path=BASE/"prepare.lock"
    elif a.action=="hook":
        candidate.hook(a.role,a.event)
        return
    else:
        candidate.read()
        lock_path=candidate.root/"operator.lock"
    fd=os.open(lock_path,os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,"r+") as lock:
        protected(lock_path,0o600)
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        if a.action=="prepare":
            candidate.prepare(a)
        elif a.action in ("capture","release-for-restore","restore","verify-restored"):
            path=BUNDLE/"recovery.py"
            require("recovery.py" in candidate.manifest["payload_sha256"],"native recovery payload missing")
            spec=importlib.util.spec_from_file_location("podmesh_manager_recovery",path)
            module=importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            api=SimpleNamespace(BASE=BASE,UNITS=UNITS,Instance=Instance,run=run,write=write,
                                jsonwrite=jsonwrite,protected=protected,image_id=image_id)
            action={"capture":module.capture,"release-for-restore":module.release,
                    "restore":module.restore,"verify-restored":module.verify}[a.action]
            try:
                result=action(candidate,a,api)
            except ValueError as error:
                raise Refusal("native recovery refused: "+str(error)) from None
            except (KeyError,TypeError,OSError):
                raise Refusal("native recovery refused; preserve private inputs, intents and observed state") from None
            print(json.dumps(result))
        elif a.action in ("start","stop"):
            candidate.control(a.action)
        elif a.action=="rollback":
            candidate.rollback()
        else:
            print(json.dumps({"scope":candidate.scope,"bundle":candidate.r["bundle"],"phase":candidate.r["phase"],
                              "resources":candidate.r["resources"],"data_retained":True},indent=2))


if __name__=="__main__":
    try:
        main()
    except Refusal as error:
        sys.exit(str(error))
    except Exception:
        sys.exit("private manager administration failed; preserve state and inspect privately")
