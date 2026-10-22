"""Dependency closure contract tests. No network, APT install or product execution."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('closure', Path(__file__).with_name('prepare-python-closure.py'))
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)


class PythonClosure(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def base(self):
        files = {'/usr/lib/os-release': b'ID=ubuntu\nVERSION_ID="24.04"\nVERSION_CODENAME=noble\n',
                 '/var/lib/dpkg/status': b'Package: base-package\nVersion: 1.0\nArchitecture: amd64\nStatus: install ok installed\n',
                 '/usr/share/keyrings/ubuntu-archive-keyring.gpg': b'public-key-fixture',
                 '/usr/bin/dpkg': b'\x7fELF\x02\x01' + bytes(12) + b'\x3e\x00'}
        for name, data in files.items():
            p = self.root/name.lstrip('/')
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(data)
        (self.root/'etc').mkdir()
        (self.root/'etc/os-release').symlink_to('/usr/lib/os-release')
        b = {'schema':'podmesh.base-root-binding/v1','base':c.BASE,'suite':'noble','arch':'amd64',
             'verification':'oci-config-diffids-checked','files':{
                 'os_release':c.digest(files['/usr/lib/os-release']),
                 'dpkg_status':c.digest(files['/var/lib/dpkg/status']),
                 'ubuntu_key':c.digest(files['/usr/share/keyrings/ubuntu-archive-keyring.gpg']),
                 'dpkg_elf':c.digest(files['/usr/bin/dpkg'])}}
        return b

    def bundle(self):
        out = self.root/'bundle';out.mkdir();(out/'debs').mkdir()
        evidence = b'{}\n';(out/'evidence.json').write_bytes(evidence)
        lines=['PODMESH_PYTHON_CLOSURE_V1','REGISTRY\t'+c.BASE['registry'],'IMAGE\t'+c.BASE['image'],
               'ARCHIVE\t'+c.BASE['archive'],'SUITE\tnoble','ARCH\tamd64']
        for field in ['STATUS','OS_RELEASE','KEY','BINDING']:
            lines.append(field+'\t'+'a'*64)
        lines.append('EVIDENCE\t'+c.digest(evidence))
        total=0
        for name,data in [('python3',b'fixture-python'),('libpython3-stdlib',b'fixture-stdlib')]:
            sha=c.digest(data);(out/'debs'/(sha+'.deb')).write_bytes(data);total+=len(data)
            lines.append('\t'.join(['PACKAGE',name,'3.12.3-0ubuntu1','amd64',sha,str(len(data)),sha+'.deb','1']))
        lines.append('END\t2\t'+str(total))
        (out/'closure.lock').write_text('\n'.join(lines)+'\n')
        return out,c.digest((out/'closure.lock').read_bytes())

    def test_absolute_image_symlink_is_resolved_inside_image(self):
        b=self.base();data=json.dumps(b).encode()
        bound,files,_=c.verify_base(self.root,data,c.digest(data))
        self.assertEqual(files['os_release'],(self.root/'usr/lib/os-release').read_bytes())
        self.assertEqual(bound['base'],c.BASE)

    def test_image_symlink_escape_and_loop_refuse(self):
        (self.root/'bad').symlink_to('../../etc/shadow')
        with self.assertRaises(ValueError):c.image_file(self.root,'/bad')
        (self.root/'loop').symlink_to('/loop')
        with self.assertRaises(ValueError):c.image_file(self.root,'/loop')

    def test_wrong_base_suite_arch_and_external_binding_refuse(self):
        b=self.base()
        for field,value in [('suite','jammy'),('arch','arm64'),('base',dict(c.BASE,image='sha256:'+'0'*64))]:
            bad=dict(b);bad[field]=value;encoded=json.dumps(bad).encode()
            with self.assertRaises(ValueError):c.verify_base(self.root,encoded,c.digest(encoded))
        with self.assertRaises(ValueError):c.verify_base(self.root,json.dumps(b).encode(),'f'*64)

    def test_reused_symlink_and_config_injection_paths_refuse(self):
        with self.assertRaises(ValueError):c.protected_absolute(self.root,absent=True)
        link=self.root/'work';link.symlink_to('/etc/apt')
        with self.assertRaises(ValueError):c.protected_absolute(link,absent=True)
        with self.assertRaises(ValueError):c.protected_absolute(Path('/root/unsafe";Dir::State "/etc/apt'),absent=True)

    def test_isolated_APT_paths_and_security_defaults(self):
        work=Path('/root/new-python-work');text=c.apt_config(work)
        for line in text.splitlines():
            if line.startswith('Dir') and not line.startswith('Dir::Bin'):
                self.assertIn(str(work),line)
        for flag in ['AllowUnauthenticated "false"','Check-Valid-Until "true"','AllowInsecureRepositories "false"']:
            self.assertIn(flag,text)
        self.assertIn('#clear DPkg::Pre-Invoke',text)
        self.assertNotIn('trusted=yes',text)

    def test_collector_download_only_fixed_plan_no_host_install(self):
        args=c.download_argv([('python3','3.12.3-0ubuntu1')])
        self.assertIn('--download-only',args);self.assertIn('--no-remove',args);self.assertIn('--no-upgrade',args)
        self.assertNotIn('unpack',args);self.assertNotIn('configure',args)
        with self.assertRaises(ValueError):c.download_argv([('--force-all','1')])
        with self.assertRaises(ValueError):c.download_argv([('python3','1;touch /bad')])

    def test_upgrades_removals_and_incomplete_python_plan_refuse(self):
        output='Inst python3 (3.12.3 Ubuntu:24.04/noble [amd64])\nInst libpython3-stdlib (3.12.3 Ubuntu:24.04/noble [amd64])\n'
        self.assertEqual(len(c.planned(output,{})),2)
        with self.assertRaises(ValueError):c.planned(output,{'python3':('3.1','amd64')})
        with self.assertRaises(ValueError):c.planned(output+'Remv libc6 [2.39]\n',{})
        with self.assertRaises(ValueError):c.planned('Inst python3-minimal (3.12 Ubuntu:24.04/noble [amd64])\n',{})

    def test_bad_package_metadata_name_hash_arch_and_path_refuse(self):
        text='Package: python3\nVersion: 3.12\nArchitecture: amd64\nSHA256: '+('a'*64)+'\nSize: 12\nInstalled-Size: 25\nFilename: pool/main/p/python3/python3.deb\n'
        self.assertEqual(c.package_record(text,'python3','3.12')['bytes'],12)
        for original,bad in [('Architecture: amd64','Architecture: arm64'),('SHA256: '+('a'*64),'SHA256: short'),('pool/main/p/python3/python3.deb','../../escape')]:
            with self.assertRaises(ValueError):c.package_record(text.replace(original,bad),'python3','3.12')

    def test_lock_mismatch_extra_artifact_and_byte_tampering_refuse(self):
        out,sha=self.bundle();self.assertEqual(len(c.verify_bundle(out,sha)),2)
        with self.assertRaises(ValueError):c.verify_bundle(out,'f'*64)
        extra=out/'debs/extra.deb';extra.write_bytes(b'extra')
        with self.assertRaises(ValueError):c.verify_bundle(out,sha)
        extra.unlink();next((out/'debs').iterdir()).write_bytes(b'tampered')
        with self.assertRaises(ValueError):c.verify_bundle(out,sha)

    def test_lock_package_path_and_duplicate_metadata_refuse(self):
        out,_=self.bundle();p=out/'closure.lock';text=p.read_text()
        p.write_text(text.replace('PACKAGE\tpython3\t','PACKAGE\t../python3\t'))
        with self.assertRaises(ValueError):c.verify_bundle(out,c.digest(p.read_bytes()))
        p.write_text(text.replace('SUITE\tnoble\n','SUITE\tnoble\nSUITE\tnoble\n'))
        with self.assertRaises(ValueError):c.verify_bundle(out,c.digest(p.read_bytes()))

    def test_offline_image_install_contract(self):
        text=Path(__file__).with_name('install-python-closure.sh').read_text()
        self.assertNotIn('apt-get update',text)
        self.assertNotIn('--force-depends',text)
        self.assertIn('--no-download --no-upgrade --no-remove',text)
        self.assertIn('exit 101',text)
        self.assertLess(text.index('sha256sum --check --strict'),text.index('apt-get --yes'))
        self.assertIn('import hashlib,json,pathlib,secrets,socket,subprocess,tempfile,unittest',text)
        container=Path(__file__).with_name('Containerfile').read_text()
        self.assertIn('PYTHON_CLOSURE_SHA256',container)
        self.assertIn('PYTHON_INSTALLER_SHA256',container)


if __name__=='__main__':unittest.main()
