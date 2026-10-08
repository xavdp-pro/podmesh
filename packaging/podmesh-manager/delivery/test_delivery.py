"""Focused recorder and topology regressions; declared build host only."""
import hashlib
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

HERE=Path(__file__).resolve().parent

def load(name,path):
    spec=importlib.util.spec_from_file_location(name,HERE/path)
    module=importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

instance=load("manager_delivery_instance","instance.py")
builder=load("manager_delivery_builder","build-deb.py")


class ManagerDeliveryTests(unittest.TestCase):
    def test_runroot_length_refuses_before_any_process_or_bundle_read(self):
        with patch.object(instance,"run") as commands, patch.object(subprocess,"run") as processes:
            for scope in ("manager225-a-20261008", "manager-native-restore-a-20261008"):
                with self.assertRaises(instance.Refusal):
                    instance.Instance(scope)
            commands.assert_not_called()
            processes.assert_not_called()
        longest=instance.BASE/("a"*15)
        self.assertEqual(len(os.fsencode(instance.private_runroot(longest))),50)
        with self.assertRaises(instance.Refusal):
            instance.private_runroot(instance.BASE/("a"*16))

    def topology(self):
        machine="11111111111111111111111111111111"
        local="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        remote="bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        config={"observation_writer_uid":1103,"control_socket":"/run/podmesh-manager/control.sock",
                "network":{"replica_id":local,"database_path":"/var/lib/podmesh-manager/manager.sqlite",
                           "bind":"0.0.0.0:19543","manager":{"logical_manager_id":"cccccccc-cccc-cccc-cccc-cccccccccccc",
                           "replicas":[{"replica_id":local,"host_id":"11111111-1111-1111-1111-111111111111"},
                                       {"replica_id":remote,"host_id":"22222222-2222-2222-2222-222222222222"}]},
                           "peers":[{"replica_id":remote,"endpoint":"192.0.2.11:19543","shared_key_hex":"a"*64}]}}
        plan={"subnet":"10.203.72.0/24","gateway":"10.203.72.1","pod_ip":"10.203.72.2",
              "peer_container_port":19543,"peer_publish":{"host_ip":"192.0.2.10","host_port":19543}}
        return config,plan,machine

    def candidate(self,root):
        candidate=instance.Instance.__new__(instance.Instance)
        candidate.scope="isolated-proof"
        candidate.prefix="podmesh-manager-private-isolated-proof"
        candidate.root=root
        candidate.r={"bundle":"same-bundle","phase":"prepared","units":{},"resources":[]}
        candidate.read=lambda:candidate.r
        candidate.save=lambda:None
        return candidate

    def test_topology_binds_actual_host_and_complete_static_peer(self):
        config,plan,machine=self.topology()
        instance.Instance.validate_inputs(config,plan,machine)
        with self.assertRaises(instance.Refusal):
            instance.Instance.validate_inputs(config,plan,"3"*32)

    def test_root_application_control_identity_refuses(self):
        config,plan,machine=self.topology()
        config["observation_writer_uid"]=0
        with self.assertRaises(instance.Refusal):
            instance.Instance.validate_inputs(config,plan,machine)

    def test_vote_authority_cannot_enter_delivery_configuration(self):
        config,plan,machine=self.topology()
        config["votes"]={"operator_supplied":True}
        with self.assertRaises(instance.Refusal):
            instance.Instance.validate_inputs(config,plan,machine)

    def test_missing_peer_or_noncanonical_hmac_refuses(self):
        config,plan,machine=self.topology()
        config["network"]["peers"][0]["shared_key_hex"]="not-a-key"
        with self.assertRaises(instance.Refusal):
            instance.Instance.validate_inputs(config,plan,machine)
        config["network"]["peers"]=[]
        with self.assertRaises(instance.Refusal):
            instance.Instance.validate_inputs(config,plan,machine)

    def test_wildcard_publication_and_database_publication_refuse(self):
        config,plan,machine=self.topology()
        plan["peer_publish"]["host_ip"]="0.0.0.0"
        with self.assertRaises(instance.Refusal):
            instance.Instance.validate_inputs(config,plan,machine)
        plan["peer_publish"]["host_ip"]="192.0.2.10"
        plan["peer_container_port"]=3306
        config["network"]["bind"]="0.0.0.0:3306"
        with self.assertRaises(instance.Refusal):
            instance.Instance.validate_inputs(config,plan,machine)

    def test_control_uses_actual_functional_caller_not_root(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["app"]="original"
            candidate.check_container=lambda role:None
            candidate.podman=lambda *args,**kwargs:subprocess.CompletedProcess(args,0,b'0\n0\nroot\n{"shutdown_requested":true}',b'')
            with self.assertRaises(instance.Refusal):
                candidate.control_api("shutdown")

    def test_failed_shutdown_ack_never_sends_stop_signal(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["app"]="original"
            candidate.owned=lambda *args:{"State":{"Running":True}}
            candidate.check_container=lambda role:{"State":{"Running":True}}
            candidate.control_api=lambda operation:{"error":"refused"}
            with patch.object(instance,"run") as commands:
                with self.assertRaises(instance.Refusal):
                    candidate.stop_units()
                commands.assert_not_called()

    def test_exit_nonzero_cannot_qualify_shutdown(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["app"]="original"
            state={"State":{"Running":False,"Pid":0,"ExitCode":137,"OOMKilled":False}}
            candidate.check_container=lambda role:state
            candidate.owned=lambda *args:state
            with self.assertRaises(instance.Refusal):
                candidate.shutdown_app()

    def test_interrupted_creation_is_never_adopted(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.no_foreign_containers=lambda:None
            candidate.r["resources"]=[{"kind":"container","name":"new-name","id":None}]
            candidate.inspect=lambda *args:{"Id":"unobserved"}
            with patch.object(instance,"run") as commands:
                with self.assertRaises(instance.Refusal):
                    candidate.rollback()
                commands.assert_not_called()

    def test_partial_sql_import_cannot_start_application(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["recovery"]={"stage":"sql-import-intent"}
            candidate.unitcheck=lambda:None
            candidate.check_container=lambda role:None
            with patch.object(instance,"run") as commands:
                with self.assertRaises(instance.Refusal):
                    candidate.hook("app","permit")
                commands.assert_not_called()

    def test_captured_source_cannot_restart_via_unit_condition(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.unitcheck=lambda:None
            candidate.check_container=lambda role:None
            for phase in ("capture-in-progress","capture-stopped","rolled-back"):
                candidate.r["phase"]=phase
                with self.assertRaises(instance.Refusal):
                    candidate.hook("app","permit")

    def test_control_append_requires_verified_native_recovery(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["recovery"]={"stage":"sql-import-intent"}
            with patch.object(instance,"run") as commands:
                with self.assertRaises(instance.Refusal):
                    candidate.control_request({"operation":"append_observation"})
                commands.assert_not_called()

    def test_replaced_labels_and_id_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            observed={"Id":"original","Config":{"Labels":{instance.LABEL:candidate.scope,"io.podmesh.bundle":"same-bundle"}}}
            candidate.inspect=lambda *args:observed
            candidate.owned("container","original")
            observed["Id"]="replacement"
            with self.assertRaises(instance.Refusal):
                candidate.owned("container","original")
            observed["Id"]="original"
            observed["Config"]["Labels"][instance.LABEL]="foreign"
            with self.assertRaises(instance.Refusal):
                candidate.owned("container","original")

    def test_completed_rollback_is_inert(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["phase"]="rolled-back"
            with patch.object(instance,"run") as commands:
                candidate.rollback()
                commands.assert_not_called()

    def test_changed_unit_refuses_before_any_service_stop(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            candidate=self.candidate(root)
            unit=root/"own.service"
            unit.write_text("changed externally")
            candidate.r["units"][unit.name]=hashlib.sha256(b"original").hexdigest()
            with patch.object(instance,"UNITS",root),patch.object(instance,"protected"),patch.object(instance,"run") as commands:
                with self.assertRaises(instance.Refusal):
                    candidate.rollback()
                commands.assert_not_called()

    def test_rollback_preserves_volumes_and_uses_original_resource_ids(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["resources"]=[{"kind":"volume","name":"app-data","id":"app-data"},
                                       {"kind":"network","name":"own-net","id":"own-net"},
                                       {"kind":"pod","name":"own-pod","id":"pod-original"},
                                       {"kind":"container","name":"own-app","id":"app-original"}]
            candidate.no_foreign_containers=lambda:None
            candidate.owned=lambda *args:{"State":{"Running":False}}
            seen=[]
            candidate.podman=lambda *args:seen.append(args)
            with patch.object(instance,"run"):
                candidate.rollback()
            self.assertEqual(seen,[("container","rm","app-original"),("pod","stop","pod-original"),
                                   ("pod","rm","pod-original"),("network","rm","own-net")])
            self.assertEqual(candidate.r["resources"][0]["id"],"app-data")
            self.assertEqual(candidate.r["phase"],"rolled-back")

    def test_foreign_container_after_stop_prevents_any_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            seen=[]
            def oracle():
                seen.append("absence")
                if len(seen)==2:
                    raise instance.Refusal("foreign container arrived during shutdown")
            candidate.no_foreign_containers=oracle
            candidate.r["resources"]=[{"kind":"container","name":"own-app","id":"app-original"}]
            candidate.owned=lambda *args:{"State":{"Running":False}}
            candidate.podman=lambda *args:self.fail("removal after failed post-stop oracle")
            with patch.object(instance,"run"):
                with self.assertRaises(instance.Refusal):
                    candidate.rollback()

    def test_created_pod_plan_is_distinct_from_running_network_observation(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["network_plan"]={"pod_ip":"10.203.72.2","gateway":"10.203.72.1"}
            pod={"InfraConfig":{"HostNetwork":False,"Networks":[candidate.prefix+"-net"],"StaticIP":"10.203.72.2"}}
            infra={"State":{"Running":False},"NetworkSettings":{"Networks":{}}}
            candidate.check_infra_network(pod,infra)
            infra["State"]["Running"]=True
            with self.assertRaises(instance.Refusal):
                candidate.check_infra_network(pod,infra)
            infra["NetworkSettings"]["Networks"]={candidate.prefix+"-net":{"IPAddress":"10.203.72.2","Gateway":"10.203.72.1"}}
            candidate.check_infra_network(pod,infra)
            infra["NetworkSettings"]["Networks"][candidate.prefix+"-net"]["IPAddress"]="10.203.72.3"
            with self.assertRaises(instance.Refusal):
                candidate.check_infra_network(pod,infra)

    def test_empty_static_ip_uses_recorded_intent_and_still_checks_running_addresses(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate=self.candidate(Path(directory))
            candidate.r["network_plan"]={"pod_ip":"10.203.72.2","gateway":"10.203.72.1"}
            # Observed Podman 5.4 schema before start: no assigned IP or IPAMConfig.
            command=["/usr/bin/podman","pod","create","--network",candidate.prefix+"-net","--ip","10.203.72.2"]
            pod={"InfraConfig":{"HostNetwork":False,"Networks":[candidate.prefix+"-net"],"StaticIP":""},"CreateCommand":command}
            infra={"State":{"Running":False},"NetworkSettings":{"Networks":{candidate.prefix+"-net":{"IPAddress":"","Gateway":"","IPAMConfig":None}}}}
            candidate.check_infra_network(pod,infra)
            for invalid in ([],command[:-1],command[:-1]+["10.203.72.3"],command+["--ip=10.203.72.3"],command+["--ip","10.203.72.2"]):
                pod["CreateCommand"]=invalid
                with self.assertRaises(instance.Refusal):
                    candidate.check_infra_network(pod,infra)
            pod["CreateCommand"]=command
            infra["State"]["Running"]=True
            with self.assertRaises(instance.Refusal):
                candidate.check_infra_network(pod,infra)
            infra["NetworkSettings"]["Networks"][candidate.prefix+"-net"].update(IPAddress="10.203.72.2",Gateway="10.203.72.1")
            candidate.check_infra_network(pod,infra)
            infra["NetworkSettings"]["Networks"][candidate.prefix+"-net"]["Gateway"]="10.203.72.3"
            with self.assertRaises(instance.Refusal):
                candidate.check_infra_network(pod,infra)

    def test_oci_compressed_integrity_does_not_replace_rootfs_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"wrong-rootfs.tar"
            layer=b"valid descriptor but wrong uncompressed bytes"
            digest=hashlib.sha256(layer).hexdigest()
            config=json.dumps({"architecture":"amd64","os":"linux","config":{},"rootfs":{"diff_ids":["sha256:"+"0"*64]}}).encode()
            config_id=hashlib.sha256(config).hexdigest()
            manifest=json.dumps({"config":{"digest":"sha256:"+config_id,"size":len(config)},
                                 "layers":[{"digest":"sha256:"+digest,"size":len(layer),"mediaType":"application/vnd.oci.image.layer.v1.tar"}]}).encode()
            manifest_id=hashlib.sha256(manifest).hexdigest()
            index=json.dumps({"manifests":[{"digest":"sha256:"+manifest_id,"size":len(manifest)}]}).encode()
            with tarfile.open(path,"w") as archive:
                for name,data in (("index.json",index),("blobs/sha256/"+manifest_id,manifest),
                                  ("blobs/sha256/"+config_id,config),("blobs/sha256/"+digest,layer)):
                    entry=tarfile.TarInfo(name)
                    entry.size=len(data)
                    archive.addfile(entry,io.BytesIO(data))
            with self.assertRaisesRegex(ValueError,"rootfs diff ID differs"):
                builder.oci(path,config_id)
            self.assertEqual(list(Path(directory).iterdir()),[path])


if __name__=="__main__":
    unittest.main()
