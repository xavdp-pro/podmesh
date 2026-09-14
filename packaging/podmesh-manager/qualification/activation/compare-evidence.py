#!/usr/bin/env python3
"""Fail-closed comparison for manager2 live-activation evidence."""
import argparse, hashlib, json, re, sys
from pathlib import Path

SCHEMA = "podmesh-manager-live-activation-evidence/v3"
OUT_SCHEMA = "podmesh-manager-live-activation-comparison/v3"
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
    fields = sidecar.read_text(encoding="ascii").strip().split(None, 1)
    # The sidecar was written on the producing host, so its directory is that host's; the digest binds the file name.
    if len(fields) != 2 or not SHA.fullmatch(fields[0]) or Path(fields[1]).name != path.name:
        raise ValueError(f"{path}: invalid evidence checksum sidecar")
    if hashlib.sha256(raw).hexdigest() != fields[0]:
        raise ValueError(f"{path}: evidence checksum mismatch")
    value=json.loads(raw)
    fields=("schema_version","host_alias","stage","package","configuration","dropin","service","manager_process","paths","listeners","stability","inspection","exchanges","graceful_shutdown")
    obj(value, str(path), fields)
    if value["schema_version"] != SCHEMA or value["stage"] not in STAGES: raise ValueError(f"{path}: unsupported evidence schema or stage")
    string(value["host_alias"], f"{path}.host_alias")
    return value

DERIVED=("schema_version","logical_manager_commitment","replica_commitment","logical_history_sha256",
         "receipt_set_sha256","audit_set_sha256","sqlite_integrity_result","history_count","receipt_count",
         "audit_event_count","incomplete_attempt_count","incomplete_attempts",
         "unaudited_import_receipt_count","unaudited_import_receipt_commitments",
         "imported_operation_commitments")
COUNTS=("history_count","receipt_count","audit_event_count","incomplete_attempt_count","unaudited_import_receipt_count")
DIRECTIONS=("inbound","outbound")
PHASES_BY_DIRECTION={
    "outbound": frozenset(("outbound_request_prepared","outbound_exchange_completed")),
    "inbound": frozenset(("inbound_request_observed","inbound_import_committed",
                           "inbound_refusal_recorded","inbound_reply_prepared",
                           "inbound_reply_write_observed","inbound_diagnostic_reply_written",
                           "inbound_connection_closed")),
}
TERMINAL_PHASES={
    "outbound": frozenset(("outbound_exchange_completed",)),
    "inbound": frozenset(("inbound_reply_write_observed","inbound_diagnostic_reply_written",
                          "inbound_connection_closed")),
}
OUTCOMES=frozenset(("accepted","authenticated_refusal","unauthenticated_diagnostic",
                    "unavailable","malformed","incomplete"))
UNDECIDED_CONDITIONS=("C2-sender-transferred-request-bytes","C4-partial-import","C5-signature",
                      "C6-exact-facts","C8-diagnostics")
TRUST_MODEL="collector-honest; cross-host joins only"
# The wire protocol prefixes each body with a big-endian u32 length
# (manager-network/src/lib.rs:1543, :1613), so a complete frame is exactly its announced
# body plus four bytes. Measured at four on every one of the fifty rows of a preserved
# campaign store, in both directions.
#
# Deriving this from the campaign instead was a tautology and not a weaker check: the row
# under test contributes to the set the overhead is taken from, so when the set had one
# member the per-row comparison could not fail. The comparator is pinned to a frozen
# candidate, so pinning its framing constant is the same commitment.
FRAME_OVERHEAD=4
AUTHORITIES=("peer-validated","pre-authentication")

def validate_attempt(v, label):
    # An incomplete attempt is published as a record, not as a tally. The count alone is
    # what made six of the eight accounting conditions impossible to evaluate: a difference
    # of totals describes nothing, because cleanup legitimately adds terminal rows.
    obj(v,label,("attempt_commitment","nonce_commitment","nonce_authority","operation_commitment","direction","last_phase"))
    commit(v["attempt_commitment"],f"{label}.attempt_commitment"); commit(v["nonce_commitment"],f"{label}.nonce_commitment")
    if v["operation_commitment"] is not None: commit(v["operation_commitment"],f"{label}.operation_commitment")
    if v["nonce_authority"] not in AUTHORITIES: raise ValueError(f"{label}.nonce_authority: unknown authority")
    if v["direction"] not in DIRECTIONS: raise ValueError(f"{label}.direction: unknown direction")
    string(v["last_phase"],f"{label}.last_phase")
    if v["last_phase"] not in PHASES_BY_DIRECTION[v["direction"]]:
        raise ValueError(f"{label}.last_phase: unknown phase for {v['direction']} direction")
    if v["last_phase"] in TERMINAL_PHASES[v["direction"]]:
        raise ValueError(f"{label}.last_phase: an incomplete attempt cannot have a terminal phase")
    # A pre-authentication nonce is minted locally and can never be joined across hosts. The
    # candidate confines it to inbound observation, so an outbound attempt claiming one is
    # not a value this collector could have observed.
    if v["nonce_authority"]=="pre-authentication" and v["direction"]!="inbound":
        raise ValueError(f"{label}: an outbound attempt cannot carry a pre-authentication nonce")

def validate_inspection(v, label):
    # Three states the collector can seal, and each is checked in full:
    #   None                  this capture did not inspect
    #   store_present false   it inspected and found no canonical store (a fresh host)
    #   store_present true    it inspected one
    # The absent case is not a shortcut past validation. Every derived field must be
    # exactly null: a capture claiming a fresh host while carrying a history digest, a
    # count or a commitment is refused, because that combination cannot be produced by
    # an honest collector and is precisely how an absent baseline would be forged.
    if v is None: return
    obj(v,label,("store_present",)+DERIVED)
    if not isinstance(v["store_present"], bool): raise ValueError(f"{label}.store_present: must be a boolean")
    if not v["store_present"]:
        if any(v[f] is not None for f in DERIVED): raise ValueError(f"{label}: absent store carries derived inspection fields")
        return
    if any(v[f] is None for f in DERIVED): raise ValueError(f"{label}: present store is missing derived inspection fields")
    if v["schema_version"] != 4 or v["sqlite_integrity_result"] != "ok": raise ValueError(f"{label}: invalid read-only inspection")
    commit(v["logical_manager_commitment"],label); commit(v["replica_commitment"],label)
    for f in ("logical_history_sha256","receipt_set_sha256","audit_set_sha256"): sha(v[f],f"{label}.{f}")
    for f in COUNTS: integer(v[f],f"{label}.{f}")
    if not isinstance(v["incomplete_attempts"],list): raise ValueError(f"{label}.incomplete_attempts: must be a list")
    for i,a in enumerate(v["incomplete_attempts"]): validate_attempt(a,f"{label}.incomplete_attempts[{i}]")
    # The count and the list are two statements of one fact, and a capture that disagrees
    # with itself is refused rather than reconciled in the comparator's favour.
    if len(v["incomplete_attempts"]) != v["incomplete_attempt_count"]:
        raise ValueError(f"{label}: incomplete_attempt_count does not match the published list")
    if not isinstance(v["unaudited_import_receipt_commitments"],list): raise ValueError(f"{label}.unaudited_import_receipt_commitments: must be a list")
    for i,c in enumerate(v["unaudited_import_receipt_commitments"]): commit(c,f"{label}.unaudited_import_receipt_commitments[{i}]")
    if len(v["unaudited_import_receipt_commitments"]) != v["unaudited_import_receipt_count"]:
        raise ValueError(f"{label}: unaudited_import_receipt_count does not match the published list")
    # Attempt identities must be distinct: a duplicate would let one strand be counted as
    # two, or two as one, and the predicate works on identities rather than on totals.
    ids=[a["attempt_commitment"] for a in v["incomplete_attempts"]]
    if len(set(ids)) != len(ids): raise ValueError(f"{label}: incomplete_attempts repeats an attempt identity")
    # Two attempts on one wire nonce would let a single honest receiver row account for
    # both -- and for ten. The predicate is about one attempt at a time, so a nonce names
    # at most one attempt.
    nonces=[a["nonce_commitment"] for a in v["incomplete_attempts"]]
    if len(set(nonces)) != len(nonces): raise ValueError(f"{label}: two incomplete attempts share one wire nonce")
    if not isinstance(v["imported_operation_commitments"],list): raise ValueError(f"{label}.imported_operation_commitments: must be a list")
    for i,c in enumerate(v["imported_operation_commitments"]): commit(c,f"{label}.imported_operation_commitments[{i}]")
    # The receipt_count bound that stood here is withdrawn, and withdrawn rather than kept
    # alongside because it is strictly weaker and rests on the wrong thing: receipt_count is
    # an integer the same host asserts, so raising it lifted the bound. The list is now
    # bound in validate() to the capture's own committed inbound rows, which no integer can
    # inflate. Keeping both meant the weaker rule answered first and masked the stronger one.
    if len(set(v["imported_operation_commitments"])) != len(v["imported_operation_commitments"]):
        raise ValueError(f"{label}: imported_operation_commitments repeats an operation")
    # An incomplete attempt is derived from audit rows, so a store reporting attempts while
    # reporting no audit event at all contradicts its own folded evidence. This check is a
    # same-host consistency check; later cross-stage rules preserve pre-existing debt.
    if v["incomplete_attempt_count"] > 0 and v["audit_event_count"] == 0:
        raise ValueError(f"{label}: incomplete attempts reported with no audit events to derive them from")

PHASE_RANK={direction:{phase:index for index,phase in enumerate(phases)}
            for direction,phases in {
                "outbound":("outbound_request_prepared","outbound_exchange_completed"),
                "inbound":("inbound_request_observed","inbound_import_committed",
                           "inbound_refusal_recorded","inbound_reply_prepared",
                           "inbound_reply_write_observed","inbound_diagnostic_reply_written",
                           "inbound_connection_closed"),
            }.items()}

# A folded row's `outcomes` is the UNION over the audit rows it collapses, so any check
# comparing it to a single value is unsatisfiable. Every row carries `incomplete` from its
# first phase, where nothing is decided yet. Measured over 4320 folded rows of a live
# three-host campaign, the vocabulary is exactly three shapes:
#     ['accepted','incomplete']    3997   a completed exchange
#     ['incomplete']                211   a strand: nothing decided
#     ['incomplete','unavailable']  112   a transfer that failed part-way
# So "this exchange completed" is `accepted` present and `unavailable` absent, and
# `outcomes == ["accepted"]` can never be true of anything.
def receiver_replica_of(cleanups, host_index):
    return cleanups[host_index]["inspection"]["replica_commitment"]

def completed(row):
    o=set(row["outcomes"])
    return "accepted" in o and "unavailable" not in o

TERMINAL_IMPORT_PHASE="inbound_import_committed"

def nonterminal(r):
    """Whether a folded exchange row has no terminal phase for its direction."""
    return not (set(r["phases_reached"]) & TERMINAL_PHASES[r["direction"]])

def corroborates_attempt(row, attempt):
    """Bind one published incomplete attempt to its exact same-host folded row."""
    direction=attempt["direction"]
    phases=row["phases_reached"]
    return (row["direction"]==direction
            and row["nonce_authority"]==attempt["nonce_authority"]
            and row["operation_commitment"]==attempt["operation_commitment"]
            and nonterminal(row)
            and attempt["last_phase"]==max(phases,key=PHASE_RANK[direction].get))


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
    semantic=("network_mode","address_families","peer_allow_count","peer_allow_prefix_length")
    active={"network_mode":"authenticated-static-peers","address_families":["AF_UNIX","AF_INET"],"peer_allow_count":2,"peer_allow_prefix_length":32}
    absent={"network_mode":None,"address_families":[],"peer_allow_count":0,"peer_allow_prefix_length":None}
    if d["present"]:
        sha(d["sha256"],f"{label}.dropin.sha256"); commit(d["effective_policy_commitment"],f"{label}.dropin.effective_policy_commitment")
        # validate-dropin.py hashes the bytes it parsed; capture-host.sh hashes the installed file. Both must name one drop-in.
        limits=obj(d["semantic_limits"],f"{label}.dropin.semantic_limits",semantic+("sha256",))
        sha(limits["sha256"],f"{label}.dropin.semantic_limits.sha256")
        if limits["sha256"] != d["sha256"]: raise ValueError(f"{label}.dropin.semantic_limits.sha256: validated drop-in hash is not the installed drop-in hash")
        if {f:limits[f] for f in semantic} != active or not d["inherited_deny_all"] or not d["effective_policy_configured"]: raise ValueError(f"{label}.dropin: effective policy differs from contract")
    else:
        limits=obj(d["semantic_limits"],f"{label}.dropin.semantic_limits",semantic)
        if d["sha256"] is not None or limits != absent or d["effective_policy_configured"] or d["effective_policy_commitment"] is not None: raise ValueError(f"{label}.dropin: absent drop-in carries effective semantics")
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
    validate_exchanges(v["exchanges"],f"{label}.exchanges")
    i=v["inspection"]
    if i is not None and i["store_present"] and v["exchanges"] is None:
        raise ValueError(f"{label}: a present store was inspected but no exchanges were published")
    if i is not None and i["store_present"]:
        # Applied to every inspected stage. These same-host checks catch a capture that
        # contradicts itself; they do not authenticate an honest producer.
        collapsed=sum(r["row_count"] for r in (v["exchanges"] or []))
        if collapsed != i["audit_event_count"]:
            raise ValueError(f"{label}: the folded exchanges collapse {collapsed} audit rows but the store reports {i['audit_event_count']}")
        # Every attempt a capture lists must be corroborated by that capture's own exchange
        # rows. This catches a self-contradicting capture where the count and row set do not
        # describe the attempt listed by that same host.
        rows={r["nonce_commitment"]: r for r in (v["exchanges"] or [])}
        for a in i["incomplete_attempts"]:
            r=rows.get(a["nonce_commitment"])
            if r is None:
                raise ValueError(f"{label}: an incomplete attempt has no exchange row in its own capture")
            if r["direction"] != a["direction"]:
                raise ValueError(f"{label}: an incomplete attempt and its corroborating row have different directions")
            if r["nonce_authority"] != a["nonce_authority"]:
                raise ValueError(f"{label}: an incomplete attempt and its corroborating row have different nonce authority")
            if r["operation_commitment"] != a["operation_commitment"]:
                raise ValueError(f"{label}: an incomplete attempt and its corroborating row name different operations")
            if not nonterminal(r):
                raise ValueError(f"{label}: an incomplete attempt is corroborated by a terminal exchange row")
            highest=max(r["phases_reached"],key=PHASE_RANK[a["direction"]].get)
            if a["last_phase"] != highest:
                raise ValueError(f"{label}: an incomplete attempt last_phase is not the highest corroborating phase")
        # Condition 6's every-replica branch rests on imported_operation_commitments, and its
        # only bound was receipt_count -- an integer the same host asserts, so raising it to
        # 999 lifted the bound and the campaign passed. Found by attacking an evidence set
        # that had just returned PASS.
        #
        # The bound uses no self-asserted integer. A replica holds an imported operation
        # exactly when one of its OWN inbound rows committed an import and carries the
        # receipt. Measured on a live three-host campaign the two sets are equal on every
        # host -- 28/28, 32/32, 32/32 -- so equality is required rather than inclusion.
        committed={r["operation_commitment"] for r in (v["exchanges"] or [])
                   if r["direction"]=="inbound" and TERMINAL_IMPORT_PHASE in r["phases_reached"]
                   and r["local_receipt_commitment"] is not None and r["operation_commitment"] is not None}
        if set(i["imported_operation_commitments"]) != committed:
            raise ValueError(f"{label}: the imported operations are not exactly the operations this capture committed an import for")
        # The converse is direction-neutral: every non-terminal folded row must publish the
        # incomplete attempt it represents. Otherwise a host could omit either an inbound
        # or outbound debt row from the explicit attempt list.
        listed={a["nonce_commitment"] for a in i["incomplete_attempts"]}
        for n,r in rows.items():
            if nonterminal(r) and n not in listed:
                raise ValueError(f"{label}: a non-terminal exchange row is missing from the incomplete-attempt list")
    # A capture that inspected a present store publishes exchanges; one that inspected an
    # absent store, or did not inspect at all, publishes none. An empty list and "no list"
    # are different claims and stay distinguishable.
    if (v["inspection"] is None or not v["inspection"]["store_present"]) and v["exchanges"] is not None:
        raise ValueError(f"{label}: exchanges published without an inspected present store")
    if v["graceful_shutdown"] is not None and v["graceful_shutdown"] != SHUTDOWN: raise ValueError(f"{label}.graceful_shutdown: invalid exact typed-shutdown proof")

EXCHANGE=("nonce_commitment","nonce_authority","joinable","direction","phases_reached","row_count",
          "peer_commitment","operation_commitment","request_sha256_commitment","reply_sha256_commitment",
          "local_receipt_commitment","remote_receipt_commitment","request_frame_bytes","reply_frame_bytes",
          "request_announced_body_bytes","reply_announced_body_bytes","outcomes","replayed")

def validate_exchanges(v, label):
    # One folded row per wire nonce, never one per audit row. An exchange is one to four
    # audit rows sharing a nonce, so a rule of one row per nonce applied to raw audit rows
    # would refuse every genuine inbound exchange.
    if v is None: return
    if not isinstance(v,list): raise ValueError(f"{label}: must be a list or null")
    seen=set()
    for i,r in enumerate(v):
        where=f"{label}[{i}]"
        obj(r,where,EXCHANGE)
        commit(r["nonce_commitment"],f"{where}.nonce_commitment")
        if r["nonce_commitment"] in seen: raise ValueError(f"{where}: two folded rows for one nonce, which the fold must have collapsed")
        seen.add(r["nonce_commitment"])
        if r["nonce_authority"] not in AUTHORITIES: raise ValueError(f"{where}.nonce_authority: unknown authority")
        if not isinstance(r["joinable"],bool): raise ValueError(f"{where}.joinable: must be a boolean")
        # joinable is derived, never asserted: a capture claiming a locally minted nonce is
        # joinable would send the comparator looking for a peer row that cannot exist.
        if r["joinable"] != (r["nonce_authority"]=="peer-validated"):
            raise ValueError(f"{where}.joinable does not follow from nonce_authority")
        if r["direction"] not in DIRECTIONS: raise ValueError(f"{where}.direction: unknown direction")
        if r["nonce_authority"]=="pre-authentication" and r["direction"]!="inbound":
            raise ValueError(f"{where}: an outbound exchange cannot carry a pre-authentication nonce")
        if not isinstance(r["phases_reached"],list) or not r["phases_reached"]: raise ValueError(f"{where}.phases_reached: must be a non-empty list")
        for p in r["phases_reached"]:
            string(p,f"{where}.phases_reached")
            if p not in PHASES_BY_DIRECTION[r["direction"]]:
                raise ValueError(f"{where}.phases_reached: unknown phase for {r['direction']} direction")
        if sorted(set(r["phases_reached"])) != sorted(r["phases_reached"]): raise ValueError(f"{where}.phases_reached repeats a phase")
        integer(r["row_count"],f"{where}.row_count")
        # The fold collapses rows; row_count says how many it collapsed, and a row claiming
        # more phases than rows collapsed did not come from this fold.
        if r["row_count"] < len(r["phases_reached"]): raise ValueError(f"{where}: more phases than collapsed rows")
        # And bounded from ABOVE, at four. The bound is MEASURED on the candidate's reachable
        # emission paths, not entailed by the phase set: AuditPhase has seven inbound
        # variants (durable.rs:220-230) and the store only forbids a repeat of one phase per
        # attempt, so the type system permits more. What makes four right is that the
        # terminal phases are mutually exclusive match arms in the sender
        # (manager-network/src/lib.rs:1100-1160), and that all 67 nonces of a preserved
        # campaign store collapse 1, 2 or 4 rows. A capture exceeding it is refused, not
        # silently accepted -- which is the safe direction if the protocol ever grows a path.
        # It closes the route where a host hides a competing receiver row by absorbing its
        # audit rows into another row's count.
        limit=2 if r["direction"]=="outbound" else 4
        if r["row_count"] > limit:
            if r["direction"]=="outbound":
                raise ValueError(f"{where}: outbound exchange cannot collapse more than two audit rows")
            raise ValueError(f"{where}: inbound exchange cannot collapse more than four audit rows")
        for f in ("peer_commitment","operation_commitment","request_sha256_commitment","reply_sha256_commitment","local_receipt_commitment","remote_receipt_commitment"):
            if r[f] is not None: commit(r[f],f"{where}.{f}")
        for f in ("request_frame_bytes","reply_frame_bytes"): integer(r[f],f"{where}.{f}")
        for f in ("request_announced_body_bytes","reply_announced_body_bytes"):
            if r[f] is not None: integer(r[f],f"{where}.{f}")
        if not isinstance(r["outcomes"],list) or not r["outcomes"]: raise ValueError(f"{where}.outcomes: must be a non-empty list")
        for o in r["outcomes"]:
            string(o,f"{where}.outcomes")
            if o not in OUTCOMES: raise ValueError(f"{where}.outcomes: unknown outcome")
        if len(set(r["outcomes"])) != len(r["outcomes"]): raise ValueError(f"{where}.outcomes repeats an outcome")
        if r["replayed"] is not None and not isinstance(r["replayed"],bool): raise ValueError(f"{where}.replayed: must be a boolean or null")

def inspection_bound(v):
    # store_present is checked here rather than left to fall out of a None comparison,
    # because every caller uses this as the gate before reading a derived field. An
    # absent store is a legitimate baseline but it is never a bound inspection: there
    # is no store for a commitment to bind to.
    i=v["inspection"]; c=v["configuration"]
    return i is not None and i["store_present"] and i["logical_manager_commitment"]==c["logical_manager_commitment"] and i["replica_commitment"]==c["local_replica_commitment"]
def same_state_metadata(a,b): return all(a[f]==b[f] for f in ("present","uid","gid","mode"))

def stage_monotonicity(earlier, later, earlier_label, later_label):
    """Return same-host append-only violations between two inspected store captures."""
    failures=[]
    a,b=earlier["inspection"],later["inspection"]
    if a is None or not a["store_present"]: return failures
    if b is None or not b["store_present"]:
        return [f"canonical store disappeared from {earlier_label} to {later_label}"]
    for field in ("history_count","receipt_count","audit_event_count"):
        if b[field] < a[field]: failures.append(f"{field} regressed from {earlier_label} to {later_label}")
    old_rows={r["nonce_commitment"]:r for r in (earlier["exchanges"] or [])}
    new_rows={r["nonce_commitment"]:r for r in (later["exchanges"] or [])}
    for nonce,old in old_rows.items():
        new=new_rows.get(nonce)
        if new is None:
            failures.append(f"exchange nonce disappeared from {earlier_label} to {later_label}")
            continue
        if old["direction"] != new["direction"]:
            failures.append(f"exchange direction changed from {earlier_label} to {later_label}")
        for field in ("peer_commitment","operation_commitment","request_sha256_commitment",
                      "request_announced_body_bytes"):
            if old[field] != new[field]:
                failures.append(f"exchange {field} changed from {earlier_label} to {later_label}")
        if not set(old["phases_reached"]).issubset(new["phases_reached"]):
            failures.append(f"exchange phases regressed from {earlier_label} to {later_label}")
        if new["row_count"] < old["row_count"]:
            failures.append(f"exchange row count regressed from {earlier_label} to {later_label}")
    new_attempts={x["attempt_commitment"]:x for x in b["incomplete_attempts"]}
    for old in a["incomplete_attempts"]:
        if old["attempt_commitment"] in new_attempts:
            if new_attempts[old["attempt_commitment"]] != old:
                failures.append(f"incomplete attempt identity changed meaning from {earlier_label} to {later_label}")
            continue
        row=new_rows.get(old["nonce_commitment"])
        if row is None or not (set(row["phases_reached"]) & TERMINAL_PHASES[old["direction"]]):
            failures.append(f"incomplete attempt disappeared without a terminal row from {earlier_label} to {later_label}")
    return failures

def preexisting_debt_failures(pre, later, later_label):
    """Require every pre-activation debt record and its row to remain byte-identical."""
    failures=[]
    pre_i,later_i=pre["inspection"],later["inspection"]
    if pre_i is None or not pre_i["store_present"]: return failures
    if later_i is None or not later_i["store_present"]:
        return [f"pre-existing debt cannot be checked at {later_label}: canonical store absent"]
    later_attempts={a["attempt_commitment"]:a for a in later_i["incomplete_attempts"]}
    later_rows={r["nonce_commitment"]:r for r in (later["exchanges"] or [])}
    pre_rows={r["nonce_commitment"]:r for r in (pre["exchanges"] or [])}
    for attempt in pre_i["incomplete_attempts"]:
        current=later_attempts.get(attempt["attempt_commitment"])
        if current is None:
            failures.append(f"pre-existing debt disappeared at {later_label}")
        elif current != attempt:
            failures.append(f"pre-existing debt attempt changed at {later_label}")
        old_row=pre_rows.get(attempt["nonce_commitment"])
        current_row=later_rows.get(attempt["nonce_commitment"])
        if old_row is None:
            failures.append("pre-existing debt has no pre-activation exchange row")
        elif current_row is None:
            failures.append(f"pre-existing debt exchange row disappeared at {later_label}")
        elif current_row != old_row:
            failures.append(f"pre-existing debt exchange row changed at {later_label}")
    return failures

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
    if pre["inspection"] is None or (pre["inspection"]["store_present"] and not inspection_bound(pre)):
        failures.append("pre-activation inspection is absent or not bound to this replica")
    if not inspection_bound(conv): failures.append("converged inspection is absent or not bound to this replica")
    if not inspection_bound(cleanup): failures.append("cleanup inspection is absent or not bound to this replica")
    for earlier,later,earlier_label,later_label in zip(captures,captures[1:],STAGES,STAGES[1:]):
        failures.extend(stage_monotonicity(earlier,later,earlier_label,later_label))
    for later,later_label in zip(captures[1:],STAGES[1:]):
        failures.extend(preexisting_debt_failures(pre,later,later_label))
    if inspection_bound(conv) and inspection_bound(cleanup):
        a,b=conv["inspection"],cleanup["inspection"]
        counts=("history_count","receipt_count","audit_event_count")
        if all(b[f]==a[f] for f in counts) and b["logical_history_sha256"]!=a["logical_history_sha256"]:
            failures.append("cleanup canonical history mutated without append-only growth")
    return failures

TERMINAL_IMPORT="inbound_import_committed"
TERMINAL_REFUSAL="inbound_refusal_recorded"
REPLY_WRITTEN="inbound_reply_write_observed"
SENDER_PREPARED="outbound_request_prepared"

def attempt_ids(capture):
    i=capture["inspection"]
    if i is None or not i["store_present"]: return set()
    return {a["attempt_commitment"] for a in i["incomplete_attempts"]}

def account_attempts(pres, bases, cleanups, convs):
    """Decide, for each incomplete attempt created inside the campaign window, whether it is
    accounted for. Returns (summary, failures).

    The predicate is about one attempt at a time. A timeout, a matching total or eventual
    convergence alone is insufficient, and count subtraction is forbidden: cleanup
    legitimately adds terminal rows, so a difference of totals describes nothing. Every set
    below is a set of attempt IDENTITIES."""
    failures=[]
    # Condition 4, and part of 7, are campaign-wide: an unaudited import receipt anywhere
    # means no attempt is accounted for, because the evidence that would account for it is
    # itself in question.
    unaudited=sum(c["inspection"]["unaudited_import_receipt_count"] for c in cleanups if c["inspection"] and c["inspection"]["store_present"])
    if unaudited: failures.append(f"{unaudited} unaudited import receipts exist; no attempt can be accounted for while an import receipt has no audit")

    # Each row carries an announced body size and a frame size, and the difference must be
    # the protocol's own framing constant. Deriving that constant from the campaign instead
    # was a tautology: the row under test contributed to the set it was compared against.
    def overhead_of(frame, announced):
        # Only rows that COMPLETED are asked to carry the framing constant. A row whose
        # outcome set names a failure is recording a transfer that stopped part-way, and its
        # frame is honestly smaller than the body it announced -- which is a faithful record,
        # not a malformed one.
        #
        # Measured on a live campaign: one sender row read the four-byte length prefix of a
        # reply announcing 690 bytes and then lost the connection, outcome `unavailable`,
        # frame 4 against announced 690. Applied to every row, this check would have failed
        # any real campaign containing a single mid-reply disconnection -- the exact failure
        # this lot exists to account for.
        d={r[frame]-r[announced] for c in cleanups if c["exchanges"]
           for r in c["exchanges"] if r[frame]>0 and r[announced] is not None
           and completed(r)}
        return d
    rq=overhead_of("request_frame_bytes","request_announced_body_bytes")
    rp=overhead_of("reply_frame_bytes","reply_announced_body_bytes")
    if rq-{FRAME_OVERHEAD}: failures.append("request frames do not carry the protocol framing overhead")
    if rp-{FRAME_OVERHEAD}: failures.append("reply frames do not carry the protocol framing overhead")
    overhead=reply_overhead=FRAME_OVERHEAD
    # Every audit row belongs to exactly one folded exchange, so the rows collapsed must
    # account for the whole audit history the same capture reports. Without this a host can
    # publish an empty exchange list beside a non-zero audit count and nothing binds them.
    # The collapsed-rows rule now lives in validate(), where it covers every stage.

    # Every folded exchange row in the campaign, indexed by nonce and by host.
    rows_by_nonce={}
    for host,c in enumerate(cleanups):
        for r in (c["exchanges"] or []):
            rows_by_nonce.setdefault(r["nonce_commitment"],[]).append((host,r))

    outbound_nonces={nonce for nonce,rows in rows_by_nonce.items()
                     if any(row["direction"]=="outbound" for _,row in rows)}
    # Report receiver rows introduced after the converged capture that have no matching
    # sender row. Older receiver-only history is outside this bounded cleanup delta.
    converged_nonces={r["nonce_commitment"] for capture in convs
                       for r in (capture["exchanges"] or [])}
    unmatched_inbound=[{"host":host,"nonce_commitment":nonce,
                        "nonce_authority":row["nonce_authority"],
                        "operation_commitment":row["operation_commitment"],
                        "outcomes":row["outcomes"]}
                       for nonce,rows in rows_by_nonce.items() for host,row in rows
                       if row["direction"]=="inbound" and nonce not in outbound_nonces
                       and nonce not in converged_nonces]

    # A history digest and a history count are two statements of one fact, and nothing bound
    # them: a host could raise its count without touching its digest and the campaign passed.
    # Found by attacking an evidence set that had just returned PASS. A comparator cannot
    # recompute the digest, but replicas claiming the SAME history must report the same
    # length, and that is exact.
    for stage_label,group in (("converged",convs),("post-cleanup",cleanups),("active-baseline",bases)):
        by_digest={}
        for c in group:
            i=c["inspection"]
            if not i or not i.get("store_present"): continue
            d=i.get("logical_history_sha256")
            if d is None: continue
            by_digest.setdefault(d,set()).add(i["history_count"])
        for d,counts in by_digest.items():
            if len(counts)>1:
                failures.append(f"{stage_label}: replicas claim one logical history with different lengths {sorted(counts)}")

    # Convergence of the canonical histories, computed once. It is not sufficient on its
    # own -- the predicate says so in terms -- but it is a necessary part of the superseded
    # branch below, which needs the facts to have actually converged somewhere.
    # `.get` rather than indexing: this runs over whatever captures the caller supplies, and
    # a capture with no history digest is simply not evidence of convergence. A missing key
    # must not raise here, because raising turns a verdict into a crash.
    digests={c["inspection"].get("logical_history_sha256") for c in convs
             if c["inspection"] and c["inspection"].get("store_present")}
    history_converged = len(digests)==1 and None not in digests
    accounted=[]; unaccounted=[]; debt=[]; terminal=[]
    for host,(pre,cleanup) in enumerate(zip(pres,cleanups)):
        baseline=attempt_ids(pre); post_capture=cleanup["inspection"]
        if post_capture is None or not post_capture["store_present"]:
            failures.append(f"host {host}: post-cleanup published no store, so no attempt can be classified"); continue
        for a in post_capture["incomplete_attempts"]:
            if a["attempt_commitment"] in baseline:
                # Pre-existing debt: retained and reported, never silently folded into a
                # success claim, and never a reason to fail a new bounded campaign.
                anchored=[]
                for other,row in rows_by_nonce.get(a["nonce_commitment"],[]):
                    if other!=host and row["direction"]=="inbound": anchored.append(other)
                for other in anchored:
                    pre_rows={r["nonce_commitment"] for r in (pres[other]["exchanges"] or [])}
                    if a["nonce_commitment"] not in pre_rows:
                        failures.append(f"host {host}: pre-existing debt has a peer row that was absent before activation")
                debt.append({"host":host,"attempt_commitment":a["attempt_commitment"],"last_phase":a["last_phase"]}); continue
            why,branch=classify(host,a,rows_by_nonce,cleanups,overhead,reply_overhead,history_converged)
            if why is None:
                accounted.append({"host":host,"attempt_commitment":a["attempt_commitment"],
                                  "branch":branch,"receiver_asserted":branch=="receiver_asserted"})
            else:
                # Two populations, reported apart. An attempt whose wire nonce appears on NO
                # other host is a request that never reached a peer -- the sender prepared it
                # and nothing was transferred. An attempt whose nonce IS elsewhere but whose
                # join fails is a different fact about the exchange itself. Both are
                # unaccounted and both fail the campaign; reporting them under one number
                # tells an operator the gate refused without telling them what to look at.
                # Measured: a live campaign produced 149 unaccounted attempts of which 123
                # had reached no peer at all.
                seen_elsewhere=any(h!=host for h,_ in rows_by_nonce.get(a["nonce_commitment"],[]))
                kind="peer-has-no-record" if not seen_elsewhere else "join-failed"
                unaccounted.append({"host":host,"attempt_commitment":a["attempt_commitment"],
                                    "reason":why,"kind":kind})
                failures.append(f"host {host}: an incomplete attempt is unaccounted for ({kind}): {why}")

        pre_nonces={r["nonce_commitment"] for r in (pre["exchanges"] or [])}
        for row in (cleanup["exchanges"] or []):
            if row["direction"]!="outbound" or "outbound_exchange_completed" not in row["phases_reached"] or row["nonce_commitment"] in pre_nonces:
                continue
            detail={"host":host,"nonce_commitment":row["nonce_commitment"],"outcomes":row["outcomes"],"self_asserted":True}
            if "accepted" in row["outcomes"]:
                why=accepted_terminal_failure(host,row,rows_by_nonce,cleanups,overhead,reply_overhead)
                detail["self_asserted"]=why is not None
                detail["cross_host_joined"]=why is None
                if why is not None:
                    detail["reason"]=why
                    failures.append(f"host {host}: an accepted terminal attempt is unaccounted for: {why}")
            else:
                contradictions=[]
                for other,recv in rows_by_nonce.get(row["nonce_commitment"],[]):
                    if other!=host and recv["direction"]=="inbound" and TERMINAL_IMPORT in recv["phases_reached"] and REPLY_WRITTEN in recv["phases_reached"]:
                        contradictions.append(other)
                detail["receiver_completion_observed"]=bool(contradictions)
                detail["receiver_hosts"]=contradictions
            terminal.append(detail)
    new_incomplete=len(accounted)+len(unaccounted)
    return ({"terminal_attempts":len(terminal),
             "new_incomplete_attempts":new_incomplete,
             "accounted_incomplete_attempts":len(accounted),
             "unaccounted_incomplete_attempts":len(unaccounted),
             # The split above, so a verdict says which fact it is describing.
             "unaccounted_peer_has_no_record":sum(1 for u in unaccounted if u["kind"]=="peer-has-no-record"),
             "unaccounted_join_failed":sum(1 for u in unaccounted if u["kind"]=="join-failed"),
             "preexisting_incomplete_attempts":len(debt),
             "accounted_detail":accounted,
             "terminal_detail":terminal,
             "unaccounted_detail":unaccounted,
             "preexisting_detail":debt,
             "unmatched_inbound_rows":{"count":len(unmatched_inbound),"detail":unmatched_inbound},
             "undecided_conditions":list(UNDECIDED_CONDITIONS),
             "trust_model":TRUST_MODEL}, failures)

def complete_reply(row, overhead):
    return (REPLY_WRITTEN in row["phases_reached"] and row["reply_sha256_commitment"] is not None
            and row["reply_announced_body_bytes"] is not None and row["reply_announced_body_bytes"]>0
            and row["reply_frame_bytes"]==row["reply_announced_body_bytes"]+overhead)

def accepted_terminal_failure(host, sender, rows_by_nonce, cleanups, overhead, reply_overhead):
    candidates=[(h,r) for h,r in rows_by_nonce.get(sender["nonce_commitment"],[])
                if h!=host and r["direction"]=="inbound"]
    if len(candidates)!=1: return "exactly one receiver-side row is required for an accepted completion"
    rhost,recv=candidates[0]
    sender_replica=cleanups[host]["inspection"]["replica_commitment"]
    receiver_replica=cleanups[rhost]["inspection"]["replica_commitment"]
    if sender["peer_commitment"]!=receiver_replica or recv["peer_commitment"]!=sender_replica:
        return "the accepted completion does not bind both declared peers"
    for field in ("operation_commitment","request_sha256_commitment","reply_sha256_commitment"):
        if sender[field] is None or sender[field]!=recv[field]: return f"the accepted completion does not bind {field}"
    if sender["remote_receipt_commitment"] is None or sender["remote_receipt_commitment"]!=recv["local_receipt_commitment"]:
        return "the accepted completion does not bind the receiver receipt"
    if sender["request_announced_body_bytes"]!=recv["request_announced_body_bytes"]:
        return "the accepted completion does not bind the announced request size"
    if recv["request_frame_bytes"]!=recv["request_announced_body_bytes"]+overhead:
        return "the accepted completion does not bind a complete request"
    if TERMINAL_IMPORT not in recv["phases_reached"] or not completed(recv):
        return "the accepted completion has no accepted receiver import"
    if not complete_reply(recv,reply_overhead): return "the accepted completion has no complete receiver reply"
    return None

def classify(host, attempt, rows_by_nonce, cleanups, overhead, reply_overhead, history_converged):
    """Return (None, branch) when accounted, otherwise (first failed condition, None).
    Each condition is a separate refusal: there is no aggregate that compensates for a
    missing one."""
    # An attempt whose nonce was minted locally can never be joined to another host. It is
    # unaccounted, and saying so is the honest answer rather than hunting a row that cannot
    # exist.
    if attempt["nonce_authority"]!="peer-validated":
        return "its wire nonce is a locally minted pre-authentication value and can never bind to a receiver",None
    if attempt["direction"]!="outbound":
        return "an inbound incomplete attempt has no receiver-side join defined",None
    nonce=attempt["nonce_commitment"]
    candidates=rows_by_nonce.get(nonce,[])
    sender=[r for h,r in candidates if h==host and r["direction"]=="outbound"]
    if len(sender)!=1: return "its own host publishes no single outbound exchange row for this nonce",None
    sender=sender[0]
    # The attempt publishes its own operation commitment; until now nothing read it, so an
    # attempt could name any operation at all and still be accounted by its host's row.
    if attempt["operation_commitment"] != sender["operation_commitment"]:
        return "the attempt and its own exchange row name different operations (condition 2)",None
    # Condition 2: exactly one authenticated receiver-side request, on exactly one OTHER host.
    receivers=[(h,r) for h,r in candidates if h!=host and r["direction"]=="inbound"]
    if not receivers: return "no receiver-side request bears this wire nonce (condition 2)",None
    if len(receivers)>1: return "more than one host claims the receiver side of this wire nonce (condition 2)",None
    rhost,recv=receivers[0]
    # The peer field is always THE OTHER PARTY, never the observer: a sender's row names the
    # receiver and a receiver's row names the sender. Requiring the two to be equal would
    # have refused every genuine join. What must hold is that each side names the other, and
    # each host publishes its own replica commitment for exactly that comparison.
    sender_replica=cleanups[host]["inspection"]["replica_commitment"]
    receiver_replica=cleanups[rhost]["inspection"]["replica_commitment"]
    if sender["peer_commitment"]!=receiver_replica:
        return "the sender's declared peer is not the host that served the request (condition 2)",None
    if recv["peer_commitment"]!=sender_replica:
        return "the receiver did not authenticate the sending host as its peer (condition 2)",None
    for field,name in (("operation_commitment","operation ID"),("request_sha256_commitment","request digest")):
        if sender[field] is None or recv[field] is None or sender[field]!=recv[field]:
            return f"the {name} does not bind sender to receiver (condition 2)",None
    if sender["request_announced_body_bytes"] is None or recv["request_announced_body_bytes"] is None \
       or sender["request_announced_body_bytes"]!=recv["request_announced_body_bytes"]:
        return "the announced request size does not bind sender to receiver (condition 2)",None
    if recv["request_frame_bytes"] != recv["request_announced_body_bytes"]+overhead:
        return "the receiver did not record a complete request frame (conditions 2 and 3)",None
    # Condition 3: a terminal, with a receipt.
    if TERMINAL_IMPORT not in recv["phases_reached"] and TERMINAL_REFUSAL not in recv["phases_reached"]:
        return "the receiver committed neither an import receipt nor a typed refusal (condition 3)",None
    if TERMINAL_IMPORT in recv["phases_reached"] and recv["local_receipt_commitment"] is None:
        return "the receiver committed an import with no receipt (condition 3)",None
    # "the EXPECTED import receipt OR A TYPED REFUSAL" -- the outcome has to agree with the
    # terminal the receiver reached. Published and never read until now, so a receiver could
    # report a refusal outcome beside a committed import and be accounted.
    if TERMINAL_IMPORT in recv["phases_reached"] and "accepted" not in recv["outcomes"]:
        return "the receiver committed an import without an accepted outcome (condition 3)",None
    if TERMINAL_REFUSAL in recv["phases_reached"] and "accepted" in recv["outcomes"]:
        return "the receiver recorded a refusal and an accepted outcome at once (condition 3)",None
    # Condition 5: the reply was completely written, and the sender honestly retained its
    # absence. This is the shape the frozen contract requires, not a defect.
    if REPLY_WRITTEN not in recv["phases_reached"] or recv["reply_frame_bytes"]<=0:
        return "the receiver did not write a reply at all (condition 5)",None
    # "Completely wrote the correctly BOUND reply" has two halves, and checking only that
    # some bytes left is the weaker one: a truncated write satisfies it as well as a
    # complete one, and an unbound reply satisfies it too.
    if recv["reply_sha256_commitment"] is None:
        return "the receiver's reply is not bound by a digest (condition 5)",None
    if recv["reply_announced_body_bytes"] is None or recv["reply_announced_body_bytes"] <= 0 \
       or recv["reply_frame_bytes"] != recv["reply_announced_body_bytes"]+reply_overhead:
        return "the receiver did not completely write the reply it announced (condition 5)",None
    if attempt["last_phase"]!=SENDER_PREPARED:
        return "the sender did not retain the absence of a confirmed reply as prepared/incomplete (condition 5)",None
    # Condition 6: a later receiver-side replay must carry the original durable receipt.
    # A matching sender retry row upgrades that receiver assertion to a cross-host replay.
    retry_receivers=[]
    for retry_nonce,rows in rows_by_nonce.items():
        if retry_nonce==nonce: continue
        for retry_host,retry in rows:
            # `replayed` is NOT required of the sibling exchange, and requiring it was a
            # reading of the flag as an ordering. It means "this request replayed an
            # operation I already held a receipt for", so it marks whichever exchange the
            # receiver saw SECOND -- and that can be the stranded one. Measured on a live
            # campaign: a strand whose own receiver row carried replayed:true beside an
            # earlier completed exchange carrying replayed:false, both bearing the same
            # durable receipt. The strand was the replay.
            #
            # What the condition actually asks is whether the durable receipt this exchange
            # produced is the one a fully bound, completed exchange of the same operation
            # also carries on the same receiver. Which of the two came first decides nothing
            # about whether the effect is durable.
            if (retry_host==rhost and retry["direction"]=="inbound"
                    and retry["nonce_authority"]=="peer-validated"
                    and retry["operation_commitment"]==sender["operation_commitment"]
                    and retry["peer_commitment"]==sender_replica
                    and (retry["replayed"] is True or recv["replayed"] is True)
                    and TERMINAL_IMPORT in retry["phases_reached"] and REPLY_WRITTEN in retry["phases_reached"]
                    and completed(retry)
                    and retry["local_receipt_commitment"] is not None
                    and retry["local_receipt_commitment"]==recv["local_receipt_commitment"]
                    and complete_reply(retry,reply_overhead)):
                retry_receivers.append((retry_nonce,retry))
    if not retry_receivers:
        # Condition 6's SECOND disjunct: the imported facts are present in the converged
        # canonical history on every replica. The candidate keys an operation to the digest
        # of the sender's snapshot (manager-resident/src/lib.rs:629-634), so when the local
        # state changes before the next round a NEW operation is minted and the stranded one
        # is never retried -- it is superseded, and its facts travel in the larger snapshot
        # that follows.
        #
        # Measured on a live campaign: both remaining strands had 19 later completed
        # exchanges to the same receiver carrying a strictly larger announced snapshot.
        # Refusing them said the effect was unproven while the evidence showed it delivered
        # nineteen times over.
        #
        # DECLARED LIMIT: announced size is a proxy for "a superset of the facts". It is
        # sound only because the canonical history is append-only, so a later snapshot from
        # one replica to one peer can only have grown. Published evidence carries no per-fact
        # identity, so the exact-facts reading of this condition stays undecidable and is
        # listed as such.
        if history_converged:
            superseding=[r for r in (cleanups[host]["exchanges"] or [])
                         if r["direction"]=="outbound"
                         and r["peer_commitment"]==receiver_replica_of(cleanups,rhost)
                         and r["operation_commitment"] not in (None, sender["operation_commitment"])
                         and completed(r)
                         and r["request_announced_body_bytes"] is not None
                         and sender["request_announced_body_bytes"] is not None
                         and r["request_announced_body_bytes"] > sender["request_announced_body_bytes"]]
            if superseding:
                return None,"superseded"
        return "no receiver observed a complete replay carrying the durable receipt, and no superseding snapshot reached that receiver (condition 6)",None
    receiver_replica=cleanups[rhost]["inspection"]["replica_commitment"]
    receiver_asserted=False
    sender_failures=[]
    for retry_nonce,retry in retry_receivers:
        retry_senders=[r for h,r in rows_by_nonce[retry_nonce]
                       if h==host and r["direction"]=="outbound"]
        if not retry_senders:
            receiver_asserted=True
            continue
        if len(retry_senders)!=1:
            sender_failures.append("the replay retry has more than one sender-side row (condition 6)")
            continue
        retry_sender=retry_senders[0]
        mismatch=None
        for field in ("operation_commitment","request_sha256_commitment",
                      "request_announced_body_bytes","reply_sha256_commitment"):
            if retry_sender[field] is None or retry_sender[field]!=retry[field]:
                mismatch=f"the replay sender does not bind {field} to the receiver (condition 6)"
                break
        if mismatch is None and retry_sender["peer_commitment"]!=receiver_replica:
            mismatch="the replay sender does not bind the receiver replica (condition 6)"
        if mismatch is None and ("outbound_exchange_completed" not in retry_sender["phases_reached"]
                                 or "accepted" not in retry_sender["outcomes"]):
            mismatch="the replay sender did not record an accepted completion (condition 6)"
        if mismatch is None and retry_sender["remote_receipt_commitment"]!=retry["local_receipt_commitment"]:
            mismatch="the replay sender does not bind the durable receiver receipt (condition 6)"
        if mismatch is None:
            return None,"replay"
        sender_failures.append(mismatch)
    if receiver_asserted:
        return None,"receiver_asserted"
    return sender_failures[0],None
    # Condition 7 is not checked here because it is already checked earlier and for every
    # capture: validate_inspection refuses any store whose sqlite_integrity_result is not
    # "ok", and the immutable-chain half is entailed by the inspection having succeeded at
    # all, since the candidate verifies it and returns an error when it fails.
    #
    # Condition 8's evidence half is satisfied by the attempt being in this very list at
    # post-cleanup: we are reading it there.
    #
    # DECLARED COVERAGE LIMITS, so that they are limits rather than silence. Each is a part
    # of a condition that published evidence cannot decide:
    #   C1  the campaign window is the stopped pre-activation-to-post-cleanup set difference
    #       and nothing narrower. An attempt already present before activation is classified as debt, not
    #       as new, and no wall-clock window is enforced anywhere. The candidate binding is
    #       checked, across all four stages and across all three hosts, but elsewhere.
    #   C4  "no partial import" is approached from both sides -- an import with no receipt,
    #       and a receipt with no audit -- but a partial import is not a state the published
    #       evidence names, so it is not directly checked.
    #   C5  the reply is proved bound and completely written; that it was SIGNED is not
    #       visible in published evidence, which carries a digest commitment, not a
    #       signature.
    #   C6  a cross-host replay binds the sender retry to the receiver receipt. When the
    #       sender retry row is absent, the result is explicitly `receiver_asserted`; neither
    #       branch proves per-fact presence on every replica.
    #   C8  the operational-diagnostics half is outside a comparator's reach entirely.
    #
    # AND ONE LIMIT THAT SPANS THEM ALL, stated because a reader would otherwise take the
    # cross-checks for more than they are. Every rule that binds one published field to
    # another -- folded rows against `audit_event_count`, imported operations against
    # `receipt_count` -- compares two values the SAME host asserts. They catch a capture
    # that contradicts itself, which is what a buggy collector or a careless edit produces,
    # and they cost a deliberate forger one extra integer. No third-party anchor exists in
    # published evidence: `audit_set_sha256` and `receipt_set_sha256` are digests over the
    # candidate's full ordered records, and the collector publishes a fold rather than those
    # records, so a comparator cannot recompute either from what it is given. Anchoring them
    # would mean publishing the records themselves, which the privacy constraint forbids.
    # The original strand is cross-host joined: a sender claim is decided by the receiver.
    # A retry without its sender row remains explicitly receiver_asserted and carries less
    # evidentiary weight than the fully joined replay branch.
    raise AssertionError("replay classification must return from the replay loop")

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
    # inspection_bound comes FIRST in each chain, and the order is load-bearing rather
    # than stylistic. It subsumes "not None" and "store_present", so nothing downstream
    # can compare a null history digest or a null count. With the old ordering, a
    # converged capture reporting an absent store would have reached `None >= 3` and
    # raised a TypeError — a crash instead of a verdict, which is not fail-closed. An
    # absent store at converged or post-cleanup is simply a failure, and is reported.
    # `incomplete_attempt_count == 0` used to be part of both lines below, and that demand
    # is withdrawn. The frozen Stage D contract REQUIRES a prepared attempt with no terminal
    # event to remain explicitly incomplete when a reply is lost after the destination
    # commits, so the old gate made a contractually required representation fail — and
    # turned a correctness gate into a reliability bar that passed only if a specified
    # failure mode happened not to occur. Convergence is now about the history; the attempts
    # are decided one at a time, by identity, below.
    live_convergence=all(inspection_bound(x) for x in convs) and len({i["logical_history_sha256"] for i in inspections})==1 and all(i["history_count"]>=3 for i in inspections)
    final_convergence=all(inspection_bound(x) for x in cleanups) and len({i["logical_history_sha256"] for i in final_inspections})==1 and all(i["history_count"]>=3 for i in final_inspections)
    convergence=live_convergence and final_convergence
    if not live_convergence: failures.append("converged captures do not prove one complete logical history of at least three events")
    if not final_convergence: failures.append("post-cleanup captures do not retain one complete logical history")
    accounting,accounting_failures=account_attempts(pres,bases,cleanups,convs)
    failures.extend(accounting_failures)
    return failures,convergence,accounting

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
        if a.phase=="host": failures=host_failures(*(g[0] for g in ordered)); convergence=False; accounting=None
        else: failures,convergence,accounting=three_host_failures(*ordered)
    except (OSError,ValueError,json.JSONDecodeError) as error:
        print(json.dumps({"status":"FAIL","error":str(error)},sort_keys=True)); return 2
    unmatched={"count":0,"detail":[]} if accounting is None else accounting["unmatched_inbound_rows"]
    print(json.dumps({"schema_version":OUT_SCHEMA,"phase":a.phase,"subject":",".join(aliases),"status":"PASS" if not failures else "FAIL","failures":failures,"canonical_convergence_evidenced":convergence,"incomplete_attempt_accounting":accounting,"unmatched_inbound_rows":unmatched,"undecided_conditions":list(UNDECIDED_CONDITIONS),"trust_model":TRUST_MODEL,"ha_claim":"absent"},sort_keys=True)); return 0 if not failures else 1
if __name__=="__main__": sys.exit(main())
