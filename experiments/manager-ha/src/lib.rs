//! Deterministic laboratory model for a replicated logical `PodMesh` manager.
//!
//! This crate models merge and activation gates. It has no networking, durable
//! storage, authentication, failure detector, fencing, DNS or Podman adapter.

use std::collections::{BTreeMap, BTreeSet};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaConfig {
    pub replica_id: String,
    pub host_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeGrant {
    pub scope: String,
    pub owner_replica_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Topology {
    logical_manager_id: String,
    replicas: BTreeMap<String, ReplicaConfig>,
    scope_owners: BTreeMap<String, String>,
}

impl Topology {
    /// Builds a declared laboratory topology.
    ///
    /// # Errors
    ///
    /// Returns an error for missing or duplicate identities, reused hosts,
    /// unknown scope owners, or duplicate scope grants.
    pub fn new(
        logical_manager_id: impl Into<String>,
        replicas: Vec<ReplicaConfig>,
        grants: Vec<ScopeGrant>,
    ) -> Result<Self> {
        let logical_manager_id = logical_manager_id.into();
        if logical_manager_id.is_empty() || replicas.is_empty() {
            return Err("logical manager identity and replicas are required".into());
        }

        let mut by_id = BTreeMap::new();
        let mut hosts = BTreeSet::new();
        for replica in replicas {
            if replica.replica_id.is_empty()
                || replica.host_id.is_empty()
                || by_id.contains_key(&replica.replica_id)
                || !hosts.insert(replica.host_id.clone())
            {
                return Err("replica and host identities must be non-empty and unique".into());
            }
            by_id.insert(replica.replica_id.clone(), replica);
        }

        let mut scope_owners: BTreeMap<String, String> = BTreeMap::new();
        for grant in grants {
            if grant.scope.is_empty()
                || !by_id.contains_key(&grant.owner_replica_id)
                || scope_owners
                    .keys()
                    .any(|existing| scopes_overlap(existing, &grant.scope))
            {
                return Err("every scope must have exactly one known replica owner".into());
            }
            scope_owners.insert(grant.scope, grant.owner_replica_id);
        }

        Ok(Self {
            logical_manager_id,
            replicas: by_id,
            scope_owners,
        })
    }

    #[must_use]
    pub fn logical_manager_id(&self) -> &str {
        &self.logical_manager_id
    }

    #[must_use]
    pub fn replica_count(&self) -> usize {
        self.replicas.len()
    }

    /// Creates an empty replica from one declared topology entry.
    ///
    /// # Errors
    ///
    /// Returns an error when the replica identity is not declared.
    pub fn instantiate(&self, replica_id: &str) -> Result<Replica> {
        let config = self
            .replicas
            .get(replica_id)
            .ok_or_else(|| "unknown replica".to_string())?
            .clone();
        Ok(Replica {
            topology: self.clone(),
            config,
            next_sequence: 1,
            history: BTreeMap::new(),
        })
    }
}

fn scopes_overlap(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    pub event_id: String,
    pub logical_manager_id: String,
    pub origin_replica_id: String,
    pub origin_host_id: String,
    pub producer_sequence: u64,
    pub scope: String,
    pub subject: String,
    pub subject_revision: u64,
    pub predecessor: Option<String>,
    pub exclusive_resource: Option<String>,
    pub active_claim: bool,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct Replica {
    topology: Topology,
    config: ReplicaConfig,
    next_sequence: u64,
    history: BTreeMap<String, Fact>,
}

impl Replica {
    #[must_use]
    pub fn replica_id(&self) -> &str {
        &self.config.replica_id
    }

    #[must_use]
    pub fn host_id(&self) -> &str {
        &self.config.host_id
    }

    #[must_use]
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    #[must_use]
    pub fn history_ids(&self) -> Vec<&str> {
        self.history.keys().map(String::as_str).collect()
    }

    /// Appends a local fact inside this replica's declared scope.
    ///
    /// # Errors
    ///
    /// Returns an error for an unowned scope, invalid fields, a forked local
    /// subject, or exhausted producer sequence.
    pub fn observe(
        &mut self,
        scope: &str,
        subject: &str,
        exclusive_resource: Option<&str>,
        active_claim: bool,
        value: &str,
    ) -> Result<Fact> {
        if self.topology.scope_owners.get(scope).map(String::as_str) != Some(self.replica_id()) {
            return Err("replica does not own this disconnected-write scope".into());
        }
        if subject.is_empty() || value.is_empty() {
            return Err("subject and value are required".into());
        }
        if active_claim && exclusive_resource.is_none() {
            return Err("an active claim requires an exclusive resource".into());
        }

        let current = self.local_subject_head(scope, subject)?;
        let subject_revision = current.map_or(1, |fact| fact.subject_revision + 1);
        let predecessor = current.map(|fact| fact.event_id.clone());
        let event_id = format!("{}:{:020}", self.replica_id(), self.next_sequence);
        let fact = Fact {
            event_id,
            logical_manager_id: self.topology.logical_manager_id.clone(),
            origin_replica_id: self.config.replica_id.clone(),
            origin_host_id: self.config.host_id.clone(),
            producer_sequence: self.next_sequence,
            scope: scope.to_string(),
            subject: subject.to_string(),
            subject_revision,
            predecessor,
            exclusive_resource: exclusive_resource.map(str::to_string),
            active_claim,
            value: value.to_string(),
        };
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| "producer sequence exhausted".to_string())?;
        self.ingest(fact.clone())?;
        Ok(fact)
    }

    /// Imports one immutable fact while preserving its original identity.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid provenance, scope, identity, structure, or
    /// an event-identity collision with different content.
    pub fn ingest(&mut self, fact: Fact) -> Result<()> {
        validate_fact(&self.topology, &fact)?;
        if let Some(existing) = self.history.get(&fact.event_id) {
            return if existing == &fact {
                Ok(())
            } else {
                Err("event identity collision with different bytes".into())
            };
        }
        if fact.origin_replica_id == self.replica_id() {
            self.next_sequence = self.next_sequence.max(
                fact.producer_sequence
                    .checked_add(1)
                    .ok_or_else(|| "producer sequence exhausted".to_string())?,
            );
        }
        self.history.insert(fact.event_id.clone(), fact);
        Ok(())
    }

    /// Exchanges the complete immutable histories of two laboratory replicas.
    ///
    /// # Errors
    ///
    /// Returns an error for different topologies or a refused imported fact.
    pub fn exchange_with(&mut self, peer: &mut Self) -> Result<()> {
        if self.topology != peer.topology {
            return Err("replicas do not share the same declared topology".into());
        }
        let left: Vec<_> = self.history.values().cloned().collect();
        let right: Vec<_> = peer.history.values().cloned().collect();
        let mut next_self = self.clone();
        let mut next_peer = peer.clone();
        for fact in right {
            next_self.ingest(fact)?;
        }
        for fact in left {
            next_peer.ingest(fact)?;
        }
        *self = next_self;
        *peer = next_peer;
        Ok(())
    }

    #[must_use]
    pub fn materialize(&self) -> View {
        materialize(&self.history)
    }

    fn local_subject_head(&self, scope: &str, subject: &str) -> Result<Option<&Fact>> {
        let mut matching: Vec<_> = self
            .history
            .values()
            .filter(|fact| {
                fact.origin_replica_id == self.replica_id()
                    && fact.scope == scope
                    && fact.subject == subject
            })
            .collect();
        matching.sort_by_key(|fact| (fact.subject_revision, fact.event_id.as_str()));
        for (index, fact) in matching.iter().enumerate() {
            let expected_revision =
                u64::try_from(index + 1).map_err(|_| "subject revision exhausted".to_string())?;
            let expected_predecessor = index
                .checked_sub(1)
                .and_then(|previous| matching.get(previous))
                .map(|previous| previous.event_id.as_str());
            if fact.subject_revision != expected_revision
                || fact.predecessor.as_deref() != expected_predecessor
            {
                return Err(
                    "subject history is incomplete or forked; further writes are blocked".into(),
                );
            }
        }
        Ok(matching.last().copied())
    }
}

fn validate_fact(topology: &Topology, fact: &Fact) -> Result<()> {
    let Some(origin) = topology.replicas.get(&fact.origin_replica_id) else {
        return Err("fact origin is not a declared replica".into());
    };
    if fact.logical_manager_id != topology.logical_manager_id
        || origin.host_id != fact.origin_host_id
        || topology.scope_owners.get(&fact.scope).map(String::as_str)
            != Some(fact.origin_replica_id.as_str())
        || fact.event_id != format!("{}:{:020}", fact.origin_replica_id, fact.producer_sequence)
        || fact.producer_sequence == 0
        || fact.subject_revision == 0
        || fact.subject.is_empty()
        || fact.value.is_empty()
        || (fact.active_claim && fact.exclusive_resource.is_none())
    {
        return Err("invalid or out-of-scope fact".into());
    }
    if (fact.subject_revision == 1) != fact.predecessor.is_none() {
        return Err("first revision and predecessor relation disagree".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubjectKey {
    pub scope: String,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    MissingPredecessor,
    SubjectFork,
    ExclusiveResource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub kind: ConflictKind,
    pub resource: String,
    pub event_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub current: BTreeMap<SubjectKey, Fact>,
    pub blocked_subjects: BTreeSet<SubjectKey>,
    pub blocked_exclusive_resources: BTreeSet<String>,
    pub conflicts: Vec<Conflict>,
}

impl View {
    #[must_use]
    pub fn exclusive_resource_blocked(&self, resource: &str) -> bool {
        self.blocked_exclusive_resources.contains(resource)
    }
}

fn materialize(history: &BTreeMap<String, Fact>) -> View {
    let mut by_subject: BTreeMap<SubjectKey, Vec<&Fact>> = BTreeMap::new();
    for fact in history.values() {
        by_subject
            .entry(SubjectKey {
                scope: fact.scope.clone(),
                subject: fact.subject.clone(),
            })
            .or_default()
            .push(fact);
    }

    let mut view = View::default();
    for (key, mut facts) in by_subject {
        facts.sort_by_key(|fact| (fact.subject_revision, fact.event_id.as_str()));
        let mut reason = None;
        for fact in &facts {
            if fact.subject_revision > 1 {
                let valid_predecessor = fact.predecessor.as_ref().and_then(|id| history.get(id));
                if !valid_predecessor.is_some_and(|previous| {
                    previous.scope == fact.scope
                        && previous.subject == fact.subject
                        && previous.origin_replica_id == fact.origin_replica_id
                        && previous.subject_revision + 1 == fact.subject_revision
                }) {
                    reason = Some(ConflictKind::MissingPredecessor);
                    break;
                }
            }
        }
        if reason.is_none()
            && facts
                .windows(2)
                .any(|pair| pair[0].subject_revision == pair[1].subject_revision)
        {
            reason = Some(ConflictKind::SubjectFork);
        }
        if let Some(kind) = reason {
            view.blocked_subjects.insert(key.clone());
            view.blocked_exclusive_resources.extend(
                facts
                    .iter()
                    .filter_map(|fact| fact.exclusive_resource.clone()),
            );
            view.conflicts.push(Conflict {
                kind,
                resource: format!("subject:{}:{}", key.scope, key.subject),
                event_ids: facts.iter().map(|fact| fact.event_id.clone()).collect(),
            });
        } else if let Some(current) = facts.last() {
            view.current.insert(key, (*current).clone());
        }
    }

    let mut claims: BTreeMap<String, Vec<(SubjectKey, String)>> = BTreeMap::new();
    for (subject, fact) in &view.current {
        if fact.active_claim {
            if let Some(resource) = &fact.exclusive_resource {
                claims
                    .entry(resource.clone())
                    .or_default()
                    .push((subject.clone(), fact.event_id.clone()));
            }
        }
    }
    for (resource, claimants) in claims {
        if claimants.len() > 1 {
            let mut event_ids = Vec::new();
            let mut blocked = Vec::new();
            for (subject, event_id) in claimants {
                view.blocked_subjects.insert(subject.clone());
                blocked.push(subject);
                event_ids.push(event_id);
            }
            for subject in blocked {
                view.current.remove(&subject);
            }
            event_ids.sort();
            view.blocked_exclusive_resources.insert(resource.clone());
            view.conflicts.push(Conflict {
                kind: ConflictKind::ExclusiveResource,
                resource,
                event_ids,
            });
        }
    }
    view.conflicts.sort_by(|left, right| {
        (&left.resource, &left.event_ids).cmp(&(&right.resource, &right.event_ids))
    });
    view
}

#[derive(Clone, Debug)]
pub struct Reconciliation {
    coordinator_replica_id: String,
    history_ids: Vec<String>,
    view: View,
}

impl Reconciliation {
    /// Produces a reconciled view only after all declared copies converge.
    ///
    /// # Errors
    ///
    /// Returns an error when a replica is missing, duplicated, belongs to a
    /// different topology, or has not exchanged the same immutable history.
    pub fn after_full_exchange(topology: &Topology, replicas: &[&Replica]) -> Result<Self> {
        if replicas.len() != topology.replica_count() {
            return Err(
                "all declared replicas must participate in laboratory reconciliation".into(),
            );
        }
        let mut seen = BTreeSet::new();
        let Some(first) = replicas.first() else {
            return Err("no replicas supplied".into());
        };
        let baseline = &first.history;
        for replica in replicas {
            if replica.topology != *topology
                || !seen.insert(replica.replica_id())
                || replica.history != *baseline
            {
                return Err("histories must be exchanged and identical before coordination".into());
            }
        }
        let coordinator_replica_id = (*seen
            .iter()
            .next()
            .ok_or_else(|| "no coordinator candidate".to_string())?)
        .to_string();
        Ok(Self {
            coordinator_replica_id,
            history_ids: baseline.keys().cloned().collect(),
            view: materialize(baseline),
        })
    }

    #[must_use]
    pub fn coordinator_replica_id(&self) -> &str {
        &self.coordinator_replica_id
    }

    #[must_use]
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Creates a modeled permit for one conflict-free exclusive service.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested replica is not the reconciled
    /// coordinator or the named exclusive resource has competing active claims.
    pub fn authorize_exclusive_service(
        &self,
        requested_replica_id: &str,
        exclusive_service: &str,
    ) -> Result<ServicePermit> {
        if requested_replica_id != self.coordinator_replica_id {
            return Err("only the reconciled coordinator is eligible".into());
        }
        if self.view.exclusive_resource_blocked(exclusive_service) {
            return Err("exclusive service is blocked by conflicting facts".into());
        }
        let supporting_fact = self
            .view
            .current
            .values()
            .find(|fact| {
                fact.active_claim
                    && fact.exclusive_resource.as_deref() == Some(exclusive_service)
                    && fact.origin_replica_id == requested_replica_id
            })
            .ok_or_else(|| {
                "coordinator has no reconciled active claim for this exclusive service".to_string()
            })?;
        Ok(ServicePermit {
            replica_id: requested_replica_id.to_string(),
            exclusive_service: exclusive_service.to_string(),
            supporting_event_id: supporting_fact.event_id.clone(),
            reconciled_history_ids: self.history_ids.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServicePermit {
    replica_id: String,
    exclusive_service: String,
    supporting_event_id: String,
    reconciled_history_ids: Vec<String>,
}

impl Replica {
    #[must_use]
    pub fn advertises(&self, service: &str, permit: Option<&ServicePermit>) -> bool {
        permit.is_some_and(|permit| {
            permit.replica_id == self.replica_id()
                && permit.exclusive_service == service
                && self.history.contains_key(&permit.supporting_event_id)
                && self.history.keys().eq(permit.reconciled_history_ids.iter())
        })
    }
}
