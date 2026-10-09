#!/usr/bin/env python3
"""Submit one bounded, idempotent observation through the local manager API."""
import argparse
import json
import os
import socket
import sys
import time

SOCKET_PATH = "/run/podmesh-manager/control.sock"
# Retries of `busy` and `uncertain` are bounded, as in the universe entrypoint: those answers
# come from a resident that is already caught up, and one that keeps giving them is failing.
RETRY_BUDGET_SECONDS = 25.0
# The catch-up state is read every five seconds -- a collision is only known once an exchange has
# carried it -- and one line of it is printed every thirty, as the universe entrypoint does.
CHECK_SECONDS = 5.0
REPORT_SECONDS = 30.0


def exchange(request):
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(2.0)
    client.connect(SOCKET_PATH)
    client.sendall(request)
    client.shutdown(socket.SHUT_WR)
    response = bytearray()
    while len(response) <= 32768:
        chunk = client.recv(4096)
        if not chunk:
            break
        response.extend(chunk)
    client.close()
    if len(response) > 32768:
        raise RuntimeError("control response exceeds bound")
    return json.loads(response)


def catch_up_state():
    """The resident's catch-up state, and what blocks its readiness when it can tell."""
    try:
        status = exchange(b'{"operation":"status"}')
        state = status["catch_up"]
    except (OSError, ValueError, KeyError, RuntimeError) as problem:
        return f"catch-up state unavailable ({type(problem).__name__}: {problem})", None
    def names(key):
        return ",".join(state.get(key) or []) or "-"

    blocker = state.get("blocked_by") or None
    summary = (
        f"imported={names('peers_imported')} matched={names('peers_matched')} "
        f"missing={names('peers_missing')} ahead={names('peers_ahead')} "
        f"not_attempted={names('peers_not_attempted')} "
        f"own_facts_at_start={state.get('own_facts_at_start')} "
        f"latest_own_fact_appended_locally={state.get('latest_own_fact_appended_locally')} "
        f"window_ms={state.get('window_ms')} blocked_by={json.dumps(blocker, sort_keys=True)}"
    )
    return summary, blocker


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--operation-id", required=True)
    parser.add_argument("--scope", required=True)
    parser.add_argument("--subject", required=True)
    parser.add_argument("--value", required=True)
    # A resident that has not caught up with its peers answers `catching_up`, touches nothing, and
    # keeps exchanging: it is running and not ready, and the universe entrypoint waits for it with
    # no deadline (see packaging/podmesh-manager/universe/entrypoint.sh). This tool waits the same
    # way, so that a campaign is not the only thing in PodMesh that calls catching up a failure.
    # `--timeout-seconds` bounds the whole wait when the caller wants a bound; without it, only a
    # catch-up blocked by an event identity collision ends the wait, since nothing will resolve it.
    parser.add_argument("--timeout-seconds", type=float, default=None)
    args = parser.parse_args()
    if os.geteuid() != 0 or (args.timeout_seconds is not None and args.timeout_seconds <= 0):
        return 2
    request = json.dumps({
        "operation": "append_observation",
        "operation_id": args.operation_id,
        "scope": args.scope,
        "subject": args.subject,
        "value": args.value,
    }, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode()
    started = time.monotonic()
    deadline = None if args.timeout_seconds is None else started + args.timeout_seconds
    retry_since = started
    reported_at = None
    checked_at = None
    attempts = 0
    catching_up = 0
    last = "no answer"
    while deadline is None or time.monotonic() < deadline:
        attempts += 1
        try:
            response = exchange(request)
        except (OSError, ValueError, json.JSONDecodeError, RuntimeError) as problem:
            response = {"error": "append_observation_uncertain"}
            last = f"append_observation_uncertain ({type(problem).__name__}: {problem})"
        else:
            last = response.get("error", "an answer without an observation")
        if response.get("response", {}).get("result") == "observed":
            print(json.dumps({
                "schema_version": "podmesh-manager-observation-injection/v1",
                "result": "observed",
                "attempts": attempts,
                "catching_up_answers": catching_up,
                "identical_request_retried": attempts > 1,
            }, sort_keys=True))
            return 0
        if response.get("error") == "append_observation_catching_up":
            # The resident touched nothing and is exchanging: retry the identical request without
            # a deadline of our own, reporting its state, and stop only on a collision.
            catching_up += 1
            now = time.monotonic()
            retry_since = now
            if checked_at is None or now - checked_at >= CHECK_SECONDS:
                checked_at = now
                summary, blocker = catch_up_state()
                if reported_at is None or now - reported_at >= REPORT_SECONDS:
                    reported_at = now
                    print(
                        f"the resident is catching up with its peers ({now - started:.0f}s, "
                        f"attempt {attempts}); {summary}",
                        file=sys.stderr,
                    )
                if blocker and blocker.get("reason") == "identity_collision":
                    print(
                        "this replica cannot catch up: an event identity collision with "
                        f"{','.join(blocker.get('peers') or []) or 'a peer'} on "
                        f"{blocker.get('event_id') or 'an event'}; it needs an operator, not time",
                        file=sys.stderr,
                    )
                    return 1
            time.sleep(0.5)
            continue
        if response.get("error") not in ("append_observation_busy", "append_observation_uncertain"):
            print(f"observation refused by typed local API: {last}", file=sys.stderr)
            return 1
        if time.monotonic() - retry_since >= RETRY_BUDGET_SECONDS:
            print(
                f"the resident kept answering {last} for {RETRY_BUDGET_SECONDS:.0f}s "
                f"after {attempts} attempt(s)",
                file=sys.stderr,
            )
            return 1
        time.sleep(0.05)
    print(
        f"observation not observed at the campaign deadline after {attempts} attempt(s) "
        f"({catching_up} of them answered catching up); last answer: {last}",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
