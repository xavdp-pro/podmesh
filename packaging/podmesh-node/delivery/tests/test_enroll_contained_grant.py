"""Standard-library guards; run on the explicit build host, never a workstation."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('enrollment', Path(__file__).resolve().parents[1] / 'enroll-contained-grant.py')
tool = importlib.util.module_from_spec(spec);spec.loader.exec_module(tool)
A = 'd8304d1a-fdd4-49fc-9af2-1926f1d03af7'
B = 'e029d6fa-35c8-4e23-a87d-e530f1a1492b'
C = 'e6ed3f27-2b3e-426a-8863-c79d706ae9db'


def grant(identity, root, uid, gid):
    return {'universe_uuid':identity,'image':'a'*64,'containment':'userns-v1',
        'uid_start':uid,'gid_start':gid,'id_count':65536,'volume_root':root,
        'mounts':[{'source':root+'/etc','destination':'/etc/shaper','read_only':False},
                  {'source':root+'/saved','destination':'/var/lib/shaper','read_only':False}], 'network':'scoped-net'}


def fixture():
    old = {'scope':'operator-owned','max_cpus':2,'allowed_operations':['create','start','stop'],
           'allowed_contained_nested':[grant(A,'/srv/universes/source',200000,300000),
                                        grant(B,'/srv/universes/previous',400000,500000)]}
    new = copy.deepcopy(old);new['allowed_contained_nested'].append(grant(C,'/srv/universes/receiving',600000,800000))
    return old, new


def encoded(value): return (json.dumps(value,indent=2)+'\n').encode()


class AppendGuards(unittest.TestCase):
    def test_only_one_new_grant_preserves_all_old_descriptor_bytes(self):
        old,new=fixture();before,after,added=tool.append_only(encoded(old),encoded(new),A)
        self.assertEqual(before,old);self.assertEqual(after,new);self.assertEqual(added['universe_uuid'],C)

    def test_non_grant_policy_bytes_and_old_grants_cannot_change(self):
        old,new=fixture()
        changed=copy.deepcopy(new);changed['max_cpus']=3
        with self.assertRaises(tool.Refusal):tool.append_only(encoded(old),encoded(changed),A)
        changed=copy.deepcopy(new);changed['allowed_contained_nested'][0]['network']='other'
        with self.assertRaises(tool.Refusal):tool.append_only(encoded(old),encoded(changed),A)
        with self.assertRaises(tool.Refusal):tool.append_only(encoded(old),encoded(new).replace(b'"max_cpus": 2',b'"max_cpus":  2'),A)

    def test_mapping_uuid_root_network_and_mount_broadening_refuse(self):
        old,new=fixture()
        for key,value in [('universe_uuid',A),('uid_start',200001),('gid_start',300001),
                          ('uid_start',0),('uid_start',2**32),('id_count',1048576),
                          ('network','host'),('image','b'*64),('containment','legacy-host-root'),
                          ('volume_root','/srv/universes/source/subdir')]:
            changed=copy.deepcopy(new);changed['allowed_contained_nested'][-1][key]=value
            with self.subTest(key=key,value=value),self.assertRaises((tool.Refusal,ValueError)):
                tool.append_only(encoded(old),encoded(changed),A)
        changed=copy.deepcopy(new);changed['allowed_contained_nested'][-1]['mounts'][0]['read_only']=True
        with self.assertRaises(tool.Refusal):tool.append_only(encoded(old),encoded(changed),A)
        changed=copy.deepcopy(new);changed['allowed_contained_nested'][-1]['mounts'][0]['source']='/etc'
        with self.assertRaises(tool.Refusal):tool.append_only(encoded(old),encoded(changed),A)

    def test_reformat_old_descriptor_duplicate_field_and_second_append_refuse(self):
        old,new=fixture()
        with self.assertRaises(tool.Refusal):tool.append_only(encoded(old),encoded(new).replace(b'"uid_start": 200000',b'"uid_start":  200000'),A)
        with self.assertRaises(tool.Refusal):tool.decode(b'{"scope":"a","scope":"b"}')
        new['allowed_contained_nested'].append(grant('dd895e7d-d96f-435f-8552-0a05a1a2d960','/srv/universes/fourth',1000000,1100000))
        with self.assertRaises(tool.Refusal):tool.append_only(encoded(old),encoded(new),A)

    def test_saved_file_or_symlink_refuses_empty_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);new=grant(C,temporary,600000,800000)
            (root/'etc').mkdir();(root/'saved').mkdir()
            tool.check_empty(new)
            (root/'saved'/'old-sql').write_bytes(b'unique')
            with self.assertRaises(tool.Refusal):tool.check_empty(new)
            (root/'saved'/'old-sql').unlink();(root/'saved'/'linked').symlink_to(root/'etc',target_is_directory=True)
            with self.assertRaises(tool.Refusal):tool.check_empty(new)

    def test_atomic_replace_error_does_not_modify_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);stage=root/'stage';destination=root/'receipt'
            stage.write_bytes(b'new');destination.write_bytes(b'original')
            with patch.object(tool.os,'replace',side_effect=OSError('injected crash')):
                with self.assertRaises(OSError):tool.replace_from(stage,destination)
            self.assertEqual(destination.read_bytes(),b'original');self.assertEqual(stage.read_bytes(),b'new')

    def test_absence_requires_sql_api_and_exact_app_process_in_stopped_generation(self):
        class Instance:
            scope='operator-owned'
            r={'host_machine_id':'machine','app':'app-cid','db':'db-cid'}
            manifest={'payload_sha256':{'podmeshd':'a'*64}}
            def check_container(self,role):
                return {'State':{'StartedAt':'1970-01-01T00:01:40Z','FinishedAt':'1970-01-01T00:03:20Z'}}
        instance=Instance();common={'scope':instance.scope,'host_machine_id':'machine','observed_at':150}
        summary={**common,'universe_uuid':C,'policy_sha256':'policy','actual_sql_entries':0,
            'inventory_absent':True,'provider_record_absent':True,'db_container_id':'db-cid',
            'db_started_at':'1970-01-01T00:01:40Z'}
        proofs={'sql':{**common,'universe_uuid':C,'database':'podmesh-node','user':'podmesh-node',
            'db_container_id':'db-cid','db_started_at':'1970-01-01T00:01:40Z','app_container_id':'app-cid',
            'query':"SELECT COUNT(*) FROM operations WHERE request LIKE '%"+C+"%';",'count':0},
            'inventory':{**common,'app_container_id':'app-cid','operation':'inventory','ok':True,'containers':[]},
            'process':{**common,'app_container_id':'app-cid','app_started_at':'1970-01-01T00:01:40Z',
                'host_pid':123,'starttime':'17','executable_sha256':'a'*64}}
        tool.validate_freshness(summary,proofs,instance,{'universe_uuid':C},'policy')
        for kind,key,value in [('sql','count',1),('sql','count',False),('sql','query','SELECT 0'),
                ('sql','db_container_id','foreign'),('sql','database','test-double'),
                ('inventory','containers',[{'universe_uuid':C}]),('inventory','app_container_id','foreign'),
                ('process','executable_sha256','b'*64),('process','host_pid',0),
                ('process','app_started_at','earlier-run'),('process','observed_at',99),
                ('sql','observed_at',201)]:
            changed=copy.deepcopy(proofs);changed[kind][key]=value
            with self.subTest(kind=kind,key=key),self.assertRaises(tool.Refusal):
                tool.validate_freshness(summary,changed,instance,{'universe_uuid':C},'policy')

    def test_native_phase_service_pid_and_container_process_require_stopped(self):
        class Instance:
            prefix='podmesh-node-private-operator-owned'
            r={'phase':'stopped','units':{'owned.service':'hash'}}
            def unitcheck(self): pass
            def check_container(self,role):return {'State':{'Running':False,'Paused':False,'Pid':0}}
        instance=Instance()
        good=subprocess.CompletedProcess([],0,b'ActiveState=inactive\nMainPID=0\nControlPID=0\n',b'')
        with patch.object(tool.subprocess,'run',return_value=good):tool.check_stopped(instance)
        live=subprocess.CompletedProcess([],0,b'ActiveState=active\nMainPID=7\nControlPID=0\n',b'')
        with patch.object(tool.subprocess,'run',return_value=live),self.assertRaises(tool.Refusal):tool.check_stopped(instance)
        instance.r={'phase':'started','units':{}}
        with self.assertRaises(tool.Refusal):tool.check_stopped(instance)

    def test_only_exact_recorded_target_may_omit_service_pid_properties(self):
        class Instance:
            prefix='podmesh-node-private-operator-owned'
            r={'phase':'stopped','units':{'podmesh-node-private-operator-owned.target':'hash'}}
            def unitcheck(self): pass
            def check_container(self,role):return {'State':{'Running':False,'Paused':False,'Pid':0}}
        instance=Instance()
        def observed(data):return subprocess.CompletedProcess([],0,data,b'')
        with patch.object(tool.subprocess,'run',return_value=observed(b'ActiveState=inactive\n')):
            tool.check_stopped(instance)
        with patch.object(tool.subprocess,'run',return_value=observed(b'ActiveState=inactive\nMainPID=0\nControlPID=0\n')):
            tool.check_stopped(instance)
        for data in (b'ActiveState=active\n',b'ActiveState=inactive\nMainPID=7\n',
                     b'ActiveState=inactive\nControlPID=7\n',b'ActiveState=inactive\nMainPID=\n'):
            with self.subTest(data=data),patch.object(tool.subprocess,'run',return_value=observed(data)),self.assertRaises(tool.Refusal):
                tool.check_stopped(instance)
        for unit in ('owned.service','another.target'):
            instance.r={'phase':'stopped','units':{unit:'hash'}}
            with self.subTest(unit=unit),patch.object(tool.subprocess,'run',return_value=observed(b'ActiveState=inactive\n')),self.assertRaises(tool.Refusal):
                tool.check_stopped(instance)


if __name__ == '__main__':unittest.main()
