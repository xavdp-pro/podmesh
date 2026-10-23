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


@unittest.skipUnless(os.geteuid()==0, 'owned root verification-report fixtures')
class OfflineInstalledVerification(unittest.TestCase):
    def run_verify(self, mock_body='pass', *, name='fixture-pkg', policy=False, restore_failure=False, foreign=False, shell_extra=''):
        import sys
        with tempfile.TemporaryDirectory(prefix='installed-verify-fixture-') as directory:
            parent=Path(directory);work=parent/'work';work.mkdir(mode=0o700)
            m=work.stat();identity=str(m.st_dev)+':'+str(m.st_ino)
            outside=parent/'outside';outside.write_text('preserve')
            destination=parent/'policy-rc.d'
            if policy:
                destination.write_text('temporary deny');(work/'policy-rc.d.saved').write_text('original policy\n')
            bindir=parent/'bin';bindir.mkdir()
            executable=bindir/'dpkg'
            executable.write_text('#!'+sys.executable+'\nimport os,sys\nif sys.argv[1:]!=["--verify","--verify-format=rpm","fixture-pkg"]:sys.exit(91)\n'+mock_body+'\n')
            executable.chmod(0o700)
            script=support_source()+'\nwork=$1\nwork_identity=$2\npolicy_path=$3\n'
            if policy:script+='policy_created=1\npolicy_saved=1\n'
            if restore_failure:script+='mv() { return 74; }\n'
            selected=work
            if foreign:
                selected=parent/'foreign';selected.mkdir(mode=0o700)
            script+=shell_extra+'\nverify_installed_package_bytes "$4"\n'
            result=subprocess.run(['/bin/bash','-c',script,'fixture',str(selected),identity,str(destination),name],
                env={'PATH':str(bindir)+':/usr/bin:/bin','LC_ALL':'C','SECRET_FIXTURE':'do-not-print-secret-value'},
                capture_output=True,text=True,timeout=20)
            reports=[]
            for report in work.glob('installed-verify.*'):
                reports.append({p.name:{'data':p.read_bytes(),'mode':p.stat().st_mode & 0o777} for p in report.iterdir()})
            preserved=outside.read_text()
            policy_after=destination.read_text() if destination.exists() else None
            saved_after=(work/'policy-rc.d.saved').read_text() if (work/'policy-rc.d.saved').exists() else None
            self.assertEqual(preserved,'preserve')
            self.assertLessEqual(len(result.stderr.encode()),8192)
            return result,reports,policy_after,saved_after

    def test_clean_status_zero_empty_reports_pass_and_private_capture(self):
        result,reports,_,_=self.run_verify()
        self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(result.stderr,'')
        self.assertEqual(len(reports),1)
        self.assertEqual(reports[0]['stdout']['data'],b'');self.assertEqual(reports[0]['stderr']['data'],b'')
        self.assertIn(b'status=0\neffective_exit=0\n',reports[0]['status']['data'])
        for entry in reports[0].values():self.assertEqual(entry['mode'],0o600)

    def test_mismatch_stdout_status_zero_reports_package_path_and_refuses(self):
        result,reports,_,_=self.run_verify('print("missing     /usr/share/doc/fixture-pkg/readme (No such file or directory)")')
        self.assertEqual(result.returncode,65)
        self.assertIn('package=fixture-pkg status=0 exit=65',result.stderr)
        self.assertIn('flags=missing path=/usr/share/doc/fixture-pkg/readme',result.stderr)
        self.assertIn(b'missing',reports[0]['stdout']['data'])

    def test_digest_conffile_report_emits_flags_and_path(self):
        result,reports,_,_=self.run_verify('print("??5?????? c /etc/fixture-pkg.conf")')
        self.assertEqual(result.returncode,65)
        self.assertIn('flags=??5?????? path=/etc/fixture-pkg.conf',result.stderr)
        self.assertIn(b'??5?????? c /etc/fixture-pkg.conf',reports[0]['stdout']['data'])

    def test_nonzero_status_with_empty_stdout_is_not_concealed(self):
        result,reports,_,_=self.run_verify('sys.exit(2)')
        self.assertEqual(result.returncode,2)
        self.assertIn('package=fixture-pkg status=2 exit=2',result.stderr)
        self.assertEqual(reports[0]['stdout']['data'],b'')

    def test_stderr_command_failure_preserves_status_and_private_report(self):
        result,reports,_,_=self.run_verify('print("dpkg: error: fixture failure",file=sys.stderr)\nsys.exit(37)')
        self.assertEqual(result.returncode,37)
        self.assertIn('status=37 exit=37',result.stderr)
        self.assertIn(b'dpkg: error: fixture failure',reports[0]['stderr']['data'])
        self.assertIn('stream=stderr detail=private-unparsed-line',result.stderr)

    def test_stderr_on_status_zero_still_refuses(self):
        result,_,_,_=self.run_verify('print("fixture warning",file=sys.stderr)')
        self.assertEqual(result.returncode,65);self.assertIn('status=0 exit=65',result.stderr)

    def test_oversized_output_is_capped_and_fails_closed(self):
        for descriptor in (1,2):
            with self.subTest(descriptor=descriptor):
                result,reports,_,_=self.run_verify('os.write('+str(descriptor)+',b"x"*100000)')
                self.assertNotEqual(result.returncode,0)
                self.assertEqual(len(reports),1)
                self.assertLessEqual(len(reports[0]['stdout']['data']),16384)
                self.assertLessEqual(len(reports[0]['stderr']['data']),16384)
                self.assertIn('report_bound=yes',result.stderr)
                self.assertIn(('effective_exit='+str(result.returncode)).encode(),reports[0]['status']['data'])

    def test_file_size_signal_status_is_preserved(self):
        import signal
        result,reports,_,_=self.run_verify('import signal\nsignal.signal(signal.SIGXFSZ,signal.SIG_DFL)\nos.write(1,b"x"*16384)\nos.write(1,b"x")')
        self.assertEqual(result.returncode,128+signal.SIGXFSZ,result.stderr)
        self.assertLessEqual(len(reports[0]['stdout']['data']),16384)
        self.assertIn('report_bound=yes',result.stderr)

    def test_timeout_force_kill_restores_policy_and_preserves_status(self):
        result,reports,policy,saved=self.run_verify('import signal,time\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)\ntime.sleep(60)',policy=True)
        self.assertEqual(result.returncode,137,result.stderr)
        self.assertIn('status=137 exit=137',result.stderr)
        self.assertEqual(policy,'original policy\n');self.assertIsNone(saved)
        self.assertEqual(len(reports),1)

    def test_auxiliary_report_failures_do_not_replace_primary_status(self):
        cases=[('stat() { if [[ ${!#} == */stdout ]]; then return 73; fi; command stat "$@"; }','capture_failed=measurement'),
               ('printf() { if [[ $1 == package=* ]]; then return 73; fi; builtin printf "$@"; }','capture_failed=yes'),
               ('awk() { if [[ $1 == -v ]]; then return 73; fi; command awk "$@"; }','summary_failed=yes')]
        for extra,diagnostic in cases:
            with self.subTest(diagnostic=diagnostic):
                result,_,_,_=self.run_verify('sys.exit(2)',shell_extra=extra)
                self.assertEqual(result.returncode,2,result.stderr)
                self.assertIn(diagnostic,result.stderr)

    def test_many_report_lines_refuse_with_bounded_path_summary(self):
        result,reports,_,_=self.run_verify('print("missing     /usr/share/doc/fixture-pkg/file\\n"*100,end="")')
        self.assertEqual(result.returncode,65);self.assertIn('report_bound=yes',result.stderr)
        self.assertIn('summary_truncated=yes',result.stderr)
        self.assertEqual(result.stderr.count('flags=missing path='),8)
        self.assertLessEqual(len(reports[0]['stdout']['data']),16384)

    def test_verification_failure_restores_original_policy_and_status(self):
        result,_,policy,saved=self.run_verify('sys.exit(23)',policy=True)
        self.assertEqual(result.returncode,23);self.assertEqual(policy,'original policy\n');self.assertIsNone(saved)

    def test_policy_restore_failure_does_not_mask_verification_failure(self):
        result,_,policy,saved=self.run_verify('sys.exit(2)',policy=True,restore_failure=True)
        self.assertEqual(result.returncode,2);self.assertIn('kind=cleanup-restore',result.stderr)
        self.assertIn('exit=74',result.stderr);self.assertIsNone(policy);self.assertEqual(saved,'original policy\n')

    def test_invalid_package_and_foreign_work_refuse_without_outside_writes(self):
        result,reports,_,_=self.run_verify(name='../outside')
        self.assertEqual(result.returncode,65);self.assertEqual(reports,[])
        result,reports,_,_=self.run_verify(foreign=True)
        self.assertEqual(result.returncode,65);self.assertEqual(reports,[])

    def test_unknown_text_never_dumps_secret_environment_or_commands(self):
        result,reports,_,_=self.run_verify('print(os.environ["SECRET_FIXTURE"])\nprint("SECRETXYZ /bin/private-command")\nprint("echo $SECRET_FIXTURE; /bin/private-command",file=sys.stderr)')
        self.assertEqual(result.returncode,65)
        self.assertNotIn('do-not-print-secret-value',result.stderr)
        self.assertNotIn('$SECRET_FIXTURE',result.stderr);self.assertNotIn('/bin/private-command',result.stderr)
        self.assertIn(b'do-not-print-secret-value',reports[0]['stdout']['data'])


@unittest.skipUnless(os.geteuid()==0, 'private native APT configuration fixtures')
class OfflineCompletePayload(unittest.TestCase):
    """Actual config generation/parser plus documented filter model, no installation."""
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='complete-payload-fixture-')
        self.parent=Path(self.temp.name);self.work=self.parent/'work';self.work.mkdir(mode=0o700)
        for name in ['etc/parts','etc/sources.d','etc/preferences.d','etc/auth.d']:
            (self.work/name).mkdir(parents=True,exist_ok=True)
        for name in ['empty.conf','sources.list','trusted.gpg','preferences','auth.conf']:
            (self.work/'etc'/name).write_text('')
        self.policy=self.parent/'base-excludes'
        self.policy.write_text('path-exclude=/usr/share/man/*\npath-exclude=/usr/share/locale/*/LC_MESSAGES/*.mo\npath-exclude=/usr/share/doc/*\npath-include=/usr/share/doc/*/copyright\npath-include=/usr/share/doc/*/changelog.*\n')
        self.policy_before=(self.policy.read_bytes(),self.policy.stat().st_mode,self.policy.stat().st_mtime_ns)

    def tearDown(self):
        try:self.assertEqual((self.policy.read_bytes(),self.policy.stat().st_mode,self.policy.stat().st_mtime_ns),self.policy_before)
        finally:self.temp.cleanup()

    def actual_options(self,prefix=''):
        import re
        text=INSTALLER.read_text();start=text.index('cat > "$work/apt.conf" <<EOF')
        heredoc=text[start:text.index('\nEOF',start)+4]
        exported=next(line for line in text.splitlines() if line.startswith('export APT_CONFIG='))
        script='set -euo pipefail\nwork=$1\n'+heredoc+'\n'+exported+'\nprintf "%s\\n" "$APT_CONFIG"\n'
        rendered=subprocess.run(['/bin/bash','-c',script,'fixture',str(self.work)],
            env={'PATH':'/usr/bin:/bin','LC_ALL':'C','HOME':str(self.parent)},cwd=self.parent,
            capture_output=True,text=True,timeout=5,check=True)
        config=Path(rendered.stdout.strip());self.assertEqual(config,self.work/'apt.conf')
        original=config.read_text()
        if prefix:config.write_text(prefix+original)
        result=subprocess.run(['/usr/bin/apt-config','dump'],
            env={'PATH':'/usr/bin:/bin','LC_ALL':'C','HOME':str(self.parent),'APT_CONFIG':str(config)},
            capture_output=True,text=True,timeout=5)
        self.assertEqual(result.returncode,0,result.stderr)
        options=re.findall(r'^DPkg::Options:: "([^"]*)";$',result.stdout,re.M|re.I)
        return options,result.stdout,original

    def documented_selection(self,options,paths):
        # Model only dpkg's documented fnmatch(flags=0), append order and last match.
        import fnmatch
        rules=[]
        for row in self.policy.read_text().splitlines():
            key,pattern=row.split('=',1);rules.append((key=='path-include',pattern))
        for option in options:
            self.assertTrue(option.startswith('--path-include='))
            rules.append((True,option.split('=',1)[1]))
        result={}
        for path in paths:
            selected=True
            for include,pattern in rules:
                if fnmatch.fnmatchcase(path,pattern):selected=include
            result[path]=selected
        return result

    def test_actual_private_APT_parser_preserves_one_literal_full_include(self):
        (self.parent/'glob-marker').write_text('must not expand')
        options,_,_=self.actual_options()
        self.assertEqual(options,['--path-include=/*'])
        self.assertEqual((self.parent/'glob-marker').read_text(),'must not expand')

    def test_final_include_overrides_inherited_doc_man_locale_exclusions(self):
        options,_,_=self.actual_options()
        paths=['/usr/share/doc/libexpat1/AUTHORS','/usr/share/man/man1/fixture.1.gz',
               '/usr/share/locale/fr/LC_MESSAGES/fixture.mo','/usr/share/doc/fixture/copyright',
               '/usr/share/doc/fixture/changelog.gz','/usr/bin/fixture']
        before=self.documented_selection([],paths);after=self.documented_selection(options,paths)
        self.assertFalse(before[paths[0]]);self.assertFalse(before[paths[1]]);self.assertFalse(before[paths[2]])
        self.assertTrue(all(after.values()))

    def test_full_include_keeps_directory_paths_and_nested_entries(self):
        options,_,_=self.actual_options()
        paths=['/','/usr','/usr/share/doc','/usr/share/doc/fixture','/usr/share/doc/fixture/subdir/',
               '/usr/share/man/man1/','/usr/share/locale/fr/LC_MESSAGES/']
        self.assertTrue(all(self.documented_selection(options,paths).values()))

    def test_inherited_APT_option_injection_is_cleared_without_policy_edits(self):
        seed='DPkg::Options:: "--force-depends";\nDPkg::Options:: "--path-exclude=/*";\n'
        options,_,_=self.actual_options(seed)
        self.assertEqual(options,['--path-include=/*'])

    def test_private_parts_redirect_prevents_foreign_option_loading(self):
        parts=self.parent/'foreign-parts';parts.mkdir()
        foreign=parts/'90foreign';foreign.write_text('DPkg::Options:: "--force-all";\nDPkg::Options:: "--path-exclude=/*";\n')
        saved=foreign.read_bytes()
        options,dump,_=self.actual_options('Dir::Etc::parts "'+str(parts)+'";\n')
        self.assertEqual(options,['--path-include=/*'])
        self.assertNotIn('"--force-all"',dump);self.assertEqual(foreign.read_bytes(),saved)

    def test_full_include_does_not_turn_missing_file_verification_into_success(self):
        options,_,_=self.actual_options();self.assertEqual(options,['--path-include=/*'])
        fixture=OfflineInstalledVerification('test_mismatch_stdout_status_zero_reports_package_path_and_refuses')
        fixture.setUp();self.addCleanup(fixture.tearDown)
        result,reports,_,_=fixture.run_verify('print("missing     /usr/share/doc/fixture-pkg/AUTHORS")')
        self.assertEqual(result.returncode,65)
        self.assertIn('flags=missing path=/usr/share/doc/fixture-pkg/AUTHORS',result.stderr)
        self.assertIn(b'status=0\neffective_exit=65\n',reports[0]['status']['data'])


if __name__=='__main__':unittest.main()
