"""Policy coverage for explicit recovery of a recorded running database.

Database spawn is replaced by a sentinel; this is not a native recovery proof.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('process_recovery_candidate', Path(__file__).with_name('entrypoint.py'))
u = importlib.util.module_from_spec(spec)
spec.loader.exec_module(u)

class AtDatabaseStart(Exception):
    pass

class ProcessRecovery(unittest.TestCase):
    @unittest.skipUnless(os.geteuid() == 0, 'requires root-owned protected fixture')
    def test_only_explicit_recorded_running_reaches_native_db_with_identity_unchanged(self):
        with tempfile.TemporaryDirectory(prefix='process-recovery-', dir='/root') as directory:
            root=Path(directory)
            state=root/'state';state.mkdir(mode=0o700)
            data=root/'data';data.mkdir(mode=0o700);(data/'mysql').mkdir(mode=0o700)
            admin=root/'runtime-admin';admin.mkdir(mode=0o700)
            password=root/'app-password';u.exclusive_file(password,'a'*64,u.APP_UID,u.APP_UID)
            root_password=root/'root-password';u.exclusive_file(root_password,'b'*64)
            profile=root/'profile';u.exclusive_file(profile,'{"engine":"mariadb"}',0,u.APP_UID,0o440)
            configuration={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in (password,root_password,profile)}
            provenance={'exact':'same-candidate'}
            recorded={'phase':'running','datadir':[data.stat().st_dev,data.stat().st_ino],
                      'configuration':configuration,'provenance':provenance,'system_accounts':[['root','localhost']]}
            marker=state/'birth.json'
            def reset(phase='running'):
                document=dict(recorded,phase=phase)
                marker.write_text(json.dumps(document));marker.chmod(0o600)
                if (admin/'client.cnf').exists():(admin/'client.cnf').unlink()
            with patch.multiple(u,STATE=state,DATA=data,ADMIN=admin,PASS=password,ROOTPASS=root_password,PROFILE=profile):
                reset()
                normal=u.Supervisor();normal.provenance=provenance
                with self.assertRaises(u.Refusal):normal.birth()
                self.assertEqual(json.loads(marker.read_text()),recorded)
                for phase in ('initializing','starting','failed'):
                    reset(phase)
                    recovery=u.Supervisor(recover_running=True);recovery.provenance=provenance
                    with self.assertRaises(u.Refusal):recovery.birth()
                    self.assertEqual(json.loads(marker.read_text())['phase'],phase)
                reset()
                recovery=u.Supervisor(recover_running=True);recovery.provenance=provenance
                recovery.mysql=types.SimpleNamespace(pw_uid=999,pw_gid=999)
                def database_boundary(name, command, **kwargs):
                    self.assertEqual(name,'db')
                    self.assertIn('/usr/sbin/mariadbd',command)
                    self.assertIn('--datadir='+str(data),command)
                    self.assertNotIn('/usr/bin/mariadb-install-db',command)
                    raise AtDatabaseStart()
                with patch.object(recovery,'spawn',side_effect=database_boundary), \
                     patch.object(recovery,'run') as initialize:
                    with self.assertRaises(AtDatabaseStart):recovery.birth()
                    initialize.assert_not_called()
                actual=json.loads(marker.read_text())
                self.assertEqual(actual['phase'],'starting')
                self.assertEqual(actual['process_recovery']['from_phase'],'running')
                for key in ('datadir','configuration','provenance'):
                    self.assertEqual(actual[key],recorded[key])
                self.assertEqual(configuration,{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in (password,root_password,profile)})

if __name__=='__main__':
    unittest.main()
