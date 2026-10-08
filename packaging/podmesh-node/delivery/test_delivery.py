"""Recorder regressions only; run on declared build host, no Podman/systemd calls."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent

def load(name, filename):
    spec = importlib.util.spec_from_file_location(name,HERE/filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

instance = load("private_delivery_instance","instance.py")
builder = load("private_delivery_builder","build-deb.py")


class DeliveryTests(unittest.TestCase):
    def candidate(self, root):
        candidate = instance.Instance.__new__(instance.Instance)
        candidate.scope = "isolated-proof"
        candidate.prefix = "podmesh-node-private-isolated-proof"
        candidate.root = root
        candidate.r = {"bundle":"immutable-bundle", "phase":"prepared", "units":{}, "resources":[]}
        candidate.read = lambda: candidate.r
        candidate.save = lambda: None
        return candidate

    def test_image_ids_accept_both_observed_serializations_and_refuse_partial(self):
        digest = "a"*64
        self.assertEqual(instance.image_id(digest),instance.image_id("sha256:"+digest))
        for value in (digest[:12],"sha256:"+"A"*64,None,"latest"):
            with self.assertRaises(instance.Refusal):
                instance.image_id(value)

    def test_ownership_requires_both_labels_and_original_id(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate = self.candidate(Path(directory))
            data = {"Id":"original","Config":{"Labels":{instance.LABEL:candidate.scope,
                                                         "io.podmesh.bundle":"immutable-bundle"}}}
            candidate.inspect = lambda *args:data
            self.assertEqual(candidate.owned("container","original"),data)
            data["Id"] = "replacement"
            with self.assertRaises(instance.Refusal):
                candidate.owned("container","original")
            data["Id"] = "original"
            data["Config"]["Labels"][instance.LABEL] = "foreign"
            with self.assertRaises(instance.Refusal):
                candidate.owned("container","original")

    def test_remaining_workload_refuses_before_any_service_or_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate = self.candidate(Path(directory))
            def refuse():
                raise instance.Refusal("remaining workload")
            candidate.assert_no_workloads = refuse
            with patch.object(instance,"run") as commands:
                with self.assertRaises(instance.Refusal):
                    candidate.rollback()
                commands.assert_not_called()

    def test_interrupted_create_is_never_adopted_by_name(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate = self.candidate(Path(directory))
            candidate.assert_no_workloads = lambda:None
            candidate.r["resources"] = [{"kind":"container","name":"known-name","id":None}]
            candidate.inspect = lambda *args:{"Id":"unobserved"}
            with patch.object(instance,"run") as commands:
                with self.assertRaises(instance.Refusal):
                    candidate.rollback()
                commands.assert_not_called()

    def test_changed_unit_refuses_before_service_stop(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = self.candidate(root)
            unit = root / "owned.service"
            unit.write_text("modified by another operator")
            candidate.r["units"][unit.name] = hashlib.sha256(b"original").hexdigest()
            with patch.object(instance,"UNITS",root), patch.object(instance,"protected"), patch.object(instance,"run") as commands:
                with self.assertRaises(instance.Refusal):
                    candidate.rollback()
                commands.assert_not_called()

    def test_completed_rollback_is_inert(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate = self.candidate(Path(directory))
            candidate.r["phase"] = "rolled-back"
            with patch.object(instance,"run") as commands:
                candidate.rollback()
                commands.assert_not_called()

    def test_rollback_retains_volume_and_uses_only_recorded_ids(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate = self.candidate(Path(directory))
            candidate.assert_no_workloads = lambda:None
            candidate.r["resources"] = [{"kind":"volume","name":"data","id":"data"},
                                         {"kind":"pod","name":"pod-name","id":"pod-original"},
                                         {"kind":"container","name":"app-name","id":"app-original"}]
            candidate.owned = lambda kind,identity:{"State":{"Running":False}}
            seen = []
            candidate.podman = lambda *args:seen.append(args)
            with patch.object(instance,"run"):
                candidate.rollback()
            self.assertEqual(seen,[("container","rm","app-original"),("pod","stop","pod-original"),
                                   ("pod","rm","pod-original")])
            self.assertEqual(candidate.r["phase"],"rolled-back")
            self.assertEqual(candidate.r["resources"][0]["id"],"data")

    def test_forced_stop_does_not_report_stopped(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate = self.candidate(Path(directory))
            candidate.r.update(app="app-original",db="db-original")
            candidate.unitcheck = lambda:None
            candidate.check_container = lambda role:None
            candidate.owned = lambda *args:{"State":{"Running":False,"Pid":0,"ExitCode":137,"OOMKilled":False}}
            with patch.object(instance,"run"):
                with self.assertRaises(instance.Refusal):
                    candidate.control("stop")
            self.assertEqual(candidate.r["phase"],"prepared")
            self.assertEqual(candidate.r["stop_observations"]["app"]["ExitCode"],137)

    def test_oci_descriptor_mismatch_refuses_without_extracting(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory)/"wrong.tar"
            config = json.dumps({"architecture":"amd64","os":"linux","config":{}}).encode()
            config_id = hashlib.sha256(config).hexdigest()
            config_desc = {"digest":"sha256:"+config_id,"size":len(config)}
            layer = b"changed layer"
            manifest = json.dumps({"config":config_desc,"layers":[{"digest":"sha256:"+"a"*64,"size":len(layer)}]}).encode()
            manifest_id = hashlib.sha256(manifest).hexdigest()
            index = json.dumps({"manifests":[{"digest":"sha256:"+manifest_id,"size":len(manifest)}]}).encode()
            with tarfile.open(archive,"w") as output:
                for name,data in (("index.json",index),("blobs/sha256/"+manifest_id,manifest),
                                  ("blobs/sha256/"+config_id,config),("blobs/sha256/"+"a"*64,layer)):
                    entry = tarfile.TarInfo(name)
                    entry.size = len(data)
                    output.addfile(entry,io.BytesIO(data))
            with self.assertRaisesRegex(ValueError,"OCI content digest mismatch"):
                builder.oci(archive,config_id)
            self.assertEqual(sorted(p.name for p in Path(directory).iterdir()),["wrong.tar"])


if __name__ == "__main__":
    unittest.main()
