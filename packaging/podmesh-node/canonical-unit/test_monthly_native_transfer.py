#!/usr/bin/python3
"""NEW affected source cases. Authored only; run on a separately authorized CT."""
import hashlib
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import Mock, MagicMock, patch

HERE = Path(__file__).resolve().parent

def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

unit = module('monthly_unit', 'entrypoint.py')
host = module('monthly_transfer', 'logical-transfer.py')
native = module('monthly_transfer_native', 'logical-transfer-native.py')

class MonthlyNativeTransfer(unittest.TestCase):
    def test_explicit_fresh_monthly_prepare_uses_real_cli_status_and_root_plan(self):
        supervisor = unit.Supervisor(prepare_observations_months=True)
        plan = Mock()
        plan.stat.return_value.st_size = 41
        lib = MagicMock()
        lib.__truediv__ = Mock(side_effect=lambda name: plan if name == 'observations-months.json' else Path('/exact')/name)
        with patch.object(unit, 'LIB', lib), patch.object(unit, 'protected') as protect, patch.object(supervisor, 'run', return_value=json.dumps({'status':'prepared_empty_schema_monthly_observations'})) as run:
            supervisor.prepare_schema(True)
        protect.assert_called_once_with(plan, 0, 0o444, 'file')
        self.assertEqual(run.call_args.args[1][-2:], ['--observations-months', str(plan)])
        self.assertEqual(run.call_args.args[0], 'schema')

    def test_monthly_constructor_never_reaches_cli_on_existing_birth(self):
        supervisor = unit.Supervisor(prepare_observations_months=True)
        with patch.object(supervisor, 'run') as run:
            with self.assertRaises(unit.Refusal):
                supervisor.prepare_schema(False)
        run.assert_not_called()
        for flags in ({'recover_running':True}, {'adopt_restored_identity':True}):
            with self.assertRaises(unit.Refusal):
                unit.Supervisor(prepare_observations_months=True, **flags)

    def test_ordinary_prepare_remains_default_and_wrong_status_is_not_translated(self):
        supervisor = unit.Supervisor()
        for fresh, status in ((True,'prepared_empty_schema'),(False,'already_current_no_ddl')):
            with patch.object(supervisor, 'run', return_value=json.dumps({'status':status})) as run:
                supervisor.prepare_schema(fresh)
            self.assertNotIn('--observations-months', run.call_args.args[1])
        with patch.object(supervisor, 'run', return_value=json.dumps({'status':'prepared_empty_schema_monthly_observations'})):
            with self.assertRaises(unit.Refusal):
                supervisor.prepare_schema(True)

    def fixture(self, overlay=True):
        node = object.__new__(host.Node)
        node.c = {'container':'new-monthly','image':'a'*64,'etc_root':'/owned/etc','sav_root':'/owned/sav','provider_dir':'/owned/provider'}
        mounts = [{'Destination':dest,'Source':node.c[key],'Type':'bind','RW':rw} for key,dest,rw in [('etc_root','/apps/podmesh-node/etc',True),('sav_root','/apps/podmesh-node/sav',True),('provider_dir','/run/podmesh-host',False)]]
        if overlay:
            node.c['artifact_root']='/owned/artifacts'
            mounts.append({'Destination':host.ARTIFACT_DEST,'Source':node.c['artifact_root'],'Type':'bind','RW':False})
        value = {'Image':'a'*64,'HostConfig':{'Privileged':False,'NetworkMode':'none','Tmpfs':{'/run':'rw'}},'Mounts':mounts,'Config':{'Entrypoint':['/usr/bin/python3','-I','/usr/lib/podmesh-node-canonical/entrypoint.py']}}
        node.inspect = Mock(return_value=value)
        return node,value

    def test_exact_readonly_artifact_overlay_is_accepted(self):
        node,value = self.fixture()
        with patch.object(host, 'protected') as protect:
            self.assertIs(node.validate(),value)
        protect.assert_called_once_with('/owned/artifacts',True)
        ordinary,_ = self.fixture(False)
        ordinary.validate()

    def test_artifact_rw_wrong_source_and_extra_volume_are_refused(self):
        for field,value in [('RW',True),('Source','/foreign/artifacts'),('Type','volume')]:
            node,inspect = self.fixture()
            inspect['Mounts'][-1][field]=value
            with patch.object(host,'protected'), self.assertRaises(RuntimeError):
                node.validate()
        node,inspect = self.fixture()
        inspect['Mounts'].append({'Destination':'/arbitrary','Type':'bind','Source':'/foreign','RW':True})
        with patch.object(host,'protected'), self.assertRaises(RuntimeError):
            node.validate()
        node,inspect = self.fixture(False)
        inspect['Mounts'].append({'Destination':host.ARTIFACT_DEST,'Type':'bind','Source':'/foreign','RW':False})
        with self.assertRaises(RuntimeError):
            node.validate()

    def test_exact_candidate_entrypoint_hash_and_old_pin_are_both_bound(self):
        digest = hashlib.sha256((HERE/'entrypoint.py').read_bytes()).hexdigest()
        self.assertEqual(host.MONTHLY_ENTRYPOINT_SHA256,digest)
        self.assertEqual(native.MONTHLY_ENTRYPOINT_SHA256,digest)
        self.assertEqual(host.ADOPTION_ENTRYPOINT_SHA256,native.ADOPTION_ENTRYPOINT_SHA256)
        for digest in (digest,host.ADOPTION_ENTRYPOINT_SHA256):
            host.restore_artifact({'image':'a'*64,'provenance':{'sha256':{'entrypoint.py':digest}}},'a'*64)
        with self.assertRaises(RuntimeError):
            host.restore_artifact({'image':'a'*64,'provenance':{'sha256':{'entrypoint.py':'f'*64}}},'a'*64)

if __name__ == '__main__':
    unittest.main()
