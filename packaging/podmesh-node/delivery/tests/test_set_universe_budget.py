"""Budget-only administration guards; execute on the declared build host."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

DIRECTORY=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('budget_tool',DIRECTORY/'set-universe-budget.py')
tool=importlib.util.module_from_spec(spec);spec.loader.exec_module(tool)
spec=importlib.util.spec_from_file_location('original_enroller',DIRECTORY/'enroll-contained-grant.py')
enroller=importlib.util.module_from_spec(spec);spec.loader.exec_module(enroller)

def encoded(value):return (json.dumps(value,indent=2)+'\n').encode()

def fixture():
    identifiers=['d8304d1a-fdd4-49fc-9af2-1926f1d03af7','e029d6fa-35c8-4e23-a87d-e530f1a1492b','e6ed3f27-2b3e-426a-8863-c79d706ae9db']
    old={'scope':'operator-owned','max_universes':2,'max_memory_bytes':2147483648,
        'allowed_operations':['create','stop'],'allowed_nested':[],
        'allowed_contained_nested':[{'universe_uuid':identifier,'image':'a'*64,'containment':'userns-v1',
            'uid_start':200000+index*100000,'gid_start':200000+index*100000,'id_count':65536,
            'volume_root':'/srv/universes/'+identifier,'network':'owned-network',
            'mounts':[{'source':'/srv/universes/'+identifier+'/etc','destination':'/etc/shaper','read_only':False}]}
            for index,identifier in enumerate(identifiers)]}
    new=copy.deepcopy(old);new['max_universes']=3;return old,new

class UniverseBudget(unittest.TestCase):
    def test_only_scalar_change_preserves_all_descriptors_and_outside_bytes(self):
        old,new=fixture();before,after=tool.budget_only(encoded(old),encoded(new),2,3,2,1)
        self.assertEqual(before,old);self.assertEqual(after,new)
        self.assertEqual(before['allowed_contained_nested'],after['allowed_contained_nested'])

    def test_semantic_or_format_changes_outside_scalar_refuse(self):
        old,new=fixture()
        for mutate in (lambda n:n.update(max_memory_bytes=1),lambda n:n['allowed_operations'].append('delete'),
            lambda n:n['allowed_contained_nested'][0].update(uid_start=0),
            lambda n:n['allowed_contained_nested'][0]['mounts'][0].update(read_only=True)):
            changed=copy.deepcopy(new);mutate(changed)
            with self.assertRaises(tool.Refusal):tool.budget_only(encoded(old),encoded(changed),2,3,2,1)
        with self.assertRaises(tool.Refusal):tool.budget_only(encoded(old),encoded(new).replace(b'"scope": ',b'"scope":  '),2,3,2,1)

    def test_bounded_explicit_increase_and_typed_values(self):
        old,new=fixture()
        for before,after,count,required in ((2,2,2,1),(3,2,1,1),(0,3,2,1),(2,257,2,1),
            (True,3,2,1),(2,3,False,1),(2,3,2,0),(2,3,2,1.0)):
            with self.subTest(values=(before,after,count,required)),self.assertRaises(tool.Refusal):
                tool.budget_only(encoded(old),encoded(new),before,after,count,required)
        float_budget=copy.deepcopy(new);float_budget['max_universes']=3.0
        with self.assertRaises(tool.Refusal):tool.budget_only(encoded(old),encoded(float_budget),2,3,2,1)

    def test_history_and_new_identity_require_real_capacity_within_approved_grants(self):
        old,new=fixture()
        with self.assertRaises(tool.Refusal):tool.budget_only(encoded(old),encoded(new),2,3,3,1)
        with self.assertRaises(tool.Refusal):tool.budget_only(encoded(old),encoded(new),2,3,2,2)
        changed=copy.deepcopy(new);changed['max_universes']=4
        with self.assertRaises(tool.Refusal):tool.budget_only(encoded(old),encoded(changed),2,4,2,1)
        old['allowed_contained_nested'][2]['universe_uuid']=old['allowed_contained_nested'][0]['universe_uuid']
        new=copy.deepcopy(old);new['max_universes']=3
        with self.assertRaises(tool.Refusal):tool.budget_only(encoded(old),encoded(new),2,3,2,1)

    def test_nested_budget_is_untouched_and_duplicate_top_level_refuses(self):
        old,new=fixture();old['metadata']={'max_universes':99};new['metadata']={'max_universes':99}
        tool.budget_only(encoded(old),encoded(new),2,3,2,1)
        duplicate=encoded(new).replace(b'"max_universes": 3',b'"max_universes": 3,"max_universes": 3',1)
        with self.assertRaises(tool.Refusal):tool.budget_only(encoded(old),duplicate,2,3,2,1)

    def test_receipt_changes_only_actual_policy_hash_and_preserves_private_identity(self):
        before={'scope':'operator-owned','phase':'stopped','app':'original-app','db':'original-db','pod':'original-pod',
            'resources':['old-unique-volume'],'configuration_sha256':{'policy.json':'a'*64,'storage.conf':'b'*64},
            'units':{'original.service':'c'*64},'application_configuration_sha256':{'passwd':'d'*64}}
        after=tool.receipt_after(encoded(before),'a'*64,'e'*64)
        expected=copy.deepcopy(before);expected['configuration_sha256']['policy.json']='e'*64
        self.assertEqual(after,expected);self.assertEqual(before['configuration_sha256']['policy.json'],'a'*64)
        with self.assertRaises(tool.Refusal):tool.receipt_after(encoded(before),'f'*64,'e'*64)

    def test_actual_native_live_control_refuses_before_preimage_files_or_mutation(self):
        class Instance:
            prefix='podmesh-node-private-operator-owned'
            r={'phase':'stopped','units':{'owned.service':'hash'}}
            def read(self):pass
            def unitcheck(self):pass
            def check_container(self,role):return {'State':{'Running':False,'Paused':False,'Pid':0}}
        live=subprocess.CompletedProcess([],0,b'ActiveState=active\nMainPID=9\nControlPID=0\n',b'')
        with patch.object(enroller.subprocess,'run',return_value=live),self.assertRaises(enroller.Refusal):
            tool.preflight_original(enroller,Instance(),'a'*64,'b'*64)
        instance=Instance();instance.r={'phase':'started','units':{}}
        with self.assertRaises(enroller.Refusal):tool.preflight_original(enroller,instance,'a'*64,'b'*64)

    def test_two_universe_records_and_nine_intents_keep_every_file_but_count_two(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);root.chmod(0o700)
            names=['d8304d1a-fdd4-49fc-9af2-1926f1d03af7.json','e029d6fa-35c8-4e23-a87d-e530f1a1492b.json']
            names += ['intent-'+str(index)+'.json' for index in range(9)]
            for name in names:(root/name).write_bytes(('retained-'+name).encode())
            if root.stat().st_uid!=0:self.skipTest('root build-host metadata fixture required')
            original={name:((root/name).read_bytes(),(root/name).stat().st_ino) for name in names}
            with patch.object(enroller,'protected',side_effect=lambda path,**kw:Path(path)):
                count,history=tool.provider_records(enroller,root)
            self.assertEqual(count,2);self.assertEqual(set(history),set(names))
            old,new=fixture();tool.budget_only(encoded(old),encoded(new),2,3,count,1)
            for name,(data,inode) in original.items():
                self.assertEqual(history[name]['sha256'],hashlib.sha256(data).hexdigest())
                self.assertEqual(history[name]['inode'],inode);self.assertEqual((root/name).read_bytes(),data)

    def test_exact_provider_uuid_syntax_not_uuid_normalization(self):
        self.assertTrue(tool.provider_identity_stem('D8304D1A-FDD4-49FC-9AF2-1926F1D03AF7'))
        self.assertTrue(tool.provider_identity_stem('00000000-0000-0000-0000-000000000000'))
        for name in ('intent-d8304d1a-fdd4-49fc-9af2-1926f1d03af7',
            'd8304d1afdd449fc9af21926f1d03af7','d8304d1a-fdd4-49fc-9af2-1926f1d03afg',
            'd8304d1a-fdd4-49fc-9af2-1926f1d03af7.json','d8304d1a-fdd4-49fc-9af2-1926f1d03af７'):
            with self.subTest(name=name):self.assertFalse(tool.provider_identity_stem(name))

    def test_provider_counts_uuid_stems_any_extension_or_directory_preserves_unknown_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);root.chmod(0o700)
            first=root/'D8304D1A-FDD4-49FC-9AF2-1926F1D03AF7.bin';first.write_bytes(b'malformed-json-kept')
            second=root/'e029d6fa-35c8-4e23-a87d-e530f1a1492b';second.mkdir()
            nested=second/'e6ed3f27-2b3e-426a-8863-c79d706ae9db.json';nested.write_bytes(b'unique-nested')
            unknown=root/'unknown.json';unknown.write_bytes(b'{not-valid-json')
            if root.stat().st_uid!=0:self.skipTest('root build-host metadata fixture required')
            with patch.object(enroller,'protected',side_effect=lambda path,**kw:Path(path)):
                count,history=tool.provider_records(enroller,root)
            self.assertEqual(count,2)
            self.assertEqual(history[first.name]['sha256'],hashlib.sha256(first.read_bytes()).hexdigest())
            self.assertEqual(history[unknown.name]['sha256'],hashlib.sha256(unknown.read_bytes()).hexdigest())
            self.assertIn(str(nested.relative_to(root)),history)

    def test_provider_history_hash_and_inode_are_retained_and_symlink_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);root.chmod(0o700);record=root/'d8304d1a-fdd4-49fc-9af2-1926f1d03af7.json';record.write_bytes(b'{"unique":"never drop"}')
            # Pure temporary fixture: do not require root-owned ancestor custody here.
            with patch.object(enroller,'protected',side_effect=lambda path,**kw:Path(path)):
                if record.stat().st_uid!=0:self.skipTest('root build-host fixture needed for actual metadata guard')
                count,history=tool.provider_records(enroller,root)
                self.assertEqual(count,1);self.assertEqual(history[record.name]['sha256'],hashlib.sha256(record.read_bytes()).hexdigest())
                self.assertEqual(history[record.name]['inode'],record.stat().st_ino)
                (root/'linked.json').symlink_to(record)
                with self.assertRaises(tool.Refusal):tool.provider_records(enroller,root)

if __name__=='__main__':unittest.main()
