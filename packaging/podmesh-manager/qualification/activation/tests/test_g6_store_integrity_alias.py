#!/usr/bin/env python3
"""G6: compare-evidence accepts store_integrity_result beside sqlite_integrity_result."""
import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("g2_compare", ROOT / "compare-evidence.py")
G2 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(G2)

C = lambda digit: "sha256:" + digit * 64
ZERO = "0" * 64


def present_inspection(**overrides):
    base = {
        "store_present": True,
        "schema_version": 4,
        "logical_manager_commitment": C("1"),
        "replica_commitment": C("2"),
        "logical_history_sha256": ZERO,
        "receipt_set_sha256": ZERO,
        "audit_set_sha256": ZERO,
        "history_count": 0,
        "receipt_count": 0,
        "audit_event_count": 0,
        "incomplete_attempt_count": 0,
        "incomplete_attempts": [],
        "unaudited_import_receipt_count": 0,
        "unaudited_import_receipt_commitments": [],
        "imported_operation_commitments": [],
    }
    base.update(overrides)
    return base


class StoreIntegrityAlias(unittest.TestCase):
    def test_store_integrity_result_alias_accepted(self):
        insp = present_inspection(store_integrity_result="ok")
        insp.pop("sqlite_integrity_result", None)
        G2.validate_inspection(insp, "alias")

    def test_conflicting_integrity_results_refused(self):
        insp = present_inspection(
            sqlite_integrity_result="ok", store_integrity_result="corrupt"
        )
        with self.assertRaisesRegex(ValueError, "conflicting integrity results"):
            G2.validate_inspection(insp, "conflict")


if __name__ == "__main__":
    unittest.main()
