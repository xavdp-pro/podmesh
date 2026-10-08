"""CT9110 only: bounded filesystem regressions; never invoke host providers.

The GNU tar roundtrip is intentional. All other tests use local temporary data
and reject any subprocess. No import or execution is authorized on NOW7.
"""
import copy
import importlib.util
import io
import json
import os
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


class RecoveryTests(unittest.TestCase):
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
