#!/usr/bin/env python3
"""Fail-closed comparison for manager2 live-activation evidence."""
import argparse, hashlib, json, re, sys
from pathlib import Path

SCHEMA = "podmesh-manager-live-activation-evidence/v2"
OUT_SCHEMA = "podmesh-manager-live-activation-comparison/v2"
COMMITMENT = re.compile(r"^sha256:[0-9a-f]{64}$")
SHA = re.compile(r"^[0-9a-f]{64}$")
STAGES = ("pre-activation", "active-baseline", "converged", "post-cleanup")
SHUTDOWN = {"schema_version":"podmesh-manager-graceful-shutdown/v1","typed_request_acknowledged":True,"process_exited_successfully":True,"service_inactive":True,"control_socket_absent":True,"forced_signal_used":False}

def obj(v, label, fields):
    if not isinstance(v, dict) or set(v) != set(fields): raise ValueError(f"{label}: unsafe shape")
    return v
def string(v, label):
    if not isinstance(v, str): raise ValueError(f"{label}: expected string")
def integer(v, label, minimum=0):
    if type(v) is not int or v < minimum: raise ValueError(f"{label}: expected integer >= {minimum}")
def boolean(v, label):
    if type(v) is not bool: raise ValueError(f"{label}: expected boolean")
def commit(v, label):
    if not isinstance(v, str) or not COMMITMENT.fullmatch(v): raise ValueError(f"{label}: invalid private commitment")
def sha(v, label):
    if not isinstance(v, str) or not SHA.fullmatch(v): raise ValueError(f"{label}: invalid SHA-256")

def read(path):
    path = Path(path)
    raw = path.read_bytes()
    sidecar = Path(f"{path}.sha256")
    fields = sidecar.read_text(encoding="ascii").strip().split()
    if len(fields) != 2 or not SHA.fullmatch(fields[0]) or fields[1] not in {str(path), path.name}:
        raise ValueError(f"{path}: invalid evidence checksum sidecar")
    if hashlib.sha256(raw).hexdigest() != fields[0]:
        raise ValueError(f"{path}: evidence checksum mismatch")
    value=json.loads(raw)
    fields=("schema_version","host_alias","stage","package","configuration","dropin","service","manager_process","paths","listeners","stability","inspection","graceful_shutdown")
    obj(value, str(path), fields)
    if value["schema_version"] != SCHEMA or value["stage"] not in STAGES: raise ValueError(f"{path}: unsupported evidence schema or stage")
    string(value["host_alias"], f"{path}.host_alias")
    return value

def validate_inspection(v, label):
    if v is None: return
    fields=("schema_version","logical_manager_commitment","replica_commitment","logical_history_sha256","sqlite_integrity_result","history_count","receipt_count","audit_event_count","incomplete_attempt_count")
    obj(v,label,fields)
    if v["schema_version"] != 3 or v["sqlite_integrity_result"] != "ok": raise ValueError(f"{label}: invalid read-only inspection")
    commit(v["logical_manager_commitment"],label); commit(v["replica_commitment"],label); sha(v["logical_history_sha256"],label)
    for f in ("history_count","receipt_count","audit_event_count","incomplete_attempt_count"): integer(v[f],f"{label}.{f}")

def validate(v,label):
    p=obj(v["package"],f"{label}.package",("name","version","binary_sha256","dpkg_verify"))
    if p["name"]!="podmesh-manager" or p["dpkg_verify"]!="clean": raise ValueError(f"{label}.package: candidate is not proven installed and clean")
    string(p["version"],f"{label}.package.version"); sha(p["binary_sha256"],f"{label}.package.binary_sha256")
    c=obj(v["configuration"],f"{label}.configuration",("document_commitment","logical_manager_commitment","local_replica_commitment","local_host_commitment","topology_commitment","peer_count","peers"))
    for f in ("document_commitment","logical_manager_commitment","local_replica_commitment","local_host_commitment","topology_commitment"): commit(c[f],f"{label}.configuration.{f}")
    integer(c["peer_count"],f"{label}.configuration.peer_count")
    if c["peer_count"] != 2 or not isinstance(c["peers"],list) or len(c["peers"]) != 2: raise ValueError(f"{label}.configuration: exactly two peers are required")
    ids=set()
    for i,peer in enumerate(c["peers"]):
        obj(peer,f"{label}.configuration.peers[{i}]",("replica_id_commitment","endpoint_commitment","shared_key_commitment"))
        for f in peer: commit(peer[f],f"{label}.configuration.peers[{i}].{f}")
        ids.add(peer["replica_id_commitment"])
    if len(ids)!=2: raise ValueError(f"{label}.configuration: duplicate peer")
    d=obj(v["dropin"],f"{label}.dropin",("present","sha256","semantic_limits","packaged_fragment_sha256","inherited_deny_all","effective_policy_configured","effective_policy_commitment"))
    boolean(d["present"],f"{label}.dropin.present"); boolean(d["inherited_deny_all"],f"{label}.dropin.inherited_deny_all"); boolean(d["effective_policy_configured"],f"{label}.dropin.effective_policy_configured"); sha(d["packaged_fragment_sha256"],f"{label}.dropin.packaged_fragment_sha256")
    limits=obj(d["semantic_limits"],f"{label}.dropin.semantic_limits",("network_mode","address_families","peer_allow_count","peer_allow_prefix_length"))
    active={"network_mode":"authenticated-static-peers","address_families":["AF_UNIX","AF_INET"],"peer_allow_count":2,"peer_allow_prefix_length":32}
    absent={"network_mode":None,"address_families":[],"peer_allow_count":0,"peer_allow_prefix_length":None}
    if d["present"]:
        sha(d["sha256"],f"{label}.dropin.sha256"); commit(d["effective_policy_commitment"],f"{label}.dropin.effective_policy_commitment")
        if limits != active or not d["inherited_deny_all"] or not d["effective_policy_configured"]: raise ValueError(f"{label}.dropin: effective policy differs from contract")
    elif d["sha256"] is not None or limits != absent or d["effective_policy_configured"] or d["effective_policy_commitment"] is not None: raise ValueError(f"{label}.dropin: absent drop-in carries effective semantics")
    s=obj(v["service"],f"{label}.service",("load_state","active_state","sub_state","unit_file_state","main_pid","invocation_commitment","n_restarts","result","exec_main_code","exec_main_status"))
    for f in ("load_state","active_state","sub_state","unit_file_state","result","exec_main_code"): string(s[f],f"{label}.service.{f}")
    for f in ("main_pid","n_restarts","exec_main_status"): integer(s[f],f"{label}.service.{f}")
    if s["invocation_commitment"] is not None: commit(s["invocation_commitment"],f"{label}.service.invocation_commitment")
    proc=obj(v["manager_process"],f"{label}.manager_process",("count","pid","uid","argv_commitment","argv_count"))
    integer(proc["count"],f"{label}.manager_process.count"); integer(proc["argv_count"],f"{label}.manager_process.argv_count")
    for f in ("pid","uid"):
        if proc[f] is not None: integer(proc[f],f"{label}.manager_process.{f}",1)
    if proc["argv_commitment"] is not None: commit(proc["argv_commitment"],f"{label}.manager_process.argv_commitment")
    paths=obj(v["paths"],f"{label}.paths",("state","runtime","control_socket"))
    for name in ("state","runtime"):
        x=obj(paths[name],f"{label}.paths.{name}",("present","uid","gid","mode","content_commitment")); boolean(x["present"],f"{label}.paths.{name}.present")
        if x["present"]:
            integer(x["uid"],label); integer(x["gid"],label); string(x["mode"],label); commit(x["content_commitment"],label)
        elif any(x[f] is not None for f in ("uid","gid","mode","content_commitment")): raise ValueError(f"{label}.paths.{name}: absent path has metadata")
    sock=obj(paths["control_socket"],f"{label}.paths.control_socket",("present","uid","gid","mode")); boolean(sock["present"],label)
    if sock["present"]:
        integer(sock["uid"],label); integer(sock["gid"],label); string(sock["mode"],label)
    elif any(sock[f] is not None for f in ("uid","gid","mode")): raise ValueError(f"{label}.paths.control_socket: absent socket has metadata")
    listeners=obj(v["listeners"],f"{label}.listeners",("status","endpoint_commitment","tcp_listener_count","udp_listener_count"))
    if listeners["status"]!="available-successful": raise ValueError(f"{label}.listeners: unavailable observation")
    commit(listeners["endpoint_commitment"],label); integer(listeners["tcp_listener_count"],label); integer(listeners["udp_listener_count"],label)
    stability=obj(v["stability"],f"{label}.stability",("existing_services_commitment","podman_containers_commitment","routes","firewall"))
    commit(stability["existing_services_commitment"],label); commit(stability["podman_containers_commitment"],label)
    for f in ("routes","firewall"):
        x=obj(stability[f],f"{label}.stability.{f}",( "status","commitment"))
        if x["status"]!="available-successful": raise ValueError(f"{label}.stability.{f}: unavailable observation")
        commit(x["commitment"],label)
    validate_inspection(v["inspection"],f"{label}.inspection")
    if v["graceful_shutdown"] is not None and v["graceful_shutdown"] != SHUTDOWN: raise ValueError(f"{label}.graceful_shutdown: invalid exact typed-shutdown proof")

def inspection_bound(v):
    i=v["inspection"]; c=v["configuration"]
    return i is not None and i["logical_manager_commitment"]==c["logical_manager_commitment"] and i["replica_commitment"]==c["local_replica_commitment"]
def same_state_metadata(a,b): return all(a[f]==b[f] for f in ("present","uid","gid","mode"))

def host_failures(pre,base,conv,cleanup):
    failures=[]; captures=(pre,base,conv,cleanup)
    if [x["stage"] for x in captures] != list(STAGES): failures.append("stage order differs from the four-stage contract")
    if len({x["host_alias"] for x in captures}) != 1: failures.append("host aliases differ")
    for f in ("package","configuration"):
        if any(x[f]!=pre[f] for x in captures[1:]): failures.append(f"{f} changed across activation")
    if [x["dropin"]["present"] for x in captures] != [False,True,True,False]: failures.append("harness drop-in lifecycle is incomplete")
    if base["dropin"] != conv["dropin"]: failures.append("drop-in or effective systemd policy changed while active")
    if any(x["dropin"]["packaged_fragment_sha256"] != pre["dropin"]["packaged_fragment_sha256"] for x in captures[1:]): failures.append("packaged unit fragment changed")
    if (pre["service"]["active_state"],pre["service"]["unit_file_state"],pre["manager_process"]["count"]) != ("inactive","disabled",0): failures.append("pre-activation manager is not disabled and inactive")
    for label,x in (("active baseline",base),("converged",conv)):
        proc=x["manager_process"]; state=x["paths"]["state"]; runtime=x["paths"]["runtime"]; sock=x["paths"]["control_socket"]
        if (x["service"]["active_state"],x["service"]["sub_state"],x["service"]["unit_file_state"],proc["count"]) != ("active","running","disabled",1): failures.append(f"{label} service/process proof is incomplete")
        if x["service"]["main_pid"]!=proc["pid"] or proc["uid"] in (None,0) or proc["argv_commitment"] is None: failures.append(f"{label} PID, UID or arguments are not proven")
        if (x["listeners"]["tcp_listener_count"],x["listeners"]["udp_listener_count"]) != (1,0): failures.append(f"{label} listener set is not one TCP and zero UDP")
        if not state["present"] or state["uid"]!=proc["uid"] or state["mode"]!="750": failures.append(f"{label} state ownership or mode differs from contract")
        if not runtime["present"] or runtime["uid"]!=proc["uid"] or runtime["mode"]!="700": failures.append(f"{label} runtime ownership or mode differs from contract")
        if not sock["present"] or sock["uid"]!=proc["uid"] or sock["mode"]!="600": failures.append(f"{label} control socket ownership or mode differs from contract")
    if (base["service"]["main_pid"] != conv["service"]["main_pid"] or
            base["service"]["invocation_commitment"] != conv["service"]["invocation_commitment"] or
            base["service"]["n_restarts"] != 0 or conv["service"]["n_restarts"] != 0):
        failures.append("manager restarted or changed invocation during the active campaign")
    s=cleanup["service"]
    if (s["active_state"],s["sub_state"],s["unit_file_state"],s["result"],s["exec_main_status"],cleanup["manager_process"]["count"],cleanup["listeners"]["tcp_listener_count"],cleanup["listeners"]["udp_listener_count"]) != ("inactive","dead","disabled","success",0,0,0,0) or s["exec_main_code"] not in {"exited","0","1"}: failures.append("cleanup did not prove a successful manager exit with no listeners")
    if cleanup["paths"]["runtime"]["present"] or cleanup["paths"]["control_socket"]["present"]: failures.append("cleanup retained runtime or control socket")
    if [x["graceful_shutdown"] for x in captures[:3]] != [None,None,None] or cleanup["graceful_shutdown"] != SHUTDOWN: failures.append("typed graceful-shutdown proof is absent, misplaced or invalid")
    if any(not same_state_metadata(pre["paths"]["state"],x["paths"]["state"]) for x in captures[1:]): failures.append("retained state ownership or mode changed")
    for f in ("existing_services_commitment","podman_containers_commitment"):
        if any(x["stability"][f]!=pre["stability"][f] for x in captures[1:]): failures.append(f"{f} changed")
    for f in ("routes","firewall"):
        if any(x["stability"][f]["commitment"]!=pre["stability"][f]["commitment"] for x in captures[1:]): failures.append(f"{f} changed")
    if not inspection_bound(conv): failures.append("converged inspection is absent or not bound to this replica")
    if not inspection_bound(cleanup): failures.append("cleanup inspection is absent or not bound to this replica")
    if inspection_bound(conv) and inspection_bound(cleanup):
        a,b=conv["inspection"],cleanup["inspection"]
        counts=("history_count","receipt_count","audit_event_count")
        unchanged_counts=all(b[f]==a[f] for f in counts)
        if b["incomplete_attempt_count"]!=0 or any(b[f]<a[f] for f in counts) or (unchanged_counts and b["logical_history_sha256"]!=a["logical_history_sha256"]): failures.append("cleanup canonical inspection regressed")
    return failures

def three_host_failures(pres,bases,convs,cleanups):
    failures=[]
    for i in range(3): failures += [f"{pres[i]['host_alias']}: {m}" for m in host_failures(pres[i],bases[i],convs[i],cleanups[i])]
    configs=[x["configuration"] for x in bases]
    if len({json.dumps(x["package"],sort_keys=True) for x in bases})!=1: failures.append("active package/binary commitments differ")
    for f,name in (("logical_manager_commitment","logical manager"),("topology_commitment","topology")):
        if len({x[f] for x in configs})!=1: failures.append(f"{name} commitments differ")
    for f,name in (("local_replica_commitment","replica"),("local_host_commitment","host")):
        if len({x[f] for x in configs})!=3: failures.append(f"local {name} commitments are not distinct")
    replicas={x["local_replica_commitment"] for x in configs}; by_replica={x["configuration"]["local_replica_commitment"]:x for x in bases}
    for c in configs:
        if {p["replica_id_commitment"] for p in c["peers"]} != replicas-{c["local_replica_commitment"]}: failures.append("peer replica commitments are not reciprocal")
        for peer in c["peers"]:
            remote=by_replica.get(peer["replica_id_commitment"]); reverse=[] if remote is None else [p for p in remote["configuration"]["peers"] if p["replica_id_commitment"]==c["local_replica_commitment"]]
            if remote is None: failures.append("peer refers to an undeclared replica")
            elif len(reverse)!=1 or reverse[0]["shared_key_commitment"]!=peer["shared_key_commitment"]: failures.append("peer pair-key commitments are not symmetric")
            elif peer["endpoint_commitment"]!=remote["listeners"]["endpoint_commitment"]: failures.append("peer endpoint does not bind the remote listener")
    inspections=[x["inspection"] for x in convs]
    final_inspections=[x["inspection"] for x in cleanups]
    live_convergence=all(i is not None for i in inspections) and len({i["logical_history_sha256"] for i in inspections})==1 and all(i["history_count"]>=3 and i["incomplete_attempt_count"]==0 for i in inspections) and all(inspection_bound(x) for x in convs)
    final_convergence=all(i is not None for i in final_inspections) and len({i["logical_history_sha256"] for i in final_inspections})==1 and all(i["history_count"]>=3 and i["incomplete_attempt_count"]==0 for i in final_inspections) and all(inspection_bound(x) for x in cleanups)
    convergence=live_convergence and final_convergence
    if not live_convergence: failures.append("converged captures do not prove one complete logical history of at least three events")
    if not final_convergence: failures.append("post-cleanup captures do not retain one complete logical history")
    return failures,convergence

def main():
    p=argparse.ArgumentParser(); p.add_argument("--phase",required=True,choices=("host","three-host")); p.add_argument("--pre",required=True,nargs="+"); p.add_argument("--active-baseline",required=True,nargs="+"); p.add_argument("--converged",required=True,nargs="+"); p.add_argument("--cleanup",required=True,nargs="+"); a=p.parse_args()
    try:
        groups=[[read(path) for path in paths] for paths in (a.pre,a.active_baseline,a.converged,a.cleanup)]
        for label,group in zip(STAGES,groups):
            for value in group: validate(value,f"{label}:{value['host_alias']}")
        count=1 if a.phase=="host" else 3
        if any(len(g)!=count for g in groups): p.error(f"{a.phase} phase requires exactly {count} evidence file(s) for each stage")
        maps=[{x["host_alias"]:x for x in g} for g in groups]
        if any(len(m)!=count for m in maps) or any(set(m)!=set(maps[0]) for m in maps[1:]): raise ValueError("evidence aliases do not form matching distinct sets")
        aliases=sorted(maps[0]); ordered=[[m[x] for x in aliases] for m in maps]
        if a.phase=="host": failures=host_failures(*(g[0] for g in ordered)); convergence=False
        else: failures,convergence=three_host_failures(*ordered)
    except (OSError,ValueError,json.JSONDecodeError) as error:
        print(json.dumps({"status":"FAIL","error":str(error)},sort_keys=True)); return 2
    print(json.dumps({"schema_version":OUT_SCHEMA,"phase":a.phase,"subject":",".join(aliases),"status":"PASS" if not failures else "FAIL","failures":failures,"canonical_convergence_evidenced":convergence,"ha_claim":"absent"},sort_keys=True)); return 0 if not failures else 1
if __name__=="__main__": sys.exit(main())
