//! Which engine carries the journal, and how it is reached.
//!
//! The node's configuration today is a directory: `state.sqlite` is opened under it and that is
//! the whole surface. The storage plan replaces that with a store profile -- an engine, and what
//! that engine needs -- so that a host may carry the node's journal in its own MariaDB instance
//! without any caller learning a new path. [`StoreConfig`] is that profile, read from the same
//! JSON the node already speaks.
//!
//! ```text
//! "store": {
//!   "engine": "sqlite",                       // or "mariadb"
//!   "sqlite": { "path": "<state dir>/state.sqlite", "busy_timeout_ms": 5000, "journal_mode": "wal" },
//!   "mariadb": { "host": "127.0.0.1", "port": 3306, "user": "podmesh-node",
//!                "database": "podmesh-node", "password_file": "/apps/podmesh-node/etc/mysql/localhost/passwd",
//!                "connect_timeout_ms": 5000, "lock_wait_timeout_seconds": 10 }
//! }
//! ```
//!
//! **A password is never a configuration value.** MariaDB's password is read from a file the
//! configuration names, root-only as the node's other secrets are, and it is never printed:
//! [`MariadbConfig`] has a hand-written `Debug` that redacts it, and a DSN that carries one is
//! redacted the same way. The defaults are the node role of the Phase 0 record -- database and
//! user `podmesh-node` -- and loopback, because a store belongs to the host whose journal it is;
//! any other address is named explicitly, never defaulted here.
use super::{Fault, Result};
use serde_json::Value as Json;
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// The file a node's journal has always been, under its state directory.
pub const DEFAULT_STATE_FILE: &str = "state.sqlite";
/// How long a statement waits for another writer before the store reports [`Fault::Busy`].
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_millis(5_000);
pub const DEFAULT_MARIADB_PORT: u16 = 3306;
/// The node role's functional slug: system user, database user and database carry one name.
pub const DEFAULT_NODE_SLUG: &str = "podmesh-node";
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(5_000);
/// How long InnoDB waits for a row lock before it reports 1205, which is [`Fault::Busy`] here.
pub const DEFAULT_LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// The DSN a laboratory spike or a test may hand to the MariaDB backend, instead of a profile.
pub const DSN_ENVIRONMENT: &str = "PODMESH_MARIADB_DSN";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Engine {
    Sqlite,
    Mariadb,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Sqlite => "sqlite",
            Engine::Mariadb => "mariadb",
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "sqlite" => Ok(Engine::Sqlite),
            "mariadb" | "mysql" => Ok(Engine::Mariadb),
            other => Err(Fault::Unsupported.error(format!("store.engine is sqlite or mariadb, not {other}"))),
        }
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct SqliteConfig {
    /// The file itself, not its directory: `<state dir>/state.sqlite` for a node.
    pub path: PathBuf,
    pub busy_timeout: Duration,
    /// Write-ahead logging, as every node journal has used since the first version.
    pub journal_wal: bool,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self { path: PathBuf::from(DEFAULT_STATE_FILE), busy_timeout: DEFAULT_BUSY_TIMEOUT, journal_wal: true }
    }
}

#[derive(Clone)]
pub struct MariadbConfig {
    /// A complete `mysql://` URL, which replaces the fields below when it is set.
    pub dsn: Option<String>,
    pub host: String,
    pub port: u16,
    /// A Unix socket, preferred over the address when the store runs on this host.
    pub socket: Option<PathBuf>,
    pub user: String,
    /// The file that carries the password, root-only; never the password itself.
    pub password_file: Option<PathBuf>,
    pub database: String,
    pub connect_timeout: Duration,
    pub lock_wait_timeout: Duration,
}

impl Default for MariadbConfig {
    fn default() -> Self {
        Self {
            dsn: None,
            host: "127.0.0.1".to_string(),
            port: DEFAULT_MARIADB_PORT,
            socket: None,
            user: DEFAULT_NODE_SLUG.to_string(),
            password_file: None,
            database: DEFAULT_NODE_SLUG.to_string(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            lock_wait_timeout: DEFAULT_LOCK_WAIT_TIMEOUT,
        }
    }
}

impl MariadbConfig {
    /// The profile a DSN alone describes. Laboratory spikes and tests use this; a packaged node
    /// uses a profile with a password file.
    pub fn from_dsn(dsn: impl Into<String>) -> Self {
        Self { dsn: Some(dsn.into()), ..Self::default() }
    }

    /// The DSN the environment names, for a laboratory spike or a test; none otherwise.
    pub fn from_environment() -> Option<Self> {
        match std::env::var(DSN_ENVIRONMENT) {
            Ok(dsn) if !dsn.trim().is_empty() => Some(Self::from_dsn(dsn.trim())),
            _ => None,
        }
    }

    /// The password the profile points at, read now and never kept in the profile.
    ///
    /// A password file readable by anyone but its owner is refused rather than used: this store
    /// is the node's journal, and its credentials follow the same rule as its secrets.
    pub fn password(&self) -> Result<Option<String>> {
        let Some(path) = &self.password_file else { return Ok(None) };
        let metadata = fs::metadata(path)
            .map_err(|e| Fault::Denied.error(format!("the password file {} could not be read: {e}", path.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(Fault::Denied.error(format!(
                    "the password file {} is readable beyond its owner (mode {mode:o}); make it 0600",
                    path.display()
                )));
            }
        }
        let password = fs::read_to_string(path)
            .map_err(|e| Fault::Denied.error(format!("the password file {} could not be read: {e}", path.display())))?;
        Ok(Some(password.trim_end_matches(['\n', '\r']).to_string()))
    }

    /// The URL the client connects with: the DSN when the profile carries one, otherwise the
    /// fields with the password read from its file. Never logged; see [`redact`].
    pub fn url(&self) -> Result<String> {
        if let Some(dsn) = &self.dsn {
            return Ok(dsn.clone());
        }
        let password = self.password()?.unwrap_or_default();
        let credentials = if password.is_empty() {
            encode(&self.user)
        } else {
            format!("{}:{}", encode(&self.user), encode(&password))
        };
        let mut url = format!(
            "mysql://{credentials}@{host}:{port}/{database}",
            host = self.host,
            port = self.port,
            database = encode(&self.database)
        );
        if let Some(socket) = &self.socket {
            url.push_str(&format!("?socket={}", encode(&socket.to_string_lossy())));
        }
        Ok(url)
    }

    /// What this profile may be written down as: everything but the password.
    pub fn described(&self) -> String {
        match &self.dsn {
            Some(dsn) => redact(dsn),
            None => format!("mysql://{}@{}:{}/{}", self.user, self.host, self.port, self.database),
        }
    }
}

impl fmt::Debug for MariadbConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MariadbConfig")
            .field("target", &self.described())
            .field("socket", &self.socket)
            .field("password_file", &self.password_file)
            .field("connect_timeout", &self.connect_timeout)
            .field("lock_wait_timeout", &self.lock_wait_timeout)
            .finish()
    }
}

/// A `mysql://` URL with whatever password it carried replaced, for a message or a log line.
pub fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let Some((credentials, target)) = rest.split_once('@') else { return url.to_string() };
    match credentials.split_once(':') {
        Some((user, _)) => format!("{scheme}://{user}:***@{target}"),
        None => url.to_string(),
    }
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Which engine carries this node's journal, and what that engine needs to be reached.
#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub engine: Engine,
    pub sqlite: SqliteConfig,
    pub mariadb: MariadbConfig,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self { engine: Engine::Sqlite, sqlite: SqliteConfig::default(), mariadb: MariadbConfig::default() }
    }
}

impl StoreConfig {
    /// The profile a node has today: SQLite, in `state.sqlite` under its state directory.
    pub fn for_state_dir(dir: &Path) -> Self {
        Self {
            sqlite: SqliteConfig { path: dir.join(DEFAULT_STATE_FILE), ..SqliteConfig::default() },
            ..Self::default()
        }
    }

    /// Read a profile from the node's JSON, either the whole document carrying a `store` object
    /// or that object alone. Everything absent keeps the default of [`StoreConfig::for_state_dir`]
    /// when a state directory is given, so an existing configuration file stays valid unchanged.
    pub fn from_json(document: &Json, state_dir: Option<&Path>) -> Result<Self> {
        let mut config = match state_dir {
            Some(dir) => Self::for_state_dir(dir),
            None => Self::default(),
        };
        let store = match document.get("store") {
            Some(store) => store,
            None if document.is_object() => document,
            None => return Ok(config),
        };
        if store.is_null() {
            return Ok(config);
        }
        if !store.is_object() {
            return Err(Fault::Type.error("store is an object"));
        }
        if let Some(engine) = string(store, "engine")? {
            config.engine = Engine::parse(&engine)?;
        }
        if let Some(sqlite) = member(store, "sqlite")? {
            if let Some(path) = string(sqlite, "path")? {
                config.sqlite.path = PathBuf::from(path);
            }
            if let Some(ms) = integer(sqlite, "busy_timeout_ms")? {
                config.sqlite.busy_timeout = Duration::from_millis(u64::try_from(ms).map_err(|_| Fault::Type.error("store.sqlite.busy_timeout_ms is not negative"))?);
            }
            if let Some(mode) = string(sqlite, "journal_mode")? {
                config.sqlite.journal_wal = match mode.to_ascii_lowercase().as_str() {
                    "wal" => true,
                    "default" => false,
                    other => return Err(Fault::Unsupported.error(format!("store.sqlite.journal_mode is wal or default, not {other}"))),
                };
            }
        }
        if let Some(mariadb) = member(store, "mariadb")? {
            if let Some(dsn) = string(mariadb, "dsn")? {
                config.mariadb.dsn = Some(dsn);
            }
            if let Some(host) = string(mariadb, "host")? {
                config.mariadb.host = host;
            }
            if let Some(port) = integer(mariadb, "port")? {
                config.mariadb.port = u16::try_from(port).map_err(|_| Fault::Type.error("store.mariadb.port is a TCP port"))?;
            }
            if let Some(socket) = string(mariadb, "socket")? {
                config.mariadb.socket = Some(PathBuf::from(socket));
            }
            if let Some(user) = string(mariadb, "user")? {
                config.mariadb.user = user;
            }
            if let Some(file) = string(mariadb, "password_file")? {
                config.mariadb.password_file = Some(PathBuf::from(file));
            }
            if mariadb.get("password").is_some() {
                return Err(Fault::Denied.error("store.mariadb.password is never a configuration value: name a password_file"));
            }
            if let Some(database) = string(mariadb, "database")? {
                config.mariadb.database = database;
            }
            if let Some(ms) = integer(mariadb, "connect_timeout_ms")? {
                config.mariadb.connect_timeout = Duration::from_millis(u64::try_from(ms).map_err(|_| Fault::Type.error("store.mariadb.connect_timeout_ms is not negative"))?);
            }
            if let Some(seconds) = integer(mariadb, "lock_wait_timeout_seconds")? {
                config.mariadb.lock_wait_timeout = Duration::from_secs(u64::try_from(seconds).map_err(|_| Fault::Type.error("store.mariadb.lock_wait_timeout_seconds is not negative"))?);
            }
        }
        Ok(config)
    }

    /// What this profile is, in one line, for an observation or a refusal. Never a password.
    pub fn described(&self) -> String {
        match self.engine {
            Engine::Sqlite => format!("sqlite:{}", self.sqlite.path.display()),
            Engine::Mariadb => self.mariadb.described(),
        }
    }
}

fn member<'a>(value: &'a Json, key: &str) -> Result<Option<&'a Json>> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(member) if member.is_object() => Ok(Some(member)),
        Some(_) => Err(Fault::Type.error(format!("store.{key} is an object"))),
    }
}

fn string(value: &Json, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(Fault::Type.error(format!("{key} is a string"))),
    }
}

fn integer(value: &Json, key: &str) -> Result<Option<i64>> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Number(number)) => number.as_i64().map(Some).ok_or_else(|| Fault::Type.error(format!("{key} is a whole number"))),
        Some(_) => Err(Fault::Type.error(format!("{key} is a whole number"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("podmesh-store-config-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_node_with_no_store_in_its_configuration_keeps_the_journal_it_has() {
        let dir = Path::new("/var/lib/podmesh");
        for document in [json!({}), json!({"store": null}), json!({"state_dir": "elsewhere"})] {
            let config = StoreConfig::from_json(&document, Some(dir)).unwrap();
            assert_eq!(config.engine, Engine::Sqlite);
            assert_eq!(config.sqlite.path, dir.join("state.sqlite"));
            assert!(config.sqlite.journal_wal);
            assert_eq!(config.described(), "sqlite:/var/lib/podmesh/state.sqlite");
        }
    }

    #[test]
    fn a_store_profile_is_read_whole_and_an_unknown_engine_refused() {
        let document = json!({"store": {
            "engine": "mariadb",
            "sqlite": {"path": "/var/lib/podmesh/state.sqlite", "busy_timeout_ms": 250, "journal_mode": "default"},
            "mariadb": {"host": "192.0.2.10", "port": 3307, "user": "podmesh-node", "database": "podmesh-node",
                        "password_file": "/apps/podmesh-node/etc/mysql/localhost/passwd",
                        "connect_timeout_ms": 1500, "lock_wait_timeout_seconds": 3}
        }});
        let config = StoreConfig::from_json(&document, None).unwrap();
        assert_eq!(config.engine, Engine::Mariadb);
        assert_eq!(config.sqlite.busy_timeout, Duration::from_millis(250));
        assert!(!config.sqlite.journal_wal);
        assert_eq!(config.mariadb.port, 3307);
        assert_eq!(config.mariadb.lock_wait_timeout, Duration::from_secs(3));
        assert_eq!(config.described(), "mysql://podmesh-node@192.0.2.10:3307/podmesh-node");

        let refused = StoreConfig::from_json(&json!({"store": {"engine": "postgres"}}), None).unwrap_err();
        assert_eq!(refused.fault, Fault::Unsupported);
        assert_eq!(StoreConfig::from_json(&json!({"store": {"engine": 1}}), None).unwrap_err().fault, Fault::Type);
        assert_eq!(StoreConfig::from_json(&json!({"store": []}), None).unwrap_err().fault, Fault::Type);
    }

    #[test]
    fn a_password_in_the_configuration_is_refused_outright() {
        let document = json!({"store": {"engine": "mariadb", "mariadb": {"password": "written down"}}});
        let refused = StoreConfig::from_json(&document, None).unwrap_err();
        assert_eq!(refused.fault, Fault::Denied);
        assert!(refused.message.contains("password_file"));
    }

    #[test]
    fn a_password_file_is_read_only_when_its_owner_alone_can_read_it() {
        let dir = scratch("password");
        let path = dir.join("passwd");
        fs::write(&path, "kept in the file\n").unwrap();
        let config = MariadbConfig { password_file: Some(path.clone()), ..MariadbConfig::default() };

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            let refused = config.password().unwrap_err();
            assert_eq!(refused.fault, Fault::Denied);
            assert!(refused.message.contains("0600"));
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(config.password().unwrap().as_deref(), Some("kept in the file"));

        // And what it builds carries the password, while what it says about itself does not.
        let url = config.url().unwrap();
        assert!(url.contains("kept%20in%20the%20file"));
        assert_eq!(redact(&url), "mysql://podmesh-node:***@127.0.0.1:3306/podmesh-node");
        assert!(!config.described().contains("kept"));
        assert!(!format!("{config:?}").contains("kept"));

        let absent = MariadbConfig::default();
        assert_eq!(absent.password().unwrap(), None);
        assert_eq!(absent.url().unwrap(), "mysql://podmesh-node@127.0.0.1:3306/podmesh-node");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_dsn_replaces_the_fields_and_is_never_printed_whole() {
        let config = MariadbConfig::from_dsn("mysql://podmesh-node:held@127.0.0.1:33061/podmesh-node");
        assert_eq!(config.url().unwrap(), "mysql://podmesh-node:held@127.0.0.1:33061/podmesh-node");
        assert_eq!(config.described(), "mysql://podmesh-node:***@127.0.0.1:33061/podmesh-node");
        assert_eq!(redact("not a url"), "not a url");
    }
}