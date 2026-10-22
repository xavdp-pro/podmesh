"""Dependency closure contract tests. No network, APT install or product execution."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('closure', Path(__file__).with_name('prepare-python-closure.py'))
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)


class PythonClosure(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.signature_mock = patch.object(c, 'verify_signed_release', side_effect=self.synthetic_signature)
        self.signature_mock.start()

    def tearDown(self):
        self.signature_mock.stop()
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
             'verification':'oci-config-diffids-checked','ubuntu_archive_fingerprints':['A'*40],'files':{
                 'os_release':c.digest(files['/usr/lib/os-release']),
                 'dpkg_status':c.digest(files['/var/lib/dpkg/status']),
                 'ubuntu_key':c.digest(files['/usr/share/keyrings/ubuntu-archive-keyring.gpg']),
                 'dpkg_elf':c.digest(files['/usr/bin/dpkg'])}}
        return b

    def synthetic_signature(self, signed, _key, fingerprints):
        # Tests cover linked data contracts, NOT real OpenPGP validity.
        obj=json.loads(signed.read_text())
        self.assertEqual(fingerprints,['A'*40])
        return obj['release'],['A'*40],c.digest(json.dumps(obj['release'],sort_keys=True).encode())

    def bundle(self):
        out=self.root/'bundle';out.mkdir();(out/'debs').mkdir();e=out/'evidence';e.mkdir()
        key=b'fixture-public-key';(e/'ubuntu-key.gpg').write_bytes(key)
        b={'schema':'podmesh.base-root-binding/v1','base':c.BASE,'suite':'noble','arch':'amd64',
           'verification':'oci-config-diffids-checked','ubuntu_archive_fingerprints':['A'*40],
           'files':{'dpkg_status':'a'*64,'os_release':'a'*64,'ubuntu_key':c.digest(key),'dpkg_elf':'a'*64}}
        binding=json.dumps(b).encode();(e/'base-binding.json').write_bytes(binding)
        versions=[];blocks=[];total=0
        for name,data in [('python3',b'fixture-python'),('libpython3-stdlib',b'fixture-stdlib')]:
            sha=c.digest(data);(out/'debs'/(sha+'.deb')).write_bytes(data);total+=len(data)
            versions.append((name,'3.12.3-0ubuntu1'))
            blocks.append('Package: '+name+'\nVersion: 3.12.3-0ubuntu1\nArchitecture: amd64\nSHA256: '+sha+
                          '\nSize: '+str(len(data))+'\nInstalled-Size: 1\nFilename: pool/main/p/'+name+'/'+name+'.deb\n')
        index=('\n'.join(blocks)+'\n').encode();chains=[]
        for position,spec in enumerate(c.source_specs()):
            release={'Origin':'Ubuntu','Codename':'noble','Suite':spec['suite'],'Components':'main','Architectures':'amd64',
                     'SHA256':'\n '+c.digest(index)+' '+str(len(index))+' '+spec['metakey']}
            signed=(json.dumps({'release':release})).encode()
            (e/f'{position}-InRelease').write_bytes(signed);(e/f'{position}-Packages').write_bytes(index)
            chains.append({'source':spec,'signed_file':f'{position}-InRelease','signed_sha256':c.digest(signed),
                           'verified_plaintext_sha256':c.digest(json.dumps(release,sort_keys=True).encode()),'signers':['A'*40],
                           'index_file':f'{position}-Packages','index':{'sha256':c.digest(index),'bytes':len(index)},
                           'encoding':'uncompressed-utf8-deb822'})
        packages=c.selected_stanzas(e,chains,versions)
        info={'schema':'podmesh.python-closure-evidence/v2','base_binding':b,'base_binding_sha256':c.digest(binding),
              'authenticated_indices':chains,'packages':packages}
        evidence=(json.dumps(info)+'\n').encode();(out/'evidence.json').write_bytes(evidence)
        lines=['PODMESH_PYTHON_CLOSURE_V1','REGISTRY\t'+c.BASE['registry'],'IMAGE\t'+c.BASE['image'],
               'ARCHIVE\t'+c.BASE['archive'],'SUITE\tnoble','ARCH\tamd64','STATUS\t'+'a'*64,
               'OS_RELEASE\t'+'a'*64,'KEY\t'+c.digest(key),'BINDING\t'+c.digest(binding),'EVIDENCE\t'+c.digest(evidence)]
        for package in packages:
            lines.append('\t'.join(['PACKAGE',package['name'],package['version'],package['arch'],package['sha256'],
                                     str(package['bytes']),package['file'],str(package['installedKiB'])]))
        lines.append('END\t2\t'+str(total));(out/'closure.lock').write_text('\n'.join(lines)+'\n')
        return out,c.file_digest(out/'closure.lock')

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

    def rewrite_evidence_lock(self, out, info):
        encoded=(json.dumps(info)+'\n').encode();(out/'evidence.json').write_bytes(encoded)
        lock=out/'closure.lock';rows=lock.read_text().splitlines()
        rows=[('EVIDENCE\t'+c.digest(encoded)) if row.startswith('EVIDENCE\t') else row for row in rows]
        lock.write_text('\n'.join(rows)+'\n')
        return c.file_digest(lock)

    def test_good_chain_replays_all_sources_and_exact_selected_stanzas(self):
        out,sha=self.bundle()
        self.assertEqual(len(c.verify_bundle(out,sha)),2)
        info=json.loads((out/'evidence.json').read_text())
        self.assertEqual([x['source']['suite'] for x in info['authenticated_indices']],['noble','noble-updates','noble-security'])
        self.assertEqual(len(info['packages'][0]['index_candidates']),3)

    def test_corrupt_index_refuses_even_if_evidence_selfhash_recomputed(self):
        out,_=self.bundle();info=json.loads((out/'evidence.json').read_text())
        index=out/'evidence/0-Packages';index.write_bytes(index.read_bytes()+b'tampered')
        info['authenticated_indices'][0]['index']['sha256']=c.file_digest(index)
        info['authenticated_indices'][0]['index']['bytes']=index.stat().st_size
        sha=self.rewrite_evidence_lock(out,info)
        with self.assertRaises(ValueError):c.verify_bundle(out,sha)

    def test_signed_index_size_and_hash_must_both_match(self):
        out,_=self.bundle();info=json.loads((out/'evidence.json').read_text());chain=info['authenticated_indices'][0]
        release=json.loads((out/'evidence'/chain['signed_file']).read_text())['release']
        spec=chain['source'];expected=c.release_index(release,spec)
        self.assertEqual(expected,chain['index'])
        for bad in ['0'*64+' '+str(expected['bytes'])+' '+spec['metakey'],expected['sha256']+' '+str(expected['bytes']+1)+' '+spec['metakey']]:
            changed=dict(release);changed['SHA256']='\n '+bad
            wrong=c.release_index(changed,spec)
            self.assertNotEqual(wrong,chain['index'])
        duplicate=dict(release);duplicate['SHA256']=release['SHA256']+release['SHA256']
        with self.assertRaises(ValueError):c.release_index(duplicate,spec)

    def test_duplicate_missing_and_wrong_suite_source_chains_refuse(self):
        out,_=self.bundle();original=json.loads((out/'evidence.json').read_text())
        for kind in ['duplicate','missing','suite','source']:
            info=json.loads(json.dumps(original));chains=info['authenticated_indices']
            if kind=='duplicate':chains[1]=chains[0]
            if kind=='missing':chains.pop()
            if kind=='suite':chains[0]['source']['suite']='noble-updates'
            if kind=='source':chains[0]['source']['uri']='https://example.invalid/ubuntu'
            sha=self.rewrite_evidence_lock(out,info)
            with self.assertRaises(ValueError):c.verify_bundle(out,sha)

    def test_candidate_stanza_fields_cannot_be_replaced_by_selfhashed_metadata(self):
        out,_=self.bundle();original=json.loads((out/'evidence.json').read_text())
        for field,value in [('name','another-package'),('version','99'),('arch','arm64'),('sha256','b'*64),('archivePath','../../bad')]:
            info=json.loads(json.dumps(original));info['packages'][0][field]=value
            sha=self.rewrite_evidence_lock(out,info)
            with self.assertRaises(ValueError):c.verify_bundle(out,sha)

    def test_duplicate_and_missing_candidate_in_authenticated_index_refuse(self):
        out,_=self.bundle();info=json.loads((out/'evidence.json').read_text());chains=info['authenticated_indices']
        root=out/'evidence';first=root/'0-Packages';data=first.read_bytes();first.write_bytes(data+data)
        with self.assertRaises(ValueError):c.selected_stanzas(root,chains,[('python3','3.12.3-0ubuntu1')])
        first.write_bytes(data)
        with self.assertRaises(ValueError):c.selected_stanzas(root,chains,[('python3','99')])

    def test_signed_stanza_architecture_and_filename_refuse(self):
        out,_=self.bundle();info=json.loads((out/'evidence.json').read_text());chains=info['authenticated_indices'];root=out/'evidence'
        path=root/'0-Packages';original=path.read_text()
        for text in [original.replace('Architecture: amd64','Architecture: arm64'),original.replace('pool/main/p/python3/python3.deb','../../outside')]:
            path.write_text(text)
            with self.assertRaises(ValueError):c.selected_stanzas(root,chains,[('python3','3.12.3-0ubuntu1')])
        path.write_text(original)

    def target_fixture(self):
        lists=self.root/'lists';lists.mkdir();blocks=[]
        for i,spec in enumerate(c.source_specs()):
            f=lists/f'{i}_Packages';f.write_text('Package: fixture\n')
            row={'URI':spec['index_uri'],'MetaKey':spec['metakey'],'Identifier':'Packages','Architecture':'amd64',
                 'Component':'main','Suite':spec['suite'],'Release':spec['suite'],'KeepCompressed':'no','Codename':'noble','Origin':'Ubuntu','Trusted':'yes','Filename':str(f)}
            blocks.append('\n'.join(k+': '+v for k,v in row.items()))
        return lists,'\n\n'.join(blocks)+'\n'

    def test_APT_targets_require_unique_exact_official_uncompressed_bindings(self):
        lists,text=self.target_fixture();self.assertEqual(len(c.index_targets(text,lists)),3)
        for bad in [text+'\n'+text.split('\n\n')[0],text.replace('https://archive.ubuntu.com','https://other.invalid'),
                    text.replace('_Packages','_Packages.lz4'),text.replace('Architecture: amd64','Architecture: arm64'),
                    '\n\n'.join(text.split('\n\n')[:-1])]:
            with self.assertRaises(ValueError):c.index_targets(bad,lists)

    def test_unknown_encoding_and_signature_failure_are_not_bypassed(self):
        out,_=self.bundle();info=json.loads((out/'evidence.json').read_text());info['authenticated_indices'][0]['encoding']='gzip'
        sha=self.rewrite_evidence_lock(out,info)
        with self.assertRaises(ValueError):c.verify_bundle(out,sha)
        info['authenticated_indices'][0]['encoding']='uncompressed-utf8-deb822';sha=self.rewrite_evidence_lock(out,info)
        with patch.object(c,'verify_signed_release',side_effect=ValueError('crypto refused')):
            with self.assertRaises(ValueError):c.verify_bundle(out,sha)
        path=out/'evidence/0-Packages';path.write_bytes(b'Package: python3\nDescription: \xff\n')
        with self.assertRaises(ValueError):list(c.index_stanzas(path))

    def test_extra_retained_signature_and_plaintext_identity_mismatch_refuse(self):
        out,_=self.bundle();info=json.loads((out/'evidence.json').read_text());(out/'evidence/extra-InRelease').write_text('extra')
        sha=self.rewrite_evidence_lock(out,info)
        with self.assertRaises(ValueError):c.verify_bundle(out,sha)
        (out/'evidence/extra-InRelease').unlink();info['authenticated_indices'][0]['verified_plaintext_sha256']='0'*64
        sha=self.rewrite_evidence_lock(out,info)
        with self.assertRaises(ValueError):c.verify_bundle(out,sha)

    def test_non_alphabetical_plan_is_canonicalized_before_lock_evidence(self):
        out,_=self.bundle();info=json.loads((out/'evidence.json').read_text())
        selected=c.selected_stanzas(out/'evidence',info['authenticated_indices'],[('python3','3.12.3-0ubuntu1'),('libpython3-stdlib','3.12.3-0ubuntu1')])
        self.assertEqual([p['name'] for p in selected],['libpython3-stdlib','python3'])

    def test_case_variant_duplicate_package_rows_cannot_hide(self):
        out,_=self.bundle();info=json.loads((out/'evidence.json').read_text());f=out/'evidence/0-Packages'
        original=f.read_text();f.write_text(original+original.replace('Package:','package:').replace('SHA256:','sha256:'))
        with self.assertRaises(ValueError):c.selected_stanzas(out/'evidence',info['authenticated_indices'],[('python3','3.12.3-0ubuntu1')])

    def test_primary_archive_fingerprint_binds_signing_subkey(self):
        status='[GNUPG:] VALIDSIG '+('B'*40)+' 2026-10-22 1 0 4 0 1 8 01 '+('A'*40)+'\n'
        self.assertEqual(c.signature_signers(status,['A'*40]),[{'signing':'B'*40,'primary':'A'*40}])
        with self.assertRaises(ValueError):c.signature_signers(status,['C'*40])
        with self.assertRaises(ValueError):c.signature_signers(status+'[GNUPG:] BADSIG anything\n',['A'*40])

    def test_uncompressed_target_flag_and_literal_release_are_required(self):
        lists,text=self.target_fixture()
        for bad in [text.replace('KeepCompressed: no','KeepCompressed: yes'),text.replace('Release: noble\n','Release: wrong\n')]:
            with self.assertRaises(ValueError):c.index_targets(bad,lists)

    def test_bounded_input_checks_precede_read(self):
        path=self.root/'oversized';path.write_bytes(b'oversized')
        with self.assertRaises(ValueError):c.read_bounded(path,3)
        alias=self.root/'alias';alias.symlink_to(path)
        with self.assertRaises(ValueError):c.read_bounded(alias,100)


if __name__=='__main__':unittest.main()
