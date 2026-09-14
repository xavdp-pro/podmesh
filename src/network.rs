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
            universe_uuid TEXT PRIMARY KEY,
            network_uuid TEXT NOT NULL,
            ip TEXT NOT NULL,
            allocated_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            released_at INTEGER,
            released_by TEXT);
         CREATE UNIQUE INDEX IF NOT EXISTS network_allocations_live_ip ON network_allocations(ip) WHERE released_at IS NULL;
         CREATE TABLE IF NOT EXISTS network_routes(
            ip TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            via TEXT NOT NULL,
            published_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL);",
    )?;
    // A table created by the first version made every address unique across released rows too, so
    // a released address could never be allocated again. Only a LIVE allocation is unique per
    // address; that table is rebuilt once, keeping its rows.
    let sql: Option<String> = db
        .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='network_allocations'", [], |r| r.get(0))
        .optional()?;
    if sql.is_some_and(|s| s.contains("UNIQUE")) {
        db.execute_batch(
            "ALTER TABLE network_allocations RENAME TO network_allocations_v1;
             CREATE TABLE network_allocations(
                universe_uuid TEXT PRIMARY KEY,
                network_uuid TEXT NOT NULL,
                ip TEXT NOT NULL,
                allocated_at INTEGER NOT NULL,
                operation_id TEXT NOT NULL,
                released_at INTEGER,
                released_by TEXT);
             INSERT INTO network_allocations SELECT * FROM network_allocations_v1;
             DROP TABLE network_allocations_v1;
             CREATE UNIQUE INDEX IF NOT EXISTS network_allocations_live_ip ON network_allocations(ip) WHERE released_at IS NULL;",
        )?;
    }
    Ok(())
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
    let mut routes = db.prepare("SELECT ip,universe_uuid,via FROM network_routes ORDER BY ip")?;
    let route_rows: Vec<(String, String, String)> = routes.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let published: Vec<Value> = route_rows
        .iter()
        .map(|(ip, u, via)| {
            let held = routes_for(&format!("{ip}/32"));
            json!({"ip": ip, "universe_uuid": u, "via": via,
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
        "declaration": d.as_ref().map(|d| json!({"network_uuid": d.network_uuid, "prefix": d.prefix, "pool": d.pool, "gateway": d.gateway, "bridge": d.bridge, "state": d.state})),
        "allocations": allocations,
        "effective": effective(db)?,
        "scope": "this host's declaration, allocations and routes; the effective state is read from Podman and the kernel now, and an observation that could not be made is null (unknown), never zero",
    }))
}

/// Allocate the next free address of the local pool to a universe, for `create`. Refuses without a
/// declaration, and returns an existing live allocation of the same universe unchanged.
pub(crate) fn allocate(db: &Connection, uuid: &str, id: &str) -> Result<(String, String, String), Error> {
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
    let mut candidate = pool.first() + 1;
    let ip = loop {
        if candidate >= pool.last() {
            return Err("The local pool has no free address left".into());
        }
        let a = Ipv4Addr::from(candidate);
        if a != gateway && !used.contains(&candidate) {
            break a;
        }
        candidate += 1;
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
    lc::journaled(db, request, |db| perform(db, request))
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
                return Err(format!("This host already carries the declaration of network {}; undeclare it first", d.network_uuid).into());
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
            let gateway = Ipv4Addr::from(pool.first() + 1).to_string();
            db.execute(
                "INSERT INTO network_declaration VALUES(?1,?2,?3,?4,?5,'declaring',?6,?7,?8)",
                params![network_uuid, prefix.text(), pool.text(), gateway, BRIDGE, now, id, reference],
            )?;
            lc::podman(lc::QUICK, &["network", "create", "--driver", "bridge", "--disable-dns", "--subnet", &pool.text(), "--gateway", &gateway, BRIDGE])?;
            for (p, via) in &peer_pools {
                ip_route(&["-4", "route", "replace", p, "via", via])?;
                db.execute("INSERT INTO network_peer_pools VALUES(?1,?2,?3)", params![p, via, network_uuid])?;
            }
            // Verified from outside before it is called effective.
            let subnets = bridge_subnets(BRIDGE).ok_or("the bridge could not be inspected after creation")?;
            if subnets != vec![pool.text()] {
                db.execute("UPDATE network_declaration SET state='failed' WHERE network_uuid=?1", [network_uuid])?;
                return Err(format!("the bridge carries subnets {subnets:?}, not {}", pool.text()).into());
            }
            for (p, via) in &peer_pools {
                let held = routes_for(p).ok_or("routes could not be read after publication")?;
                if !held.iter().any(|l| l.contains(&format!("via {via}"))) {
                    db.execute("UPDATE network_declaration SET state='failed' WHERE network_uuid=?1", [network_uuid])?;
                    return Err(format!("the route for {p} via {via} is not effective: {held:?}").into());
                }
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
            db.execute("DELETE FROM network_peer_pools WHERE network_uuid=?1", [network_uuid])?;
            db.execute("DELETE FROM network_declaration WHERE network_uuid=?1", [network_uuid])?;
        }
        "network_route_publish" => {
            let uuid = lc::text(request, "universe_uuid")?;
            lc::token(uuid)?;
            let ip = identifier_ip(lc::text(request, "ip")?, "ip")?;
            let via = identifier_ip(lc::text(request, "via")?, "via")?;
            let d = declared(db)?.ok_or("No network is declared on this host")?;
            if !Cidr::parse(&d.prefix, "prefix")?.contains(ip) {
                return Err(format!("{ip} is outside the declared prefix {}", d.prefix)).map_err(|e| e.into());
            }
            let placed_here: i64 = db.query_row("SELECT COUNT(*) FROM network_allocations WHERE universe_uuid=?1 AND released_at IS NULL", [uuid], |r| r.get(0))?;
            if placed_here > 0 {
                return Err("the universe is allocated on this host; a route to elsewhere would announce it twice".into());
            }
            let dst = format!("{ip}/32");
            match routes_for(&dst) {
                None => return Err("the kernel's routes could not be read; refusing on an unknown state".into()),
                Some(r) if !r.is_empty() => return Err(format!("a route for {ip} is already effective: {}; withdraw it first", r.join("; ")).into()),
                _ => {}
            }
            ip_route(&["-4", "route", "replace", &dst, "via", &via.to_string()])?;
            let held = routes_for(&dst).ok_or("routes could not be read after publication")?;
            if !held.iter().any(|l| l.contains(&format!("via {via}"))) {
                return Err(format!("the route for {ip} via {via} is not effective: {held:?}").into());
            }
            db.execute(
                "INSERT INTO network_routes VALUES(?1,?2,?3,?4,?5)",
                params![ip.to_string(), uuid, via.to_string(), now, id],
            )?;
        }
        "network_route_withdraw" => {
            let uuid = lc::text(request, "universe_uuid")?;
            lc::token(uuid)?;
            let row: Option<(String, String)> = db
                .query_row("SELECT ip,via FROM network_routes WHERE universe_uuid=?1", [uuid], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((ip, via)) = row else { return Err("No route is published here for this universe".into()) };
            let dst = format!("{ip}/32");
            if routes_for(&dst).is_some_and(|h| h.iter().any(|l| l.contains(&format!("via {via}")))) {
                ip_route(&["-4", "route", "del", &dst, "via", &via])?;
            }
            match routes_for(&dst) {
                None => return Err("routes could not be read after the withdrawal; the route's state is unknown".into()),
                Some(r) if !r.is_empty() => return Err(format!("a route for {ip} is still effective after the withdrawal: {}", r.join("; ")).into()),
                _ => {}
            }
            db.execute("DELETE FROM network_routes WHERE universe_uuid=?1", [uuid])?;
        }
        _ => return Err("Unsupported network operation".into()),
    }
    view(db)
}
