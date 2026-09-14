# Fold a canonical store's audit rows into one published row per wire nonce.
#
# An exchange is not an audit row. Measured on a preserved campaign store, every nonce
# carries one of exactly three phase sets: a stranded sender attempt (one row), a
# completed sender attempt (two rows), or a served receiver exchange (four rows). A
# comparator that expected one row per nonce would refuse every genuine inbound exchange.
#
# Input:  the candidate's --inspect-store object, plus $commitments, a map from
#         "<label>\t<raw value>" to its salted commitment, computed by the collector.
# Output: an array of folded rows. Raw identifiers never appear in the output.
#
# Folding is fail-closed. Each field takes the single SET value among the nonce's rows;
# two different set values abort the capture rather than publishing a guess. "Set" is not
# "non-null": the byte counters are not nullable, so a phase that does not carry one
# writes zero, and a non-null fold would read those zeros as values and refuse every
# four-row exchange. Zero means "this phase did not carry it".

# Labels name the KIND of value, never the place it was observed, and two kinds must never
# share one. Established by reading the candidate's own assignments and comparisons:
#   receipt-digest      ReceiptEvidence.sha256 = local_receipt_sha256 = remote_receipt_sha256
#                       (durable.rs:583 assigns one from the other)
#   receipt-operation-id  ReceiptEvidence.operation_id = local/remote_receipt_operation_id,
#                       and that is what unaudited_import_receipt_ids holds (durable.rs:582,
#                       :2670-2676)
#   wire-operation-id   ExchangeAuditEvent.operation_id = ReceiptEvidence.wire_operation_id
#                       = IncompleteAttempt.wire_operation_id (durable.rs:1873, :2584)
#   replica-id          authenticated_peer_id = peer_claim = the store's own replica_id
#                       (durable.rs:536)
# The receipt fields were briefly labelled `receipt-id` here. That is the name of the
# operation id, not the digest, so the two kinds would have collided under one label and the
# join would have failed silently — the failure this discipline exists to prevent.
#
# `label` is a jq keyword, so the parameter cannot be named $label.
def commitment($kind; $value):
  if $value == null then null
  else ($commitments[$kind + "\t" + $value]
        // error("no commitment for " + $kind + "; the collector and this fold disagree on labels"))
  end;

# Distinct set values of one field across a nonce's rows. `$zero_is_unset` distinguishes
# the two field kinds; see the note above.
# No pipe into $zero_is_unset here, deliberately. Written as `select($zero_is_unset | not
# or . != 0)` the pipe rebinds `.` to the boolean, so the zero test silently examines the
# flag instead of the value and every four-row exchange is refused. Real campaign data
# caught it; no synthetic fixture would have.
def setvals($field; $zero_is_unset):
  [ .[] | .[$field] | select(. != null) | select($zero_is_unset == false or . != 0) ] | unique;

def fold($field; $zero_is_unset; $nonce_index):
  setvals($field; $zero_is_unset) as $v
  | if ($v | length) > 1
    then error("exchange #\($nonce_index): field \($field) carries \($v|length) different set values; a folded exchange must agree with itself")
    elif ($v | length) == 0 then null
    else $v[0]
    end;

[ .ordered_audit_events[].event ]
| group_by(.wire_nonce)
| to_entries
| map(
    .key as $i | .value as $rows
    | ($rows[0].wire_nonce) as $nonce
    # A pre-authentication nonce is minted locally from a process id, a counter and a
    # timestamp, so two hosts never commit the same value for one exchange. Such a row is
    # published as an observation and is never joined: under a fail-closed comparator its
    # absence from the other side would otherwise be a refusal for a reason that is not a
    # defect. The store confines it to inbound observation and diagnostic terminals.
    | (if ($nonce | startswith("preauth:")) then "pre-authentication" else "peer-validated" end) as $authority
    | ($rows | map(.direction) | unique) as $directions
    | (if ($directions | length) != 1
       then error("exchange #\($i): rows for one nonce disagree on direction")
       else $directions[0] end) as $direction
    | ($rows | map(.phase) | unique) as $phases
    # The terminal phase is derived from the set rather than asserted, so a set missing a
    # phase stays visible instead of being papered over by a single claimed terminal.
    | ($rows | fold("authenticated_peer_id"; false; $i)) as $peer
    | ($rows | fold("operation_id"; false; $i)) as $operation
    | ($rows | fold("request_sha256"; false; $i)) as $request_digest
    | ($rows | fold("reply_sha256"; false; $i)) as $reply_digest
    | ($rows | fold("local_receipt_sha256"; false; $i)) as $local_receipt
    | ($rows | fold("remote_receipt_sha256"; false; $i)) as $remote_receipt
    | ($rows | fold("request_frame_bytes"; true; $i)) as $request_frame
    | ($rows | fold("reply_frame_bytes"; true; $i)) as $reply_frame
    | ($rows | fold("request_announced_body_bytes"; true; $i)) as $announced
    | ($rows | map(select(.phase | endswith("request_observed") or endswith("request_prepared") | not)) ) as $after_request
    | ([ $after_request[].replayed ] | unique) as $replayed_values
    | {
        nonce_commitment: commitment("wire-nonce"; $nonce),
        nonce_authority: $authority,
        joinable: ($authority == "peer-validated"),
        direction: $direction,
        phases_reached: $phases,
        row_count: ($rows | length),
        peer_commitment: commitment("replica-id"; $peer),
        operation_commitment: commitment("wire-operation-id"; $operation),
        request_sha256_commitment: commitment("request-digest"; $request_digest),
        reply_sha256_commitment: commitment("reply-digest"; $reply_digest),
        local_receipt_commitment: commitment("receipt-digest"; $local_receipt),
        remote_receipt_commitment: commitment("receipt-digest"; $remote_receipt),
        request_frame_bytes: ($request_frame // 0),
        reply_frame_bytes: ($reply_frame // 0),
        request_announced_body_bytes: $announced,
        outcomes: ($rows | map(.outcome) | unique),
        replayed: (if ($replayed_values | length) > 1
                   then error("exchange #\($i): rows disagree on replayed")
                   elif ($replayed_values | length) == 0 then null
                   else $replayed_values[0] end)
      }
  )
