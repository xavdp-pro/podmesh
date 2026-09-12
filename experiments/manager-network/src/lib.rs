//! A bounded, mutually authenticated local TCP exchange for the durable manager lab.
//!
//! This crate maps one authenticated replica snapshot into the existing durable
//! manager `Import` request. It does not enroll peers, make control decisions,
//! publish DNS, activate a service, execute commands, or perform fencing.

use std::fmt::Write as _;
use std::{
    collections::BTreeSet,
    fmt,
    fs::File,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use hmac::{Hmac, Mac};
use podmesh_manager_ha_lab::durable::{Configuration, Request, Response, Snapshot, Store};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Maximum network frame size, including the four-byte length prefix excluded.
pub const MAX_FRAME_BYTES: usize = 512 * 1024;
/// Maximum duration for connect, read and write operations.
pub const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// Exactly one TCP connection is admitted per `serve_once` process invocation.
pub const MAX_CONNECTIONS_PER_PROCESS: usize = 1;
/// Maximum local configuration JSON size, read before parsing or opening SQLite.
pub const MAX_CONFIGURATION_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub replica_id: String,
    pub endpoint: SocketAddr,
    /// A 32-byte shared key encoded as lower-case hexadecimal.
    pub shared_key_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationFile {
    pub replica_id: String,
    pub database_path: PathBuf,
    pub manager: Configuration,
    pub bind: SocketAddr,
    pub peers: Vec<Peer>,
}

/// Reads one operator-owned configuration without an unbounded allocation.
///
/// # Errors
///
/// Returns unavailable when the file cannot be read and malformed when its size or
/// JSON shape is invalid. It never opens the durable store.
pub fn load_configuration(path: &Path) -> Result<ConfigurationFile, Error> {
    let file = File::open(path).map_err(Error::unavailable)?;
    let mut bytes = Vec::with_capacity(MAX_CONFIGURATION_BYTES + 1);
    file.take((MAX_CONFIGURATION_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(Error::unavailable)?;
    if bytes.len() > MAX_CONFIGURATION_BYTES {
        return Err(Error::malformed(
            "configuration exceeds the fixed size limit",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::malformed("invalid configuration JSON"))
}

impl ConfigurationFile {
    /// Validates complete static peer identity without opening a store or socket.
    ///
    /// # Errors
    ///
    /// Returns a refusal when topology, peer identities or pair keys are invalid.
    pub fn validate(&self) -> Result<(), Error> {
        let topology = self.manager.topology().map_err(Error::refused)?;
        topology
            .instantiate(&self.replica_id)
            .map_err(Error::refused)?;
        if self.peers.len() + 1 != topology.replica_count() {
            return Err(Error::refused(
                "every other configured manager replica must have exactly one peer entry",
            ));
        }
        let mut peer_ids = BTreeSet::new();
        let mut peer_keys = BTreeSet::new();
        for peer in &self.peers {
            topology
                .instantiate(&peer.replica_id)
                .map_err(Error::refused)?;
            if peer.replica_id == self.replica_id || !peer_ids.insert(&peer.replica_id) {
                return Err(Error::refused(
                    "peer identities must be distinct and non-local",
                ));
            }
            parse_key(&peer.shared_key_hex)?;
            if !peer_keys.insert(peer.shared_key_hex.to_ascii_lowercase()) {
                return Err(Error::refused(
                    "each local peer must use a distinct pair key",
                ));
            }
        }
        Ok(())
    }

    /// Opens the configured durable store after pure static validation.
    ///
    /// # Errors
    /// Returns a refusal on invalid static configuration or durable-store failure.
    pub fn open(&self) -> Result<Node, Error> {
        self.validate()?;
        let store = Store::open(&self.database_path, self.manager.clone(), &self.replica_id)
            .map_err(Error::refused)?;
        Ok(Node {
            replica_id: self.replica_id.clone(),
            configuration: self.manager.clone(),
            bind: self.bind,
            peers: self.peers.clone(),
            store,
        })
    }
}

/// Locally configured durable replica. Peers cannot alter this configuration.
pub struct Node {
    replica_id: String,
    configuration: Configuration,
    bind: SocketAddr,
    peers: Vec<Peer>,
    store: Store,
}

impl Node {
    /// Accepts one signed request and then exits. This is intentional for the lab.
    ///
    /// # Errors
    ///
    /// Returns an unavailable error for local bind or I/O failures. Malformed and
    /// refused requests receive a bounded signed-independent error response.
    pub fn serve_once(&mut self) -> Result<(), Error> {
        let listener = TcpListener::bind(self.bind).map_err(Error::unavailable)?;
        listener.set_nonblocking(true).map_err(Error::unavailable)?;
        let deadline = Instant::now() + IO_TIMEOUT;
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(Error::unavailable("listener accept timeout"));
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(Error::unavailable(error)),
            }
        };
        self.serve_connection(stream)
    }

    /// Handles exactly one bounded authenticated exchange on an accepted stream.
    /// The caller owns listener admission and concurrency limits.
    ///
    /// # Errors
    /// Returns a local I/O error when the bounded request/reply cannot complete.
    pub fn serve_connection(&mut self, mut stream: TcpStream) -> Result<(), Error> {
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(Error::unavailable)?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(Error::unavailable)?;
        let reply = match read_frame(&mut stream).and_then(|body| self.handle(&body)) {
            Ok(reply) => reply,
            Err(error) => WireReply::Diagnostic {
                server_replica_id: self.replica_id.clone(),
                category: error.category(),
                detail: error.detail().into(),
            },
        };
        write_frame(&mut stream, &encode(&reply)?).map_err(Error::unavailable)
    }

    /// Sends one locally exported snapshot to one exact configured peer.
    ///
    /// # Errors
    ///
    /// Returns a locally derived error for local configuration, bounded I/O, reply
    /// binding or validation failure. An honest remote refusal is returned only as
    /// an [`ErrorSource::UnauthenticatedRemoteDiagnostic`] because error replies have
    /// no MAC and must never be used as management authority.
    pub fn sync_to(
        &mut self,
        peer_id: &str,
        operation_id: &str,
        nonce: &str,
    ) -> Result<ImportResult, Error> {
        validate_token("operation ID", operation_id)?;
        validate_token("nonce", nonce)?;
        let peer = self.peer(peer_id)?.clone();
        let Response::Snapshot { snapshot } = self
            .store
            .execute(&Request::Export {})
            .map_err(Error::refused)?
        else {
            return Err(Error::refused(
                "durable manager returned an unexpected export response",
            ));
        };
        let request = WireRequest {
            protocol: "podmesh-manager-network-lab/1".into(),
            source_replica_id: self.replica_id.clone(),
            destination_replica_id: peer.replica_id.clone(),
            operation_id: operation_id.into(),
            nonce: nonce.into(),
            snapshot,
            mac_hex: String::new(),
        };
        let request = sign_request(request, &peer.shared_key_hex)?;
        let mut stream = connect(&peer.endpoint)?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(Error::unavailable)?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(Error::unavailable)?;
        write_frame(&mut stream, &encode(&request)?).map_err(Error::unavailable)?;
        let response_bytes = read_frame(&mut stream)?;
        let reply: WireReply = decode(&response_bytes)?;
        match reply {
            WireReply::Imported {
                inserted,
                history_len,
                ..
            } => {
                self.verify_reply(&peer, operation_id, nonce, &reply)?;
                Ok(ImportResult {
                    inserted,
                    history_len,
                })
            }
            WireReply::Diagnostic {
                server_replica_id,
                category,
                detail,
            } if server_replica_id == peer.replica_id => {
                Err(Error::remote_diagnostic(category, detail))
            }
            WireReply::Diagnostic { .. } => Err(Error::refused(
                "diagnostic claims a different server identity",
            )),
        }
    }

    fn handle(&mut self, bytes: &[u8]) -> Result<WireReply, Error> {
        let request: WireRequest = decode(bytes)?;
        if request.protocol != "podmesh-manager-network-lab/1" {
            return Err(Error::malformed("unsupported protocol"));
        }
        if request.destination_replica_id != self.replica_id {
            return Err(Error::refused(
                "request is addressed to a different replica",
            ));
        }
        validate_token("operation ID", &request.operation_id)?;
        validate_token("nonce", &request.nonce)?;
        let peer = self.peer(&request.source_replica_id)?.clone();
        verify_request(&request, &peer.shared_key_hex)?;
        if request.snapshot.replica_id != request.source_replica_id {
            return Err(Error::refused(
                "snapshot identity does not match authenticated peer",
            ));
        }
        if request.snapshot.configuration != self.configuration {
            return Err(Error::refused(
                "snapshot topology does not match local configuration",
            ));
        }
        let result = self
            .store
            .execute(&Request::Import {
                operation_id: format!(
                    "network/{}/{}",
                    request.source_replica_id, request.operation_id
                ),
                snapshot: request.snapshot,
            })
            .map_err(Error::refused)?;
        let Response::Imported {
            inserted,
            history_len,
        } = result
        else {
            return Err(Error::refused(
                "durable manager returned an unexpected import response",
            ));
        };
        sign_reply(
            WireReply::Imported {
                source_replica_id: self.replica_id.clone(),
                destination_replica_id: peer.replica_id.clone(),
                operation_id: request.operation_id,
                nonce: request.nonce,
                inserted,
                history_len,
                mac_hex: String::new(),
            },
            &peer.shared_key_hex,
        )
    }

    fn verify_reply(
        &self,
        peer: &Peer,
        operation_id: &str,
        nonce: &str,
        reply: &WireReply,
    ) -> Result<(), Error> {
        match reply {
            WireReply::Imported {
                source_replica_id,
                destination_replica_id,
                operation_id: actual_operation,
                nonce: actual_nonce,
                ..
            } => {
                if source_replica_id != &peer.replica_id
                    || destination_replica_id != &self.replica_id
                    || actual_operation != operation_id
                    || actual_nonce != nonce
                {
                    return Err(Error::refused("reply identity or replay binding mismatch"));
                }
                verify_reply(reply, &peer.shared_key_hex)
            }
            WireReply::Diagnostic { .. } => Err(Error::refused(
                "diagnostic cannot authenticate a successful import reply",
            )),
        }
    }

    fn peer(&self, id: &str) -> Result<&Peer, Error> {
        self.peers
            .iter()
            .find(|peer| peer.replica_id == id)
            .ok_or_else(|| Error::refused("peer is not configured"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImportResult {
    pub inserted: usize,
    pub history_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum WireReply {
    Imported {
        source_replica_id: String,
        destination_replica_id: String,
        operation_id: String,
        nonce: String,
        inserted: usize,
        history_len: usize,
        mac_hex: String,
    },
    Diagnostic {
        server_replica_id: String,
        category: ErrorCategory,
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    protocol: String,
    source_replica_id: String,
    destination_replica_id: String,
    operation_id: String,
    nonce: String,
    snapshot: Snapshot,
    mac_hex: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Unavailable,
    Refused,
    Malformed,
}

/// Explains whether an [`Error`] was computed locally or conveyed by an unsigned peer diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorSource {
    /// The local process derived this outcome from configuration, I/O or verified data.
    Local,
    /// A configured endpoint sent an unsigned diagnostic; it is not authority or proof.
    UnauthenticatedRemoteDiagnostic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    category: ErrorCategory,
    source: ErrorSource,
    detail: String,
}

impl Error {
    fn unavailable(value: impl fmt::Display) -> Self {
        Self::new(ErrorCategory::Unavailable, value)
    }
    fn refused(value: impl fmt::Display) -> Self {
        Self::new(ErrorCategory::Refused, value)
    }
    fn malformed(value: impl fmt::Display) -> Self {
        Self::new(ErrorCategory::Malformed, value)
    }
    fn new(category: ErrorCategory, value: impl fmt::Display) -> Self {
        Self {
            category,
            source: ErrorSource::Local,
            detail: value.to_string(),
        }
    }
    fn remote_diagnostic(category: ErrorCategory, detail: String) -> Self {
        Self {
            category,
            source: ErrorSource::UnauthenticatedRemoteDiagnostic,
            detail,
        }
    }
    /// Returns the bounded outcome category.
    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        self.category
    }
    /// Returns whether this error is local or an unsigned remote diagnostic.
    #[must_use]
    pub fn source(&self) -> ErrorSource {
        self.source
    }
    fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.detail)
    }
}

impl std::error::Error for Error {}

fn encode(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(value).map_err(Error::malformed)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(Error::malformed("message exceeds the fixed frame limit"));
    }
    Ok(bytes)
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::malformed("invalid network message"))
}

fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, Error> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut length = [0_u8; 4];
    read_exact(stream, &mut length, deadline)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(Error::malformed("invalid frame length"));
    }
    let mut bytes = vec![0; length];
    read_exact(stream, &mut bytes, deadline)?;
    Ok(bytes)
}

fn read_exact(
    stream: &mut TcpStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), Error> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| Error::unavailable("frame read deadline exceeded"))?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(Error::unavailable)?;
        match stream.read(bytes) {
            Ok(0) => return Err(Error::malformed("truncated frame")),
            Ok(length) => bytes = &mut bytes[length..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Error::unavailable(error)),
        }
    }
    Ok(())
}

fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    let length: u32 = bytes
        .len()
        .try_into()
        .map_err(|_| std::io::Error::other("frame limit"))?;
    let deadline = Instant::now() + IO_TIMEOUT;
    for mut part in [&length.to_be_bytes()[..], bytes] {
        while !part.is_empty() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "frame write deadline exceeded",
                    )
                })?;
            stream.set_write_timeout(Some(remaining))?;
            let written = match stream.write(part) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if written == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            part = &part[written..];
        }
    }
    stream.flush()
}

fn connect(endpoint: &SocketAddr) -> Result<TcpStream, Error> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let last_error = loop {
        match TcpStream::connect_timeout(endpoint, Duration::from_millis(100)) {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() >= deadline => break error,
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    };
    Err(Error::unavailable(last_error))
}

fn validate_token(kind: &str, value: &str) -> Result<(), Error> {
    let valid = (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(Error::malformed(format!(
            "{kind} must be 1-128 ASCII token characters"
        )))
    }
}

fn parse_key(value: &str) -> Result<Vec<u8>, Error> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::refused(
            "peer key must be a 32-byte hexadecimal value",
        ));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).map_err(Error::refused))
        .collect()
}

fn mac(key_hex: &str, value: &impl Serialize) -> Result<String, Error> {
    let key = parse_key(key_hex)?;
    let bytes = serde_json::to_vec(value).map_err(Error::malformed)?;
    let mut mac = HmacSha256::new_from_slice(&key).map_err(Error::refused)?;
    mac.update(b"podmesh-manager-network-lab/1\\0");
    mac.update(&bytes);
    Ok(hex(&mac.finalize().into_bytes()))
}

fn sign_request(mut request: WireRequest, key: &str) -> Result<WireRequest, Error> {
    request.mac_hex = mac(
        key,
        &(
            &request.protocol,
            &request.source_replica_id,
            &request.destination_replica_id,
            &request.operation_id,
            &request.nonce,
            &request.snapshot,
        ),
    )?;
    Ok(request)
}

fn verify_request(request: &WireRequest, key: &str) -> Result<(), Error> {
    let expected = mac(
        key,
        &(
            &request.protocol,
            &request.source_replica_id,
            &request.destination_replica_id,
            &request.operation_id,
            &request.nonce,
            &request.snapshot,
        ),
    )?;
    if constant_time_eq(expected.as_bytes(), request.mac_hex.as_bytes()) {
        Ok(())
    } else {
        Err(Error::refused("request authentication failed"))
    }
}

fn sign_reply(mut reply: WireReply, key: &str) -> Result<WireReply, Error> {
    let WireReply::Imported {
        source_replica_id,
        destination_replica_id,
        operation_id,
        nonce,
        inserted,
        history_len,
        mac_hex,
    } = &mut reply
    else {
        return Ok(reply);
    };
    *mac_hex = mac(
        key,
        &(
            source_replica_id,
            destination_replica_id,
            operation_id,
            nonce,
            *inserted,
            *history_len,
        ),
    )?;
    Ok(reply)
}

fn verify_reply(reply: &WireReply, key: &str) -> Result<(), Error> {
    let WireReply::Imported {
        source_replica_id,
        destination_replica_id,
        operation_id,
        nonce,
        inserted,
        history_len,
        mac_hex,
    } = reply
    else {
        return Ok(());
    };
    let expected = mac(
        key,
        &(
            source_replica_id,
            destination_replica_id,
            operation_id,
            nonce,
            *inserted,
            *history_len,
        ),
    )?;
    if constant_time_eq(expected.as_bytes(), mac_hex.as_bytes()) {
        Ok(())
    } else {
        Err(Error::refused("reply authentication failed"))
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("writing to a string cannot fail");
    }
    result
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        net::{Ipv4Addr, Shutdown},
        thread,
        time::Duration,
    };

    use podmesh_manager_ha_lab::{
        durable::{Request, Response},
        ReplicaConfig, ScopeGrant,
    };
    use tempfile::TempDir;

    use super::*;

    const WRONG_KEY: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

    #[test]
    fn pure_configuration_validation_opens_no_store_or_listener() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = unused_addresses();
        let mut config = configuration(&directory, "r1", &addresses);
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        config.bind = occupied.local_addr().unwrap();
        config.validate().unwrap();
        assert!(!config.database_path.exists());
        fs::write(&config.database_path, b"not SQLite").unwrap();
        config.validate().unwrap();
        assert_eq!(fs::read(&config.database_path).unwrap(), b"not SQLite");
        config.peers[0].shared_key_hex = "invalid".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn accepted_connection_seam_preserves_authentication_and_one_frame_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = unused_addresses();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let destination_address = listener.local_addr().unwrap();
        let mut destination = configuration(&directory, "r2", &addresses);
        destination.bind = destination_address;
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            destination.open().unwrap().serve_connection(stream)
        });
        let mut source_configuration = configuration(&directory, "r1", &addresses);
        source_configuration
            .peers
            .iter_mut()
            .find(|peer| peer.replica_id == "r2")
            .unwrap()
            .endpoint = destination_address;
        let mut source = source_configuration.open().unwrap();
        let receipt = source
            .sync_to("r2", "accepted-stream", "accepted-nonce")
            .unwrap();
        assert_eq!(receipt.history_len, 0);
        assert!(worker.join().unwrap().is_ok());
    }

    #[test]
    fn frame_deadline_is_absolute_despite_trickling_bytes() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let start = Instant::now();
            let result = read_frame(&mut stream);
            (result, start.elapsed())
        });
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(&100_u32.to_be_bytes()).unwrap();
        for _ in 0..24 {
            if stream.write_all(b" ").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let (result, elapsed) = worker.join().unwrap();
        assert!(result.is_err());
        assert!(elapsed < Duration::from_secs(3));
    }

    fn key_for(left: usize, right: usize) -> String {
        match (left.min(right), left.max(right)) {
            (1, 2) => "11".repeat(32),
            (1, 3) => "22".repeat(32),
            (2, 3) => "33".repeat(32),
            _ => unreachable!(),
        }
    }

    fn unused_addresses() -> [SocketAddr; 3] {
        let listeners: Vec<_> = (0..3)
            .map(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap())
            .collect();
        let addresses: Vec<_> = listeners
            .iter()
            .map(|listener| listener.local_addr().unwrap())
            .collect();
        drop(listeners);
        addresses.try_into().unwrap()
    }

    fn manager() -> Configuration {
        Configuration {
            logical_manager_id: "network-manager".into(),
            replicas: (1..=3)
                .map(|number| ReplicaConfig {
                    replica_id: format!("r{number}"),
                    host_id: format!("h{number}"),
                })
                .collect(),
            grants: (1..=3)
                .map(|number| ScopeGrant {
                    scope: format!("scope{number}"),
                    owner_replica_id: format!("r{number}"),
                })
                .collect(),
        }
    }

    fn configuration(
        directory: &TempDir,
        replica_id: &str,
        addresses: &[SocketAddr],
    ) -> ConfigurationFile {
        let replica_index: usize = replica_id[1..].parse().unwrap();
        ConfigurationFile {
            replica_id: replica_id.into(),
            database_path: directory.path().join(format!("{replica_id}.sqlite")),
            manager: manager(),
            bind: addresses[replica_index - 1],
            peers: (1..=3)
                .filter(|index| *index != replica_index)
                .map(|index| Peer {
                    replica_id: format!("r{index}"),
                    endpoint: addresses[index - 1],
                    shared_key_hex: key_for(replica_index, index),
                })
                .collect(),
        }
    }

    fn spawn_server(configuration: ConfigurationFile) -> thread::JoinHandle<Result<(), Error>> {
        thread::spawn(move || match configuration.open() {
            Ok(mut node) => node.serve_once(),
            Err(error) => panic!("server configuration rejected: {error}"),
        })
    }

    fn wait_for_listener() {
        thread::sleep(Duration::from_millis(40));
    }

    fn observe(configuration: &ConfigurationFile, operation_id: &str) {
        let mut node = configuration.open().unwrap();
        node.store
            .execute(&Request::Observe {
                operation_id: operation_id.into(),
                scope: "scope1".into(),
                subject: "universe".into(),
                exclusive_resource: None,
                active_claim: false,
                value: "observed".into(),
            })
            .unwrap();
    }

    fn history_len(configuration: &ConfigurationFile) -> usize {
        let mut node = configuration.open().unwrap();
        match node.store.execute(&Request::Inspect {}).unwrap() {
            Response::Inspection { history_len, .. } => history_len,
            _ => unreachable!(),
        }
    }

    #[test]
    fn exact_configured_peer_replay_and_stale_catch_up_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = unused_addresses();
        let r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        let r3 = configuration(&directory, "r3", &addresses);
        observe(&r1, "origin-observe");

        let r2_server = spawn_server(r2.clone());
        let r3_server = spawn_server(r3.clone());
        wait_for_listener();
        let sync = r1
            .open()
            .unwrap()
            .sync_to("r2", "op-one", "nonce-one")
            .unwrap();
        assert_eq!(sync.inserted, 1);
        assert_eq!(history_len(&r2), 1);
        assert!(r2_server.join().unwrap().is_ok());

        let sync = r1
            .open()
            .unwrap()
            .sync_to("r3", "op-two", "nonce-two")
            .unwrap();
        assert_eq!(sync.inserted, 1);
        assert!(r3_server.join().unwrap().is_ok());
        assert_eq!(history_len(&r3), 1);

        let replay_server = spawn_server(r2.clone());
        wait_for_listener();
        let replay = r1
            .open()
            .unwrap()
            .sync_to("r2", "op-one", "nonce-one")
            .unwrap();
        assert_eq!(replay.inserted, 1);
        assert!(replay_server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 1);
    }

    #[test]
    fn wrong_peer_malformed_and_oversize_requests_do_not_mutate() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = unused_addresses();
        let r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        observe(&r1, "origin-observe");
        let Response::Snapshot { snapshot } = r1
            .open()
            .unwrap()
            .store
            .execute(&Request::Export {})
            .unwrap()
        else {
            unreachable!();
        };

        let wrong = sign_request(
            WireRequest {
                protocol: "podmesh-manager-network-lab/1".into(),
                source_replica_id: "r1".into(),
                destination_replica_id: "r2".into(),
                operation_id: "wrong-peer".into(),
                nonce: "wrong-nonce".into(),
                snapshot: snapshot.clone(),
                mac_hex: String::new(),
            },
            WRONG_KEY,
        )
        .unwrap();
        let server = spawn_server(r2.clone());
        wait_for_listener();
        let mut stream = connect(&r2.bind).unwrap();
        write_frame(&mut stream, &encode(&wrong).unwrap()).unwrap();
        let reply: WireReply = decode(&read_frame(&mut stream).unwrap()).unwrap();
        assert!(matches!(
            reply,
            WireReply::Diagnostic {
                category: ErrorCategory::Refused,
                ..
            }
        ));
        assert!(server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 0);

        let server = spawn_server(r2.clone());
        wait_for_listener();
        let mut stream = connect(&r2.bind).unwrap();
        stream.write_all(&8_u32.to_be_bytes()).unwrap();
        stream.write_all(b"bad").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let reply: WireReply = decode(&read_frame(&mut stream).unwrap()).unwrap();
        assert!(matches!(
            reply,
            WireReply::Diagnostic {
                category: ErrorCategory::Malformed,
                ..
            }
        ));
        assert!(server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 0);

        let server = spawn_server(r2.clone());
        wait_for_listener();
        let mut stream = connect(&r2.bind).unwrap();
        stream
            .write_all(&(u32::try_from(MAX_FRAME_BYTES).unwrap() + 1).to_be_bytes())
            .unwrap();
        let reply: WireReply = decode(&read_frame(&mut stream).unwrap()).unwrap();
        assert!(matches!(
            reply,
            WireReply::Diagnostic {
                category: ErrorCategory::Malformed,
                ..
            }
        ));
        assert!(server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 0);
    }

    #[test]
    fn invalid_signed_snapshot_is_refused_before_any_durable_import() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = unused_addresses();
        let r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        observe(&r1, "origin-observe");
        let Response::Snapshot { mut snapshot } = r1
            .open()
            .unwrap()
            .store
            .execute(&Request::Export {})
            .unwrap()
        else {
            unreachable!();
        };
        let mut invalid = snapshot.facts[0].clone();
        invalid.event_id = "not-the-fact-id".into();
        snapshot.facts.push(invalid);
        let request = sign_request(
            WireRequest {
                protocol: "podmesh-manager-network-lab/1".into(),
                source_replica_id: "r1".into(),
                destination_replica_id: "r2".into(),
                operation_id: "atomic-import".into(),
                nonce: "atomic-nonce".into(),
                snapshot,
                mac_hex: String::new(),
            },
            &key_for(1, 2),
        )
        .unwrap();
        let server = spawn_server(r2.clone());
        wait_for_listener();
        let mut stream = connect(&r2.bind).unwrap();
        write_frame(&mut stream, &encode(&request).unwrap()).unwrap();
        let reply: WireReply = decode(&read_frame(&mut stream).unwrap()).unwrap();
        assert!(matches!(
            reply,
            WireReply::Diagnostic {
                category: ErrorCategory::Refused,
                ..
            }
        ));
        assert!(server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 0);
    }

    #[test]
    fn unavailable_peer_is_distinct_from_refusal() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = unused_addresses();
        let r1 = configuration(&directory, "r1", &addresses);
        let error = r1
            .open()
            .unwrap()
            .sync_to("r2", "offline", "offline-nonce")
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Unavailable);
        assert_eq!(error.source(), ErrorSource::Local);
    }

    #[test]
    fn sync_to_exposes_an_honest_refusal_only_as_remote_diagnostic() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = unused_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        r1.peers
            .iter_mut()
            .find(|peer| peer.replica_id == "r2")
            .unwrap()
            .shared_key_hex = WRONG_KEY.into();
        let server = spawn_server(r2);
        wait_for_listener();
        let error = r1
            .open()
            .unwrap()
            .sync_to("r2", "wrong-key-sync", "wrong-key-nonce")
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Refused, "{error}");
        assert_eq!(
            error.source(),
            ErrorSource::UnauthenticatedRemoteDiagnostic,
            "{error}"
        );
        assert!(server.join().unwrap().is_ok());
    }

    #[test]
    fn duplicate_pair_keys_are_refused_before_opening_the_store() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = unused_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        r1.peers[1].shared_key_hex = r1.peers[0].shared_key_hex.clone();
        assert!(r1.open().is_err());
        assert!(!r1.database_path.exists());
    }

    #[test]
    fn oversize_configuration_is_refused_before_any_durable_store_open() {
        let directory = tempfile::tempdir().unwrap();
        let configuration_path = directory.path().join("oversize.json");
        fs::write(&configuration_path, vec![b'x'; MAX_CONFIGURATION_BYTES + 1]).unwrap();
        let expected_database = directory.path().join("must-not-exist.sqlite");
        assert!(load_configuration(&configuration_path).is_err());
        assert!(!expected_database.exists());
    }
}
