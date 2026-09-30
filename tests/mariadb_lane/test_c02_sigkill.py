#!/usr/bin/env python3
"""Offline controller refusals only: no SQL, Podman, or actual copier SIGKILL proof."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

spec = importlib.util.spec_from_file_location("controller", Path(__file__).with_name("c02_sigkill.py"))
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)


class Guards(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.binary = self.root / "binary"
        self.binary.write_bytes(b"offline-unexecuted-binary")
        self.binary.chmod(0o700)
        caps = json.loads((Path(__file__).parents[2] / "fixtures/C/copy-v1/resource-caps.json").read_text())
        self.contract = {key: "synthetic" for key in controller.BINDINGS}
        self.contract.update(schema="podmesh-c02-fixture/1", status="ready", server_execution_authorized=True,
                             role="copy", resource_caps=caps, SQL_hashes={"sql": "pinned"},
                             target_resource_identity={"container_id": "new-c", "volume_name": "new-v", "image_id": "new-i"},
                             binary_sha256=controller.sha(self.binary), maximum_campaign_seconds=1,
                             connect_timeout_ms=1, lock_wait_timeout_seconds=1, statement_timeout_ms=1,
                             killpoint="before_data_COMMIT:metadata", kill_event_path=str(self.root / "event"))
        provenance = {key: self.contract[key] for key in controller.PROVENANCE}
        tables = [{"name": f"table-{n}"} for n in range(38)]
        for key, kind in (("capability_receipt", "podmesh-c02-capability/1"),
                          ("seeded_restore_receipt", "podmesh-c02-restoration/1")):
            path = self.root / key
            controller.write_new(path, {**provenance, "format": kind, "status": "PASS", "tables": tables,
                                        "plan_sha256": "same-plan", "restoration_kind": "seeded_restore"})
            self.contract[key] = {"path": str(path), "sha256": controller.sha(path)}

    def tearDown(self):
        self.tmp.cleanup()

    def arguments(self, first=None, resumes=None, operation="copy"):
        first = copy.deepcopy(self.contract if first is None else first)
        if resumes is None:
            resume = copy.deepcopy(first)
            resume.update(role="copy_resume")
            resume.pop("killpoint", None)
            resume.pop("kill_event_path", None)
            resumes = [resume, copy.deepcopy(resume)]
        paths = [self.root / f"contract-{n}.json" for n in range(3)]
        for path, value in zip(paths, [first, *resumes]):
            if path.exists():
                path.unlink()
            controller.write_new(path, value)
        return SimpleNamespace(contract=paths[0], resume_contract=paths[1:], binary=self.binary,
                               operation=operation, output=self.root / "proof", created_output=False)

    def test_complete_contract_receipts_pass_only_offline_validation(self):
        first, resumes = controller.validate_inputs(self.arguments())
        self.assertEqual(first["migration_id"], resumes[1]["migration_id"])

    def test_missing_fields_are_named_before_spawn(self):
        for key in ("kill_event_path", "maximum_campaign_seconds", "resource_caps", "source_commit", "capability_receipt"):
            first = copy.deepcopy(self.contract)
            del first[key]
            args = self.arguments(first)
            with self.subTest(key=key), mock.patch.object(controller, "spawn") as spawn:
                with self.assertRaisesRegex(ValueError, "required_field_missing_or_invalid:" + key):
                    controller.validate_inputs(args)
                spawn.assert_not_called()

    def test_nonpositive_or_boolean_bounds_refuse(self):
        for bound in (0, -1, True, "1"):
            changed = copy.deepcopy(self.contract)
            changed["maximum_campaign_seconds"] = bound
            with self.assertRaisesRegex(ValueError, "required_field_missing_or_invalid:maximum_campaign_seconds"):
                controller.validate_inputs(self.arguments(changed))

    def test_swapped_caps_or_canonical_contract_or_source_refuse(self):
        for key in ("resource_caps", "canonical_contract_sha256", "resource_caps_contract_sha256", "source_manifest_sha256"):
            resume = copy.deepcopy(self.contract)
            resume.update(role="copy_resume")
            resume.pop("killpoint")
            resume[key] = {"changed": 1} if key == "resource_caps" else "changed"
            with self.subTest(key=key), self.assertRaises(ValueError):
                controller.validate_inputs(self.arguments(resumes=[resume, copy.deepcopy(resume)]))

    def test_receipt_plan_swap_and_provenance_swap_refuse(self):
        for key in ("plan_sha256", "migration_id", "resource_caps", "binary_sha256"):
            path = Path(self.contract["seeded_restore_receipt"]["path"])
            original = controller.load(path)
            changed = copy.deepcopy(original)
            changed[key] = "swapped"
            path.unlink()
            controller.write_new(path, changed)
            self.contract["seeded_restore_receipt"]["sha256"] = controller.sha(path)
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "prerequisite_(binding|plan_or_tables)_changed"):
                controller.validate_inputs(self.arguments())
            path.unlink()
            controller.write_new(path, original)
            self.contract["seeded_restore_receipt"]["sha256"] = controller.sha(path)

    def test_snapshot_campaign_consumes_no_database_receipt(self):
        first = copy.deepcopy(self.contract)
        del first["capability_receipt"]
        del first["seeded_restore_receipt"]
        controller.validate_inputs(self.arguments(first, operation="snapshot-campaign"))

    def test_private_fifo_and_executable_control_refuse(self):
        fifo = self.root / "fifo"
        os.mkfifo(fifo, 0o600)
        with self.assertRaisesRegex(ValueError, "private_regular_input_required"):
            controller.load(fifo)
        private = self.root / "executable.json"
        controller.write_new(private, {"synthetic": True})
        private.chmod(0o700)
        with self.assertRaisesRegex(ValueError, "private_regular_input_required"):
            controller.load(private)

    def test_missing_resource_binding_refuse(self):
        for key in ("container_id", "volume_name", "image_id"):
            first = copy.deepcopy(self.contract)
            del first["target_resource_identity"][key]
            with self.assertRaisesRegex(ValueError, "required_field_missing_or_invalid:" + key):
                controller.validate_inputs(self.arguments(first))


if __name__ == "__main__":
    unittest.main()
