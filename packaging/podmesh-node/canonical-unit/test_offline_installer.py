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

    def test_release_token_literal_hyphen_and_allowed_punctuation(self):
        # Each punctuation is exercised through the actual extracted AWK parser.
        for label in ['local-deb', 'Ubuntu:24.04/noble-updates,', 'A_z0.9:+~/,-']:
            with self.subTest(label=label):
                result=self.run_plan(self.good().replace('local-deb',label))
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(result.stderr,'')

    def test_release_token_illegal_characters_report_release_shape(self):
        for character in ['@','!','=',';','[',']','(',')','\\','%','?','"',"'",'é']:
            with self.subTest(character=character):
                result=self.rejected(self.good().replace('local-deb','local'+character+'deb'))
                self.assertIn('stage=plan_validate guard=release-shape input_line=3 exit=65',result.stderr)
                self.assertNotIn('Invalid range',result.stderr)

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


@unittest.skipUnless(os.geteuid()==0, 'owned root image-path fixtures')
class OfflineArchiveStaging(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='offline-archive-fixture-')
        self.parent=Path(self.temp.name);self.root=self.parent/'bundle';self.work=self.parent/'work'
        (self.root/'debs').mkdir(parents=True);(self.work/'cache/archives/partial').mkdir(parents=True)
        self.root.chmod(0o700);self.work.chmod(0o700)
        for path in self.parent.rglob('*'):
            if path.is_dir():path.chmod(0o700)
        self.rows=[];self.payloads={}
        self.add_package('python3','3.12.3-0ubuntu2.1','amd64')
        self.add_package('libpython3-stdlib','1:3.12.3-0ubuntu2~24.04.1','all')
        self.write_manifest()
        m=self.work.stat();self.identity=str(m.st_dev)+':'+str(m.st_ino)
        self.outside=self.parent/'outside';self.outside.write_text('preserve')

    def tearDown(self):self.temp.cleanup()

    def add_package(self,name,version,arch):
        import hashlib
        directory=self.parent/('source-'+name);(directory/'DEBIAN').mkdir(parents=True)
        (directory/'DEBIAN/control').write_text('Package: '+name+'\nVersion: '+version+'\nArchitecture: '+arch+'\nMaintainer: Fixture <fixture@example.invalid>\nDescription: isolated archive fixture\n')
        output=self.parent/(name+'.deb')
        subprocess.run(['/usr/bin/dpkg-deb','--build','--root-owner-group',str(directory),str(output)],
                       capture_output=True,check=True,timeout=10)
        data=output.read_bytes();digest=hashlib.sha256(data).hexdigest();filename=digest+'.deb'
        output.rename(self.root/'debs'/filename);(self.root/'debs'/filename).chmod(0o600)
        self.rows.append([name,version,arch,digest,str(len(data)),filename]);self.payloads[filename]=data

    def write_manifest(self):
        (self.work/'packages.tsv').write_text(''.join('\t'.join(row)+'\n' for row in self.rows))
        (self.work/'packages.tsv').chmod(0o600)

    def run_stage(self,work=None,root=None):
        script=support_source()+'\nroot=$1\nwork=$2\nwork_identity=$3\nstage_verified_archives\n'
        return subprocess.run(['/bin/bash','-c',script,'fixture',str(root or self.root),str(work or self.work),self.identity],
            env={'PATH':'/usr/bin:/bin','LC_ALL':'C'},capture_output=True,text=True,timeout=15)

    def assert_refused(self,result):
        self.assertNotEqual(result.returncode,0,result.stderr)
        self.assertIn('stage=archive_stage',result.stderr)
        self.assertEqual(self.outside.read_text(),'preserve')
        self.assertLess(len(result.stderr),4096)

    def test_native_absolute_cache_copies_epoch_and_exact_bytes(self):
        result=self.run_stage();self.assertEqual(result.returncode,0,result.stderr)
        cache=self.work/'cache/archives'
        expected=['python3_3.12.3-0ubuntu2.1_amd64.deb','libpython3-stdlib_1%3a3.12.3-0ubuntu2~24.04.1_all.deb']
        for row,name in zip(self.rows,expected):
            path=cache/name;self.assertTrue(path.is_absolute());self.assertFalse(path.is_symlink())
            self.assertEqual(path.read_bytes(),self.payloads[row[5]])
            self.assertEqual(path.stat().st_mode & 0o777,0o600)
            self.assertEqual((self.root/'debs'/row[5]).read_bytes(),self.payloads[row[5]])
        self.assertEqual({p.name for p in cache.iterdir()},set(expected)|{'partial'})
        self.assertEqual(self.outside.read_text(),'preserve')

    def test_native_filename_encoding_and_invalid_components(self):
        script=support_source()+'\nnative_archive_name "$1" "$2" "$3"\n'
        for version,expected in [('1:2.0+ubuntu1~24.04','pkg_1%3a2.0+ubuntu1~24.04_amd64.deb'),('2.0-1ubuntu0.17','pkg_2.0-1ubuntu0.17_amd64.deb')]:
            result=subprocess.run(['/bin/bash','-c',script,'fixture','pkg',version,'amd64'],env={'PATH':'/usr/bin:/bin','LC_ALL':'C'},capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(result.stdout.strip(),expected)
        for name,version,arch in [('../pkg','1','amd64'),('/pkg','1','all'),('pkg','../1','all'),('pkg','1/2','amd64'),('pkg','1%3a2','all'),('pkg','1','arm64'),('pkg','1_2','all')]:
            result=subprocess.run(['/bin/bash','-c',script,'fixture',name,version,arch],env={'PATH':'/usr/bin:/bin','LC_ALL':'C'},capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,65,result.stderr)

    def test_hash_size_and_package_metadata_mismatches_refuse_before_copies(self):
        for field,value in [(0,'wrong-name'),(1,'99:9-ubuntu1'),(2,'all'),(4,str(int(self.rows[0][4])+1))]:
            original=self.rows[0][field];self.rows[0][field]=value;self.write_manifest()
            self.assert_refused(self.run_stage());self.rows[0][field]=original
            self.assertEqual([p.name for p in (self.work/'cache/archives').iterdir()],['partial'])
        self.write_manifest();path=self.root/'debs'/self.rows[0][5];original=path.read_bytes()
        path.write_bytes(original[:-1]+bytes([original[-1]^1]))
        result=self.run_stage();self.assert_refused(result);self.assertIn('guard=archive-file-checksum',result.stderr)

    def test_traversal_absolute_filename_and_duplicate_destination_refuse(self):
        original=self.rows[0][5]
        for bad in ['../outside',str(self.outside),'/tmp/archive.deb']:
            self.rows[0][5]=bad;self.write_manifest();self.assert_refused(self.run_stage())
        self.rows[0][5]=original;self.rows.append(self.rows[0].copy());self.write_manifest()
        self.assert_refused(self.run_stage())
        self.assertEqual([p.name for p in (self.work/'cache/archives').iterdir()],['partial'])

    def test_symlink_and_hardlink_source_refuse_without_outside_writes(self):
        path=self.root/'debs'/self.rows[0][5];data=path.read_bytes();path.unlink();path.symlink_to(self.outside)
        self.assert_refused(self.run_stage());path.unlink();path.write_bytes(data)
        os.link(path,self.parent/'second-link');self.assert_refused(self.run_stage())

    def test_existing_destination_symlink_and_extra_source_refuse(self):
        path=self.work/'cache/archives/python3_3.12.3-0ubuntu2.1_amd64.deb'
        path.write_text('existing');self.assert_refused(self.run_stage());self.assertEqual(path.read_text(),'existing')
        path.unlink();path.symlink_to(self.outside);self.assert_refused(self.run_stage());path.unlink()
        extra=self.root/'debs/extra.deb';extra.write_text('extra');self.assert_refused(self.run_stage())
        self.assertEqual([p.name for p in (self.work/'cache/archives').iterdir()],['partial'])

    def test_foreign_work_identity_symlink_ancestor_and_writable_path_refuse(self):
        foreign=self.parent/'foreign';foreign.mkdir(mode=0o700)
        self.assert_refused(self.run_stage(work=foreign));self.assertEqual(list(foreign.iterdir()),[])
        alias=self.parent/'alias';alias.symlink_to(self.root,target_is_directory=True)
        self.assert_refused(self.run_stage(root=alias))
        self.work.chmod(0o777);self.assert_refused(self.run_stage());self.work.chmod(0o700)
        os.chown(self.root/'debs',42,42);self.assert_refused(self.run_stage());os.chown(self.root/'debs',0,0)

    def test_archive_partial_and_cache_ancestor_symlinks_refuse(self):
        partial=self.work/'cache/archives/partial';partial.rmdir();partial.symlink_to(self.parent,target_is_directory=True)
        self.assert_refused(self.run_stage());partial.unlink();partial.mkdir()
        cache=self.work/'cache';cache.rename(self.work/'real-cache');cache.symlink_to(self.work/'real-cache',target_is_directory=True)
        self.assert_refused(self.run_stage())

    def test_actual_APT_configuration_binds_staged_cache_for_both_calls(self):
        text=INSTALLER.read_text()
        start=text.index('cat > "$work/apt.conf" <<EOF')
        configuration=text[start:text.index('\nEOF',start)+4]
        exported=next(line for line in text.splitlines() if line.startswith('export APT_CONFIG='))
        simulate=next(line for line in text.splitlines() if line.startswith('apt-get --simulate '))
        install=next(line for line in text.splitlines() if line.startswith('apt-get --yes '))
        script='set -euo pipefail\nwork=$1\ndebs=("$2" "$3")\napt-get() { printf "%s\\0" "$APT_CONFIG" "$@"; }\n'+configuration+'\n'+exported+'\n'+simulate+'\n'+install+'\n'
        paths=[str(self.root/'debs'/row[5]) for row in self.rows]
        result=subprocess.run(['/bin/bash','-c',script,'fixture',str(self.work),*paths],
            env={'PATH':'/usr/bin:/bin','LC_ALL':'C'},capture_output=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr)
        config=self.work/'apt.conf'
        self.assertIn('Dir::Cache "'+str(self.work/'cache')+'";',config.read_text())
        self.assertIn('Dir::Cache::archives "'+str(self.work/'cache/archives')+'";',config.read_text())
        real=result.stdout.decode().split('\0')[:-1]
        plan=(self.work/'plan.log').read_bytes().decode().split('\0')[:-1]
        self.assertEqual(real,[str(config),'--yes','--no-download','--no-upgrade','--no-remove','--no-install-recommends','install',*paths])
        self.assertEqual(plan,[str(config),'--simulate','--no-download','--no-upgrade','--no-remove','--no-install-recommends','install',*paths])

    def test_real_install_argv_seam_stays_offline_and_absolute(self):
        command=next(line for line in INSTALLER.read_text().splitlines() if line.lstrip().startswith('apt-get --yes '))
        script='set -euo pipefail\napt-get() { printf "%s\\0" "$@"; }\ndebs=("$1" "$2")\n'+command+'\n'
        paths=[str(self.root/'debs'/row[5]) for row in self.rows]
        result=subprocess.run(['/bin/bash','-c',script,'fixture',*paths],capture_output=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(result.stdout.decode().split('\0')[:-1],['--yes','--no-download','--no-upgrade','--no-remove','--no-install-recommends','install',*paths])


if __name__=='__main__':unittest.main()
