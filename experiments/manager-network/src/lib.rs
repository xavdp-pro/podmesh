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
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hmac::{Hmac, Mac};
use podmesh_manager_ha_lab::durable::{
    authenticated_import_receipt_id, AuditDirection, AuditErrorCategory, AuditOutcome, AuditPhase,
    Configuration, DurableError, ExchangeAuditEvent, RefusalReason, Request, Response, Snapshot,
    Store,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;
static NEXT_PREAUTH_NONCE: AtomicU64 = AtomicU64::new(1);

/// Maximum network frame body size; the four-byte length prefix is excluded.
pub const MAX_FRAME_BYTES: usize = 512 * 1024;
/// Maximum duration for connect, read and write operations.
pub const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// Maximum connections admitted while seeking one authenticated request.
pub const MAX_CONNECTIONS_PER_PROCESS: usize = 8;
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
            .map_err(Error::durable)?;
        Ok(Node {
            replica_id: self.replica_id.clone(),
            configuration: self.manager.clone(),
            bind: self.bind,
            peers: self.peers.clone(),
            store,
            audit_failure_phase: None,
            #[cfg(test)]
            post_auth_failure: None,
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
    audit_failure_phase: Option<AuditPhase>,
    #[cfg(test)]
    post_auth_failure: Option<PostAuthFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostAuthFailure {
    MissingReceipt,
    SignReply,
    EncodeReply,
}

#[allow(clippy::too_many_arguments)]
fn audit_event(
    attempt_id: &str,
    wire_nonce: &str,
    direction: AuditDirection,
    phase: AuditPhase,
    peer_claim: Option<&str>,
    authenticated_peer_id: Option<&str>,
    operation_id: Option<&str>,
    outcome: AuditOutcome,
) -> ExchangeAuditEvent {
    let (error_category, reason_code) = match outcome {
        AuditOutcome::AuthenticatedRefusal => (
            Some(AuditErrorCategory::Refused),
            Some(RefusalReason::InvalidRequest),
        ),
        AuditOutcome::Unavailable => (
            Some(AuditErrorCategory::Unavailable),
            Some(RefusalReason::TransportUnavailable),
        ),
        AuditOutcome::Malformed | AuditOutcome::UnauthenticatedDiagnostic => (
            Some(AuditErrorCategory::Malformed),
            Some(RefusalReason::InvalidRequest),
        ),
        AuditOutcome::Accepted | AuditOutcome::Incomplete => (None, None),
    };
    ExchangeAuditEvent {
        audit_event_id: format!(
            "audit-{}",
            sha256(format!("{attempt_id}:{phase:?}").as_bytes())
        ),
        attempt_id: attempt_id.into(),
        wire_nonce: wire_nonce.into(),
        direction,
        phase,
        authenticated_peer_id: authenticated_peer_id.map(str::to_string),
        peer_claim: peer_claim.map(str::to_string),
        operation_id: operation_id.map(str::to_string),
        request_frame_bytes: 0,
        request_announced_body_bytes: None,
        request_sha256: None,
        reply_frame_bytes: 0,
        reply_announced_body_bytes: None,
        reply_sha256: None,
        outcome,
        error_category,
        reason_code,
        local_receipt_operation_id: None,
        local_receipt_sha256: None,
        remote_receipt_operation_id: None,
        remote_receipt_sha256: None,
        replayed: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn audit_event_with_reason(
    attempt_id: &str,
    wire_nonce: &str,
    direction: AuditDirection,
    phase: AuditPhase,
    peer_claim: Option<&str>,
    authenticated_peer_id: Option<&str>,
    operation_id: Option<&str>,
    outcome: AuditOutcome,
    reason: RefusalReason,
) -> ExchangeAuditEvent {
    let mut event = audit_event(
        attempt_id,
        wire_nonce,
        direction,
        phase,
        peer_claim,
        authenticated_peer_id,
        operation_id,
        outcome,
    );
    event.reason_code = Some(reason);
    event
}

fn apply_request_evidence(event: &mut ExchangeAuditEvent, evidence: &FrameEvidence) {
    event.request_frame_bytes = evidence.frame_bytes;
    event.request_announced_body_bytes = evidence.announced_body_bytes;
    event.request_sha256.clone_from(&evidence.body_sha256);
}

fn apply_reply_evidence(event: &mut ExchangeAuditEvent, evidence: &FrameEvidence) {
    event.reply_frame_bytes = evidence.frame_bytes;
    event.reply_announced_body_bytes = evidence.announced_body_bytes;
    event.reply_sha256.clone_from(&evidence.body_sha256);
}

fn apply_local_receipt(event: &mut ExchangeAuditEvent, receipt: Option<&(String, String)>) {
    if let Some((operation_id, checksum)) = receipt {
        event.local_receipt_operation_id = Some(operation_id.clone());
        event.local_receipt_sha256 = Some(checksum.clone());
    }
}

fn outcome_for_error(error: &Error) -> AuditOutcome {
    match error.category() {
        ErrorCategory::Unavailable => AuditOutcome::Unavailable,
        ErrorCategory::Refused | ErrorCategory::Malformed => AuditOutcome::Malformed,
    }
}

fn signable_reason(reason: RefusalReason) -> bool {
    matches!(
        reason,
        RefusalReason::InvalidRequest
            | RefusalReason::PolicyViolation
            | RefusalReason::OperationIdReused
    )
}

impl Node {
    /// Seeks one authenticated request within a bounded connection and time budget.
    ///
    /// # Errors
    ///
    /// Returns an unavailable error for local bind or I/O failures. Malformed and
    /// refused requests receive a bounded signed-independent error response.
    pub fn serve_once(&mut self) -> Result<(), Error> {
        self.serve_once_with_ready(|_| Ok(()), false)
    }

    /// Binds first, reports the actual address, and only then begins bounded admission.
    ///
    /// # Errors
    /// Returns a local error when bind, readiness, accept, or exchange fails.
    pub fn serve_once_reporting_address(
        &mut self,
        ready: impl FnOnce(SocketAddr) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.serve_once_with_ready(ready, false)
    }

    /// Laboratory-only deterministic seam for losing a reply after a durable decision.
    ///
    /// # Errors
    /// Returns unavailable after the destination decision is durable.
    pub fn serve_once_drop_reply_after_decision(
        &mut self,
        ready: impl FnOnce(SocketAddr) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.serve_once_with_ready(ready, true)
    }

    fn serve_once_with_ready(
        &mut self,
        ready: impl FnOnce(SocketAddr) -> Result<(), Error>,
        drop_after_decision: bool,
    ) -> Result<(), Error> {
        let listener = TcpListener::bind(self.bind).map_err(Error::unavailable)?;
        ready(listener.local_addr().map_err(Error::unavailable)?)?;
        listener.set_nonblocking(true).map_err(Error::unavailable)?;
        let deadline = Instant::now() + IO_TIMEOUT;
        let mut last_unauthenticated_error = None;
        for _ in 0..MAX_CONNECTIONS_PER_PROCESS {
            let stream = loop {
                if Instant::now() >= deadline {
                    return Err(last_unauthenticated_error.unwrap_or_else(|| {
                        Error::unavailable("listener admission deadline exceeded")
                    }));
                }
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(last_unauthenticated_error.unwrap_or_else(|| {
                                Error::unavailable("listener admission deadline exceeded")
                            }));
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => return Err(Error::unavailable(error)),
                }
            };
            let mut authenticated = false;
            let result =
                self.serve_connection_inner(stream, drop_after_decision, &mut authenticated);
            if authenticated {
                return result;
            }
            if let Err(error) = result {
                last_unauthenticated_error = Some(error);
            }
        }
        Err(last_unauthenticated_error
            .unwrap_or_else(|| Error::unavailable("listener connection limit reached")))
    }

    /// Handles exactly one bounded authenticated exchange on an accepted stream.
    /// The caller owns listener admission and concurrency limits.
    ///
    /// # Errors
    /// Returns a local I/O error when the bounded request/reply cannot complete.
    pub fn serve_connection(&mut self, stream: TcpStream) -> Result<(), Error> {
        let mut authenticated = false;
        self.serve_connection_inner(stream, false, &mut authenticated)
    }

    /// Sends one locally exported snapshot to one exact configured peer.
    ///
    /// # Errors
    ///
    /// Returns a locally derived error for local configuration, bounded I/O, reply
    /// binding or validation failure. A verified signed refusal is returned as
    /// [`ErrorSource::AuthenticatedRemoteRefusal`]; an unsigned diagnostic remains
    /// [`ErrorSource::UnauthenticatedRemoteDiagnostic`] and carries no authority.
    #[allow(clippy::too_many_lines)]
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
            .map_err(Error::durable)?
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
        let request_body = encode(&request)?;
        let request_intent = FrameEvidence::intent(&request_body);
        let sent_request_sha256 = request_intent
            .body_sha256
            .as_deref()
            .ok_or_else(|| Error::malformed("outbound request intent lacks digest"))?;
        let attempt_id = self.store.new_attempt_id(nonce).map_err(Error::durable)?;
        let mut prepared = audit_event(
            &attempt_id,
            nonce,
            AuditDirection::Outbound,
            AuditPhase::OutboundRequestPrepared,
            Some(&peer.replica_id),
            Some(&peer.replica_id),
            Some(operation_id),
            AuditOutcome::Incomplete,
        );
        apply_request_evidence(&mut prepared, &request_intent);
        self.record_audit(&prepared)?;
        let mut stream = match connect(&peer.endpoint) {
            Ok(stream) => stream,
            Err(error) => {
                return self.finish_outbound(
                    &attempt_id,
                    nonce,
                    &peer,
                    operation_id,
                    &request_intent,
                    &FrameEvidence::default(),
                    AuditOutcome::Unavailable,
                    None,
                    None,
                    false,
                    Err(error),
                );
            }
        };
        let request_transfer =
            match metered_write_frame(&mut stream, &request_body, Instant::now() + IO_TIMEOUT) {
                Ok(evidence) => evidence,
                Err(failure) => {
                    return self.finish_outbound(
                        &attempt_id,
                        nonce,
                        &peer,
                        operation_id,
                        &failure.evidence,
                        &FrameEvidence::default(),
                        AuditOutcome::Unavailable,
                        None,
                        None,
                        false,
                        Err(failure.error),
                    );
                }
            };
        let received = match metered_read_frame(&mut stream, Instant::now() + IO_TIMEOUT) {
            Ok(frame) => frame,
            Err(failure) => {
                if failure.evidence.frame_bytes == 0
                    && failure.error.category() == ErrorCategory::Unavailable
                {
                    // A complete request followed by total reply loss is uncertain.
                    // Stage D intentionally preserves only the prepared attempt.
                    return Err(failure.error);
                }
                return self.finish_outbound(
                    &attempt_id,
                    nonce,
                    &peer,
                    operation_id,
                    &request_transfer,
                    &failure.evidence,
                    outcome_for_error(&failure.error),
                    None,
                    None,
                    false,
                    Err(failure.error),
                );
            }
        };
        let reply: WireReply = match decode(&received.body) {
            Ok(reply) => reply,
            Err(error) => {
                return self.finish_outbound(
                    &attempt_id,
                    nonce,
                    &peer,
                    operation_id,
                    &request_transfer,
                    &received.evidence,
                    AuditOutcome::Malformed,
                    None,
                    None,
                    false,
                    Err(error),
                );
            }
        };
        match reply {
            WireReply::Imported {
                inserted,
                history_len,
                ref receipt_operation_id,
                ref receipt_sha256,
                replayed,
                ..
            } => {
                let result = self
                    .verify_reply(
                        &peer,
                        operation_id,
                        nonce,
                        sent_request_sha256,
                        &request,
                        &reply,
                    )
                    .map(|()| ImportResult {
                        inserted,
                        history_len,
                        receipt_operation_id: receipt_operation_id.clone(),
                        receipt_sha256: receipt_sha256.clone(),
                        replayed,
                    });
                let valid = result.is_ok();
                self.finish_outbound(
                    &attempt_id,
                    nonce,
                    &peer,
                    operation_id,
                    &request_transfer,
                    &received.evidence,
                    if valid {
                        AuditOutcome::Accepted
                    } else {
                        AuditOutcome::Malformed
                    },
                    None,
                    valid.then_some((receipt_operation_id.clone(), receipt_sha256.clone())),
                    valid && replayed,
                    result,
                )
            }
            WireReply::Refused { reason, .. } => {
                let result = self
                    .verify_reply(
                        &peer,
                        operation_id,
                        nonce,
                        sent_request_sha256,
                        &request,
                        &reply,
                    )
                    .and_then(|()| Err(Error::authenticated_refusal(reason)));
                let authenticated = result
                    .as_ref()
                    .is_err_and(|error| error.source() == ErrorSource::AuthenticatedRemoteRefusal);
                self.finish_outbound(
                    &attempt_id,
                    nonce,
                    &peer,
                    operation_id,
                    &request_transfer,
                    &received.evidence,
                    if authenticated {
                        AuditOutcome::AuthenticatedRefusal
                    } else {
                        AuditOutcome::Malformed
                    },
                    authenticated.then_some(reason),
                    None,
                    false,
                    result,
                )
            }
            WireReply::Diagnostic {
                server_replica_id,
                category,
                detail,
            } => self.finish_outbound(
                &attempt_id,
                nonce,
                &peer,
                operation_id,
                &request_transfer,
                &received.evidence,
                AuditOutcome::UnauthenticatedDiagnostic,
                None,
                None,
                false,
                if server_replica_id == peer.replica_id {
                    Err(Error::remote_diagnostic(category, detail))
                } else {
                    Err(Error::malformed(
                        "diagnostic claims a different server identity",
                    ))
                },
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_outbound(
        &mut self,
        attempt_id: &str,
        nonce: &str,
        peer: &Peer,
        operation_id: &str,
        request: &FrameEvidence,
        reply: &FrameEvidence,
        outcome: AuditOutcome,
        reason: Option<RefusalReason>,
        remote_receipt: Option<(String, String)>,
        replayed: bool,
        result: Result<ImportResult, Error>,
    ) -> Result<ImportResult, Error> {
        let authenticated_peer = matches!(
            outcome,
            AuditOutcome::Accepted | AuditOutcome::AuthenticatedRefusal
        )
        .then_some(peer.replica_id.as_str());
        let mut event = if let Some(reason) = reason {
            audit_event_with_reason(
                attempt_id,
                nonce,
                AuditDirection::Outbound,
                AuditPhase::OutboundExchangeCompleted,
                Some(&peer.replica_id),
                authenticated_peer,
                Some(operation_id),
                outcome,
                reason,
            )
        } else {
            audit_event(
                attempt_id,
                nonce,
                AuditDirection::Outbound,
                AuditPhase::OutboundExchangeCompleted,
                Some(&peer.replica_id),
                authenticated_peer,
                Some(operation_id),
                outcome,
            )
        };
        apply_request_evidence(&mut event, request);
        apply_reply_evidence(&mut event, reply);
        if let Some((operation_id, checksum)) = remote_receipt {
            event.remote_receipt_operation_id = Some(operation_id);
            event.remote_receipt_sha256 = Some(checksum);
        }
        event.replayed = replayed;
        self.record_audit(&event).map_err(|error| {
            Error::unavailable(format!(
                "outbound result is uncertain because terminal audit failed: {error}"
            ))
        })?;
        result
    }

    fn record_audit(&mut self, event: &ExchangeAuditEvent) -> Result<(), Error> {
        if self.audit_failure_phase == Some(event.phase) {
            self.audit_failure_phase = None;
            return Err(Error::durable(DurableError::Storage(
                "injected network audit failure".into(),
            )));
        }
        self.store
            .record_exchange_audit(event)
            .map(|_| ())
            .map_err(Error::durable)
    }

    fn execute_authenticated_import(
        &mut self,
        operation_id: &str,
        snapshot: &Snapshot,
        event: ExchangeAuditEvent,
    ) -> Result<podmesh_manager_ha_lab::durable::AuthenticatedImport, DurableError> {
        if self.audit_failure_phase == Some(AuditPhase::InboundImportCommitted) {
            self.audit_failure_phase = None;
            // Seed a valid row with the import event ID. The real Store import
            // then rejects the conflicting audit identity during prevalidation,
            // before its transaction writes a fact, receipt, or import audit.
            let nonce = new_preauth_nonce();
            let attempt_id = self.store.new_attempt_id(&nonce)?;
            let mut conflict = audit_event(
                &attempt_id,
                &nonce,
                AuditDirection::Inbound,
                AuditPhase::InboundRequestObserved,
                None,
                None,
                None,
                AuditOutcome::Malformed,
            );
            conflict.audit_event_id.clone_from(&event.audit_event_id);
            self.store.record_exchange_audit(&conflict)?;
            let conflict_close = audit_event(
                &attempt_id,
                &nonce,
                AuditDirection::Inbound,
                AuditPhase::InboundConnectionClosed,
                None,
                None,
                None,
                AuditOutcome::Malformed,
            );
            self.store.record_exchange_audit(&conflict_close)?;
        }
        self.store
            .execute_authenticated_import(operation_id, snapshot, event)
    }

    #[cfg(test)]
    fn fail_next_audit_at(&mut self, phase: AuditPhase) {
        self.audit_failure_phase = Some(phase);
    }

    #[cfg(test)]
    fn fail_post_auth_at(&mut self, failure: PostAuthFailure) {
        self.post_auth_failure = Some(failure);
    }

    fn take_post_auth_failure(&mut self, failure: PostAuthFailure) -> bool {
        #[cfg(test)]
        if self.post_auth_failure == Some(failure) {
            self.post_auth_failure = None;
            return true;
        }
        #[cfg(not(test))]
        let _ = &self.replica_id;
        let _ = failure;
        false
    }

    #[allow(clippy::too_many_lines)]
    fn serve_connection_inner(
        &mut self,
        mut stream: TcpStream,
        drop_after_decision: bool,
        authenticated: &mut bool,
    ) -> Result<(), Error> {
        let preauth_nonce = new_preauth_nonce();
        let received = match metered_read_frame(&mut stream, Instant::now() + IO_TIMEOUT) {
            Ok(frame) => frame,
            Err(failure) => {
                return self.observe_and_diagnose_preauth(
                    &mut stream,
                    &preauth_nonce,
                    &failure.evidence,
                    &failure.error,
                );
            }
        };
        let request: WireRequest = match decode(&received.body) {
            Ok(request) => request,
            Err(error) => {
                return self.observe_and_diagnose_preauth(
                    &mut stream,
                    &preauth_nonce,
                    &received.evidence,
                    &error,
                );
            }
        };
        if !valid_token(&request.nonce) {
            let error = Error::malformed("invalid request nonce");
            return self.observe_and_diagnose_preauth(
                &mut stream,
                &preauth_nonce,
                &received.evidence,
                &error,
            );
        }
        let attempt_id = self
            .store
            .new_attempt_id(&request.nonce)
            .map_err(Error::durable)?;
        let peer_claim =
            valid_token(&request.source_replica_id).then_some(request.source_replica_id.as_str());
        let operation = valid_token(&request.operation_id).then_some(request.operation_id.as_str());
        let peer = match self.authenticate_request(&request) {
            Ok(peer) => peer,
            Err(error) => {
                let outcome = if self.peer(&request.source_replica_id).is_ok() {
                    AuditOutcome::Malformed
                } else {
                    AuditOutcome::UnauthenticatedDiagnostic
                };
                let mut observed = audit_event(
                    &attempt_id,
                    &request.nonce,
                    AuditDirection::Inbound,
                    AuditPhase::InboundRequestObserved,
                    peer_claim,
                    None,
                    operation,
                    outcome,
                );
                apply_request_evidence(&mut observed, &received.evidence);
                self.record_audit(&observed)?;
                return self.send_diagnostic(
                    &mut stream,
                    &attempt_id,
                    &request.nonce,
                    peer_claim,
                    operation,
                    outcome,
                    &error,
                );
            }
        };
        *authenticated = true;
        let mut observed = audit_event(
            &attempt_id,
            &request.nonce,
            AuditDirection::Inbound,
            AuditPhase::InboundRequestObserved,
            Some(&peer.replica_id),
            Some(&peer.replica_id),
            Some(&request.operation_id),
            AuditOutcome::Accepted,
        );
        apply_request_evidence(&mut observed, &received.evidence);
        self.record_audit(&observed)?;
        if request.snapshot.replica_id != request.source_replica_id {
            return self.refuse_authenticated(
                &mut stream,
                &attempt_id,
                &request,
                &received.evidence,
                &peer,
                RefusalReason::InvalidRequest,
                drop_after_decision,
            );
        }
        if request.snapshot.configuration != self.configuration {
            return self.refuse_authenticated(
                &mut stream,
                &attempt_id,
                &request,
                &received.evidence,
                &peer,
                RefusalReason::PolicyViolation,
                drop_after_decision,
            );
        }
        let import_event = audit_event(
            &attempt_id,
            &request.nonce,
            AuditDirection::Inbound,
            AuditPhase::InboundImportCommitted,
            Some(&peer.replica_id),
            Some(&peer.replica_id),
            Some(&request.operation_id),
            AuditOutcome::Accepted,
        );
        let imported = match self.execute_authenticated_import(
            &request.operation_id,
            &request.snapshot,
            import_event,
        ) {
            Ok(imported) => imported,
            Err(DurableError::Refused(reason)) if signable_reason(reason) => {
                return self.refuse_authenticated(
                    &mut stream,
                    &attempt_id,
                    &request,
                    &received.evidence,
                    &peer,
                    reason,
                    drop_after_decision,
                );
            }
            Err(error) => {
                let original = Error::durable(error);
                return Err(self.close_authenticated_preserving(
                    &attempt_id,
                    &request,
                    &peer,
                    original,
                ));
            }
        };
        let Response::Imported {
            inserted,
            history_len,
        } = imported.executed.response
        else {
            return Err(Error::refused(
                "durable manager returned an unexpected import response",
            ));
        };
        let Some(receipt) = (if self.take_post_auth_failure(PostAuthFailure::MissingReceipt) {
            None
        } else {
            imported.executed.receipt
        }) else {
            return Err(Error::refused("authenticated import returned no receipt"));
        };
        let unsigned_reply = WireReply::Imported {
            source_replica_id: self.replica_id.clone(),
            destination_replica_id: peer.replica_id.clone(),
            operation_id: request.operation_id.clone(),
            nonce: request.nonce.clone(),
            request_sha256: received
                .evidence
                .body_sha256
                .clone()
                .ok_or_else(|| Error::malformed("authenticated request lacks digest"))?,
            inserted,
            history_len,
            receipt_operation_id: receipt.operation_id.clone(),
            receipt_sha256: receipt.sha256.clone(),
            replayed: imported.executed.replayed,
            mac_hex: String::new(),
        };
        let signed = if self.take_post_auth_failure(PostAuthFailure::SignReply) {
            Err(Error::malformed(
                "injected signed reply construction failure",
            ))
        } else {
            sign_reply(unsigned_reply, &peer.shared_key_hex)
        };
        let reply = signed?;
        let local_receipt = (receipt.operation_id, receipt.sha256);
        self.send_signed_reply(
            &mut stream,
            &attempt_id,
            &request,
            &peer,
            &reply,
            Some(&local_receipt),
            imported.executed.replayed,
            AuditOutcome::Accepted,
            None,
            drop_after_decision,
        )
    }

    fn authenticate_request(&self, request: &WireRequest) -> Result<Peer, Error> {
        if request.protocol != "podmesh-manager-network-lab/1" {
            return Err(Error::malformed("unsupported protocol"));
        }
        if request.destination_replica_id != self.replica_id {
            return Err(Error::malformed("request destination mismatch"));
        }
        validate_token("source replica ID", &request.source_replica_id)?;
        validate_token("operation ID", &request.operation_id)?;
        let peer = self.peer(&request.source_replica_id)?.clone();
        verify_request(request, &peer.shared_key_hex)?;
        Ok(peer)
    }

    fn close_authenticated_preserving(
        &mut self,
        attempt_id: &str,
        request: &WireRequest,
        peer: &Peer,
        original: Error,
    ) -> Error {
        let close = audit_event(
            attempt_id,
            &request.nonce,
            AuditDirection::Inbound,
            AuditPhase::InboundConnectionClosed,
            Some(&peer.replica_id),
            Some(&peer.replica_id),
            Some(&request.operation_id),
            AuditOutcome::Unavailable,
        );
        // Preserve the causal error. A writable store closes the attempt; an
        // unwritable store leaves it incomplete without replacing the cause.
        let _ = self.record_audit(&close);
        original
    }

    fn observe_and_diagnose_preauth(
        &mut self,
        stream: &mut TcpStream,
        preauth_nonce: &str,
        request_evidence: &FrameEvidence,
        error: &Error,
    ) -> Result<(), Error> {
        let attempt_id = self
            .store
            .new_attempt_id(preauth_nonce)
            .map_err(Error::durable)?;
        let outcome = outcome_for_error(error);
        let mut observed = audit_event(
            &attempt_id,
            preauth_nonce,
            AuditDirection::Inbound,
            AuditPhase::InboundRequestObserved,
            None,
            None,
            None,
            outcome,
        );
        apply_request_evidence(&mut observed, request_evidence);
        self.record_audit(&observed)?;
        self.send_diagnostic(
            stream,
            &attempt_id,
            preauth_nonce,
            None,
            None,
            outcome,
            error,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn refuse_authenticated(
        &mut self,
        stream: &mut TcpStream,
        attempt_id: &str,
        request: &WireRequest,
        request_evidence: &FrameEvidence,
        peer: &Peer,
        reason: RefusalReason,
        drop_after_decision: bool,
    ) -> Result<(), Error> {
        let decision = audit_event_with_reason(
            attempt_id,
            &request.nonce,
            AuditDirection::Inbound,
            AuditPhase::InboundRefusalRecorded,
            Some(&peer.replica_id),
            Some(&peer.replica_id),
            Some(&request.operation_id),
            AuditOutcome::AuthenticatedRefusal,
            reason,
        );
        self.record_audit(&decision)?;
        let reply = sign_reply(
            WireReply::Refused {
                source_replica_id: self.replica_id.clone(),
                destination_replica_id: peer.replica_id.clone(),
                operation_id: request.operation_id.clone(),
                nonce: request.nonce.clone(),
                request_sha256: request_evidence
                    .body_sha256
                    .clone()
                    .ok_or_else(|| Error::malformed("authenticated request lacks digest"))?,
                reason,
                mac_hex: String::new(),
            },
            &peer.shared_key_hex,
        )?;
        self.send_signed_reply(
            stream,
            attempt_id,
            request,
            peer,
            &reply,
            None,
            false,
            AuditOutcome::AuthenticatedRefusal,
            Some(reason),
            drop_after_decision,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn send_signed_reply(
        &mut self,
        stream: &mut TcpStream,
        attempt_id: &str,
        request: &WireRequest,
        peer: &Peer,
        reply: &WireReply,
        local_receipt: Option<&(String, String)>,
        replayed: bool,
        decision_outcome: AuditOutcome,
        reason: Option<RefusalReason>,
        drop_after_decision: bool,
    ) -> Result<(), Error> {
        if drop_after_decision {
            return Err(Error::unavailable("test reply loss after durable decision"));
        }
        let encoded = if self.take_post_auth_failure(PostAuthFailure::EncodeReply) {
            Err(Error::malformed("injected signed reply encoding failure"))
        } else {
            encode(reply)
        };
        let body = encoded?;
        let intent = FrameEvidence::intent(&body);
        let mut prepared = audit_event(
            attempt_id,
            &request.nonce,
            AuditDirection::Inbound,
            AuditPhase::InboundReplyPrepared,
            Some(&peer.replica_id),
            Some(&peer.replica_id),
            Some(&request.operation_id),
            AuditOutcome::Incomplete,
        );
        apply_reply_evidence(&mut prepared, &intent);
        apply_local_receipt(&mut prepared, local_receipt);
        prepared.replayed = replayed;
        self.record_audit(&prepared)?;
        match metered_write_frame(stream, &body, Instant::now() + IO_TIMEOUT) {
            Ok(written) => {
                let mut terminal = if let Some(reason) = reason {
                    audit_event_with_reason(
                        attempt_id,
                        &request.nonce,
                        AuditDirection::Inbound,
                        AuditPhase::InboundReplyWriteObserved,
                        Some(&peer.replica_id),
                        Some(&peer.replica_id),
                        Some(&request.operation_id),
                        decision_outcome,
                        reason,
                    )
                } else {
                    audit_event(
                        attempt_id,
                        &request.nonce,
                        AuditDirection::Inbound,
                        AuditPhase::InboundReplyWriteObserved,
                        Some(&peer.replica_id),
                        Some(&peer.replica_id),
                        Some(&request.operation_id),
                        decision_outcome,
                    )
                };
                apply_reply_evidence(&mut terminal, &written);
                apply_local_receipt(&mut terminal, local_receipt);
                terminal.replayed = replayed;
                self.record_audit(&terminal)
            }
            Err(failure) if failure.evidence.frame_bytes == 0 => {
                let close = audit_event(
                    attempt_id,
                    &request.nonce,
                    AuditDirection::Inbound,
                    AuditPhase::InboundConnectionClosed,
                    Some(&peer.replica_id),
                    Some(&peer.replica_id),
                    Some(&request.operation_id),
                    AuditOutcome::Unavailable,
                );
                self.record_audit(&close)?;
                Err(failure.error)
            }
            Err(failure) => {
                let mut terminal = audit_event(
                    attempt_id,
                    &request.nonce,
                    AuditDirection::Inbound,
                    AuditPhase::InboundReplyWriteObserved,
                    Some(&peer.replica_id),
                    Some(&peer.replica_id),
                    Some(&request.operation_id),
                    AuditOutcome::Unavailable,
                );
                apply_reply_evidence(&mut terminal, &failure.evidence);
                apply_local_receipt(&mut terminal, local_receipt);
                terminal.replayed = replayed;
                self.record_audit(&terminal)?;
                Err(failure.error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send_diagnostic(
        &mut self,
        stream: &mut TcpStream,
        attempt_id: &str,
        nonce: &str,
        peer_claim: Option<&str>,
        operation_id: Option<&str>,
        observed_outcome: AuditOutcome,
        error: &Error,
    ) -> Result<(), Error> {
        let category = match observed_outcome {
            AuditOutcome::Unavailable => ErrorCategory::Unavailable,
            AuditOutcome::Malformed | AuditOutcome::UnauthenticatedDiagnostic => {
                ErrorCategory::Malformed
            }
            AuditOutcome::Accepted
            | AuditOutcome::AuthenticatedRefusal
            | AuditOutcome::Incomplete => error.category(),
        };
        let body = encode(&WireReply::Diagnostic {
            server_replica_id: self.replica_id.clone(),
            category,
            detail: diagnostic_detail(category).into(),
        })?;
        match metered_write_frame(stream, &body, Instant::now() + IO_TIMEOUT) {
            Ok(written) => {
                let mut terminal = audit_event(
                    attempt_id,
                    nonce,
                    AuditDirection::Inbound,
                    AuditPhase::InboundDiagnosticReplyWritten,
                    peer_claim,
                    None,
                    operation_id,
                    observed_outcome,
                );
                apply_reply_evidence(&mut terminal, &written);
                self.record_audit(&terminal)?;
                let _ = stream.shutdown(Shutdown::Write);
                Ok(())
            }
            Err(failure) if failure.evidence.frame_bytes == 0 => {
                let close = audit_event(
                    attempt_id,
                    nonce,
                    AuditDirection::Inbound,
                    AuditPhase::InboundConnectionClosed,
                    peer_claim,
                    None,
                    operation_id,
                    observed_outcome,
                );
                self.record_audit(&close)?;
                Err(failure.error)
            }
            Err(failure) => {
                let mut terminal = audit_event(
                    attempt_id,
                    nonce,
                    AuditDirection::Inbound,
                    AuditPhase::InboundDiagnosticReplyWritten,
                    peer_claim,
                    None,
                    operation_id,
                    AuditOutcome::Unavailable,
                );
                apply_reply_evidence(&mut terminal, &failure.evidence);
                self.record_audit(&terminal)?;
                Err(failure.error)
            }
        }
    }

    fn verify_reply(
        &self,
        peer: &Peer,
        operation_id: &str,
        nonce: &str,
        sent_request_sha256: &str,
        request: &WireRequest,
        reply: &WireReply,
    ) -> Result<(), Error> {
        match reply {
            WireReply::Imported {
                source_replica_id,
                destination_replica_id,
                operation_id: actual_operation,
                nonce: actual_nonce,
                request_sha256,
                inserted,
                history_len,
                receipt_operation_id,
                receipt_sha256,
                ..
            } => {
                if source_replica_id != &peer.replica_id
                    || destination_replica_id != &self.replica_id
                    || actual_operation != operation_id
                    || actual_nonce != nonce
                    || request_sha256 != sent_request_sha256
                {
                    return Err(Error::malformed(
                        "reply identity or replay binding mismatch",
                    ));
                }
                verify_reply(reply, &peer.shared_key_hex)?;
                let expected = authenticated_import_receipt_id(
                    &self.configuration.topology().map_err(Error::refused)?,
                    &self.replica_id,
                    operation_id,
                )
                .map_err(Error::durable)?;
                if receipt_operation_id != &expected
                    || !valid_hash(receipt_sha256)
                    || *inserted > request.snapshot.facts.len()
                    || *history_len < *inserted
                {
                    return Err(Error::malformed("reply receipt or count binding mismatch"));
                }
                Ok(())
            }
            WireReply::Refused {
                source_replica_id,
                destination_replica_id,
                operation_id: actual_operation,
                nonce: actual_nonce,
                request_sha256,
                reason,
                ..
            } => {
                verify_reply(reply, &peer.shared_key_hex)?;
                if source_replica_id != &peer.replica_id
                    || destination_replica_id != &self.replica_id
                    || actual_operation != operation_id
                    || actual_nonce != nonce
                    || request_sha256 != sent_request_sha256
                    || !signable_reason(*reason)
                {
                    return Err(Error::malformed("refusal binding mismatch"));
                }
                Ok(())
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResult {
    pub inserted: usize,
    pub history_len: usize,
    pub receipt_operation_id: String,
    pub receipt_sha256: String,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum WireReply {
    Imported {
        source_replica_id: String,
        destination_replica_id: String,
        operation_id: String,
        nonce: String,
        request_sha256: String,
        inserted: usize,
        history_len: usize,
        receipt_operation_id: String,
        receipt_sha256: String,
        replayed: bool,
        mac_hex: String,
    },
    Refused {
        source_replica_id: String,
        destination_replica_id: String,
        operation_id: String,
        nonce: String,
        request_sha256: String,
        reason: RefusalReason,
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
    /// A configured peer authenticated a closed refusal bound to this request.
    AuthenticatedRemoteRefusal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    category: ErrorCategory,
    source: ErrorSource,
    detail: String,
}

impl Error {
    /// Converts a local operating-system I/O failure without granting peer authority.
    #[must_use]
    pub fn from_io(value: std::io::Error) -> Self {
        Self::unavailable(value)
    }
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
    fn authenticated_refusal(reason: RefusalReason) -> Self {
        Self {
            category: ErrorCategory::Refused,
            source: ErrorSource::AuthenticatedRemoteRefusal,
            detail: format!("authenticated remote refusal: {reason}"),
        }
    }
    fn durable(error: DurableError) -> Self {
        let category = match &error {
            DurableError::Refused(_) | DurableError::Corrupt(_) => ErrorCategory::Refused,
            DurableError::Storage(_) => ErrorCategory::Unavailable,
            DurableError::InvalidAudit(_) => ErrorCategory::Malformed,
        };
        Self::new(category, error)
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FrameEvidence {
    frame_bytes: u64,
    announced_body_bytes: Option<u64>,
    body_sha256: Option<String>,
}

impl FrameEvidence {
    fn intent(body: &[u8]) -> Self {
        Self {
            frame_bytes: 0,
            announced_body_bytes: Some(body.len() as u64),
            body_sha256: Some(sha256(body)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReceivedFrame {
    body: Vec<u8>,
    evidence: FrameEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TransferFailure {
    error: Error,
    evidence: FrameEvidence,
}

trait DeadlineRead: Read {
    fn set_deadline_timeout(&self, timeout: Duration) -> std::io::Result<()>;
}

impl DeadlineRead for TcpStream {
    fn set_deadline_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))
    }
}

trait DeadlineWrite: Write {
    fn set_deadline_timeout(&self, timeout: Duration) -> std::io::Result<()>;
}

impl DeadlineWrite for TcpStream {
    fn set_deadline_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_write_timeout(Some(timeout))
    }
}

#[cfg(test)]
fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, Error> {
    metered_read_frame(stream, Instant::now() + IO_TIMEOUT)
        .map(|frame| frame.body)
        .map_err(|failure| failure.error)
}

fn metered_read_frame<R: DeadlineRead>(
    reader: &mut R,
    deadline: Instant,
) -> Result<ReceivedFrame, TransferFailure> {
    let mut evidence = FrameEvidence::default();
    let mut length = [0_u8; 4];
    if let Err(error) = read_exact_metered(reader, &mut length, deadline, &mut evidence.frame_bytes)
    {
        return Err(TransferFailure { error, evidence });
    }
    let length = u32::from_be_bytes(length) as usize;
    evidence.announced_body_bytes = Some(length as u64);
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(TransferFailure {
            error: Error::malformed("invalid frame length"),
            evidence,
        });
    }
    let mut bytes = vec![0; length];
    if let Err(error) = read_exact_metered(reader, &mut bytes, deadline, &mut evidence.frame_bytes)
    {
        return Err(TransferFailure { error, evidence });
    }
    evidence.body_sha256 = Some(sha256(&bytes));
    Ok(ReceivedFrame {
        body: bytes,
        evidence,
    })
}

fn read_exact_metered<R: DeadlineRead>(
    reader: &mut R,
    mut bytes: &mut [u8],
    deadline: Instant,
    transferred: &mut u64,
) -> Result<(), Error> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| Error::unavailable("frame read deadline exceeded"))?;
        reader
            .set_deadline_timeout(remaining)
            .map_err(Error::unavailable)?;
        match reader.read(bytes) {
            Ok(0) if *transferred == 0 => {
                return Err(Error::unavailable("peer closed before sending a frame"));
            }
            Ok(0) => return Err(Error::malformed("truncated frame")),
            Ok(length) => {
                *transferred += length as u64;
                bytes = &mut bytes[length..];
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Error::unavailable(error)),
        }
    }
    Ok(())
}

#[cfg(test)]
fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    metered_write_frame(stream, bytes, Instant::now() + IO_TIMEOUT)
        .map(|_| ())
        .map_err(|failure| std::io::Error::other(failure.error.to_string()))
}

fn metered_write_frame<W: DeadlineWrite>(
    writer: &mut W,
    bytes: &[u8],
    deadline: Instant,
) -> Result<FrameEvidence, TransferFailure> {
    let length: u32 = bytes.len().try_into().map_err(|_| TransferFailure {
        error: Error::malformed("frame limit"),
        evidence: FrameEvidence::default(),
    })?;
    let mut evidence = FrameEvidence {
        frame_bytes: 0,
        announced_body_bytes: Some(bytes.len() as u64),
        body_sha256: Some(sha256(bytes)),
    };
    for mut part in [&length.to_be_bytes()[..], bytes] {
        while !part.is_empty() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| TransferFailure {
                    error: Error::unavailable("frame write deadline exceeded"),
                    evidence: evidence.clone(),
                })?;
            writer
                .set_deadline_timeout(remaining)
                .map_err(|error| TransferFailure {
                    error: Error::unavailable(error),
                    evidence: evidence.clone(),
                })?;
            let written = match writer.write(part) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(written) => written,
                Err(error) => {
                    return Err(TransferFailure {
                        error: Error::unavailable(error),
                        evidence,
                    });
                }
            };
            if written == 0 {
                return Err(TransferFailure {
                    error: Error::unavailable(std::io::ErrorKind::WriteZero),
                    evidence,
                });
            }
            evidence.frame_bytes += written as u64;
            part = &part[written..];
        }
    }
    Ok(evidence)
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
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

fn new_preauth_nonce() -> String {
    let sequence = NEXT_PREAUTH_NONCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!(
        "preauth:{}",
        sha256(format!("{}:{sequence}:{timestamp}", std::process::id()).as_bytes())
    )
}

fn diagnostic_detail(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::Unavailable => "bounded transport unavailable",
        ErrorCategory::Refused => "request not accepted",
        ErrorCategory::Malformed => "invalid network request",
    }
}

fn valid_token(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_token(kind: &str, value: &str) -> Result<(), Error> {
    if valid_token(value) {
        Ok(())
    } else {
        Err(Error::malformed(format!(
            "{kind} must be 1-128 ASCII token characters"
        )))
    }
}

fn parse_key(value: &str) -> Result<Vec<u8>, Error> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
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
    mac.update(b"podmesh-manager-network-lab/1\0");
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
    match &mut reply {
        WireReply::Imported {
            source_replica_id,
            destination_replica_id,
            operation_id,
            nonce,
            request_sha256,
            inserted,
            history_len,
            receipt_operation_id,
            receipt_sha256,
            replayed,
            mac_hex,
        } => {
            *mac_hex = mac(
                key,
                &(
                    "imported",
                    source_replica_id,
                    destination_replica_id,
                    operation_id,
                    nonce,
                    request_sha256,
                    *inserted,
                    *history_len,
                    receipt_operation_id,
                    receipt_sha256,
                    *replayed,
                ),
            )?;
        }
        WireReply::Refused {
            source_replica_id,
            destination_replica_id,
            operation_id,
            nonce,
            request_sha256,
            reason,
            mac_hex,
        } => {
            *mac_hex = mac(
                key,
                &(
                    "refused",
                    source_replica_id,
                    destination_replica_id,
                    operation_id,
                    nonce,
                    request_sha256,
                    *reason,
                ),
            )?;
        }
        WireReply::Diagnostic { .. } => {}
    }
    Ok(reply)
}

fn verify_reply(reply: &WireReply, key: &str) -> Result<(), Error> {
    let (expected, actual) = match reply {
        WireReply::Imported {
            source_replica_id,
            destination_replica_id,
            operation_id,
            nonce,
            request_sha256,
            inserted,
            history_len,
            receipt_operation_id,
            receipt_sha256,
            replayed,
            mac_hex,
        } => (
            mac(
                key,
                &(
                    "imported",
                    source_replica_id,
                    destination_replica_id,
                    operation_id,
                    nonce,
                    request_sha256,
                    *inserted,
                    *history_len,
                    receipt_operation_id,
                    receipt_sha256,
                    *replayed,
                ),
            )?,
            mac_hex,
        ),
        WireReply::Refused {
            source_replica_id,
            destination_replica_id,
            operation_id,
            nonce,
            request_sha256,
            reason,
            mac_hex,
        } => (
            mac(
                key,
                &(
                    "refused",
                    source_replica_id,
                    destination_replica_id,
                    operation_id,
                    nonce,
                    request_sha256,
                    *reason,
                ),
            )?,
            mac_hex,
        ),
        WireReply::Diagnostic { .. } => return Ok(()),
    };
    if constant_time_eq(expected.as_bytes(), actual.as_bytes()) {
        Ok(())
    } else {
        Err(Error::malformed("reply authentication failed"))
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
        io::{Cursor, ErrorKind},
        net::{Ipv4Addr, Shutdown},
        sync::mpsc,
        thread,
        time::Duration,
    };

    use podmesh_manager_ha_lab::{
        durable::{inspect_read_only, Request, Response},
        ReplicaConfig, ScopeGrant,
    };
    use tempfile::TempDir;

    use super::*;

    const WRONG_KEY: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

    impl DeadlineRead for Cursor<Vec<u8>> {
        fn set_deadline_timeout(&self, _timeout: Duration) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct InterruptingReader {
        inner: Cursor<Vec<u8>>,
        interrupted: bool,
    }

    impl Read for InterruptingReader {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::Error::from(ErrorKind::Interrupted));
            }
            self.inner.read(bytes)
        }
    }

    impl DeadlineRead for InterruptingReader {
        fn set_deadline_timeout(&self, _timeout: Duration) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct LimitedWriter {
        limit: usize,
        bytes: Vec<u8>,
        interrupted: bool,
    }

    impl Write for LimitedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::Error::from(ErrorKind::Interrupted));
            }
            let remaining = self.limit.saturating_sub(self.bytes.len());
            let written = remaining.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..written]);
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            panic!("framing must not add a post-frame flush failure state")
        }
    }

    impl DeadlineWrite for LimitedWriter {
        fn set_deadline_timeout(&self, _timeout: Duration) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut frame = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
        frame.extend_from_slice(body);
        frame
    }

    #[test]
    fn read_metering_is_prefix_inclusive_at_exact_boundaries_and_retries_interrupted() {
        let body = b"five!";
        let frame = framed(body);
        let total = frame.len();
        for transferred in [0, 1, 3, 4, total - 1] {
            let mut reader = Cursor::new(frame[..transferred].to_vec());
            let failure = metered_read_frame(&mut reader, Instant::now() + IO_TIMEOUT).unwrap_err();
            assert_eq!(failure.evidence.frame_bytes, transferred as u64);
            assert_eq!(
                failure.error.category(),
                if transferred == 0 {
                    ErrorCategory::Unavailable
                } else {
                    ErrorCategory::Malformed
                }
            );
            assert_eq!(
                failure.evidence.announced_body_bytes,
                (transferred >= 4).then_some(body.len() as u64)
            );
            assert_eq!(failure.evidence.body_sha256, None);
        }
        let mut reader = InterruptingReader {
            inner: Cursor::new(frame),
            interrupted: false,
        };
        let received = metered_read_frame(&mut reader, Instant::now() + IO_TIMEOUT).unwrap();
        assert_eq!(received.body, body);
        assert_eq!(received.evidence.frame_bytes, total as u64);
        assert_eq!(
            received.evidence.announced_body_bytes,
            Some(body.len() as u64)
        );
        assert_eq!(received.evidence.body_sha256, Some(sha256(body)));
    }

    #[test]
    fn oversized_prefix_is_bounded_without_reading_or_allocating_its_body() {
        let announced = u32::try_from(MAX_FRAME_BYTES).unwrap() + 1;
        let mut reader = Cursor::new(announced.to_be_bytes().to_vec());
        let failure = metered_read_frame(&mut reader, Instant::now() + IO_TIMEOUT).unwrap_err();
        assert_eq!(failure.error.category(), ErrorCategory::Malformed);
        assert_eq!(failure.evidence.frame_bytes, 4);
        assert_eq!(
            failure.evidence.announced_body_bytes,
            Some(u64::from(announced))
        );
        assert_eq!(failure.evidence.body_sha256, None);
    }

    #[test]
    fn write_metering_is_prefix_inclusive_at_exact_boundaries_and_retries_interrupted() {
        let body = b"five!";
        let total = body.len() + 4;
        for transferred in [0, 1, 3, 4, total - 1] {
            let mut writer = LimitedWriter {
                limit: transferred,
                bytes: Vec::new(),
                interrupted: false,
            };
            let failure =
                metered_write_frame(&mut writer, body, Instant::now() + IO_TIMEOUT).unwrap_err();
            assert_eq!(failure.evidence.frame_bytes, transferred as u64);
            assert_eq!(
                failure.evidence.announced_body_bytes,
                Some(body.len() as u64)
            );
            assert_eq!(failure.evidence.body_sha256, Some(sha256(body)));
        }
        let mut writer = LimitedWriter {
            limit: total,
            bytes: Vec::new(),
            interrupted: false,
        };
        let written = metered_write_frame(&mut writer, body, Instant::now() + IO_TIMEOUT).unwrap();
        assert_eq!(written.frame_bytes, total as u64);
        assert_eq!(writer.bytes, framed(body));
    }

    #[test]
    fn request_mac_binds_every_identity_nonce_and_snapshot_field() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let r1 = configuration(&directory, "r1", &addresses);
        let Response::Snapshot { snapshot } = r1
            .open()
            .unwrap()
            .store
            .execute(&Request::Export {})
            .unwrap()
        else {
            unreachable!();
        };
        let request = sign_request(
            WireRequest {
                protocol: "podmesh-manager-network-lab/1".into(),
                source_replica_id: "r1".into(),
                destination_replica_id: "r2".into(),
                operation_id: "tamper-operation".into(),
                nonce: "tamper-nonce".into(),
                snapshot,
                mac_hex: String::new(),
            },
            &key_for(1, 2),
        )
        .unwrap();
        let mut variants = Vec::new();
        let mut changed = request.clone();
        changed.protocol.push('x');
        variants.push(changed);
        let mut changed = request.clone();
        changed.source_replica_id = "r3".into();
        variants.push(changed);
        let mut changed = request.clone();
        changed.destination_replica_id = "r3".into();
        variants.push(changed);
        let mut changed = request.clone();
        changed.operation_id.push('x');
        variants.push(changed);
        let mut changed = request.clone();
        changed.nonce.push('x');
        variants.push(changed);
        let mut changed = request.clone();
        changed.snapshot.replica_id = "r2".into();
        variants.push(changed);
        assert!(variants
            .iter()
            .all(|changed| verify_request(changed, &key_for(1, 2)).is_err()));
    }

    #[test]
    fn signed_success_and_refusal_mac_bind_all_conveyed_fields() {
        let success = sign_reply(
            WireReply::Imported {
                source_replica_id: "r2".into(),
                destination_replica_id: "r1".into(),
                operation_id: "reply-operation".into(),
                nonce: "reply-nonce".into(),
                request_sha256: "d".repeat(64),
                inserted: 2,
                history_len: 3,
                receipt_operation_id: format!("network:{}", "a".repeat(64)),
                receipt_sha256: "b".repeat(64),
                replayed: false,
                mac_hex: String::new(),
            },
            &key_for(1, 2),
        )
        .unwrap();
        let mut variants = Vec::new();
        for field in 0..10 {
            let mut changed = success.clone();
            if let WireReply::Imported {
                source_replica_id,
                destination_replica_id,
                operation_id,
                nonce,
                request_sha256,
                inserted,
                history_len,
                receipt_operation_id,
                receipt_sha256,
                replayed,
                ..
            } = &mut changed
            {
                match field {
                    0 => source_replica_id.push('x'),
                    1 => destination_replica_id.push('x'),
                    2 => operation_id.push('x'),
                    3 => nonce.push('x'),
                    4 => request_sha256.replace_range(..1, "e"),
                    5 => *inserted += 1,
                    6 => *history_len += 1,
                    7 => receipt_operation_id.push('x'),
                    8 => receipt_sha256.replace_range(..1, "c"),
                    9 => *replayed = true,
                    _ => unreachable!(),
                }
            }
            variants.push(changed);
        }
        assert!(variants
            .iter()
            .all(|changed| verify_reply(changed, &key_for(1, 2)).is_err()));

        let refusal = sign_reply(
            WireReply::Refused {
                source_replica_id: "r2".into(),
                destination_replica_id: "r1".into(),
                operation_id: "reply-operation".into(),
                nonce: "reply-nonce".into(),
                request_sha256: "d".repeat(64),
                reason: RefusalReason::PolicyViolation,
                mac_hex: String::new(),
            },
            &key_for(1, 2),
        )
        .unwrap();
        for field in 0..6 {
            let mut changed = refusal.clone();
            if let WireReply::Refused {
                source_replica_id,
                destination_replica_id,
                operation_id,
                nonce,
                request_sha256,
                reason,
                ..
            } = &mut changed
            {
                match field {
                    0 => source_replica_id.push('x'),
                    1 => destination_replica_id.push('x'),
                    2 => operation_id.push('x'),
                    3 => nonce.push('x'),
                    4 => request_sha256.replace_range(..1, "e"),
                    5 => *reason = RefusalReason::InvalidRequest,
                    _ => unreachable!(),
                }
            }
            assert!(verify_reply(&changed, &key_for(1, 2)).is_err());
        }
    }

    #[test]
    fn captured_success_cannot_be_swapped_into_changed_same_operation_and_nonce() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let r1 = configuration(&directory, "r1", &addresses);
        let mut source = r1.open().unwrap();
        let peer = source.peer("r2").unwrap().clone();
        let Response::Snapshot { snapshot: first } =
            source.store.execute(&Request::Export {}).unwrap()
        else {
            unreachable!();
        };
        let first_request = sign_request(
            WireRequest {
                protocol: "podmesh-manager-network-lab/1".into(),
                source_replica_id: "r1".into(),
                destination_replica_id: "r2".into(),
                operation_id: "same-operation".into(),
                nonce: "same-nonce".into(),
                snapshot: first,
                mac_hex: String::new(),
            },
            &peer.shared_key_hex,
        )
        .unwrap();
        let first_digest = sha256(&encode(&first_request).unwrap());
        let receipt_operation_id = authenticated_import_receipt_id(
            &source.configuration.topology().unwrap(),
            "r1",
            "same-operation",
        )
        .unwrap();
        let captured = sign_reply(
            WireReply::Imported {
                source_replica_id: "r2".into(),
                destination_replica_id: "r1".into(),
                operation_id: "same-operation".into(),
                nonce: "same-nonce".into(),
                request_sha256: first_digest,
                inserted: 0,
                history_len: 0,
                receipt_operation_id,
                receipt_sha256: "a".repeat(64),
                replayed: false,
                mac_hex: String::new(),
            },
            &peer.shared_key_hex,
        )
        .unwrap();
        source
            .store
            .execute(&Request::Observe {
                operation_id: "request-changed".into(),
                scope: "scope1".into(),
                subject: "universe".into(),
                exclusive_resource: None,
                active_claim: false,
                value: "changed".into(),
            })
            .unwrap();
        let Response::Snapshot { snapshot: changed } =
            source.store.execute(&Request::Export {}).unwrap()
        else {
            unreachable!();
        };
        let changed_request = sign_request(
            WireRequest {
                snapshot: changed,
                ..first_request
            },
            &peer.shared_key_hex,
        )
        .unwrap();
        let changed_digest = sha256(&encode(&changed_request).unwrap());
        let WireReply::Imported {
            request_sha256: captured_request_digest,
            ..
        } = &captured
        else {
            unreachable!();
        };
        assert_ne!(&changed_digest, captured_request_digest);
        let error = source
            .verify_reply(
                &peer,
                "same-operation",
                "same-nonce",
                &changed_digest,
                &changed_request,
                &captured,
            )
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Malformed);
    }

    #[test]
    fn sync_rejects_captured_success_and_records_malformed_without_remote_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let receipt_operation_id = authenticated_import_receipt_id(
            &r1.manager.topology().unwrap(),
            "r1",
            "captured-sync-operation",
        )
        .unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        point_peer(&mut r1, "r2", listener.local_addr().unwrap());
        let (captured_sender, captured_receiver) = mpsc::sync_channel(1);
        let key = key_for(1, 2);
        let first_server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let body = read_frame(&mut stream).unwrap();
            let request: WireRequest = decode(&body).unwrap();
            let reply = sign_reply(
                WireReply::Imported {
                    source_replica_id: "r2".into(),
                    destination_replica_id: "r1".into(),
                    operation_id: request.operation_id,
                    nonce: request.nonce,
                    request_sha256: sha256(&body),
                    inserted: 0,
                    history_len: 0,
                    receipt_operation_id,
                    receipt_sha256: "a".repeat(64),
                    replayed: false,
                    mac_hex: String::new(),
                },
                &key,
            )
            .unwrap();
            captured_sender.send(reply.clone()).unwrap();
            write_frame(&mut stream, &encode(&reply).unwrap()).unwrap();
        });
        r1.open()
            .unwrap()
            .sync_to("r2", "captured-sync-operation", "captured-sync-nonce")
            .unwrap();
        first_server.join().unwrap();
        let captured = captured_receiver.recv().unwrap();

        observe(&r1, "captured-sync-change");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        point_peer(&mut r1, "r2", listener.local_addr().unwrap());
        let second_server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_frame(&mut stream).unwrap();
            write_frame(&mut stream, &encode(&captured).unwrap()).unwrap();
        });
        let error = r1
            .open()
            .unwrap()
            .sync_to("r2", "captured-sync-operation", "captured-sync-nonce")
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Malformed);
        second_server.join().unwrap();
        let source = inspection(&r1);
        let malformed = source
            .ordered_audit_events
            .iter()
            .filter(|evidence| {
                evidence.event.phase == AuditPhase::OutboundExchangeCompleted
                    && evidence.event.operation_id.as_deref() == Some("captured-sync-operation")
                    && evidence.event.outcome == AuditOutcome::Malformed
            })
            .collect::<Vec<_>>();
        assert_eq!(malformed.len(), 1);
        assert!(malformed[0].event.remote_receipt_operation_id.is_none());
        assert!(malformed[0].event.remote_receipt_sha256.is_none());
    }

    #[test]
    fn source_audit_preserves_each_authenticated_refusal_reason() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        for (index, reason) in [
            RefusalReason::PolicyViolation,
            RefusalReason::OperationIdReused,
        ]
        .into_iter()
        .enumerate()
        {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            point_peer(&mut r1, "r2", address);
            let key = key_for(1, 2);
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let body = read_frame(&mut stream).unwrap();
                let request: WireRequest = decode(&body).unwrap();
                let reply = sign_reply(
                    WireReply::Refused {
                        source_replica_id: "r2".into(),
                        destination_replica_id: "r1".into(),
                        operation_id: request.operation_id,
                        nonce: request.nonce,
                        request_sha256: sha256(&body),
                        reason,
                        mac_hex: String::new(),
                    },
                    &key,
                )
                .unwrap();
                write_frame(&mut stream, &encode(&reply).unwrap()).unwrap();
            });
            let operation = format!("refusal-operation-{index}");
            let nonce = format!("refusal-nonce-{index}");
            let error = r1
                .open()
                .unwrap()
                .sync_to("r2", &operation, &nonce)
                .unwrap_err();
            assert_eq!(error.source(), ErrorSource::AuthenticatedRemoteRefusal);
            server.join().unwrap();
        }
        let source = inspection(&r1);
        for reason in [
            RefusalReason::PolicyViolation,
            RefusalReason::OperationIdReused,
        ] {
            assert!(source.ordered_audit_events.iter().any(|evidence| {
                evidence.event.phase == AuditPhase::OutboundExchangeCompleted
                    && evidence.event.outcome == AuditOutcome::AuthenticatedRefusal
                    && evidence.event.reason_code == Some(reason)
            }));
        }
    }

    #[test]
    fn pure_configuration_validation_opens_no_store_or_listener() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
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
        let addresses = placeholder_addresses();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let destination_address = listener.local_addr().unwrap();
        let mut destination = configuration(&directory, "r2", &addresses);
        destination.bind = destination_address;
        let destination_for_worker = destination.clone();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            destination_for_worker
                .open()
                .unwrap()
                .serve_connection(stream)
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
        drop(source);
        let source_audit = inspection(&source_configuration);
        let destination_audit = inspection(&destination);
        assert_eq!(source_audit.audit_event_count, 2);
        assert_eq!(destination_audit.audit_event_count, 4);
        let prepared = source_audit
            .ordered_audit_events
            .iter()
            .find(|evidence| evidence.event.phase == AuditPhase::OutboundRequestPrepared)
            .unwrap();
        let completed = source_audit
            .ordered_audit_events
            .iter()
            .find(|evidence| evidence.event.phase == AuditPhase::OutboundExchangeCompleted)
            .unwrap();
        assert_eq!(prepared.event.request_frame_bytes, 0);
        assert_eq!(
            completed.event.request_frame_bytes,
            completed.event.request_announced_body_bytes.unwrap() + 4
        );
        assert_eq!(
            completed.event.reply_frame_bytes,
            completed.event.reply_announced_body_bytes.unwrap() + 4
        );
        assert_eq!(
            destination_audit
                .ordered_audit_events
                .iter()
                .map(|evidence| evidence.event.phase)
                .collect::<std::collections::BTreeSet<_>>(),
            [
                AuditPhase::InboundRequestObserved,
                AuditPhase::InboundImportCommitted,
                AuditPhase::InboundReplyPrepared,
                AuditPhase::InboundReplyWriteObserved,
            ]
            .into_iter()
            .collect()
        );
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

    fn placeholder_addresses() -> [SocketAddr; 3] {
        [SocketAddr::from((Ipv4Addr::LOCALHOST, 0)); 3]
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

    fn spawn_connection_server(
        configuration: ConfigurationFile,
    ) -> (SocketAddr, thread::JoinHandle<Result<(), Error>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            configuration.open().unwrap().serve_connection(stream)
        });
        (address, worker)
    }

    fn spawn_ready_server(
        mut configuration: ConfigurationFile,
        drop_after_decision: bool,
        fail_phase: Option<AuditPhase>,
    ) -> (SocketAddr, thread::JoinHandle<Result<(), Error>>) {
        configuration.bind = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let mut node = configuration.open().unwrap();
            if let Some(phase) = fail_phase {
                node.fail_next_audit_at(phase);
            }
            let ready = |address| sender.send(address).map_err(Error::unavailable);
            if drop_after_decision {
                node.serve_once_drop_reply_after_decision(ready)
            } else {
                node.serve_once_reporting_address(ready)
            }
        });
        (receiver.recv_timeout(IO_TIMEOUT).unwrap(), worker)
    }

    fn spawn_ready_server_with_post_auth_failure(
        mut configuration: ConfigurationFile,
        failure: PostAuthFailure,
    ) -> (SocketAddr, thread::JoinHandle<Result<(), Error>>) {
        configuration.bind = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let mut node = configuration.open().unwrap();
            node.fail_post_auth_at(failure);
            node.serve_once_reporting_address(|address| {
                sender.send(address).map_err(Error::unavailable)
            })
        });
        (receiver.recv_timeout(IO_TIMEOUT).unwrap(), worker)
    }

    fn point_peer(configuration: &mut ConfigurationFile, peer_id: &str, endpoint: SocketAddr) {
        configuration
            .peers
            .iter_mut()
            .find(|peer| peer.replica_id == peer_id)
            .unwrap()
            .endpoint = endpoint;
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

    fn inspection(
        configuration: &ConfigurationFile,
    ) -> podmesh_manager_ha_lab::durable::CanonicalStoreInspection {
        inspect_read_only(
            &configuration.database_path,
            &configuration.manager,
            &configuration.replica_id,
        )
        .unwrap()
    }

    #[test]
    fn exact_configured_peer_replay_and_stale_catch_up_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        let r3 = configuration(&directory, "r3", &addresses);
        observe(&r1, "origin-observe");

        let (r2_address, r2_server) = spawn_ready_server(r2.clone(), false, None);
        let (r3_address, r3_server) = spawn_ready_server(r3.clone(), false, None);
        point_peer(&mut r1, "r2", r2_address);
        point_peer(&mut r1, "r3", r3_address);
        let first = r1
            .open()
            .unwrap()
            .sync_to("r2", "op-one", "nonce-one")
            .unwrap();
        assert_eq!(first.inserted, 1);
        assert!(!first.replayed);
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

        let (replay_address, replay_server) = spawn_ready_server(r2.clone(), false, None);
        point_peer(&mut r1, "r2", replay_address);
        let replay = r1
            .open()
            .unwrap()
            .sync_to("r2", "op-one", "nonce-one-retry")
            .unwrap();
        assert_eq!(replay.inserted, 1);
        assert!(replay.replayed);
        assert_eq!(replay.receipt_operation_id, first.receipt_operation_id);
        assert_eq!(replay.receipt_sha256, first.receipt_sha256);
        assert!(replay_server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 1);
        let destination = inspection(&r2);
        assert_eq!(
            destination.unaudited_import_receipt_ids,
            Vec::<String>::new()
        );
        assert_eq!(destination.incomplete_attempts, Vec::new());
        assert_eq!(destination.audit_event_count, 8);
        assert!(destination.ordered_audit_events.iter().any(|evidence| {
            evidence.event.phase == AuditPhase::InboundReplyWriteObserved
                && evidence.event.replayed
                && evidence.event.local_receipt_operation_id.as_deref()
                    == Some(replay.receipt_operation_id.as_str())
        }));
    }

    #[test]
    fn lost_reply_after_commit_retries_exact_operation_with_fresh_attempt_and_nonce() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        observe(&r1, "lost-reply-fact");

        let (first_address, first_server) = spawn_ready_server(r2.clone(), true, None);
        point_peer(&mut r1, "r2", first_address);
        let first_error = r1
            .open()
            .unwrap()
            .sync_to("r2", "lost-reply-operation", "lost-reply-nonce-one")
            .unwrap_err();
        assert_eq!(first_error.category(), ErrorCategory::Unavailable);
        assert!(first_server.join().unwrap().is_err());
        assert_eq!(history_len(&r2), 1);
        let source_after_loss = inspection(&r1);
        assert_eq!(source_after_loss.incomplete_attempts.len(), 1);
        assert_eq!(
            source_after_loss.incomplete_attempts[0].last_phase,
            AuditPhase::OutboundRequestPrepared
        );
        assert_eq!(source_after_loss.audit_event_count, 1);
        let after_loss = inspection(&r2);
        assert_eq!(after_loss.incomplete_attempts.len(), 1);
        assert_eq!(
            after_loss.incomplete_attempts[0].last_phase,
            AuditPhase::InboundImportCommitted
        );

        let (retry_address, retry_server) = spawn_ready_server(r2.clone(), false, None);
        point_peer(&mut r1, "r2", retry_address);
        let replay = r1
            .open()
            .unwrap()
            .sync_to("r2", "lost-reply-operation", "lost-reply-nonce-two")
            .unwrap();
        assert!(replay.replayed);
        assert!(retry_server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 1);
        let final_inspection = inspection(&r2);
        assert_eq!(final_inspection.incomplete_attempts.len(), 1);
        assert_eq!(
            final_inspection.unaudited_import_receipt_ids,
            Vec::<String>::new()
        );
        assert_eq!(
            final_inspection
                .ordered_receipts
                .iter()
                .filter(|receipt| receipt.operation_id == replay.receipt_operation_id)
                .count(),
            1
        );
    }

    #[test]
    fn changed_snapshot_under_same_wire_operation_is_authenticated_refusal() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        observe(&r1, "first-content");
        let (first_address, first_server) = spawn_ready_server(r2.clone(), false, None);
        point_peer(&mut r1, "r2", first_address);
        r1.open()
            .unwrap()
            .sync_to("r2", "stable-operation", "first-nonce")
            .unwrap();
        assert!(first_server.join().unwrap().is_ok());

        observe(&r1, "changed-content");
        let (second_address, second_server) = spawn_ready_server(r2.clone(), false, None);
        point_peer(&mut r1, "r2", second_address);
        let error = r1
            .open()
            .unwrap()
            .sync_to("r2", "stable-operation", "fresh-nonce")
            .unwrap_err();
        assert_eq!(error.source(), ErrorSource::AuthenticatedRemoteRefusal);
        assert!(second_server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 1);
        assert!(inspection(&r2).ordered_audit_events.iter().any(|evidence| {
            evidence.event.phase == AuditPhase::InboundRefusalRecorded
                && evidence.event.reason_code == Some(RefusalReason::OperationIdReused)
        }));
        assert!(inspection(&r1).ordered_audit_events.iter().any(|evidence| {
            evidence.event.phase == AuditPhase::OutboundExchangeCompleted
                && evidence.event.outcome == AuditOutcome::AuthenticatedRefusal
                && evidence.event.reason_code == Some(RefusalReason::OperationIdReused)
        }));
    }

    #[test]
    fn outbound_terminal_audit_failure_returns_local_uncertainty_after_signed_success() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        observe(&r1, "uncertain-fact");
        let (address, server) = spawn_ready_server(r2.clone(), false, None);
        point_peer(&mut r1, "r2", address);
        let mut source = r1.open().unwrap();
        source.fail_next_audit_at(AuditPhase::OutboundExchangeCompleted);
        let error = source
            .sync_to("r2", "uncertain-operation", "uncertain-nonce")
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Unavailable);
        assert!(error.to_string().contains("uncertain"));
        assert!(server.join().unwrap().is_ok());
        let source_inspection = inspection(&r1);
        assert_eq!(source_inspection.incomplete_attempts.len(), 1);
        assert_eq!(
            source_inspection.incomplete_attempts[0].last_phase,
            AuditPhase::OutboundRequestPrepared
        );
        assert_eq!(history_len(&r2), 1);
    }

    #[test]
    fn preparation_and_destination_store_prevalidation_failures_block_success() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        observe(&r1, "blocked-success-fact");
        let mut source = r1.open().unwrap();
        source.fail_next_audit_at(AuditPhase::OutboundRequestPrepared);
        assert!(source
            .sync_to("r2", "blocked-before-connect", "blocked-before-nonce")
            .is_err());
        assert_eq!(inspection(&r1).audit_event_count, 0);

        let (address, server) =
            spawn_ready_server(r2.clone(), false, Some(AuditPhase::InboundImportCommitted));
        point_peer(&mut r1, "r2", address);
        let error = r1
            .open()
            .unwrap()
            .sync_to("r2", "destination-audit-fails", "destination-failure-nonce")
            .unwrap_err();
        assert_ne!(error.source(), ErrorSource::AuthenticatedRemoteRefusal);
        assert!(server.join().unwrap().is_err());
        assert_eq!(history_len(&r2), 0);
        let destination = inspection(&r2);
        assert_eq!(destination.audit_event_count, 4);
        assert!(destination.ordered_receipts.is_empty());
        assert_eq!(destination.incomplete_attempts, Vec::new());
        assert!(destination.ordered_audit_events.iter().any(|evidence| {
            evidence.event.phase == AuditPhase::InboundConnectionClosed
                && evidence.event.authenticated_peer_id.as_deref() == Some("r1")
                && evidence.event.operation_id.as_deref() == Some("destination-audit-fails")
                && evidence.event.outcome == AuditOutcome::Unavailable
        }));
    }

    #[test]
    fn destination_reply_preparation_failure_sends_no_signed_success() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        observe(&r1, "reply-preparation-fact");
        let (address, server) =
            spawn_ready_server(r2.clone(), false, Some(AuditPhase::InboundReplyPrepared));
        point_peer(&mut r1, "r2", address);
        let error = r1
            .open()
            .unwrap()
            .sync_to("r2", "reply-preparation-fails", "reply-preparation-nonce")
            .unwrap_err();
        assert_ne!(error.source(), ErrorSource::AuthenticatedRemoteRefusal);
        assert!(server.join().unwrap().is_err());
        assert_eq!(history_len(&r2), 1);
        let destination = inspection(&r2);
        assert_eq!(destination.audit_event_count, 2);
        assert_eq!(destination.incomplete_attempts.len(), 1);
        assert_eq!(
            destination.incomplete_attempts[0].last_phase,
            AuditPhase::InboundImportCommitted
        );
    }

    #[test]
    fn post_decision_reply_construction_failures_remain_honestly_incomplete() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        observe(&r1, "post-auth-close-fact");
        for (index, failure) in [
            PostAuthFailure::MissingReceipt,
            PostAuthFailure::SignReply,
            PostAuthFailure::EncodeReply,
        ]
        .into_iter()
        .enumerate()
        {
            let (address, server) = spawn_ready_server_with_post_auth_failure(r2.clone(), failure);
            point_peer(&mut r1, "r2", address);
            let error = r1
                .open()
                .unwrap()
                .sync_to(
                    "r2",
                    &format!("post-auth-close-{index}"),
                    &format!("post-auth-close-nonce-{index}"),
                )
                .unwrap_err();
            assert_eq!(error.category(), ErrorCategory::Unavailable);
            let server_error = server.join().unwrap().unwrap_err();
            match failure {
                PostAuthFailure::MissingReceipt => {
                    assert_eq!(server_error.category(), ErrorCategory::Refused);
                }
                PostAuthFailure::SignReply | PostAuthFailure::EncodeReply => {
                    assert_eq!(server_error.category(), ErrorCategory::Malformed);
                }
            }
        }
        let destination = inspection(&r2);
        assert_eq!(destination.audit_event_count, 6);
        assert_eq!(destination.incomplete_attempts.len(), 3);
        assert_eq!(
            destination.unaudited_import_receipt_ids,
            Vec::<String>::new()
        );
        assert!(destination
            .incomplete_attempts
            .iter()
            .all(|attempt| { attempt.last_phase == AuditPhase::InboundImportCommitted }));
        assert!(destination
            .ordered_audit_events
            .iter()
            .all(|evidence| { evidence.event.phase != AuditPhase::InboundConnectionClosed }));
    }

    #[test]
    fn durable_error_variants_keep_distinct_local_categories_and_refusal_allowlist() {
        assert_eq!(
            Error::durable(DurableError::Refused(RefusalReason::PolicyViolation)).category(),
            ErrorCategory::Refused
        );
        assert_eq!(
            Error::durable(DurableError::Corrupt("corrupt".into())).category(),
            ErrorCategory::Refused
        );
        assert_eq!(
            Error::durable(DurableError::Storage("storage".into())).category(),
            ErrorCategory::Unavailable
        );
        assert_eq!(
            Error::durable(DurableError::InvalidAudit("audit".into())).category(),
            ErrorCategory::Malformed
        );
        assert!(signable_reason(RefusalReason::InvalidRequest));
        assert!(signable_reason(RefusalReason::PolicyViolation));
        assert!(signable_reason(RefusalReason::OperationIdReused));
        assert!(!signable_reason(RefusalReason::TransportUnavailable));
        assert!(!signable_reason(RefusalReason::UnsafeStore));
        assert!(!signable_reason(RefusalReason::IdentityMismatch));
    }

    #[test]
    fn wrong_peer_malformed_and_oversize_requests_do_not_mutate() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
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
        let (address, server) = spawn_connection_server(r2.clone());
        let mut stream = connect(&address).unwrap();
        write_frame(&mut stream, &encode(&wrong).unwrap()).unwrap();
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
        let audit = inspection(&r2);
        assert!(audit.ordered_audit_events.iter().any(|evidence| {
            evidence.event.phase == AuditPhase::InboundRequestObserved
                && evidence.event.operation_id.as_deref() == Some("wrong-peer")
                && evidence.event.authenticated_peer_id.is_none()
                && evidence.event.outcome == AuditOutcome::Malformed
                && evidence.event.error_category == Some(AuditErrorCategory::Malformed)
        }));

        let (address, server) = spawn_connection_server(r2.clone());
        let mut stream = connect(&address).unwrap();
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

        let (address, server) = spawn_connection_server(r2.clone());
        let mut stream = connect(&address).unwrap();
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
    fn unknown_peer_receives_only_unsigned_diagnostic_without_authority() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        let Response::Snapshot { snapshot } = r1
            .open()
            .unwrap()
            .store
            .execute(&Request::Export {})
            .unwrap()
        else {
            unreachable!();
        };
        let unknown = sign_request(
            WireRequest {
                protocol: "podmesh-manager-network-lab/1".into(),
                source_replica_id: "r9".into(),
                destination_replica_id: "r2".into(),
                operation_id: "unknown-peer".into(),
                nonce: "unknown-peer-nonce".into(),
                snapshot,
                mac_hex: String::new(),
            },
            WRONG_KEY,
        )
        .unwrap();
        let (address, server) = spawn_ready_server(r2.clone(), false, None);
        let mut stream = connect(&address).unwrap();
        write_frame(&mut stream, &encode(&unknown).unwrap()).unwrap();
        let reply: WireReply = decode(&read_frame(&mut stream).unwrap()).unwrap();
        assert!(matches!(reply, WireReply::Diagnostic { .. }));
        drop(stream);
        point_peer(&mut r1, "r2", address);
        r1.open()
            .unwrap()
            .sync_to("r2", "valid-after-unknown", "valid-after-unknown-nonce")
            .unwrap();
        assert!(server.join().unwrap().is_ok());
        let audit = inspection(&r2);
        assert_eq!(audit.audit_event_count, 6);
        assert!(audit.ordered_audit_events.iter().any(|evidence| evidence
            .event
            .peer_claim
            .as_deref()
            == Some("r9")));
        assert!(audit.ordered_audit_events.iter().any(|evidence| {
            evidence.event.phase == AuditPhase::InboundReplyWriteObserved
                && evidence.event.authenticated_peer_id.as_deref() == Some("r1")
        }));
        assert_eq!(audit.unaudited_import_receipt_ids, Vec::<String>::new());
        assert_eq!(history_len(&r2), 0);
    }

    #[test]
    fn one_shot_listener_stops_at_the_unauthenticated_connection_cap() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let r2 = configuration(&directory, "r2", &addresses);
        let (address, server) = spawn_ready_server(r2.clone(), false, None);
        for _ in 0..MAX_CONNECTIONS_PER_PROCESS {
            let mut stream = connect(&address).unwrap();
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
        }
        let error = server.join().unwrap().unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Unavailable);
        let audit = inspection(&r2);
        assert_eq!(audit.audit_event_count, MAX_CONNECTIONS_PER_PROCESS * 2);
        assert!(audit.ordered_audit_events.iter().all(|evidence| {
            evidence.event.authenticated_peer_id.is_none()
                && evidence.event.local_receipt_operation_id.is_none()
        }));
    }

    #[test]
    fn admission_deadline_rejects_a_queued_connection_after_slow_first_exchange() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        let Response::Snapshot { snapshot } = r1
            .open()
            .unwrap()
            .store
            .execute(&Request::Export {})
            .unwrap()
        else {
            unreachable!();
        };
        let queued_request = sign_request(
            WireRequest {
                protocol: "podmesh-manager-network-lab/1".into(),
                source_replica_id: "r1".into(),
                destination_replica_id: "r2".into(),
                operation_id: "must-not-be-admitted".into(),
                nonce: "must-not-be-admitted-nonce".into(),
                snapshot,
                mac_hex: String::new(),
            },
            &key_for(1, 2),
        )
        .unwrap();
        let (address, server) = spawn_ready_server(r2.clone(), false, None);

        let mut slow = connect(&address).unwrap();
        slow.write_all(&100_u32.to_be_bytes()).unwrap();
        slow.write_all(b"x").unwrap();

        let mut queued = connect(&address).unwrap();
        write_frame(&mut queued, &encode(&queued_request).unwrap()).unwrap();

        let server_error = server.join().unwrap().unwrap_err();
        assert_eq!(server_error.category(), ErrorCategory::Unavailable);
        assert!(read_frame(&mut queued).is_err());
        let audit = inspection(&r2);
        assert_eq!(audit.audit_event_count, 2);
        assert!(audit.ordered_audit_events.iter().all(|evidence| {
            evidence.event.operation_id.as_deref() != Some("must-not-be-admitted")
                && evidence.event.authenticated_peer_id.is_none()
        }));
        assert_eq!(history_len(&r2), 0);
    }

    #[test]
    fn invalid_signed_snapshot_is_refused_before_any_durable_import() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
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
        let (address, server) = spawn_connection_server(r2.clone());
        let mut stream = connect(&address).unwrap();
        write_frame(&mut stream, &encode(&request).unwrap()).unwrap();
        let reply: WireReply = decode(&read_frame(&mut stream).unwrap()).unwrap();
        assert!(matches!(
            &reply,
            WireReply::Refused {
                reason: RefusalReason::PolicyViolation,
                ..
            }
        ));
        verify_reply(&reply, &key_for(1, 2)).unwrap();
        let WireReply::Refused { request_sha256, .. } = &reply else {
            unreachable!();
        };
        assert_eq!(request_sha256, &sha256(&encode(&request).unwrap()));
        assert!(server.join().unwrap().is_ok());
        assert_eq!(history_len(&r2), 0);
    }

    #[test]
    fn unavailable_peer_is_distinct_from_refusal() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
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
    fn sync_to_exposes_wrong_key_as_unsigned_malformed_diagnostic() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
        let mut r1 = configuration(&directory, "r1", &addresses);
        let r2 = configuration(&directory, "r2", &addresses);
        r1.peers
            .iter_mut()
            .find(|peer| peer.replica_id == "r2")
            .unwrap()
            .shared_key_hex = WRONG_KEY.into();
        let (address, server) = spawn_ready_server(r2, false, None);
        point_peer(&mut r1, "r2", address);
        let error = r1
            .open()
            .unwrap()
            .sync_to("r2", "wrong-key-sync", "wrong-key-nonce")
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Malformed, "{error}");
        assert_eq!(
            error.source(),
            ErrorSource::UnauthenticatedRemoteDiagnostic,
            "{error}"
        );
        assert!(server.join().unwrap().is_err());
    }

    #[test]
    fn duplicate_pair_keys_are_refused_before_opening_the_store() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = placeholder_addresses();
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
