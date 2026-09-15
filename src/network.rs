//! The managed network profile: one logical /16, one /24 allocation pool per host, one stable
//! address per universe UUID, and the /32 routes that follow a universe placed elsewhere.
//!
//! Contract: docs/UNIVERSE-NETWORK-CONTRACT.md. Every effect here is a journaled operation the
//! agent asks for and verified from outside afterwards -- `podman network inspect` and `ip route`
//! are the witnesses, never this module's own tables. An observation that cannot be made is
//! `unknown`, and an operation that needs it refuses.
//!
//! What this module decides: nothing about prefixes, hosts or placement. The operator declares the
//! prefix and the pools; the agent asks for a route; the universe's address is bound to its UUID.
//! What it refuses: two declarations on one host, an allocation outside the local pool, a second
//! active route for one address, and any cleanup that cannot prove what remains.
use crate::lifecycle as lc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::net::Ipv4Addr;

type Error = Box<dyn std::error::Error>;

pub const PROFILE_ISOLATED: &str = "isolated";
pub const PROFILE_MANAGED: &str = "managed";
pub const LABEL_PROFILE: &str = "io.podmesh.network-profile";
pub const LABEL_IP: &str = "io.podmesh.universe-ip";
pub const LABEL_NETWORK: &str = "io.podmesh.network-uuid";
/// The one bridge a host carries for its pool; the name says what it is and nothing about a prefix.
pub const BRIDGE: &str = "podmesh-managed";
/// The nftables table that keeps Podman's source NAT off traffic inside the logical prefix.
pub const NFT_TABLE: &str = "podmesh-managed";

pub fn ensure_schema(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS network_declaration(
            network_uuid TEXT PRIMARY KEY,
            prefix TEXT NOT NULL,
            pool TEXT NOT NULL,
            gateway TEXT NOT NULL,
            bridge TEXT NOT NULL,
            state TEXT NOT NULL,
            declared_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            authorization_ref TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS network_peer_pools(
            pool TEXT PRIMARY KEY,
            via TEXT NOT NULL,
            network_uuid TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS network_allocations(
            universe_uuid TEXT NOT NULL,
            network_uuid TEXT NOT NULL,
            ip TEXT NOT NULL,
            allocated_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            released_at INTEGER,
            released_by TEXT);
         CREATE UNIQUE INDEX IF NOT EXISTS network_allocations_live_ip ON network_allocations(ip) WHERE released_at IS NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS network_allocations_live_universe ON network_allocations(universe_uuid) WHERE released_at IS NULL;
         CREATE TABLE IF NOT EXISTS network_routes(
            ip TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            via TEXT NOT NULL,
            published_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            exclusive_resource TEXT,
            alias_universe_uuid TEXT);",
    )?;
    for column in ["exclusive_resource", "alias_universe_uuid"] {
        let present: bool = db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('network_routes') WHERE name=?1",
            [column],
            |r| Ok(r.get::<_, i64>(0)? > 0),
        )?;
        if !present {
            db.execute_batch(&format!("ALTER TABLE network_routes ADD COLUMN {column} TEXT;"))?;
        }
    }
    // A table created by the first version made every address unique across released rows too, so
    // a released address could never be allocated again; the second keyed the table by universe,
    // so a universe deleted and put back (promoted from a recovery point at its own address) could
    // never be allocated again either. Only a LIVE allocation is unique, per address and per
    // universe; the released rows are history. An older table is rebuilt once, keeping its rows.
    let sql: Option<String> = db
        .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='network_allocations'", [], |r| r.get(0))
        .optional()?;
    if sql.is_some_and(|s| s.contains("UNIQUE") || s.contains("PRIMARY KEY")) {
        db.execute_batch(
            "DROP INDEX IF EXISTS network_allocations_live_ip;
             ALTER TABLE network_allocations RENAME TO network_allocations_old;
             CREATE TABLE network_allocations(
                universe_uuid TEXT NOT NULL,
                network_uuid TEXT NOT NULL,
                ip TEXT NOT NULL,
                allocated_at INTEGER NOT NULL,
                operation_id TEXT NOT NULL,
                released_at INTEGER,
                released_by TEXT);
             INSERT INTO network_allocations SELECT * FROM network_allocations_old;
             DROP TABLE network_allocations_old;
             CREATE UNIQUE INDEX IF NOT EXISTS network_allocations_live_ip ON network_allocations(ip) WHERE released_at IS NULL;
             CREATE UNIQUE INDEX IF NOT EXISTS network_allocations_live_universe ON network_allocations(universe_uuid) WHERE released_at IS NULL;",
        )?;
    }
    ensure_ledger(db)
}

/// `a.b.c.d/n` with the network bits only, refused otherwise.
struct Cidr {
    address: Ipv4Addr,
    bits: u8,
}
impl Cidr {
    fn parse(text: &str, field: &str) -> Result<Cidr, Error> {
        let (a, b) = text.split_once('/').ok_or_else(|| format!("{field} must be a.b.c.d/n"))?;
        let address: Ipv4Addr = a.parse().map_err(|_| format!("{field} has an invalid address"))?;
        let bits: u8 = b.parse().map_err(|_| format!("{field} has an invalid prefix length"))?;
        if !(8..=30).contains(&bits) {
            return Err(format!("{field} prefix length must be from 8 to 30").into());
        }
        let cidr = Cidr { address, bits };
        if cidr.first() != u32::from(address) {
            return Err(format!("{field} must name the network address, not a host in it").into());
        }
        Ok(cidr)
    }
    fn mask(&self) -> u32 {
        if self.bits == 0 { 0 } else { u32::MAX << (32 - self.bits) }
    }
    fn first(&self) -> u32 {
        u32::from(self.address) & self.mask()
    }
    fn last(&self) -> u32 {
        self.first() | !self.mask()
    }
    fn contains(&self, ip: Ipv4Addr) -> bool {
        u32::from(ip) & self.mask() == self.first()
    }
    fn contains_cidr(&self, other: &Cidr) -> bool {
        other.bits >= self.bits && self.contains(other.address)
    }
    fn text(&self) -> String {
        format!("{}/{}", self.address, self.bits)
    }
}

fn identifier_ip(text: &str, field: &str) -> Result<Ipv4Addr, Error> {
    text.parse().map_err(|_| format!("{field} must be an IPv4 address").into())
}

pub struct Declaration {
    pub network_uuid: String,
    pub prefix: String,
    pub pool: String,
    pub gateway: String,
    pub bridge: String,
    pub state: String,
}

pub fn declared(db: &Connection) -> Result<Option<Declaration>, Error> {
    Ok(db
        .query_row(
            "SELECT network_uuid,prefix,pool,gateway,bridge,state FROM network_declaration LIMIT 1",
            [],
            |r| Ok(Declaration { network_uuid: r.get(0)?, prefix: r.get(1)?, pool: r.get(2)?, gateway: r.get(3)?, bridge: r.get(4)?, state: r.get(5)? }),
        )
        .optional()?)
}

/// `ip -4 route show <dst>`: the routes the kernel holds for exactly this destination, or None when
/// the command itself could not be run -- which is unknown, not "no route".
fn routes_for(dst: &str) -> Option<Vec<String>> {
    let out = std::process::Command::new("ip").args(["-4", "route", "show", dst]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

fn ip_route(args: &[&str]) -> Result<(), Error> {
    let out = std::process::Command::new("ip").args(args).output()?;
    if !out.status.success() {
        return Err(format!("ip {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    Ok(())
}

fn nft(args: &[&str]) -> Result<(), Error> {
    let out = std::process::Command::new("nft").args(args).output()?;
    if !out.status.success() {
        return Err(format!("nft {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    Ok(())
}

/// Whether PodMesh's table exists, from `nft list tables`; `None` when nft cannot be asked.
fn nat_exemption_present() -> Option<bool> {
    let out = std::process::Command::new("nft").args(["list", "tables"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == format!("table inet {NFT_TABLE}")))
}

fn nft_rules() -> Option<Vec<String>> {
    if nat_exemption_present() != Some(true) {
        return Some(vec![]);
    }
    let out = std::process::Command::new("nft").args(["list", "table", "inet", NFT_TABLE]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| l.contains("notrack")).map(str::to_string).collect())
}

/// Two raw-hook chains, prerouting and output, leaving prefix-to-prefix traffic untracked in
/// both directions; the table is PodMesh's own and holds nothing else.
fn nat_exemption_add(prefix: &str) -> Result<(), Error> {
    nft(&["add", "table", "inet", NFT_TABLE])?;
    let rule = format!("ip saddr {prefix} ip daddr {prefix} notrack");
    for (chain, hook) in [("prerouting", "prerouting"), ("output", "output")] {
        nft(&["add", "chain", "inet", NFT_TABLE, chain, &format!("{{ type filter hook {hook} priority raw; }}")])?;
        nft(&["add", "rule", "inet", NFT_TABLE, chain, &rule])?;
    }
    Ok(())
}

fn bridge_subnets(bridge: &str) -> Option<Vec<String>> {
    let out = std::process::Command::new("podman")
        .args(["network", "inspect", bridge, "--format", "{{range .Subnets}}{{.Subnet}} {{end}}"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).split_whitespace().map(str::to_string).collect())
}

fn bridge_exists(bridge: &str) -> Option<bool> {
    let out = std::process::Command::new("podman").args(["network", "exists", bridge]).output().ok()?;
    Some(out.status.success())
}

/// The effective state, read from the kernel and Podman -- what a caller may rely on.
fn effective(db: &Connection) -> Result<Value, Error> {
    let d = declared(db)?;
    let mut peers = db.prepare("SELECT pool,via FROM network_peer_pools ORDER BY pool")?;
    let peer_rows: Vec<(String, String)> = peers.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let peer_routes: Vec<Value> = peer_rows
        .iter()
        .map(|(pool, via)| {
            let held = routes_for(pool);
            json!({"pool": pool, "via": via,
                   "effective": held.as_ref().map(|h| h.iter().any(|l| l.contains(&format!("via {via}")))),
                   "routes": held})
        })
        .collect();
    let mut routes = db.prepare("SELECT ip,universe_uuid,via,exclusive_resource,alias_universe_uuid,state FROM network_routes ORDER BY ip")?;
    type RouteRow = (String, String, String, Option<String>, Option<String>, Option<String>);
    let route_rows: Vec<RouteRow> =
        routes.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect::<Result<_, _>>()?;
    let published: Vec<Value> = route_rows
        .iter()
        .map(|(ip, u, via, resource, alias, state)| {
            let held = routes_for(&format!("{ip}/32"));
            let alias_effective = alias.as_ref().and_then(|a| running_pid(a).ok().flatten()).and_then(|pid| alias_present(pid, ip));
            json!({"ip": ip, "universe_uuid": u, "via": via, "exclusive_resource": resource, "alias_universe_uuid": alias, "alias_effective": alias_effective,
                   "state": state.clone().unwrap_or_else(|| "effective".into()),
                   "effective": held.as_ref().map(|h| h.iter().any(|l| l.contains(&format!("via {via}")))), "routes": held})
        })
        .collect();
    Ok(json!({
        "bridge": d.as_ref().map(|d| json!({"name": d.bridge, "exists": bridge_exists(&d.bridge), "subnets": bridge_subnets(&d.bridge)})),
        "peer_pool_routes": peer_routes,
        "published_routes": published,
        "ip_forward": std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").ok().map(|s| s.trim() == "1"),
    }))
}

fn view(db: &Connection) -> Result<Value, Error> {
    let d = declared(db)?;
    let mut s = db.prepare("SELECT universe_uuid,ip,released_at FROM network_allocations ORDER BY ip")?;
    let allocations: Vec<Value> = s
        .query_map([], |r| Ok(json!({"universe_uuid": r.get::<_, String>(0)?, "ip": r.get::<_, String>(1)?, "released_at": r.get::<_, Option<i64>>(2)?})))?
        .collect::<Result<_, _>>()?;
    Ok(json!({
        "declaration": d.as_ref().map(|d| json!({"network_uuid": d.network_uuid, "prefix": d.prefix, "pool": d.pool, "gateway": d.gateway, "bridge": d.bridge, "state": d.state,
                                                  "observed": db.query_row("SELECT observed FROM network_declaration WHERE network_uuid=?1", [&d.network_uuid], |r| r.get::<_, Option<String>>(0)).ok().flatten().and_then(|s| serde_json::from_str::<Value>(&s).ok())})),
        "allocations": allocations,
        "effective": effective(db)?,
        "nat_exemption": {"table": NFT_TABLE, "present": nat_exemption_present(), "rules": nft_rules()},
        "effects": effect_rows(db, None)?.iter().map(|e| json!({"kind": e.kind, "key": e.key, "owner": e.owner, "state": e.state, "present": effect_verify(e)})).collect::<Vec<_>>(),
        "incomplete_effects": effect_rows(db, None)?.iter().filter(|e| e.state != "effective").count(),
        "scope": "this host's declaration, allocations and routes; the effective state is read from Podman and the kernel now, and an observation that could not be made is null (unknown), never zero",
    }))
}

/// Allocate the next free address of the local pool to a universe, for `create`. Refuses without a
/// declaration, and returns an existing live allocation of the same universe unchanged.
pub(crate) fn allocate(db: &Connection, uuid: &str, id: &str, requested: Option<&str>) -> Result<(String, String, String), Error> {
    ensure_schema(db)?;
    let d = declared(db)?.ok_or("The managed profile needs a network declared on this host (network_declare); none is")?;
    if d.state != "effective" {
        return Err(format!("The network declaration on this host is in state {}, not effective", d.state).into());
    }
    if let Some(ip) = db
        .query_row("SELECT ip FROM network_allocations WHERE universe_uuid=?1 AND released_at IS NULL", [uuid], |r| r.get::<_, String>(0))
        .optional()?
    {
        return Ok((d.bridge, ip, d.network_uuid));
    }
    let pool = Cidr::parse(&d.pool, "pool")?;
    let gateway: Ipv4Addr = d.gateway.parse()?;
    let mut taken = db.prepare("SELECT ip FROM network_allocations WHERE released_at IS NULL")?;
    let used: std::collections::BTreeSet<u32> = taken
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .filter_map(|s| s.parse::<Ipv4Addr>().ok().map(u32::from))
        .collect();
    // A requested address is honoured only inside the local pool, not the gateway, and free: it
    // lets an operator choose addresses that other universes' configurations name before those
    // universes exist. Without one, the next free address of the pool.
    let ip = match requested {
        Some(text) => {
            let a = identifier_ip(text, "network_address")?;
            if !pool.contains(a) || u32::from(a) == pool.first() || u32::from(a) == pool.last() {
                return Err(format!("network_address {a} is outside the local pool {}", pool.text()).into());
            }
            if a == gateway {
                return Err(format!("network_address {a} is the pool's gateway").into());
            }
            if used.contains(&u32::from(a)) {
                return Err(format!("network_address {a} is allocated to another universe").into());
            }
            a
        }
        None => {
            let mut candidate = pool.first() + 1;
            loop {
                if candidate >= pool.last() {
                    return Err("The local pool has no free address left".into());
                }
                let a = Ipv4Addr::from(candidate);
                if a != gateway && !used.contains(&candidate) {
                    break a;
                }
                candidate += 1;
            }
        }
    };
    db.execute(
        "INSERT INTO network_allocations(universe_uuid,network_uuid,ip,allocated_at,operation_id) VALUES(?1,?2,?3,?4,?5)",
        params![uuid, d.network_uuid, ip.to_string(), crate::now() as i64, id],
    )?;
    Ok((d.bridge, ip.to_string(), d.network_uuid))
}

/// Release a universe's allocation, for `delete` and for a `create` that could not be observed.
pub(crate) fn release(db: &Connection, uuid: &str, id: &str) -> Result<Option<String>, Error> {
    ensure_schema(db)?;
    let ip: Option<String> = db
        .query_row("SELECT ip FROM network_allocations WHERE universe_uuid=?1 AND released_at IS NULL", [uuid], |r| r.get(0))
        .optional()?;
    if ip.is_some() {
        db.execute(
            "UPDATE network_allocations SET released_at=?2, released_by=?3 WHERE universe_uuid=?1 AND released_at IS NULL",
            params![uuid, crate::now() as i64, id],
        )?;
    }
    Ok(ip)
}

/// The requested and effective network of a container, from its labels and Podman's view.
pub(crate) fn of_container(c: &Value) -> Value {
    let labels = &c["Config"]["Labels"];
    let profile = labels[LABEL_PROFILE].as_str();
    let requested_ip = labels[LABEL_IP].as_str();
    let networks = &c["NetworkSettings"]["Networks"];
    let effective: Vec<Value> = networks
        .as_object()
        .map(|m| m.iter().map(|(name, n)| json!({"network": name, "ip": n["IPAddress"].as_str().filter(|s| !s.is_empty())})).collect())
        .unwrap_or_default();
    json!({
        "profile": profile.unwrap_or("unknown"),
        "requested": {"ip": requested_ip, "network_uuid": labels[LABEL_NETWORK].as_str()},
        "effective": effective,
    })
}

// ---------------------------------------------------------------- the effects ledger
//
// Every kernel or Podman mutation this module makes is recorded here BEFORE it is made, with what
// is needed to verify it and to undo it. The daemon runs operations in autocommit, so the row is
// durable before `ip`, `podman` or `nft` run. Its state follows the mutation: `applying` until the
// effect is verified from outside, `effective` afterwards, `removing` while it is undone, gone
// when the removal is verified. `reconcile` undoes every row that is not `effective` -- after a
// crash, a storage failure, or an operation that could not compensate -- and reports what it did;
// it runs at daemon startup, before every network operation and before every fence. What this
// buys: no route, address, bridge or nftables table this module created can outlive its record,
// so the fence always has something durable to find. What it does not buy: an effect somebody
// else made, which is reported as drift and never touched.

fn ensure_ledger(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS network_effects(
            id INTEGER PRIMARY KEY,
            kind TEXT NOT NULL,
            key TEXT NOT NULL,
            owner TEXT NOT NULL,
            intent TEXT NOT NULL,
            state TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            changed_at INTEGER NOT NULL,
            observed TEXT);",
    )?;
    for (table, column) in [("network_routes", "state"), ("network_declaration", "observed")] {
        let present: bool = db.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name=?1"),
            [column],
            |r| Ok(r.get::<_, i64>(0)? > 0),
        )?;
        if !present {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} TEXT;"))?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Effect {
    id: i64,
    kind: String,
    key: String,
    owner: String,
    intent: Value,
    state: String,
}

fn effect_begin(db: &Connection, kind: &str, key: &str, owner: &str, intent: Value, id: &str) -> Result<Effect, Error> {
    db.execute(
        "INSERT INTO network_effects(kind,key,owner,intent,state,operation_id,changed_at) VALUES(?1,?2,?3,?4,'applying',?5,?6)",
        params![kind, key, owner, intent.to_string(), id, crate::now() as i64],
    )?;
    Ok(Effect { id: db.last_insert_rowid(), kind: kind.into(), key: key.into(), owner: owner.into(), intent, state: "applying".into() })
}

fn effect_state(db: &Connection, e: &Effect, state: &str, observed: Option<&Value>) -> Result<(), Error> {
    db.execute(
        "UPDATE network_effects SET state=?2, changed_at=?3, observed=?4 WHERE id=?1",
        params![e.id, state, crate::now() as i64, observed.map(Value::to_string)],
    )?;
    Ok(())
}

fn effect_rows(db: &Connection, owner: Option<&str>) -> Result<Vec<Effect>, Error> {
    let mut s = db.prepare("SELECT id,kind,key,owner,intent,state FROM network_effects WHERE (?1 IS NULL OR owner=?1) ORDER BY id")?;
    let rows = s
        .query_map([owner], |r| {
            Ok(Effect {
                id: r.get(0)?,
                kind: r.get(1)?,
                key: r.get(2)?,
                owner: r.get(3)?,
                intent: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                state: r.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_string()
}

/// The kernel or Podman mutation of one effect.
fn effect_apply(e: &Effect) -> Result<(), Error> {
    match e.kind.as_str() {
        "bridge" => lc::podman(
            lc::QUICK,
            &["network", "create", "--driver", "bridge", "--disable-dns", "--subnet", &text_of(&e.intent, "subnet"), "--gateway", &text_of(&e.intent, "gateway"), &e.key],
        )
        .map(|_| ()),
        "peer_route" | "route" => ip_route(&["-4", "route", "replace", &e.key, "via", &text_of(&e.intent, "via")]),
        "nat_table" => nat_exemption_add(&text_of(&e.intent, "prefix")),
        "alias" => {
            let pid = running_pid(&text_of(&e.intent, "universe"))?.ok_or("the universe that is to carry the address is not running")?;
            alias_add(pid, &text_of(&e.intent, "ip"))
        }
        other => Err(format!("unknown effect kind {other}").into()),
    }
}

/// Whether the effect is present, read from outside; `None` when that could not be read.
fn effect_verify(e: &Effect) -> Option<bool> {
    match e.kind.as_str() {
        "bridge" => match bridge_exists(&e.key)? {
            false => Some(false),
            true => Some(bridge_subnets(&e.key)? == vec![text_of(&e.intent, "subnet")]),
        },
        "peer_route" | "route" => routes_for(&e.key).map(|h| h.iter().any(|l| l.contains(&format!("via {}", text_of(&e.intent, "via"))))),
        "nat_table" => match nat_exemption_present()? {
            false => Some(false),
            true => Some(!nft_rules()?.is_empty()),
        },
        "alias" => match running_pid(&text_of(&e.intent, "universe")).ok()? {
            // A universe that no longer runs took the address down with its namespace.
            None => Some(false),
            Some(pid) => alias_present(pid, &text_of(&e.intent, "ip")),
        },
        _ => None,
    }
}

/// The removal of one effect; idempotent, and a no-op for what is already gone.
fn effect_undo(e: &Effect) -> Result<(), Error> {
    match e.kind.as_str() {
        "bridge" => {
            if bridge_exists(&e.key) == Some(true) {
                lc::podman(lc::QUICK, &["network", "rm", &e.key])?;
            }
            Ok(())
        }
        "peer_route" | "route" => {
            let via = text_of(&e.intent, "via");
            if routes_for(&e.key).is_some_and(|h| h.iter().any(|l| l.contains(&format!("via {via}")))) {
                ip_route(&["-4", "route", "del", &e.key, "via", &via])?;
            }
            Ok(())
        }
        "nat_table" => {
            if nat_exemption_present() == Some(true) {
                nft(&["delete", "table", "inet", NFT_TABLE])?;
            }
            Ok(())
        }
        "alias" => alias_remove(&text_of(&e.intent, "universe"), &text_of(&e.intent, "ip")),
        other => Err(format!("unknown effect kind {other}").into()),
    }
}

/// Apply, then verify from outside, then record effective. An error leaves the row `applying`
/// for the caller's compensation, or for reconciliation if the caller cannot.
fn effect_do(db: &Connection, e: &Effect) -> Result<(), Error> {
    effect_apply(e)?;
    fault(&format!("after-{}", e.kind))?;
    match effect_verify(e) {
        Some(true) => effect_state(db, e, "effective", None),
        Some(false) => Err(format!("{} {} is not effective after being applied", e.kind, e.key).into()),
        None => Err(format!("{} {} could not be verified after being applied; its state is unknown", e.kind, e.key).into()),
    }
}

/// Record removing, undo, verify gone, drop the row. An error leaves the row `removing` with
/// what was observed, for reconciliation.
fn effect_remove(db: &Connection, e: &Effect) -> Result<(), Error> {
    effect_state(db, e, "removing", None)?;
    if let Err(err) = effect_undo(e) {
        effect_state(db, e, "removing", Some(&json!({"error": err.to_string()})))?;
        return Err(err);
    }
    match effect_verify(e) {
        Some(false) => {
            db.execute("DELETE FROM network_effects WHERE id=?1", [e.id])?;
            Ok(())
        }
        Some(true) => {
            effect_state(db, e, "removing", Some(&json!({"still_present": true})))?;
            Err(format!("{} {} is still present after its removal", e.kind, e.key).into())
        }
        None => {
            effect_state(db, e, "removing", Some(&json!({"unknown": true})))?;
            Err(format!("{} {} could not be verified after its removal; its state is unknown", e.kind, e.key).into())
        }
    }
}

/// Undo a list of effects, last first, and say for each whether it is gone.
fn compensate(db: &Connection, effects: &[Effect]) -> Vec<Value> {
    let mut report = vec![];
    for e in effects.iter().rev() {
        match effect_remove(db, e) {
            Ok(()) => report.push(json!({"kind": e.kind, "key": e.key, "gone": true})),
            Err(err) => report.push(json!({"kind": e.kind, "key": e.key, "gone": false, "error": err.to_string()})),
        }
    }
    report
}

/// Lab-only fault injection, read from `PODMESH_FAULT`: `<point>` makes the daemon fail at that
/// point as a storage failure would; `<point>:crash` ends the process there, as a crash would.
/// Nothing sets this variable in the packaged units; the crash-safety check sets it on the
/// transient laboratory unit and restarts the daemon to watch reconciliation.
fn fault(point: &str) -> Result<(), Error> {
    let Ok(spec) = std::env::var("PODMESH_FAULT") else { return Ok(()) };
    if spec == format!("{point}:crash") {
        eprintln!("PodMesh fault injection: crashing at {point}");
        std::process::exit(70);
    }
    if spec == point {
        return Err(format!("simulated storage failure at {point}; nothing is recorded as effective").into());
    }
    Ok(())
}

/// Undo every effect that is not effective, complete every declaration or route whose effects
/// are gone, release every allocation whose container is gone, and report drift: an effective
/// effect the kernel no longer shows. Deterministic: an incomplete effect is always undone, never
/// finished, so the agent re-asks and exclusivity is never assumed. Anything that could not be
/// undone stays in the ledger and is reported as remaining; every network mutation refuses while
/// something remains, and `network_status` shows it.
pub fn reconcile(db: &Connection) -> Result<Value, Error> {
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    let mut undone = vec![];
    let mut remaining = vec![];
    let mut drift = vec![];
    let mut undo_all = |owner: &str, why: &str| -> Result<bool, Error> {
        // Every effect of an owner whose making or unmaking was interrupted, last first, whatever
        // its own state: a half-made route or declaration is undone whole, never finished.
        let mut all_gone = true;
        for e in effect_rows(db, Some(owner))?.iter().rev() {
            match effect_remove(db, e) {
                Ok(()) => undone.push(json!({"kind": e.kind, "key": e.key, "owner": owner, "was": e.state, "why": why})),
                Err(err) => {
                    all_gone = false;
                    remaining.push(json!({"kind": e.kind, "key": e.key, "owner": owner, "was": e.state, "why": why, "error": err.to_string()}));
                }
            }
        }
        Ok(all_gone)
    };
    // Routes whose making or withdrawal was interrupted.
    let mut s = db.prepare("SELECT ip,state FROM network_routes WHERE state IS NOT NULL AND state!='effective'")?;
    let interrupted: Vec<(String, String)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut routes_dropped = vec![];
    for (ip, state) in interrupted {
        if undo_all(&ip, &format!("route {state}"))? {
            db.execute("DELETE FROM network_routes WHERE ip=?1", [&ip])?;
            routes_dropped.push(ip);
        }
    }
    // A declaration whose making or unmaking was interrupted: undone whole; gone when nothing of
    // it remains, failed with what remains otherwise.
    let mut declarations = vec![];
    if let Some(d) = declared(db)? {
        if d.state == "declaring" || d.state == "undeclaring" || d.state == "failed" {
            let all_gone = undo_all(&d.network_uuid, &format!("declaration {}", d.state))?;
            if all_gone {
                db.execute("DELETE FROM network_peer_pools WHERE network_uuid=?1", [&d.network_uuid])?;
                db.execute("DELETE FROM network_declaration WHERE network_uuid=?1", [&d.network_uuid])?;
                declarations.push(json!({"network_uuid": d.network_uuid, "was": d.state, "now": "gone"}));
            } else {
                let observed = json!({"reconciled": true, "remaining": remaining.clone()});
                db.execute("UPDATE network_declaration SET state='failed', observed=?2 WHERE network_uuid=?1", params![d.network_uuid, observed.to_string()])?;
                declarations.push(json!({"network_uuid": d.network_uuid, "was": d.state, "now": "failed"}));
            }
        }
    }
    // Any other effect that is not effective; an orphan -- an effect whose owner (route or
    // declaration) no longer has a record, left by an interrupted cleanup -- is PodMesh's own and
    // is undone and dropped; and the drift of effective effects with an owner: reported, never
    // touched, since the kernel may have been changed by somebody else.
    let mut orphans = vec![];
    for e in effect_rows(db, None)? {
        if e.state == "effective" {
            let owned: i64 = db.query_row(
                "SELECT (SELECT COUNT(*) FROM network_routes WHERE ip=?1) + (SELECT COUNT(*) FROM network_declaration WHERE network_uuid=?1)",
                [&e.owner],
                |r| r.get(0),
            )?;
            if owned == 0 {
                match effect_remove(db, &e) {
                    Ok(()) => orphans.push(json!({"kind": e.kind, "key": e.key, "owner": e.owner, "gone": true})),
                    Err(err) => remaining.push(json!({"kind": e.kind, "key": e.key, "owner": e.owner, "was": "effective orphan", "error": err.to_string()})),
                }
                continue;
            }
            if effect_verify(&e) != Some(true) {
                drift.push(json!({"kind": e.kind, "key": e.key, "owner": e.owner, "observed": effect_verify(&e)}));
            }
            continue;
        }
        match effect_remove(db, &e) {
            Ok(()) => undone.push(json!({"kind": e.kind, "key": e.key, "owner": e.owner, "was": e.state, "why": "effect alone"})),
            Err(err) => remaining.push(json!({"kind": e.kind, "key": e.key, "owner": e.owner, "was": e.state, "why": "effect alone", "error": err.to_string()})),
        }
    }
    // An allocation whose container never came to be, or is gone without its delete.
    let mut a = db.prepare("SELECT universe_uuid,ip FROM network_allocations WHERE released_at IS NULL")?;
    let live: Vec<(String, String)> = a.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut released = vec![];
    for (u, ip) in live {
        if lc::inspect(&format!("podmesh-{u}"))?.is_none() {
            release(db, &u, "reconciliation")?;
            released.push(json!({"universe_uuid": u, "ip": ip}));
        }
    }
    Ok(json!({"undone": undone, "remaining": remaining, "drift": drift, "orphans": orphans, "routes_dropped": routes_dropped, "declarations": declarations, "allocations_released": released}))
}

fn refuse_while_incomplete(db: &Connection) -> Result<Value, Error> {
    let report = reconcile(db)?;
    if report["remaining"].as_array().is_some_and(|r| !r.is_empty()) {
        return Err(format!("incomplete network effects remain after reconciliation; refusing every network mutation until they are gone: {}", report["remaining"]).into());
    }
    Ok(report)
}

pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    if operation == "network_status" {
        return view(db);
    }
    if request.get("universe_uuid").is_some() && !operation.starts_with("network_route_") {
        return Err(format!("{operation} is host-wide and takes no universe_uuid").into());
    }
    lc::journaled(db, request, |db| {
        let reconciliation = refuse_while_incomplete(db)?;
        let mut answer = perform(db, request)?;
        answer["reconciliation_before"] = reconciliation;
        Ok(answer)
    })
}

fn perform(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let id = lc::text(request, "operation_id")?;
    let reference = lc::text(request, "authorization_ref")?;
    let now = crate::now() as i64;
    match operation {
        "network_declare" => {
            let network_uuid = lc::text(request, "network_uuid")?;
            lc::token(network_uuid)?;
            let prefix = Cidr::parse(lc::text(request, "prefix")?, "prefix")?;
            let pool = Cidr::parse(lc::text(request, "pool")?, "pool")?;
            if !prefix.contains_cidr(&pool) {
                return Err("pool must lie inside prefix".into());
            }
            if let Some(d) = declared(db)? {
                return Err(format!("This host already carries the declaration of network {} (state {}); undeclare it first", d.network_uuid, d.state).into());
            }
            let peer_pools: Vec<(String, String)> = match request.get("peer_pools") {
                None => vec![],
                Some(v) => v
                    .as_array()
                    .ok_or("peer_pools must be a list of {pool, via}")?
                    .iter()
                    .map(|e| -> Result<(String, String), Error> {
                        let p = Cidr::parse(lc::text(e, "pool")?, "peer_pools[].pool")?;
                        if !prefix.contains_cidr(&p) || p.first() == pool.first() {
                            return Err("a peer pool must lie inside prefix and differ from the local pool".into());
                        }
                        let via = identifier_ip(lc::text(e, "via")?, "peer_pools[].via")?;
                        Ok((p.text(), via.to_string()))
                    })
                    .collect::<Result<_, _>>()?,
            };
            // Nothing may already route any of these prefixes: an overlap is somebody else's network.
            for p in std::iter::once(pool.text()).chain(peer_pools.iter().map(|(p, _)| p.clone())) {
                match routes_for(&p) {
                    None => return Err("the kernel's routes could not be read; refusing to declare on an unknown state".into()),
                    Some(r) if !r.is_empty() => return Err(format!("a route for {p} already exists: {}", r.join("; ")).into()),
                    _ => {}
                }
            }
            if bridge_exists(BRIDGE) != Some(false) {
                return Err(format!("the Podman network {BRIDGE} already exists or could not be checked; refusing to declare over it").into());
            }
            if nat_exemption_present() != Some(false) {
                return Err(format!("the nftables table {NFT_TABLE} already exists or could not be checked; refusing to declare over it").into());
            }
            let gateway = Ipv4Addr::from(pool.first() + 1).to_string();
            db.execute(
                "INSERT INTO network_declaration(network_uuid,prefix,pool,gateway,bridge,state,declared_at,operation_id,authorization_ref) VALUES(?1,?2,?3,?4,?5,'declaring',?6,?7,?8)",
                params![network_uuid, prefix.text(), pool.text(), gateway, BRIDGE, now, id, reference],
            )?;
            // The plan, each step recorded before it is made: the bridge, one route per peer pool,
            // and the table that keeps Podman's source NAT off the prefix (see nat_exemption_add).
            let mut plan: Vec<(&str, String, Value)> = vec![("bridge", BRIDGE.to_string(), json!({"subnet": pool.text(), "gateway": gateway}))];
            for (p, via) in &peer_pools {
                plan.push(("peer_route", p.clone(), json!({"via": via})));
            }
            plan.push(("nat_table", NFT_TABLE.to_string(), json!({"prefix": prefix.text()})));
            let mut done: Vec<Effect> = vec![];
            let mut failure: Option<String> = None;
            for (kind, key, intent) in plan {
                let e = effect_begin(db, kind, &key, network_uuid, intent, id)?;
                done.push(e.clone());
                if let Err(err) = effect_do(db, &e) {
                    failure = Some(err.to_string());
                    break;
                }
            }
            if failure.is_none() {
                if let Err(err) = fault("before-declaration-effective") {
                    failure = Some(err.to_string());
                }
            }
            if let Some(err) = failure {
                // Compensation: everything made so far is undone, last first, and what is observed
                // afterwards is recorded with the failed declaration -- which `network_undeclare`
                // then removes, or reconciliation finishes.
                let report = compensate(db, &done);
                let observed = json!({"error": err, "compensation": report});
                db.execute("UPDATE network_declaration SET state='failed', observed=?2 WHERE network_uuid=?1", params![network_uuid, observed.to_string()])?;
                return Err(format!("{err}; compensation: {}", json!(report)).into());
            }
            for (p, via) in &peer_pools {
                db.execute("INSERT INTO network_peer_pools VALUES(?1,?2,?3)", params![p, via, network_uuid])?;
            }
            db.execute("UPDATE network_declaration SET state='effective' WHERE network_uuid=?1", [network_uuid])?;
        }
        "network_undeclare" => {
            let network_uuid = lc::text(request, "network_uuid")?;
            let Some(d) = declared(db)? else { return Err("No network is declared on this host".into()) };
            if d.network_uuid != network_uuid {
                return Err(format!("This host carries network {}, not {network_uuid}", d.network_uuid).into());
            }
            let live: i64 = db.query_row("SELECT COUNT(*) FROM network_allocations WHERE released_at IS NULL", [], |r| r.get(0))?;
            let routes: i64 = db.query_row("SELECT COUNT(*) FROM network_routes", [], |r| r.get(0))?;
            if live > 0 || routes > 0 {
                return Err(format!("{live} live allocation(s) and {routes} published route(s) remain; nothing is undeclared").into());
            }
            db.execute("UPDATE network_declaration SET state='undeclaring' WHERE network_uuid=?1", [network_uuid])?;
            // Every recorded effect of this network, last first; a failed or interrupted declaration
            // is cleaned the same way, and an effect already gone is a no-op.
            let effects = effect_rows(db, Some(network_uuid))?;
            for e in effects.iter().rev() {
                effect_remove(db, e)?;
                fault(&format!("undeclare-after-{}", e.kind))?;
            }
            // Declarations made before the ledger existed have no effect rows: their pieces are
            // removed from the tables that named them, and verified the same way.
            let mut peers = db.prepare("SELECT pool,via FROM network_peer_pools")?;
            let peer_rows: Vec<(String, String)> = peers.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
            for (p, via) in &peer_rows {
                if routes_for(p).is_some_and(|h| h.iter().any(|l| l.contains(&format!("via {via}")))) {
                    ip_route(&["-4", "route", "del", p, "via", via])?;
                }
                if routes_for(p).is_none_or(|h| h.iter().any(|l| l.contains(&format!("via {via}")))) {
                    return Err(format!("the route for {p} is still effective or unknown after its removal").into());
                }
            }
            if bridge_exists(&d.bridge) == Some(true) {
                lc::podman(lc::QUICK, &["network", "rm", &d.bridge])?;
            }
            if bridge_exists(&d.bridge) != Some(false) {
                return Err("the bridge still exists or could not be checked after its removal".into());
            }
            if nat_exemption_present() == Some(true) {
                nft(&["delete", "table", "inet", NFT_TABLE])?;
            }
            if nat_exemption_present() != Some(false) {
                return Err(format!("the nftables table {NFT_TABLE} still exists or could not be checked after its removal").into());
            }
            db.execute("DELETE FROM network_peer_pools WHERE network_uuid=?1", [network_uuid])?;
            db.execute("DELETE FROM network_declaration WHERE network_uuid=?1", [network_uuid])?;
        }
        "network_route_publish" => {
            let uuid = lc::text(request, "universe_uuid")?;
            lc::token(uuid)?;
            let ip = identifier_ip(lc::text(request, "ip")?, "ip")?;
            let via = identifier_ip(lc::text(request, "via")?, "via")?;
            let d = declared(db)?.ok_or("No network is declared on this host")?;
            if d.state != "effective" {
                return Err(format!("the network declaration on this host is in state {}, not effective", d.state).into());
            }
            if !Cidr::parse(&d.prefix, "prefix")?.contains(ip) {
                return Err(format!("{ip} is outside the declared prefix {}", d.prefix)).map_err(|e| e.into());
            }
            let placed_here: i64 = db.query_row("SELECT COUNT(*) FROM network_allocations WHERE universe_uuid=?1 AND released_at IS NULL", [uuid], |r| r.get(0))?;
            if placed_here > 0 {
                return Err("the universe is allocated on this host; a route to elsewhere would announce it twice".into());
            }
            // An exclusive route -- the service address of a role only one host may hold -- is published
            // only by the host holding that role's live activation lease under the epoch gate. The
            // route records the resource, and the self-fence withdraws it when the lease is gone.
            let resource = match request.get("exclusive_resource") {
                None => None,
                Some(v) => {
                    let r = v.as_str().ok_or("exclusive_resource must be a string")?;
                    lc::token(r)?;
                    if crate::activation::policy(db, r)?.is_none() {
                        return Err(format!("exclusive_resource {r} is under no activation policy on this host; an exclusive route needs the lease of a declared resource").into());
                    }
                    crate::activation::refuse_if_not_activated(db, r, "network_route_publish")?;
                    Some(r.to_string())
                }
            };
            let dst = format!("{ip}/32");
            match routes_for(&dst) {
                None => return Err("the kernel's routes could not be read; refusing on an unknown state".into()),
                Some(r) if !r.is_empty() => return Err(format!("a route for {ip} is already effective: {}; withdraw it first", r.join("; ")).into()),
                _ => {}
            }
            let existing: i64 = db.query_row("SELECT COUNT(*) FROM network_routes WHERE ip=?1", [ip.to_string()], |r| r.get(0))?;
            if existing > 0 {
                return Err(format!("a route for {ip} is already recorded here; withdraw it first").into());
            }
            // An exclusive route announces a role's address; the replica it points at must carry
            // that address to answer there. So the route must point at a running universe of this
            // host, which receives the address as an alias inside its own network namespace, added
            // before the route and verified from inside; the route follows. Both are recorded before
            // they are made, under the route's address, and the route's own row is written first,
            // `applying`, so that the fence has a durable target from the first moment.
            let mut alias_universe: Option<String> = None;
            if resource.is_some() {
                let Some((carrier, _pid)) = universe_at(db, &via.to_string())? else {
                    return Err(format!("an exclusive route must point at a running universe of this host, which then carries {ip}; nothing runs at {via} here").into());
                };
                alias_universe = Some(carrier);
            }
            db.execute(
                "INSERT INTO network_routes(ip,universe_uuid,via,published_at,operation_id,exclusive_resource,alias_universe_uuid,state) VALUES(?1,?2,?3,?4,?5,?6,?7,'applying')",
                params![ip.to_string(), uuid, via.to_string(), now, id, resource, alias_universe],
            )?;
            let mut plan: Vec<(&str, String, Value)> = vec![];
            if let Some(carrier) = &alias_universe {
                plan.push(("alias", format!("{ip}@{carrier}"), json!({"ip": ip.to_string(), "universe": carrier})));
            }
            plan.push(("route", dst.clone(), json!({"via": via.to_string()})));
            let mut done: Vec<Effect> = vec![];
            let mut failure: Option<String> = None;
            for (kind, key, intent) in plan {
                let e = effect_begin(db, kind, &key, &ip.to_string(), intent, id)?;
                done.push(e.clone());
                if let Err(err) = effect_do(db, &e) {
                    failure = Some(err.to_string());
                    break;
                }
            }
            if failure.is_none() {
                if let Err(err) = fault("before-route-effective") {
                    failure = Some(err.to_string());
                }
            }
            if let Some(err) = failure {
                let report = compensate(db, &done);
                let all_gone = report.iter().all(|r| r["gone"] == json!(true));
                if all_gone {
                    db.execute("DELETE FROM network_routes WHERE ip=?1", [ip.to_string()])?;
                }
                return Err(format!("{err}; compensation: {}{}", json!(report), if all_gone { "" } else { "; the route's record is kept for reconciliation" }).into());
            }
            db.execute("UPDATE network_routes SET state='effective' WHERE ip=?1", [ip.to_string()])?;
        }
        "network_route_withdraw" => {
            let uuid = lc::text(request, "universe_uuid")?;
            lc::token(uuid)?;
            let ip: Option<String> = db.query_row("SELECT ip FROM network_routes WHERE universe_uuid=?1", [uuid], |r| r.get(0)).optional()?;
            let Some(ip) = ip else { return Err("No route is published here for this universe".into()) };
            let report = withdraw_route(db, &ip)?;
            if report["withdrawn"] != json!(true) {
                return Err(format!("the withdrawal of the route for {ip} did not complete: {report}").into());
            }
        }
        _ => return Err("Unsupported network operation".into()),
    }
    view(db)
}

/// Withdraw one recorded route with everything it carried: the row marked `removing` first, each
/// effect undone last first and verified gone, then the row dropped. Routes recorded before the
/// ledger existed are undone from the row's own fields. Used by the withdrawal and by the fence.
fn withdraw_route(db: &Connection, ip: &str) -> Result<Value, Error> {
    let row: Option<(String, String, Option<String>, Option<String>)> = db
        .query_row("SELECT universe_uuid,via,exclusive_resource,alias_universe_uuid FROM network_routes WHERE ip=?1", [ip], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .optional()?;
    let Some((universe, via, resource, alias)) = row else { return Ok(json!({"ip": ip, "withdrawn": false, "error": "no such route recorded"})) };
    db.execute("UPDATE network_routes SET state='removing' WHERE ip=?1", [ip])?;
    let effects = effect_rows(db, Some(ip))?;
    let mut steps = vec![];
    let mut failed = false;
    if effects.is_empty() {
        // Before the ledger: the route and the alias from the row itself.
        let dst = format!("{ip}/32");
        let route = Effect { id: 0, kind: "route".into(), key: dst, owner: ip.into(), intent: json!({"via": via}), state: "effective".into() };
        match effect_undo(&route).and_then(|_| match effect_verify(&route) { Some(false) => Ok(()), _ => Err("still present or unknown".into()) }) {
            Ok(()) => steps.push(json!({"kind": "route", "gone": true})),
            Err(e) => { failed = true; steps.push(json!({"kind": "route", "gone": false, "error": e.to_string()})) }
        }
        if let Some(carrier) = &alias {
            match alias_remove(carrier, ip) {
                Ok(()) => steps.push(json!({"kind": "alias", "gone": true})),
                Err(e) => { failed = true; steps.push(json!({"kind": "alias", "gone": false, "error": e.to_string()})) }
            }
        }
    } else {
        for e in effects.iter().rev() {
            match effect_remove(db, e) {
                Ok(()) => steps.push(json!({"kind": e.kind, "key": e.key, "gone": true})),
                Err(err) => { failed = true; steps.push(json!({"kind": e.kind, "key": e.key, "gone": false, "error": err.to_string()})) }
            }
            if !failed {
                fault(&format!("withdraw-after-{}", e.kind))?;
            }
        }
    }
    if !failed {
        db.execute("DELETE FROM network_routes WHERE ip=?1", [ip])?;
    }
    let still = routes_for(&format!("{ip}/32"));
    Ok(json!({"ip": ip, "universe_uuid": universe, "exclusive_resource": resource, "alias_universe_uuid": alias, "withdrawn": !failed,
              "alias_withdrawn": steps.iter().filter(|s| s["kind"] == "alias").all(|s| s["gone"] == json!(true)), "steps": steps, "routes_now": still}))
}

/// Withdraw every published route bound to an exclusive resource this host no longer holds --
/// called by the self-fence, which is the one operation that acts on what a host is not entitled
/// to, after reconciliation has undone every effect that never became effective. Each withdrawal
/// is verified from the kernel; one that does not take is reported, never reported as done.
pub(crate) fn withdraw_unentitled(db: &Connection, entitled: &dyn Fn(&str) -> bool) -> Result<Vec<Value>, Error> {
    ensure_schema(db)?;
    let mut s = db.prepare("SELECT ip,exclusive_resource FROM network_routes WHERE exclusive_resource IS NOT NULL")?;
    let rows: Vec<(String, String)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut report = vec![];
    for (ip, resource) in rows {
        if entitled(&resource) {
            continue;
        }
        report.push(withdraw_route(db, &ip)?);
    }
    Ok(report)
}

/// The running universe allocated at `ip` on this host, with its PID: the replica an exclusive
/// route points at, which will carry the announced address.
fn universe_at(db: &Connection, ip: &str) -> Result<Option<(String, i64)>, Error> {
    let uuid: Option<String> = db
        .query_row("SELECT universe_uuid FROM network_allocations WHERE ip=?1 AND released_at IS NULL", [ip], |r| r.get(0))
        .optional()?;
    let Some(uuid) = uuid else { return Ok(None) };
    Ok(running_pid(&uuid)?.map(|pid| (uuid, pid)))
}

fn running_pid(uuid: &str) -> Result<Option<i64>, Error> {
    let Some(c) = lc::inspect(&format!("podmesh-{uuid}"))? else { return Ok(None) };
    if c["State"]["Running"] != json!(true) {
        return Ok(None);
    }
    Ok(c["State"]["Pid"].as_i64().filter(|p| *p > 0))
}

/// `ip` run inside a universe's network namespace, through util-linux's nsenter on the host's own
/// binary: nothing is required inside the universe.
fn in_netns(pid: i64, args: &[&str]) -> Result<String, Error> {
    let out = std::process::Command::new("nsenter").args(["-t", &pid.to_string(), "-n", "--", "ip"]).args(args).output()?;
    if !out.status.success() {
        return Err(format!("ip {} inside the universe: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The interface inside the universe that carries an address of the managed bridge, and whether
/// `ip` is already on it. `None` when the namespace could not be read.
fn alias_present(pid: i64, ip: &str) -> Option<bool> {
    let listing = in_netns(pid, &["-4", "-o", "addr", "show"]).ok()?;
    Some(listing.lines().any(|l| l.split_whitespace().any(|w| w == format!("{ip}/32"))))
}

fn bridge_interface(pid: i64) -> Result<String, Error> {
    let listing = in_netns(pid, &["-4", "-o", "addr", "show"])?;
    listing
        .lines()
        .filter(|l| !l.contains(" lo "))
        .find_map(|l| l.split_whitespace().nth(1).map(str::to_string))
        .ok_or_else(|| "the universe has no interface but lo; nothing can carry the address".into())
}

fn alias_add(pid: i64, ip: &str) -> Result<(), Error> {
    if alias_present(pid, ip) == Some(true) {
        return Ok(());
    }
    let dev = bridge_interface(pid)?;
    in_netns(pid, &["-4", "addr", "add", &format!("{ip}/32"), "dev", &dev])?;
    Ok(())
}

fn alias_del(pid: i64, ip: &str) -> Result<(), Error> {
    if alias_present(pid, ip) != Some(true) {
        return Ok(());
    }
    let dev = bridge_interface(pid)?;
    in_netns(pid, &["-4", "addr", "del", &format!("{ip}/32"), "dev", &dev])?;
    Ok(())
}

/// Remove the address from the universe that carried it, verified gone from inside; a universe
/// that no longer runs took it down with its namespace, and that is reported as gone too.
fn alias_remove(carrier: &str, ip: &str) -> Result<(), Error> {
    let Some(pid) = running_pid(carrier)? else { return Ok(()) };
    alias_del(pid, ip)?;
    match alias_present(pid, ip) {
        Some(false) => Ok(()),
        Some(true) => Err(format!("{ip} is still carried by universe {carrier} after its removal").into()),
        None => Err(format!("the addresses of universe {carrier} could not be read after the removal; its state is unknown").into()),
    }
}
