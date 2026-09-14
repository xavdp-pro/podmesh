"""Candidate-shaped, three-host G2 evidence fixtures for adversarial tests."""

from __future__ import annotations

import copy
import hashlib
import json
import re
from pathlib import Path


ALIASES = ("lab-a", "lab-b", "lab-c")
STAGES = ("pre-activation", "active-baseline", "converged", "post-cleanup")


def commitment(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode()).hexdigest()


def digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def comparator_versions(comparator: Path) -> tuple[str, str, int]:
    """Read the comparator's accepted versions without weakening the bump test."""
    text = comparator.read_text()
    evidence = re.search(r'^SCHEMA\s*=\s*"([^"]+)"', text, re.MULTILINE)
    result = re.search(r'^OUT_SCHEMA\s*=\s*"([^"]+)"', text, re.MULTILINE)
    inspection_constant = re.search(r'^INSPECTION_SCHEMA\s*=\s*(\d+)', text, re.MULTILINE)
    inspection_literal = re.search(r'v\["schema_version"\]\s*!=\s*(\d+)', text)
    if not evidence or not result or not (inspection_constant or inspection_literal):
        raise AssertionError("could not discover the comparator's pinned schema versions")
    inspection = int((inspection_constant or inspection_literal).group(1))
    return evidence.group(1), result.group(1), inspection


def build_fixture(comparator: Path) -> dict[tuple[str, str], dict]:
    evidence_schema, _, inspection_schema = comparator_versions(comparator)
    replicas = [commitment("replica-" + alias) for alias in ALIASES]
    hosts = [commitment("host-" + alias) for alias in ALIASES]
    keys = {
        (0, 1): commitment("key-ab"),
        (0, 2): commitment("key-ac"),
        (1, 2): commitment("key-bc"),
    }
    shutdown = {
        "schema_version": "podmesh-manager-graceful-shutdown/v1",
        "typed_request_acknowledged": True,
        "process_exited_successfully": True,
        "service_inactive": True,
        "control_socket_absent": True,
        "forced_signal_used": False,
    }

    def evidence(index: int, stage: str) -> dict:
        running = stage in ("active-baseline", "converged")
        cleanup = stage == "post-cleanup"
        peers = [
            {
                "replica_id_commitment": replicas[peer],
                "endpoint_commitment": commitment(f"endpoint-{peer}"),
                "shared_key_commitment": keys[tuple(sorted((index, peer)))],
            }
            for peer in range(3)
            if peer != index
        ]
        limits = (
            {
                "network_mode": "authenticated-static-peers",
                "address_families": ["AF_UNIX", "AF_INET"],
                "peer_allow_count": 2,
                "peer_allow_prefix_length": 32,
                "sha256": digest("dropin"),
            }
            if running
            else {
                "network_mode": None,
                "address_families": [],
                "peer_allow_count": 0,
                "peer_allow_prefix_length": None,
            }
        )

        def inspection(history: str, history_count: int, receipt_count: int, exchanges: list) -> dict:
            return {
                "store_present": True,
                "schema_version": inspection_schema,
                "logical_manager_commitment": commitment("logical"),
                "replica_commitment": replicas[index],
                "logical_history_sha256": history,
                "receipt_set_sha256": digest(f"receipts-{index}-{stage}"),
                "audit_set_sha256": digest(f"audits-{index}-{stage}"),
                "sqlite_integrity_result": "ok",
                "history_count": history_count,
                "receipt_count": receipt_count,
                "audit_event_count": sum(row["row_count"] for row in exchanges),
                "incomplete_attempt_count": 0,
                "incomplete_attempts": [],
                "unaudited_import_receipt_count": 0,
                "unaudited_import_receipt_commitments": [],
                "imported_operation_commitments": [],
            }

        def served(sequence: int) -> dict:
            return {
                "nonce_commitment": commitment(f"nonce-{index}-{sequence}"),
                "nonce_authority": "peer-validated",
                "joinable": True,
                "direction": "inbound",
                "phases_reached": [
                    "inbound_request_observed",
                    "inbound_import_committed",
                    "inbound_reply_prepared",
                    "inbound_reply_write_observed",
                ],
                "row_count": 4,
                "peer_commitment": replicas[(index + 1) % 3],
                "operation_commitment": commitment(f"op-{index}-{sequence}"),
                "request_sha256_commitment": commitment(f"rq-{index}-{sequence}"),
                "reply_sha256_commitment": commitment(f"rp-{index}-{sequence}"),
                "local_receipt_commitment": commitment(f"rcpt-{index}-{sequence}"),
                "remote_receipt_commitment": None,
                "request_frame_bytes": 2735,
                "reply_frame_bytes": 626,
                "request_announced_body_bytes": 2731,
                "reply_announced_body_bytes": 622,
                "outcomes": ["accepted"],
                "replayed": False,
            }

        exchanges = [] if stage in ("pre-activation", "active-baseline") else [served(0), served(1)]
        inspected = inspection(
            digest(f"baseline-{index}") if stage in ("pre-activation", "active-baseline") else digest("history"),
            0 if stage in ("pre-activation", "active-baseline") else 3,
            0 if stage in ("pre-activation", "active-baseline") else 3,
            exchanges,
        )
        return {
            "schema_version": evidence_schema,
            "host_alias": ALIASES[index],
            "stage": stage,
            "package": {
                "name": "podmesh-manager",
                "version": "0.1.0~manager2",
                "binary_sha256": digest("binary"),
                "dpkg_verify": "clean",
            },
            "configuration": {
                "document_commitment": commitment(f"config-{index}"),
                "logical_manager_commitment": commitment("logical"),
                "local_replica_commitment": replicas[index],
                "local_host_commitment": hosts[index],
                "topology_commitment": commitment("topology"),
                "peer_count": 2,
                "peers": peers,
            },
            "dropin": {
                "present": running,
                "sha256": digest("dropin") if running else None,
                "semantic_limits": limits,
                "packaged_fragment_sha256": digest("fragment"),
                "inherited_deny_all": True,
                "effective_policy_configured": running,
                "effective_policy_commitment": commitment("effective-policy") if running else None,
            },
            "service": {
                "load_state": "loaded",
                "active_state": "active" if running else "inactive",
                "sub_state": "running" if running else "dead",
                "unit_file_state": "disabled",
                "main_pid": 101 + index if running else 0,
                "invocation_commitment": commitment(f"invocation-{index}") if running or cleanup else None,
                "n_restarts": 0,
                "result": "success" if running or cleanup else "",
                "exec_main_code": "exited" if running else "0" if cleanup else "",
                "exec_main_status": 0,
            },
            "manager_process": {
                "count": 1 if running else 0,
                "pid": 101 + index if running else None,
                "uid": 995 if running else None,
                "argv_commitment": commitment(f"argv-{index}") if running else None,
                "argv_count": 7 if running else 0,
            },
            "paths": {
                "state": {"present": True, "uid": 995, "gid": 995, "mode": "750", "content_commitment": commitment(f"state-{index}-{stage}")},
                "runtime": {"present": running, "uid": 995 if running else None, "gid": 995 if running else None, "mode": "700" if running else None, "content_commitment": commitment(f"runtime-{index}") if running else None},
                "control_socket": {"present": running, "uid": 995 if running else None, "gid": 995 if running else None, "mode": "600" if running else None},
            },
            "listeners": {
                "status": "available-successful",
                "endpoint_commitment": commitment(f"endpoint-{index}"),
                "tcp_listener_count": 1 if running else 0,
                "udp_listener_count": 0,
            },
            "stability": {
                "existing_services_commitment": commitment("existing"),
                "podman_containers_commitment": commitment("containers"),
                "routes": {"status": "available-successful", "commitment": commitment("routes")},
                "firewall": {"status": "available-successful", "commitment": commitment("firewall")},
            },
            "inspection": inspected,
            "exchanges": exchanges,
            "graceful_shutdown": copy.deepcopy(shutdown) if cleanup else None,
        }

    return {(alias, stage): evidence(index, stage) for index, alias in enumerate(ALIASES) for stage in STAGES}


def recount(capture: dict) -> None:
    inspection = capture["inspection"]
    inspection["audit_event_count"] = sum(row["row_count"] for row in capture["exchanges"])
    inspection["incomplete_attempt_count"] = len(inspection["incomplete_attempts"])


def seal(directory: Path, fixtures: dict[tuple[str, str], dict]) -> None:
    for (host, stage), value in fixtures.items():
        path = directory / f"{host}-{stage}.json"
        path.write_text(json.dumps(value, sort_keys=True))
        checksum = hashlib.sha256(path.read_bytes()).hexdigest()
        Path(f"{path}.sha256").write_text(f"{checksum}  {path}\n")
