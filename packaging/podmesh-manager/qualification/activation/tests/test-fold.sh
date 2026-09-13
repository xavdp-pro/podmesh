#!/bin/bash
# Tests for fold-exchanges.jq, the projection that turns a canonical store's audit rows
# into one published row per wire nonce.
#
# The fixture is DERIVED FROM A REAL CAMPAIGN STORE, not invented. Its three phase-set
# shapes, the phase that carries each field, and the zeros the counters hold on phases
# that do not carry them are all properties measured on a preserved store copy and then
# reproduced here with every identifier replaced. That matters: the first version of the
# fold was refuted by real data on its second line, and no synthetic fixture written from
# the schema would have caught it.
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
fixture="$root/tests/fixtures/audit-events-from-campaign.json"

# A stand-in commitment map. The collector computes real salted commitments; the fold only
# ever looks values up, so a distinguishable stub is enough here and keeps the salt out of
# a test. Every value the fold can ask for must be present, or the fold refuses — which is
# itself one of the cases below.
keys() {
  jq -r '[.ordered_audit_events[].event
          | [["wire-nonce",.wire_nonce],["replica-id",.authenticated_peer_id],
             ["wire-operation-id",.operation_id],["request-digest",.request_sha256],
             ["reply-digest",.reply_sha256],["receipt-id",.local_receipt_sha256],
             ["receipt-id",.remote_receipt_sha256]]]
         | add | map(select(.[1] != null)) | map(.[0] + "\t" + .[1]) | unique | .[]' "$1"
}
keys "$fixture" | jq -Rn '[inputs | {key:., value:("sha256:stub-" + (.|@base64))}] | from_entries' > "$work/commitments.json"
commitments=$(cat "$work/commitments.json")

fold() { jq --argjson commitments "$commitments" -f "$root/fold-exchanges.jq" "$1"; }

# --- the positive case -------------------------------------------------------------
fold "$fixture" > "$work/folded.json"
jq -e 'length == 9' "$work/folded.json" >/dev/null || { echo 'fold did not produce one row per nonce' >&2; exit 1; }
jq -e '[.[] | {d:.direction, r:.row_count}] | sort_by(.d, .r)
       | . == [{d:"inbound",r:4},{d:"inbound",r:4},{d:"inbound",r:4},
               {d:"outbound",r:1},{d:"outbound",r:1},{d:"outbound",r:1},
               {d:"outbound",r:2},{d:"outbound",r:2},{d:"outbound",r:2}]' "$work/folded.json" >/dev/null \
  || { echo 'the three measured phase-set shapes are not reproduced' >&2; exit 1; }

# A stranded sender attempt must carry everything the receiver-side join needs. This is the
# whole point of the lot: if a strand published nothing to join on, no predicate could
# account for it.
jq -e '[.[] | select(.direction=="outbound" and .row_count==1)]
       | length == 3 and all(.[];
           .joinable and .peer_commitment != null and .operation_commitment != null
           and .request_sha256_commitment != null and .request_announced_body_bytes > 0
           and .phases_reached == ["outbound_request_prepared"])' "$work/folded.json" >/dev/null \
  || { echo 'a stranded attempt does not publish the fields condition 2 joins on' >&2; exit 1; }

# The byte counters must survive the fold. A non-null fold reads the zeros that
# non-carrying phases write as real values; this asserts the numbers came through.
jq -e '[.[] | select(.direction=="inbound")]
       | all(.[]; .request_frame_bytes > 0 and .reply_frame_bytes > 0)' "$work/folded.json" >/dev/null \
  || { echo 'inbound byte counters were lost or zeroed by the fold' >&2; exit 1; }

# No raw identifier may reach the output.
if grep -qE '"(nonce|att|op|replica|rqd|rpd|rcpt)-' "$work/folded.json"; then
  echo 'a raw identifier reached the folded output' >&2; exit 1
fi

# --- the refusals ------------------------------------------------------------------
# Each is verified to fire for its OWN reason, not merely to fail. Two earlier versions of
# these cases passed for the wrong reason: one mutated a field to the value it already
# held, and one created a disagreement while claiming to test a missing commitment.
nonce=$(jq -r '[.ordered_audit_events[].event | select(.direction=="inbound")][0].wire_nonce' "$fixture")
refuse() {
  local label=$1 filter=$2 expected=$3
  jq "$filter" "$fixture" > "$work/mutated.json"
  if fold "$work/mutated.json" > "$work/out.json" 2>"$work/err.txt"; then
    echo "accepted $label" >&2; exit 1
  fi
  [ -s "$work/err.txt" ] || { echo "$label failed without a message, which is a crash and not a refusal" >&2; exit 1; }
  grep -q -- "$expected" "$work/err.txt" || { echo "$label was refused for another reason: $(head -1 "$work/err.txt")" >&2; exit 1; }
}
refuse 'two different request sizes in one exchange' \
  '(.ordered_audit_events[] | select(.event.phase=="inbound_import_committed") | .event.request_frame_bytes) = 999' \
  'field request_frame_bytes carries 2 different set values'
refuse 'two different peers in one exchange' \
  '(.ordered_audit_events[0].event.authenticated_peer_id) = "replica-impostor"' \
  'field authenticated_peer_id carries 2 different set values'
refuse 'rows of one nonce split across directions' \
  "(.ordered_audit_events[] | select(.event.wire_nonce==\"$nonce\" and .event.phase==\"inbound_reply_prepared\") | .event.direction) = \"outbound\"" \
  'rows for one nonce disagree on direction'
refuse 'a value the collector never committed' \
  "(.ordered_audit_events[] | select(.event.wire_nonce==\"$nonce\") | .event.operation_id) = \"op-never-committed\"" \
  'no commitment for wire-operation-id'
refuse 'rows disagreeing on replayed' \
  '(.ordered_audit_events[] | select(.event.phase=="inbound_reply_prepared") | .event.replayed) = true' \
  'rows disagree on replayed'

# --- pre-authentication nonces are published, never joined --------------------------
# Not observed in the measured store, but permitted by the candidate's own validator for
# inbound observation and diagnostic terminals. They are minted locally, so two hosts never
# commit the same value: a fail-closed join would refuse them for a reason that is not a
# defect.
#
# The case is built from ONE observation row, which is the only shape such a nonce can
# have. An earlier version relabelled every `inbound_request_observed` row with the same
# preauth value, which collapsed three unrelated exchanges into one nonce carrying three
# different peers — the fold refused it, correctly, and for a reason that had nothing to
# do with pre-authentication.
preauth="preauth:$(printf 'x' | sha256sum | awk '{print $1}')"
jq --arg n "$preauth" '
  [ .ordered_audit_events[] | select(.event.phase == "inbound_request_observed") ][0] as $row
  | {ordered_audit_events: [ $row | .event.wire_nonce = $n
                                  | .event.authenticated_peer_id = null
                                  | .event.operation_id = null ]}' "$fixture" > "$work/preauth.json"
keys "$work/preauth.json" | jq -Rn '[inputs | {key:., value:("sha256:stub-" + (.|@base64))}] | from_entries' > "$work/preauth-commitments.json"
jq --argjson commitments "$(cat "$work/preauth-commitments.json")" -f "$root/fold-exchanges.jq" "$work/preauth.json" > "$work/preauth-folded.json"
jq -e 'length >= 1 and all(.[]; .nonce_authority == "pre-authentication" and .joinable == false)' "$work/preauth-folded.json" >/dev/null \
  || { echo 'a pre-authentication nonce was not marked non-joinable' >&2; exit 1; }

printf '%s\n' 'PASS: exchange fold — three measured shapes, stranded-attempt join fields, byte counters, five refusals, pre-authentication nonces published and not joined.'
