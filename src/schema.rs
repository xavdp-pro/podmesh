//! The machine-readable contract of every operation: what the console draws its forms from, and
//! what an agent reads before it asks. Each entry says what the operation is (a read, a mutation
//! on a universe, a host-wide mutation, or a step of a chain that a tool drives across hosts),
//! which gate stands before it, and each field with its type, bounds and permitted values --
//! the same bounds the daemon enforces. Every advertised operation appears here; one whose fields
//! are not yet described says so (`fields: null`), and the console then offers it as a raw JSON
//! request rather than pretending to know it.
use serde_json::{json, Value};

fn f(name: &str, kind: &str, required: bool, description: &str) -> Value {
    json!({"name": name, "type": kind, "required": required, "description": description})
}
fn int(name: &str, required: bool, min: u64, max: Option<u64>, description: &str) -> Value {
    let mut v = f(name, "integer", required, description);
    v["min"] = json!(min);
    if let Some(m) = max { v["max"] = json!(m); }
    v
}
fn num(name: &str, required: bool, min: f64, max: Option<f64>, description: &str) -> Value {
    let mut v = f(name, "number", required, description);
    v["min"] = json!(min);
    if let Some(m) = max { v["max"] = json!(m); }
    v
}
fn en(name: &str, required: bool, values: &[&str], description: &str) -> Value {
    let mut v = f(name, "enum", required, description);
    v["values"] = json!(values);
    v
}
fn op(kind: &str, gate: &str, description: &str, fields: Option<Vec<Value>>) -> Value {
    json!({"kind": kind, "gate": gate, "description": description, "fields": fields})
}

/// kind: `read` (no journal row, no effect), `universe` (a journaled mutation naming one universe),
/// `host` (a journaled mutation of this host), `tool` (one step of a chain across hosts, driven by
/// a tool from the workstation: `tools/move-universe.py`, `tools/ha-standby.py`). gate: `none`,
/// `lease` (refused without a live activation lease on this host when a policy names one),
/// `reservation` (refused while a migration reservation holds the universe), or both.
pub fn all() -> Value {
    let uuid = |name: &str, d: &str| f(name, "uuid", true, d);
    let mut m = serde_json::Map::new();
    let mut put = |name: &str, v: Value| { m.insert(name.to_string(), v); };

    put("capabilities", op("read", "none", "what this host offers: operations, contracts and these schemas", Some(vec![])));
    put("identity", op("read", "none", "this host's UUID", Some(vec![])));
    put("inventory", op("read", "none", "the containers Podman holds, PodMesh's and others'", Some(vec![])));
    put("observations", op("read", "none", "the last twenty journaled operations", Some(vec![])));
    put("host_status", op("read", "none", "this host's CPU count, load averages, memory, swap, uptime and storage, read now", Some(vec![])));
    put("universe_stats", op("read", "none", "every PodMesh universe on this host: CPU over a short sample, memory and its limit, CPU allowance, processes, disk written", Some(vec![])));
    put("storage_status", op("read", "none", "what carries Podman's storage on this host and whether a universe's space can grow there", Some(vec![])));

    put("create", op("universe", "reservation", "a stopped container for a new universe, from a local image, on the isolated or the managed network", Some(vec![
        f("image", "string", true, "a local image by digest: sha256:<64 hex>"),
        f("command", "string[]", true, "the command, as a list of strings"),
        en("network_profile", true, &["isolated", "managed"], "isolated has no network; managed joins the host's declared pool"),
        f("network_address", "string", false, "an address in the host's pool, for the managed profile"),
        f("secrets", "object[]", false, "declared secrets to mount: [{name, target}]"),
    ])));
    put("clone", op("universe", "lease,reservation", "a new universe from a stopped, mount-free source, through a committed snapshot image", Some(vec![
        uuid("source_uuid", "the universe to clone"),
    ])));
    put("delete", op("universe", "reservation", "removes the universe's container; refused while a reservation or a restore claim holds it", Some(vec![])));
    put("start", op("universe", "lease,reservation", "starts a stopped universe and observes it for a bounded window", Some(vec![
        int("observe_seconds", false, 0, Some(30), "how long to watch after the start (default 2)"),
    ])));
    put("stop", op("universe", "none", "sends the stop signal and waits; kill lets Podman escalate to SIGKILL after the timeout", Some(vec![
        int("timeout_seconds", true, 0, Some(300), "the graceful wait"),
        en("on_timeout", true, &["kill", "leave_running"], "escalate to SIGKILL, or leave the universe running"),
    ])));
    put("pause", op("universe", "reservation", "freezes every process of a running universe; memory and address stay", Some(vec![])));
    put("resume", op("universe", "lease,reservation", "thaws a paused universe; passes the same gate as start", Some(vec![])));
    put("resources", op("universe", "reservation", "the memory limit and the CPU allowance, applied to the live cgroup and read back from the kernel when the universe runs", Some(vec![
        int("memory_bytes", false, 32 * 1024 * 1024, None, "bytes, from 32 MiB to this host's memory"),
        num("cpus", false, 0.1, None, "cores, fractions allowed, up to this host's cores"),
    ])));

    put("activation_require", op("universe", "none", "declares the activation policy of a universe: its lease, its takeover margin, its standbys, and the external authority whose epochs bind it", Some(vec![
        int("lease_seconds", true, 5, Some(3600), "the lease's length"),
        int("takeover_margin_seconds", true, 5, Some(3600), "the guard between a lapsed lease and a takeover; also the clock-skew budget"),
        int("desired_standbys", false, 0, Some(16), "how many standbys should hold a copy"),
        f("eligible_hosts", "uuid[]", false, "the hosts allowed to run it; absent means unstated"),
        f("authority_id", "string", false, "the external gate whose epochs bind activation"),
        f("authority_key", "string", false, "the authority's Ed25519 public key, 32 bytes as lowercase hex; makes signed takeover documents required"),
        f("authority_quorum", "object", false, "the authority as a quorum of replica keys, instead of authority_key: {threshold, keys: [{key_id, public_key}]}, 1 to 9 keys, a threshold that is a strict majority of them; every exclusive decision is then a certificate that many distinct keys signed, and no permit is accepted"),
        f("replaces_policy_digest", "string", false, "the operator's explicit re-declaration of the authority set: the digest of the one it replaces (activation_status's authority_policy_digest), which must be current; required, unless policy_change_certificate is given, when a quorum replaces or is replaced by another authority set"),
        f("policy_change_certificate", "object", false, "the change certified by the policy in place: a podmesh-policy-change/quorum-ed25519 certificate binding this universe, from_serial (this host's serial), new_serial (one more) and the new policy's digest at that serial"),
        int("authority_serial", false, 0, None, "the authority set's serial, which its digest covers: one more than this host's at a change (the default), unchanged otherwise; for a first authority set, the serial its peers reached (default the stored one, never lower)"),
    ])));
    put("activation_acquire", op("universe", "none", "takes the lease for this host; idempotent while it holds one; needs the authority's permit, or a certificate, when the policy names one, and a certificate under a quorum", Some(vec![
        f("permit", "object", false, "the gate's permit: authority_id, resource, epoch, replica_id, instance_id, grant_id; refused under a quorum"),
        f("certificate", "object", false, "a takeover certificate (podmesh-takeover-proof/quorum-ed25519) the policy's keys signed, for this host in this boot, live and past its barrier; under a single key, its 1-of-1 certificate"),
    ])));
    put("activation_renew", op("universe", "none", "extends this host's live lease", Some(vec![])));
    put("activation_release", op("universe", "none", "gives this host's lease up", Some(vec![])));
    put("activation_supersede", op("universe", "none", "records a higher epoch's permit or certificate: this host's lease is superseded and its screen advances", Some(vec![
        f("permit", "object", false, "the newer permit; refused under a quorum"),
        f("certificate", "object", false, "the newer takeover certificate, whoever it names; its signatures and epoch are checked, not its life"),
    ])));
    put("activation_status", op("read", "none", "the policy, the lease, the epoch screen and the authority key of a universe", Some(vec![])));
    put("activation_fence", op("host", "none", "stops every universe this host no longer holds a lease for, and withdraws their publishers and exclusive routes", Some(vec![
        int("timeout_seconds", true, 0, Some(300), "the graceful wait for each stop"),
    ])));
    put("activation_fence_preview", op("read", "none", "what a fence would do now, without journaling anything", Some(vec![])));
    put("network_reapply", op("host", "none", "after a boot: re-applies the declaration's bridge, peer routes and NAT exemption the kernel lost, and withdraws the /32 routes whose kernel route is gone and every route row left resuming", Some(vec![])));
    put("boot_restore", op("host", "lease,reservation", "starts, at most once per boot each, the universes this host's journal says should run, through the start gates; called at boot by the local unit under the operator's mandate", Some(vec![
        int("observe_seconds", false, 0, Some(30), "how long each start is observed; default 2"),
    ])));
    put("boot_restore_status", op("read", "none", "this boot's restore passes, what a pass would decide now, and operations left pending", Some(vec![])));

    put("network_declare", op("host", "none", "the host's managed network: its bridge, its pool inside the prefix, its peers' pools and the NAT exemption", Some(vec![
        uuid("network_uuid", "the declaration's identity"),
        f("prefix", "string", true, "the logical prefix, e.g. 10.86.0.0/16"),
        f("pool", "string", true, "this host's /24 inside the prefix"),
        f("peer_pools", "object[]", true, "[{pool, via}]: each other host's pool and the address that reaches it"),
        en("nat_exemption", false, &["null-snat", "notrack", "none"], "how Podman's source NAT is kept off the prefix (default null-snat)"),
    ])));
    put("network_undeclare", op("host", "none", "removes the declaration and every effect it made, verified", Some(vec![uuid("network_uuid", "the declaration")])));
    put("network_route_publish", op("host", "lease", "a /32 route to a service address via a universe's address, exclusive under a resource when named", Some(vec![
        f("ip", "string", true, "the service address"),
        f("via", "string", true, "the carrier universe's address"),
        uuid("exclusive_resource", "the resource whose lease must be held here (optional)"),
    ])));
    put("network_route_withdraw", op("host", "none", "removes the route and the alias, verified", Some(vec![])));
    put("network_route_resume", op("host", "lease", "puts back the recorded exclusive route and alias of a role this host still holds, after the carrier lost them, in place: the recorded row kept, its dead effects removed, the alias and route made again with the recorded ip, via and resource, a failure leaving the row for the next resume; only under a lease live, held here, unsuperseded and acquired or renewed during this boot, a universe running at via and no other kernel route for the address", Some(vec![
        uuid("exclusive_resource", "the resource whose recorded exclusive route is resumed"),
    ])));
    put("network_status", op("read", "none", "the declaration, the allocations, the effects ledger and what the kernel holds now", Some(vec![])));

    put("secret_declare", op("host", "none", "a secret from the root-only inbox into Podman's store, its digest verified; names are immutable", Some(vec![
        f("name", "string", true, "the secret's name"),
        f("source", "string", true, "the file under the state directory's inbox/secrets"),
    ])));
    put("secret_remove", op("host", "none", "removes a declared secret not carried by any universe", Some(vec![f("name", "string", true, "the secret's name")])));
    put("secret_status", op("read", "none", "declared secrets by name and state, never their content", Some(vec![])));

    put("publisher_declare", op("host", "none", "the publishing connector of a resource: hostname, tunnel and credential, by reference", Some(vec![
        uuid("resource", "the logical resource, e.g. the logical manager"),
        f("hostname", "string", true, "the public hostname"),
        uuid("tunnel_uuid", "the Cloudflare tunnel"),
        f("credential", "string", true, "the declared secret holding the tunnel credential"),
        int("origin_port", false, 1, Some(65535), "the origin's port (default 8080)"),
    ])));
    put("publisher_start", op("host", "lease", "starts the connector, under the lease, the service address and the authority's takeover document (held, whatever its method, until its eligible_after), or resumes at the same epoch under the one this host already verified", Some(vec![
        uuid("resource", "the resource"),
        f("takeover_proof", "object", false, "the gate's document for this epoch, signed when the policy names a key, a certificate of k keys under a quorum; without it, or when it is refused, the start resumes under the proof this host verified for this epoch while the lease is the same incarnation, live, held here, unsuperseded, in the same boot"),
        f("previous", "object", false, "the agent's account of the previous publisher, recorded as provenance"),
    ])));
    put("publisher_stop", op("host", "none", "stops the connector and removes the active manager's mark (active_manager_mark)", Some(vec![uuid("resource", "the resource")])));
    put("publisher_status", op("read", "none", "the declaration, the unit and its current run, the connector's identity and registration from that run only, the lease, the origin's readiness and whether it is ready at the lease's epoch, the active manager's mark, read as present with its epoch, absent or unknown (active_manager_mark, active_manager_mark_read, active_manager_mark_epoch; governor_mark, deprecated, carries the mark's presence for one release), eligibility with each gate by name, and whether a same-epoch resume would be accepted", Some(vec![uuid("resource", "the resource")])));
    put("publisher_observed", op("host", "none", "records what the public hostname answered, as provenance", Some(vec![
        uuid("resource", "the resource"), f("observation", "object", true, "what was observed"),
    ])));

    put("manager_status", op("read", "none", "the manager resident's status through its control door, bound to the container's identity", Some(vec![])));
    put("manager_observe", op("universe", "none", "one observation appended in the replica's own scope through the control door", Some(vec![
        f("scope", "string", true, "the replica's granted scope"), f("subject", "string", true, "1-128 safe ASCII"), f("value", "string", true, "at most 4096 bytes"),
    ])));

    put("recovery_point_prepare", op("universe", "reservation", "captures a universe as a recovery point, honestly unsigned: stopped (its rootfs, class quiescent) or live (a memory checkpoint resumed in place, class memory-coherent)", Some(vec![
        en("capture", false, &["stopped", "live"], "stopped (default) exports a universe already stopped; live checkpoints a running universe and resumes it in place, interrupted about half a second, gated by the activation lease as a start is"),
        f("resume", "boolean", false, "live only: false makes a final capture, the universe left stopped with its images kept, to be promoted elsewhere or resumed here by recovery_point_resume; default true"),
    ])));
    put("recovery_point_stage", op("universe", "none", "holds a live point's archive on this host for a later promotion, verified against its manifest; no container is created", Some(vec![uuid("recovery_point_uuid", "the live point, in this host's inbox")])));
    put("recovery_point_resume", op("universe", "lease", "brings a universe left stopped by a final live capture back in place, with its memory", Some(vec![uuid("recovery_point_uuid", "the final capture")])));
    put("recovery_point_discard", op("universe", "none", "removes a staged live point's archive from this host", Some(vec![uuid("recovery_point_uuid", "the staged point")])));
    put("recovery_point_status", op("read", "none", "the recovery points of this host", Some(vec![])));
    put("recovery_point_restore", op("universe", "none", "restores a point into quarantine, isolated", Some(vec![uuid("recovery_point_uuid", "the point"), uuid("restored_universe_uuid", "the quarantine universe")])));
    put("recovery_point_promote", op("universe", "lease", "promotes a quarantined copy (restored_universe_uuid, on a network profile, not started) or a staged live point (recovery_point_uuid, resumed running and isolated) into the universe's own identity", Some(vec![
        f("restored_universe_uuid", "uuid", false, "the quarantined copy of a stopped point"), f("recovery_point_uuid", "uuid", false, "the staged live point"),
        en("network_profile", false, &["isolated", "managed"], "the profile, required for a quarantined copy; a live point is always isolated"), f("network_address", "string", false, "the address, for managed"), f("secrets", "object[]", false, "secrets to mount again (quarantined copy)"),
    ])));

    put("collection_retention_declare", op("universe", "none", "how many recovery points to keep and the minimum age before one may be collected", Some(vec![
        int("keep_latest", true, 0, None, "points kept"), int("minimum_age_seconds", true, 0, None, "age before collection"),
    ])));
    put("collection_hold_declare", op("universe", "none", "an evidence hold on a universe's artifacts", Some(vec![f("hold_id", "string", true, "the hold"), f("scope", "string", true, "what it holds"), f("reason", "string", true, "why")])));
    put("collection_hold_release", op("universe", "none", "releases a hold", Some(vec![f("hold_id", "string", true, "the hold")])));
    put("collection_status", op("read", "none", "retention and holds", Some(vec![])));
    put("garbage_collect_plan", op("read", "none", "host-wide: the dead ends the collector could end, their class, proofs and blockers", Some(vec![])));
    put("garbage_collect_apply", op("host", "none", "ends the candidates of a recorded plan, each re-proven first", Some(vec![
        uuid("plan_operation_id", "the plan applied"), f("candidates", "object[]", true, "[{class, universe_uuid}]"),
        int("max_effects", false, 1, Some(10), "bound"), int("max_runtime_reclaims", false, 0, Some(5), "bound"), f("reclaim_processes", "boolean", false, "explicit, default false"),
    ])));

    put("migration_status", op("read", "none", "the reservation, claims, authorizations and which recovery operation the state permits", Some(vec![])));
    for (name, d) in [
        ("migration_preflight", "compatibility report before a checkpoint"), ("migration_checkpoint", "captures the running universe and reserves it"),
        ("migration_authorize_transfer", "authorizes the handoff to a named destination"), ("migration_complete_transfer", "applies the destination's outcome"),
        ("migration_retire_source", "removes the transferred source"), ("migration_release", "releases a reservation never transferred"),
        ("migration_abandon", "records a reservation that will never complete"), ("migration_restore_local", "resumes a released reservation here"),
        ("migration_destination_preflight", "the destination checks the handoff"), ("migration_restore", "the destination restores the universe"),
        ("migration_restore_abort", "ends an unverified restore claim"),
    ] {
        put(name, op("tool", "reservation", &format!("{d}; one step of the chain tools/move-universe.py drives across two hosts"), None));
    }
    Value::Object(m)
}
