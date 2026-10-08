"""Bounded filesystem regressions; never invoke host providers.

The GNU tar roundtrip is intentional. All other tests use local temporary data
and reject any subprocess. Use an explicitly designated isolated test host.
"""
import copy
import importlib.util
import io
import json
import os
import sqlite3
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("manager_native_recovery", HERE / "recovery.py")
recovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recovery)


class RealEngineLayoutTests(unittest.TestCase):
    """Require a private isolated copy of real engine metadata, never a live root.

    The fixture host reconstructs only metadata and payload/volume directories
    from a stopped capture, preserving root path bindings and original bytes.
    No private application inputs, image layer bytes or provider calls are needed.
    """
    def fixture(self):
        descriptor = Path(os.environ["PODMESH_ENGINE_LAYOUT_FIXTURE"])
        obj = json.loads(descriptor.read_text())
        self.assertTrue(obj["isolated_metadata_copy"])
        self.assertEqual(obj["version"], "5.4.2")
        root = Path(obj["root"])
        self.assertTrue(root.is_absolute())
        return root, obj

    def check(self):
        root, obj = self.fixture()
        layout = recovery.closed_graphroot(root, set(obj["images"]), set(obj["containers"]),
            set(obj["volumes"]), obj["version"], obj["pod"], "sqlite")
        recovery.closed_runtime(root, set(obj["containers"]), obj["version"])
        return layout

    def test_real_stopped_sqlite_graphroot_and_runtime_pass_without_rewriting(self):
        root, obj = self.fixture()
        before = recovery.tree(root)
        with patch.object(subprocess, "run", side_effect=AssertionError("provider forbidden")):
            result = self.check()
        self.assertEqual(result["engine_sqlite"]["sha256"], obj["database_sha256"])
        self.assertEqual(result["engine_secrets"], "empty-lock-only")
        self.assertEqual(result["engine_database_backend"], "sqlite")
        self.assertEqual(recovery.tree(root), before)

    def test_real_engine_database_unknown_schema_resource_and_sidecar_refuse(self):
        root, obj = self.fixture()
        database = root / "graphroot/db.sql"
        original = database.read_bytes()
        try:
            for statement in ("CREATE TABLE foreign_state(value TEXT)",
                              "INSERT INTO IDNamespace VALUES ('" + "f" * 64 + "')"):
                try:
                    with sqlite3.connect(database) as connection:
                        connection.execute(statement)
                    connection.close()
                    with self.assertRaisesRegex(ValueError, "schema|resource identity"):
                        self.check()
                finally:
                    database.write_bytes(original)
            sidecar = database.with_name("db.sql-wal")
            sidecar.write_bytes(b"preserve-unknown-sidecar")
            try:
                with self.assertRaisesRegex(ValueError, "sidecar|graphroot component"):
                    self.check()
            finally:
                sidecar.unlink()
        finally:
            database.write_bytes(original)
        self.assertEqual(recovery.digest(database), obj["database_sha256"])

    def test_real_empty_secrets_unknown_runtime_and_oci_mount_refuse(self):
        root, obj = self.fixture()
        secret = root / "graphroot/secrets/secrets.json"
        secret.write_bytes(b"{}")
        try:
            with self.assertRaisesRegex(ValueError, "secret store"):
                self.check()
        finally:
            secret.unlink()
        unknown = root / "tmp/persist/foreign-state"
        unknown.write_bytes(b"preserve")
        try:
            with self.assertRaisesRegex(ValueError, "exit records"):
                self.check()
        finally:
            unknown.unlink()
        configuration = next((root / "graphroot/vfs-containers").glob("*/userdata/config.json"))
        original = configuration.read_bytes()
        try:
            config = json.loads(original)
            config["mounts"].append({"type": "bind", "source": "/foreign/private", "destination": "/foreign"})
            configuration.write_text(json.dumps(config))
            with self.assertRaisesRegex(ValueError, "OCI bookkeeping mount"):
                self.check()
        finally:
            configuration.write_bytes(original)

    def test_real_empty_volatile_metadata_nonempty_and_symlink_refuse(self):
        root, obj = self.fixture()
        for name in ("vfs-containers/volatile-containers.json", "vfs-layers/volatile-layers.json"):
            path = root / "graphroot" / name
            original, mode = path.read_bytes(), path.stat().st_mode & 0o777
            self.assertEqual(original, b"[]")
            try:
                path.write_bytes(b'[{"id":"foreign"}]')
                with self.assertRaisesRegex(ValueError, "volatile VFS state"):
                    self.check()
                path.unlink()
                path.symlink_to(root / "graphroot/db.sql")
                with self.assertRaisesRegex(ValueError, "volatile VFS state"):
                    self.check()
            finally:
                if path.is_symlink():
                    path.unlink()
                path.write_bytes(original)
                path.chmod(mode)
        self.check()

    def test_real_unsigned_image_unknown_metadata_and_signature_state_refuse(self):
        root, obj = self.fixture()
        path = root / "graphroot/vfs-images/images.json"
        original = path.read_bytes()
        try:
            for state in ({"unknown-state": True}, {"signatures-sizes": {"foreign": [1]}}):
                rows = json.loads(original)
                rows[0]["metadata"] = json.dumps(state)
                path.write_text(json.dumps(rows))
                with self.assertRaisesRegex(ValueError, "image metadata/signature state"):
                    self.check()
            rows = json.loads(original)
            self.assertEqual(json.loads(rows[0]["metadata"]), {})
            rows[0]["big-data-names"].append("signature-" + rows[0]["digest"].removeprefix("sha256:"))
            path.write_text(json.dumps(rows))
            with self.assertRaisesRegex(ValueError, "image metadata/signature state"):
                self.check()
        finally:
            path.write_bytes(original)
        self.check()


class RecoveryTests(unittest.TestCase):
    def test_oracle_uses_durable_migration_marker_not_legacy_history_version(self):
        instance = SimpleNamespace(prefix="owned", r={"replica_id": "original"})
        def results(candidate, statement):
            self.assertIs(candidate, instance)
            if statement.startswith("SELECT CURRENT_USER()"):
                return "podmesh-manager@%\tpodmesh-manager\towned-db\n"
            if statement == "SHOW TABLES;":
                return "\n".join(recovery.TABLES) + "\n"
            if "information_schema.TRIGGERS" in statement:
                return "8\n"
            if "information_schema.ROUTINES" in statement:
                return "0\n0\n"
            if statement.startswith("SELECT version FROM store_schema"):
                return marker + "\n"
            if statement.startswith("SELECT replica_id FROM identity"):
                return "original\n"
            if statement == "SHOW GRANTS FOR CURRENT_USER;":
                return "GRANT ALL PRIVILEGES ON `podmesh-manager`.* TO `podmesh-manager`@`%`\n"
            self.fail("unexpected oracle SQL")
        marker = "1"
        with patch.object(recovery, "query", side_effect=results), patch.object(
                recovery, "inspect_quiescent_store", return_value={"incomplete_attempts": [], "ordered_audit_events": []}):
            self.assertEqual(recovery.oracle(instance)["tables"], recovery.TABLES)
            marker = "3"
            with self.assertRaisesRegex(ValueError, "DurableStore migration version"):
                recovery.oracle(instance)

    def uncertainty(self):
        # Shape returned by the product's canonical validator, not a replacement
        # validation machine. Include all inbound prefix classes, not just replies.
        attempts, rows = [], []
        for direction, phase in (("outbound", "outbound_request_prepared"),
                                 ("inbound", "inbound_request_observed"),
                                 ("inbound", "inbound_import_committed"),
                                 ("inbound", "inbound_refusal_recorded"),
                                 ("inbound", "inbound_reply_prepared")):
            identifier = "attempt:" + phase
            attempts.append({"direction": direction, "attempt_id": identifier,
                             "wire_nonce": "nonce-" + phase, "wire_operation_id": None, "last_phase": phase})
            rows.append({"event": {"audit_event_id": "audit-" + phase, "direction": direction,
                                   "attempt_id": identifier, "phase": phase}, "sha256": "a" * 64})
        return {"incomplete_attempts": attempts, "ordered_audit_events": rows}

    def test_uncertainty_retains_exact_canonical_attempts_and_all_prefix_evidence(self):
        inspection = self.uncertainty()
        # A multi-row prefix must retain every predecessor, not just its last row.
        predecessor = copy.deepcopy(inspection["ordered_audit_events"][-1])
        predecessor["event"]["audit_event_id"] = "predecessor"
        predecessor["event"]["phase"] = "inbound_request_observed"
        inspection["ordered_audit_events"].append(predecessor)
        inventory = recovery.uncertainty_inventory(inspection)
        self.assertEqual(len(inventory["attempts"]), 5)
        self.assertEqual(len(inventory["audit_events"]), 6)
        self.assertEqual(set(row["sha256"] for row in inventory["audit_events"]), {"a" * 64})
        recovery.same_uncertainty(inventory, copy.deepcopy(inventory))

    def test_uncertainty_refuses_loss_alteration_addition_and_synthetic_completion(self):
        original = recovery.uncertainty_inventory(self.uncertainty())
        lost = copy.deepcopy(original)
        lost["attempts"].pop()
        altered = copy.deepcopy(original)
        altered["audit_events"][0]["sha256"] = "b" * 64
        added = copy.deepcopy(original)
        added["attempts"].append({"attempt_id": "new"})
        completed = {"type": original["type"], "attempts": [], "audit_events": []}
        for candidate in (lost, altered, added, completed):
            with self.subTest(candidate=candidate), self.assertRaisesRegex(ValueError, "uncertainty lost, altered or added"):
                recovery.same_uncertainty(original, candidate)

    def test_uncertainty_refuses_duplicate_or_missing_canonical_evidence(self):
        inspection = self.uncertainty()
        inspection["incomplete_attempts"].append(inspection["incomplete_attempts"][0])
        with self.assertRaisesRegex(ValueError, "duplicate canonical"):
            recovery.uncertainty_inventory(inspection)
        inspection = self.uncertainty()
        inspection["ordered_audit_events"].pop()
        with self.assertRaisesRegex(ValueError, "lacks audit evidence"):
            recovery.uncertainty_inventory(inspection)

    def test_canonical_recovery_inspection_refuses_live_app_before_provider(self):
        with tempfile.TemporaryDirectory() as directory:
            instance = SimpleNamespace(root=Path(directory), check_container=lambda role:
                {"State": {"Running": True, "Pid": 42, "ExitCode": 0}},
                podman=lambda *args, **kwargs: self.fail("live APP must prevent provider call"))
            with self.assertRaisesRegex(ValueError, "quiescent clean APP"):
                recovery.inspect_quiescent_store(instance)

    def test_canonical_recovery_inspection_reuses_exact_image_read_only_and_propagates_invalid_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            calls = []
            inspection = dict(self.uncertainty(), replica_id="original")
            def provider(*args, **kwargs):
                calls.append(args)
                return SimpleNamespace(stdout=json.dumps(inspection).encode())
            instance = SimpleNamespace(root=Path(directory), prefix="owned", scope="scope",
                r={"bundle": "immutable", "pod": "own-pod", "replica_id": "original"},
                manifest={"application_image": "sha256:" + "a" * 64},
                check_container=lambda role: {"State": {"Running": False, "Pid": 0, "ExitCode": 0}},
                inspect=lambda *args: None, podman=provider)
            self.assertEqual(recovery.inspect_quiescent_store(instance), inspection)
            command = calls[0]
            for argument in ("--rm", "--user=1103:1103", "--read-only", "--inspect-store",
                             "owned-app:/var/lib/podmesh-manager:ro", "own-pod", instance.manifest["application_image"]):
                self.assertIn(argument, command)
            def invalid(*args, **kwargs):
                raise ValueError("canonical store corrupt: invalid audit prefix")
            instance.podman = invalid
            with self.assertRaisesRegex(ValueError, "invalid audit prefix"):
                recovery.inspect_quiescent_store(instance)

    def test_engine_version_provenance_does_not_substitute_for_schema_validation(self):
        calls = []
        def provider(*args):
            calls.append(args)
            self.assertEqual(args, ("--version",))
            return SimpleNamespace(stdout=b"podman version 5.4.2\n")
        self.assertEqual(recovery.engine_version(SimpleNamespace(podman=provider)), "5.4.2")
        self.assertEqual(calls, [("--version",)])

    def inputs(self, root):
        for directory, files in (("app-config", ("config.json", "store.json", "passwd")),
                                 ("db-admin", ("passwd",)), ("api", ()), ("r", ()), ("tmp", ())):
            (root / directory).mkdir()
            for name in files:
                (root / directory / name).write_bytes(b"owned-input")

    def graph(self, root):
        graph = root / "graphroot"
        graph.mkdir()
        ids = {"images": "a" * 64, "containers": "b" * 64, "layers": "c" * 64}
        for kind in ids:
            directory = graph / ("vfs-" + kind)
            directory.mkdir()
            row = {"id": ids[kind]}
            if kind != "layers":
                row["layer"] = "d" * 64 if kind == "containers" else ids["layers"]
            if kind == "containers":
                row["image"] = ids["images"]
            rows = [row]
            if kind == "layers":
                rows.append({"id": "d" * 64, "parent": ids["layers"]})
            (directory / (kind + ".json")).write_text(json.dumps(rows))
            if kind == "images":
                (directory / ids[kind]).mkdir()
            if kind == "containers":
                (directory / ids[kind] / "userdata").mkdir(parents=True)
        (graph / "vfs" / "dir" / ids["layers"]).mkdir(parents=True)
        (graph / "vfs" / "dir" / ("d" * 64)).mkdir()
        (graph / "volumes" / "own-app" / "_data").mkdir(parents=True)
        (graph / "libpod").mkdir()
        return graph, ids

    def test_extra_configuration_rejected_without_process(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(subprocess, "run", side_effect=AssertionError("process")):
            root = Path(directory)
            self.inputs(root)
            recovery.closed_inputs(root, stopped=True)
            (root / "app-config" / "extra-state.json").write_text("durable")
            with self.assertRaisesRegex(ValueError, "unmapped"):
                recovery.closed_inputs(root)

    def test_unknown_root_and_stopped_transient_file_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.inputs(root)
            unknown = root / "unknown-durable-state"
            unknown.write_text("preserve")
            with self.assertRaises(ValueError):
                recovery.closed_inputs(root)
            unknown.unlink()
            (root / "tmp" / "unknown-state").write_text("preserve")
            with self.assertRaises(ValueError):
                recovery.closed_runtime(root, set())

    def test_foreign_graphroot_and_unknown_layer_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            graph, ids = self.graph(root)
            check = lambda: recovery.closed_graphroot(root, {ids["images"]}, {ids["containers"]}, {"own-app"})
            check()
            (graph / "unmapped-durable-file").write_text("durable")
            with self.assertRaisesRegex(ValueError, "unmapped"):
                check()
            (graph / "unmapped-durable-file").unlink()
            path = graph / "vfs-layers/layers.json"
            rows = json.loads(path.read_text())
            rows.append({"id": "e" * 64})
            path.write_text(json.dumps(rows))
            with self.assertRaisesRegex(ValueError, "unmapped durable VFS layer"):
                check()

    def test_external_metadata_symlink_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            graph, ids = self.graph(root)
            path = graph / "vfs-images/images.json"
            path.unlink()
            path.symlink_to("/etc/passwd")
            with self.assertRaises(ValueError):
                recovery.closed_graphroot(root, {ids["images"]}, {ids["containers"]}, {"own-app"})

    def test_durable_writable_layer_has_no_sql_only_fallback(self):
        recovery.safe_diff([{"Path": "/run/mysqld", "Kind": 1}], "db")
        for role, path in (("db", "/etc/extra-state"), ("app", "/tmp/state"), ("infra", "/run/state")):
            with self.assertRaises(ValueError):
                recovery.safe_diff([{"Path": path, "Kind": 1}], role)

    def test_original_scope_owner_required(self):
        config = {"network": {"manager": {"grants": [{"scope": "custom/source", "owner_replica_id": "original"}]}}}
        previous = {"request": {"scope": "custom/source"}}
        self.assertEqual(recovery.observation_scope(config, "original", previous), "custom/source")
        with self.assertRaises(ValueError):
            recovery.observation_scope(config, "different", previous)

    def test_all_five_histories_preserved(self):
        original = {table: ["original-" + table] for table in recovery.TABLES}
        extended = copy.deepcopy(original)
        for table in ("facts", "receipts", "exchange_audit_events"):
            extended[table].append("new-" + table)
        recovery.preserved_rows(original, extended)
        for table in recovery.TABLES:
            damaged = copy.deepcopy(extended)
            damaged[table].remove("original-" + table)
            with self.assertRaises(ValueError):
                recovery.preserved_rows(original, damaged)

    def test_captured_operation_cannot_be_used_as_fresh_nominal(self):
        original = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        fresh = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        snapshot = {"receipts": [json.dumps([original, "observe", None, None, "request", "response", "hash"]).encode().hex()]}
        recovery.absent_operation(snapshot, fresh)
        with self.assertRaises(ValueError):
            recovery.absent_operation(snapshot, original)

    def test_every_peer_and_fresh_ack_required(self):
        peers = {"p1", "p2"}
        status = {"catch_up": {"caught_up": True, "caught_up_by": "every_peer", "peers_matched": list(peers)},
                  "peers": {peer: {"authenticated_successes": 2, "last_success_age_ms": 1,
                            "acknowledged_unchanged": True, "outcome": "authenticated_import_receipt",
                            "acknowledged_history_len": 4, "local_history_len_at_attempt": 4,
                            "history_count_delta": 0} for peer in peers}}
        recovery.peer_complete(status, peers, 4, {peer: 1 for peer in peers})
        status["catch_up"]["caught_up_by"] = "window"
        with self.assertRaises(ValueError):
            recovery.peer_complete(status, peers)
        status["catch_up"]["caught_up_by"] = "every_peer"
        status["peers"]["p2"]["authenticated_successes"] = 1
        with self.assertRaises(ValueError):
            recovery.peer_complete(status, peers, 4, {peer: 1 for peer in peers})

    def test_gnu_tar_first_link_differs_from_manifest_anchor(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            root = parent / "source"
            root.mkdir()
            (root / "z/deep").mkdir(parents=True)
            # Explicit tar ordering emits z/deep/data before its lexical anchor a.
            data = root / "z/deep/data"
            with data.open("wb") as stream:
                stream.seek(1024 * 1024)
                stream.write(b"sparse-durable")
            os.link(data, root / "a")
            os.setxattr(data, "user.recovery", b"metadata")
            (root / "alias").symlink_to("a")
            rows = recovery.tree(root)
            archive = parent / "source.tar"
            subprocess.run(["/usr/bin/tar", "--format=pax", "--xattrs", "--acls", "--sparse", "--numeric-owner",
                            "--no-recursion", "-cpf", str(archive), "-C", str(parent), "source", "source/z",
                            "source/z/deep", "source/z/deep/data", "source/a", "source/alias"], check=True,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            with tarfile.open(archive) as handle:
                self.assertTrue(handle.getmember("source/a").islnk())
                self.assertEqual(handle.getmember("source/a").linkname, "source/z/deep/data")
            target = parent / "target"
            target.mkdir()
            recovery.restore_archive(archive, rows, "source", target)
            self.assertEqual(recovery.tree(target), rows)
            self.assertEqual((target / "a").stat().st_ino, (target / "z/deep/data").stat().st_ino)

    def test_external_hardlink_and_manifest_traversal_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            root = parent / "root"
            root.mkdir()
            outside = parent / "outside"
            outside.write_text("external")
            os.link(outside, root / "alias")
            with self.assertRaises(ValueError):
                recovery.tree(root)
            (root / "alias").unlink()
            rows = recovery.tree(root)
            rows["../outside"] = copy.deepcopy(rows["."])
            with self.assertRaises(ValueError):
                recovery.validate_tree(rows)

    def test_manifest_missing_parent_and_inconsistent_hardlink_metadata_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a").write_bytes(b"owned")
            os.link(root / "a", root / "z")
            rows = recovery.tree(root)
            recovery.validate_tree(rows)
            inconsistent = copy.deepcopy(rows)
            inconsistent["z"]["mode"] ^= 0o100
            with self.assertRaises(ValueError):
                recovery.validate_tree(inconsistent)
            missing = copy.deepcopy(rows)
            missing["absent/child"] = copy.deepcopy(rows["."])
            with self.assertRaises(ValueError):
                recovery.validate_tree(missing)

    def test_duplicate_and_outside_archive_members_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            root = parent / "source"
            root.mkdir()
            (root / "data").write_bytes(b"owned")
            rows = recovery.tree(root)
            for kind in ("duplicate", "outside", "foreign-hardlink"):
                archive = parent / (kind + ".tar")
                with tarfile.open(archive, "w") as handle:
                    handle.add(root, arcname="source")
                    member = tarfile.TarInfo("../escape" if kind == "outside" else "source/data")
                    if kind == "foreign-hardlink":
                        # Replace data with an outside link in an otherwise complete archive.
                        member.name = "source/extra"
                        rows_extra = copy.deepcopy(rows)
                        rows_extra["extra"] = copy.deepcopy(rows["data"])
                        member.type = tarfile.LNKTYPE
                        member.linkname = "outside/data"
                        handle.addfile(member)
                    else:
                        member.size = 5
                        handle.addfile(member, io.BytesIO(b"owned"))
                target = parent / kind
                target.mkdir()
                with self.assertRaises(ValueError):
                    recovery.restore_archive(archive, rows_extra if kind == "foreign-hardlink" else rows, "source", target)
                self.assertEqual(list(target.iterdir()), [])

    def test_both_new_receipts_must_link_to_prepared_requests(self):
        events = []
        for peer in ("p1", "p2"):
            prepared = {"audit_event_id": "prepared-" + peer, "attempt_id": peer,
                        "phase": "outbound_request_prepared", "direction": "outbound",
                        "wire_nonce": peer, "request_sha256": "a" * 64}
            completed = {**prepared, "audit_event_id": "completed-" + peer,
                         "phase": "outbound_exchange_completed", "outcome": "accepted",
                         "authenticated_peer_id": peer, "remote_receipt_operation_id": "remote-" + peer,
                         "remote_receipt_sha256": "b" * 64}
            events.extend((prepared, completed))
        self.assertEqual(set(recovery.linked_peer_receipts(events, set(), {"p1", "p2"})), {"p1", "p2"})
        with self.assertRaises(ValueError):
            recovery.linked_peer_receipts(events, {"completed-p2"}, {"p1", "p2"})
        events[-1]["request_sha256"] = "c" * 64
        with self.assertRaises(ValueError):
            recovery.linked_peer_receipts(events, set(), {"p1", "p2"})


if __name__ == "__main__":
    unittest.main()
