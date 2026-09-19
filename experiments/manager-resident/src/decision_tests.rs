//! Tests of the voters' rules (V3-5, `decisions.rs`): the view a store's votes prove, and each rule a
//! proposal is checked by, one refusal at a time. Test-only keys from a one-byte seed.
use crate::decisions::{self, Baseline, ResourceRules, View, ViewSource};
use crate::quorum::{testkit, Quorum, QUORUM_PROOF_KIND};
use crate::vote::{self, LedgerStamp};
use serde_json::{json, Value};

const R: &str = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
const N0: &str = "0a0a0a0a-0000-4000-8000-000000000000";
const N1: &str = "1b1b1b1b-1111-4111-8111-111111111111";
const STRANGER: &str = "9c9c9c9c-9999-4999-8999-999999999999";
const BOOT: &str = "0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b";
const ABC: &[(&str, u8)] = &[("replica-a", 1), ("replica-b", 2), ("replica-c", 3)];
const NOW: i64 = 1_800_000_000;
const LIFE: i64 = 60;
const LEASE: i64 = 20;
const MARGIN: i64 = 5;
const RENEWAL: i64 = NOW + 500;

fn policy() -> Quorum {
    Quorum::declared("replicas", &testkit::policy(2, ABC)).unwrap()
}

fn nodes() -> Vec<String> {
    vec![N0.to_string(), N1.to_string()]
}

fn rules() -> ResourceRules {
    ResourceRules {
        resource: R.into(),
        lease_seconds: LEASE,
        takeover_margin_seconds: MARGIN,
        renewal_not_after: RENEWAL,
        baseline: None,
    }
}

/// A takeover payload of epoch `epoch` from `previous` to `holder`, by `method`, live from `at`.
fn payload(
    epoch: i64,
    previous: Option<&str>,
    holder: &str,
    method: &str,
    eligible: i64,
    at: i64,
) -> Value {
    json!({"kind": QUORUM_PROOF_KIND, "authority_id": "replicas", "policy_digest": policy().digest(), "resource": R,
           "new_epoch": epoch, "previous_epoch": epoch - 1, "new_holder": holder, "previous_holder": previous,
           "holder_boot_id": BOOT, "grant_id": format!("g{epoch}"), "method": method,
           "eligible_after": eligible, "issued_at": at, "expires_at": at + LIFE})
}

/// The votes of `signers` for `payload`, sealed as their ledgers would.
fn votes(payload: &Value, signers: &[(&str, u8)]) -> Vec<Value> {
    signers
        .iter()
        .map(|(id, seed)| {
            vote::seal(
                &testkit::key(*seed),
                id,
                payload,
                &LedgerStamp {
                    nonce: "00",
                    sequence: 1,
                },
            )
            .unwrap()
        })
        .collect()
}

fn check(view: &View, p: &Value) -> Result<(), &'static str> {
    decisions::check(&policy(), &rules(), &nodes(), view, p, NOW, LIFE)
        .map(|_| ())
        .map_err(|e| e.code)
}

/// Proves: the view is the highest certificate the store's votes assemble into, with its holder, its
/// barrier and its expiry; one vote is no certificate and moves nothing; a baseline stands until a
/// certificate passes it; two certified decisions for one epoch are named as a conflict.
#[test]
fn the_view_is_the_highest_certificate_or_the_baseline() {
    let q = policy();
    let empty = decisions::view(&q, &rules(), &[]);
    assert_eq!(
        (empty.epoch, empty.holder.clone(), empty.source),
        (0, None, ViewSource::None)
    );
    let first = payload(1, None, N0, "first", NOW - 10, NOW - 10);
    let second = payload(2, Some(N0), N0, "same_holder", NOW - 5, NOW - 5);
    let mut all = votes(&first, &ABC[..2]);
    all.extend(votes(&second, &ABC[2..]));
    let v = decisions::view(&q, &rules(), &decisions::verified(&q, &all));
    assert_eq!(
        (v.epoch, v.holder.as_deref(), v.source),
        (1, Some(N0), ViewSource::Certificate),
        "one vote moved the view"
    );
    all.extend(votes(&second, &ABC[..1]));
    let v = decisions::view(&q, &rules(), &decisions::verified(&q, &all));
    assert_eq!(
        (v.epoch, v.barrier, v.expires_at),
        (2, NOW - 5, NOW - 5 + LIFE)
    );
    q.verify(
        v.certificate.as_ref().unwrap(),
        QUORUM_PROOF_KIND,
        crate::quorum::TAKEOVER_FIELDS,
    )
    .unwrap();
    let mut with_baseline = rules();
    with_baseline.baseline = Some(Baseline {
        epoch: 157,
        holder: N1.into(),
        eligible_after: NOW - 100,
    });
    let v = decisions::view(&q, &with_baseline, &decisions::verified(&q, &all));
    assert_eq!(
        (v.epoch, v.holder.as_deref(), v.barrier, v.source),
        (157, Some(N1), NOW - 100, ViewSource::Baseline)
    );
    // Two decisions certified at epoch 3: only signatures made outside a ledger can do that.
    all.extend(votes(
        &payload(3, Some(N0), N0, "same_holder", NOW, NOW),
        &ABC[..2],
    ));
    all.extend(votes(
        &payload(3, Some(N0), N1, "lease_barrier", NOW + 900, NOW),
        &ABC[1..],
    ));
    let v = decisions::view(&q, &rules(), &decisions::verified(&q, &all));
    assert_eq!(v.conflicts, [3]);
    assert_eq!(
        check(&v, &payload(4, Some(N0), N0, "same_holder", NOW, NOW)),
        Err("conflict_in_view")
    );
}

/// Proves each rule a voter checks, by its refusal, and that the proposal which keeps them all passes:
/// the kind, no unbound field, the policy, the resource, the life, the holder among the nodes, the
/// identifiers, the barrier before the expiry, the next epoch after the view's with the view's holder,
/// `first` only before any epoch, `same_holder` only to the current holder, `fence_receipt` never, an
/// unknown method never, and the barrier each method requires.
#[test]
fn a_proposal_is_checked_against_the_view_rule_by_rule() {
    let q = policy();
    let none = decisions::view(&q, &rules(), &[]);
    let first = payload(1, None, N0, "first", NOW, NOW);
    assert_eq!(check(&none, &first), Ok(()));
    let edit = |p: &Value, f: &dyn Fn(&mut Value)| {
        let mut p = p.clone();
        f(&mut p);
        p
    };
    let mut change = first.clone();
    change["kind"] = json!(crate::quorum::POLICY_CHANGE_KIND);
    assert_eq!(check(&none, &change), Err("decision_kind"));
    assert_eq!(
        check(&none, &edit(&first, &|p| p["note"] = json!("x"))),
        Err("payload_unknown_field")
    );
    assert_eq!(
        check(
            &none,
            &edit(&first, &|p| p["policy_digest"] = json!("0".repeat(64)))
        ),
        Err("policy_mismatch")
    );
    assert_eq!(
        check(&none, &edit(&first, &|p| p["resource"] = json!(N1))),
        Err("resource_not_decided_here")
    );
    assert_eq!(
        check(&none, &edit(&first, &|p| p["expires_at"] = json!(NOW))),
        Err("certificate_life"),
        "expired"
    );
    assert_eq!(
        check(
            &none,
            &edit(&first, &|p| p["expires_at"] = json!(NOW + LIFE + 1))
        ),
        Err("certificate_life"),
        "too long"
    );
    assert_eq!(
        check(
            &none,
            &edit(&first, &|p| {
                p["issued_at"] = json!(NOW + 31);
                p["expires_at"] = json!(NOW + 60)
            })
        ),
        Err("certificate_life"),
        "issued too far ahead"
    );
    assert_eq!(
        check(&none, &edit(&first, &|p| p["new_holder"] = json!(STRANGER))),
        Err("holder_not_a_node")
    );
    assert_eq!(
        check(&none, &edit(&first, &|p| p["grant_id"] = json!("-g"))),
        Err("payload_invalid")
    );
    assert_eq!(
        check(
            &none,
            &edit(&first, &|p| p["eligible_after"] = json!(NOW + LIFE + 1))
        ),
        Err("barrier_after_expiry")
    );
    assert_eq!(
        check(&none, &payload(2, None, N0, "lease_barrier", NOW, NOW)),
        Err("epoch_not_next"),
        "an epoch skipped"
    );
    assert_eq!(
        check(&none, &payload(1, Some(N1), N0, "lease_barrier", NOW, NOW)),
        Err("previous_holder_mismatch")
    );
    assert_eq!(
        check(&none, &payload(1, None, N0, "elected", NOW, NOW)),
        Err("method_unknown")
    );
    assert_eq!(
        check(&none, &payload(1, None, N0, "fence_receipt", NOW, NOW)),
        Err("fence_receipt_unverifiable")
    );

    // An epoch decided: epoch 1 to N0, barrier NOW - 10, expiring at NOW - 10 + LIFE.
    let decided = payload(1, None, N0, "first", NOW - 10, NOW - 10);
    let view = decisions::view(
        &q,
        &rules(),
        &decisions::verified(&q, &votes(&decided, &ABC[..2])),
    );
    assert_eq!(
        check(&view, &payload(1, None, N0, "first", NOW, NOW)),
        Err("epoch_not_next"),
        "the epoch again"
    );
    assert_eq!(
        check(&view, &payload(2, None, N0, "first", NOW, NOW)),
        Err("previous_holder_mismatch")
    );
    assert_eq!(
        check(&view, &payload(2, Some(N0), N0, "first", NOW, NOW)),
        Err("first_after_an_epoch")
    );
    assert_eq!(
        check(&view, &payload(2, Some(N0), N1, "same_holder", NOW, NOW)),
        Err("not_the_same_holder")
    );
    // The same holder carries the barrier, and needs no more.
    assert_eq!(
        check(
            &view,
            &payload(2, Some(N0), N0, "same_holder", NOW - 11, NOW)
        ),
        Err("barrier_too_early")
    );
    assert_eq!(
        check(
            &view,
            &payload(2, Some(N0), N0, "same_holder", NOW - 10, NOW)
        ),
        Ok(())
    );
    assert_eq!(
        check(
            &view,
            &payload(2, Some(N0), N0, "lease_barrier", NOW - 10, NOW)
        ),
        Ok(()),
        "a lease barrier to the same holder"
    );
    // Another holder: the barrier covers the renewal bound, the current certificate's expiry and the
    // proposal's issue, each plus the lease and the margin; the latest of them here is the renewal.
    let wait = LEASE + MARGIN;
    let required = RENEWAL + wait;
    assert!(required > NOW - 10 + LIFE + wait && required > NOW + wait);
    let rotation = |eligible| payload(2, Some(N0), N1, "lease_barrier", eligible, NOW);
    let mut r = rules();
    r.renewal_not_after = 0;
    let check_with = |r: &ResourceRules, p: &Value| {
        decisions::check(&q, r, &nodes(), &view, p, NOW, LIFE)
            .map(|_| ())
            .map_err(|e| e.code)
    };
    // Without a renewal bound, the current certificate's expiry is the latest term.
    let by_expiry = NOW - 10 + LIFE + wait;
    assert!(by_expiry > NOW + wait);
    let mut long = rotation(by_expiry - 1);
    long["expires_at"] = json!(by_expiry + 1);
    long["issued_at"] = json!(by_expiry + 1 - LIFE);
    assert_eq!(
        check_with(&r, &long),
        Err("barrier_too_early"),
        "before the current certificate's expiry plus the wait"
    );
    long["eligible_after"] = json!(by_expiry);
    assert_eq!(check_with(&r, &long), Ok(()));
    // With it, the renewal bound; a proposal cannot even carry a barrier that late within its life,
    // so a rotation away from a holder that may renew by itself waits for the bound's end.
    assert_eq!(
        decisions::required_barrier(&rules(), &view, &rotation(0)),
        Some(required)
    );
    // Issued at the renewal bound, so that the issue's own term is the same second.
    let mut at_bound = rotation(required);
    at_bound["issued_at"] = json!(RENEWAL);
    at_bound["expires_at"] = json!(RENEWAL + LIFE);
    let later =
        decisions::check(&q, &rules(), &nodes(), &view, &at_bound, RENEWAL, LIFE).map(|_| ());
    assert_eq!(
        later.map_err(|e| e.code),
        Ok(()),
        "at the bound, from a clock that has reached it"
    );
    at_bound["eligible_after"] = json!(required - 1);
    let early =
        decisions::check(&q, &rules(), &nodes(), &view, &at_bound, RENEWAL, LIFE).map(|_| ());
    assert_eq!(
        early.map_err(|e| e.code),
        Err("barrier_too_early"),
        "one second before the bound"
    );
    // The proposal's issue: an acquisition by the previous holder at the moment of the proposal.
    let mut no_cert = rules();
    no_cert.renewal_not_after = 0;
    no_cert.baseline = Some(Baseline {
        epoch: 5,
        holder: N0.into(),
        eligible_after: 0,
    });
    let base = decisions::view(&q, &no_cert, &[]);
    let from_base = |eligible: i64| payload(6, Some(N0), N1, "lease_barrier", eligible, NOW);
    let base_check = |p: &Value| {
        decisions::check(&q, &no_cert, &nodes(), &base, p, NOW, LIFE)
            .map(|_| ())
            .map_err(|e| e.code)
    };
    assert_eq!(
        base_check(&from_base(NOW + wait - 1)),
        Err("barrier_too_early"),
        "before the issue plus the wait"
    );
    assert_eq!(base_check(&from_base(NOW + wait)), Ok(()));
    // The carried barrier binds every method.
    no_cert.baseline = Some(Baseline {
        epoch: 5,
        holder: N0.into(),
        eligible_after: NOW + 50,
    });
    let base = decisions::view(&q, &no_cert, &[]);
    let base_check = |p: &Value| {
        decisions::check(&q, &no_cert, &nodes(), &base, p, NOW, LIFE)
            .map(|_| ())
            .map_err(|e| e.code)
    };
    assert_eq!(
        base_check(&payload(6, Some(N0), N0, "same_holder", NOW + 49, NOW)),
        Err("barrier_too_early")
    );
    assert_eq!(
        base_check(&payload(6, Some(N0), N1, "lease_barrier", NOW + wait, NOW)),
        Err("barrier_too_early")
    );
    assert_eq!(
        base_check(&payload(6, Some(N0), N1, "lease_barrier", NOW + 50, NOW)),
        Ok(())
    );
}
