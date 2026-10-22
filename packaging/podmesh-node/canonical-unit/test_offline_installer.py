"""Isolated installer seams only; never invoke the installer or APT on the host."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

INSTALLER = Path(__file__).with_name('install-python-closure.sh')


def support_source():
    text = INSTALLER.read_text()
    body = text.split('# BEGIN OFFLINE_INSTALLER_SUPPORT\n', 1)[1].split('# END OFFLINE_INSTALLER_SUPPORT', 1)[0]
    traps = text.split('# END OFFLINE_INSTALLER_SUPPORT\n',1)[1].split('policy_path=',1)[0]
    return 'set -Eeuo pipefail\n' + body + traps + '\n'


class OfflinePlanParser(unittest.TestCase):
    def run_plan(self, plan, manifest=None):
        if manifest is None:
            manifest = 'python3\t3.12.3-0ubuntu2.1\tamd64\thash\t1\tfile.deb\nlibpython3-stdlib\t1:3.12.3-0ubuntu2~24.04.1\tall\thash2\t2\tfile2.deb\n'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'packages.tsv').write_text(manifest)
            (root/'plan.log').write_text(plan)
            script = support_source() + '\nstage=plan_validate guard=fixture-plan\nvalidate_plan "$1/packages.tsv" "$1/plan.log"\n'
            return subprocess.run(['/bin/bash', '-c', script, 'fixture', directory],
                                  env={'PATH':'/usr/bin:/bin','LC_ALL':'C'}, capture_output=True, text=True, timeout=10)

    def good(self):
        return ('Reading package lists... Done\nBuilding dependency tree... Done\n'
                'Inst python3 (3.12.3-0ubuntu2.1 local-deb [amd64])\n'
                'Inst libpython3-stdlib (1:3.12.3-0ubuntu2~24.04.1 local-deb [all])\n'
                'Conf python3 (3.12.3-0ubuntu2.1 local-deb [amd64])\n'
                'Conf libpython3-stdlib (1:3.12.3-0ubuntu2~24.04.1 local-deb [all])\n')

    def rejected(self, plan, manifest=None):
        result = self.run_plan(plan, manifest)
        self.assertEqual(result.returncode,65,result.stderr)
        self.assertIn('stage=plan_validate',result.stderr)
        self.assertLess(len(result.stderr),2048)
        return result

    def test_native_rows_whitespace_epochs_and_ubuntu_suffixes(self):
        self.assertEqual(self.run_plan(self.good()).returncode,0)
        self.assertEqual(self.run_plan(self.good().replace('Inst ', ' \tInst\t').replace('Conf ', '\tConf  ')).returncode,0)
        self.assertEqual(self.run_plan(self.good().replace('local-deb','Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security')).returncode,0)

    def test_qualified_names_only_match_locked_architecture(self):
        plan=self.good().replace('python3 (','python3:amd64 (').replace('libpython3-stdlib (','libpython3-stdlib:all (')
        self.assertEqual(self.run_plan(plan).returncode,0)
        for bad in [plan.replace(':amd64 ',':arm64 '),plan.replace(':all ',':amd64 '),plan.replace(':amd64 ',':amd64:amd64 ')]:
            self.rejected(bad)

    def test_extra_missing_and_duplicate_install_refuse(self):
        rows=self.good().splitlines(True)
        for bad in [self.good()+'Inst extra (1 local-deb [amd64])\n', ''.join(r for r in rows if not r.startswith('Inst python3 ')), self.good()+rows[2]]:
            self.rejected(bad)

    def test_wrong_version_architecture_and_metadata_identity_refuse(self):
        for bad in [self.good().replace('(3.12.3-0ubuntu2.1 ', '(3.12.3-0ubuntu2.2 '),self.good().replace('[amd64]','[all]'),self.good().replace('[all]','[arm64]')]:
            self.rejected(bad)
        manifest='python3\t3.12.3-0ubuntu2.1\tarm64\thash\t1\tfile.deb\n'
        self.rejected(self.good(),manifest)

    def test_removal_purge_upgrade_and_reinstall_refuse(self):
        for prefix in ['Remv base [1]\n',' \tPurg base [1]\n']:
            self.rejected(self.good()+prefix)
        for old in ['3.12.3-0ubuntu2.1','3.12.0-1']:
            self.rejected(self.good().replace('Inst python3 (','Inst python3 ['+old+'] ('))

    def test_malformed_ambiguous_and_broken_action_rows_refuse(self):
        for bad in [self.good().replace('Inst python3 (','Inst python3 ('+'bad '),self.good().replace('[amd64])','[amd64]) extra'),self.good().replace('Inst python3 ','Install python3 '),self.good().replace('local-deb [amd64])','local-deb [broken] [amd64])'),self.good().replace('Inst python3 (','Inst python3 '),self.good()+'Conf python3 broken\n']:
            self.rejected(bad)

    def test_missing_duplicate_and_out_of_order_configuration_refuse(self):
        rows=self.good().splitlines(True)
        for bad in [self.good().replace(rows[4],''),self.good()+rows[4],rows[4]+self.good()]:
            self.rejected(bad)

    def test_manifest_and_plan_bounds_refuse(self):
        empty=self.rejected('Reading package lists... Done\n','')
        self.assertIn('guard=empty-manifest',empty.stderr)
        row='python3\t3.12.3-0ubuntu2.1\tamd64\thash\t1\tfile.deb\n'
        duplicate=self.rejected(self.good(),row+row)
        self.assertIn('guard=manifest-identity',duplicate.stderr)
        oversized=self.rejected(self.good()+'x'*4097+'\n')
        self.assertIn('guard=plan-bound',oversized.stderr)
        self.rejected(self.good()+'note\n'*8193)


class OfflineInstallerDiagnostics(unittest.TestCase):
    def run_harness(self, body, *, policy=False, mock=''):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);work=root/'work';work.mkdir()
            destination=root/'policy-rc.d'
            if policy:
                destination.write_text('temporary deny')
                (work/'policy-rc.d.saved').write_text('original policy\n')
            script=support_source()+ '\nwork=$1\npolicy_path=$2\nstage=fixture guard=fixed-guard\n'
            if policy:script+='policy_created=1\npolicy_saved=1\n'
            script+=mock+'\n'+body+'\n'
            result=subprocess.run(['/bin/bash','-c',script,'fixture',str(work),str(destination)],
                                  env={'PATH':'/usr/bin:/bin','LC_ALL':'C','SECRET_FIXTURE':'must-not-appear'},capture_output=True,text=True,timeout=10)
            return result, destination.read_text() if destination.exists() else None, (work/'policy-rc.d.saved').read_text() if (work/'policy-rc.d.saved').exists() else None

    def test_silent_explicit_guard_reports_line_and_preserves_65(self):
        result,_,_=self.run_harness('[[ 1 == 2 ]] || refuse 65')
        self.assertEqual(result.returncode,65)
        self.assertRegex(result.stderr,r'stage=fixture guard=fixed-guard kind=guard line=[0-9]+ exit=65')
        self.assertNotIn('must-not-appear',result.stderr)
        self.assertLess(len(result.stderr),1024)

    def test_command_failure_preserves_status_and_restores_policy(self):
        result,policy,saved=self.run_harness('failing_command() { return 37; }; failing_command',policy=True)
        self.assertEqual(result.returncode,37)
        self.assertIn('kind=command',result.stderr);self.assertIn('exit=37',result.stderr)
        self.assertEqual(policy,'original policy\n');self.assertIsNone(saved)

    def test_cleanup_remove_failure_keeps_saved_original_and_failure_status(self):
        result,policy,saved=self.run_harness('refuse 65',policy=True,mock='rm() { return 73; }')
        self.assertEqual(result.returncode,65)
        self.assertIn('kind=cleanup-remove',result.stderr);self.assertIn('exit=73',result.stderr)
        self.assertEqual(policy,'temporary deny');self.assertEqual(saved,'original policy\n')

    def test_cleanup_restore_failure_fails_success_and_keeps_saved_original(self):
        result,policy,saved=self.run_harness(':',policy=True,mock='mv() { return 74; }')
        self.assertEqual(result.returncode,74)
        self.assertIn('kind=cleanup-restore',result.stderr);self.assertIsNone(policy)
        self.assertEqual(saved,'original policy\n')

    def test_cleanup_is_idempotent_and_success_restores_original(self):
        result,policy,saved=self.run_harness('cleanup; cleanup',policy=True)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(policy,'original policy\n');self.assertIsNone(saved)

    def test_failed_explicit_cleanup_is_not_retried_by_exit_trap(self):
        result,policy,saved=self.run_harness('cleanup',policy=True,mock='mv() { printf "fixture-move-attempt\\n" >&2; return 74; }')
        self.assertEqual(result.returncode,74)
        self.assertEqual(result.stderr.count('fixture-move-attempt'),1)
        self.assertEqual(saved,'original policy\n');self.assertIsNone(policy)


if __name__=='__main__':unittest.main()
