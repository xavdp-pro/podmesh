"""Read-only guard regressions; execute on the explicit root build host."""
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

DIRECTORY=Path(__file__).resolve().parents[1]
def module(name,path):
    spec=importlib.util.spec_from_file_location(name,path)
    result=importlib.util.module_from_spec(spec);spec.loader.exec_module(result);return result

tool=module('constructed_enrollment',DIRECTORY/'enroll-constructed-image-grant.py')
fixtures=module('original_enrollment_tests',Path(__file__).with_name('test_enroll_contained_grant.py'))
enc=fixtures.encoded
IMAGE='b'*64;SOURCE='c'*40;MATRIX='d'*64

def policies():
    old,new=fixtures.fixture();old['allowed_images']=['a'*64];new['allowed_images']=['a'*64,IMAGE]
    new['allowed_contained_nested'][-1]['image']=IMAGE
    return old,new


class AppendGuards(unittest.TestCase):
    def test_exact_two_append_arrays_delegate_frozen_grant_guard(self):
        old,new=policies();a,b,g=tool.append_only(enc(old),enc(new),fixtures.A,IMAGE)
        self.assertEqual(a,old);self.assertEqual(b,new);self.assertEqual(g['image'],IMAGE)
        self.assertEqual(tool.sha(tool.base_path),tool.BASE_GUARD_SHA)
        self.assertIs(tool.validate_freshness,tool.base.validate_freshness)
        self.assertIs(tool.check_stopped,tool.base.check_stopped)

    def test_changes_second_append_overlap_network_and_reformat_refuse(self):
        old,new=policies()
        changes=[lambda p:p.update(max_cpus=3),
            lambda p:p['allowed_images'].append('e'*64),
            lambda p:p['allowed_images'].__setitem__(0,'e'*64),
            lambda p:p['allowed_contained_nested'].append(copy.deepcopy(p['allowed_contained_nested'][-1])),
            lambda p:p['allowed_contained_nested'][0].update(network='changed'),
            lambda p:p['allowed_contained_nested'][-1].update(uid_start=200001),
            lambda p:p['allowed_contained_nested'][-1].update(network='host'),
            lambda p:p['allowed_contained_nested'][-1].update(image='f'*64)]
        for change in changes:
            proposed=copy.deepcopy(new);change(proposed)
            with self.subTest(change=change),self.assertRaises(tool.Refusal):
                tool.append_only(enc(old),enc(proposed),fixtures.A,IMAGE)
        for before,after in [(enc(old),enc(new).replace(b'"uid_start": 200000',b'"uid_start":  200000')),
                (enc(old),enc(new).replace(b'"max_cpus": 2',b'"max_cpus":  2'))]:
            with self.assertRaises(tool.Refusal):tool.append_only(before,after,fixtures.A,IMAGE)

    def test_prefix_alias_is_not_a_fresh_image(self):
        old,new=policies();old['allowed_images']=['sha256:'+IMAGE];new['allowed_images']=['sha256:'+IMAGE,IMAGE]
        with self.assertRaises(tool.Refusal):tool.append_only(enc(old),enc(new),fixtures.A,IMAGE)


class ConstructionGuards(unittest.TestCase):
    def setUp(self):
        self.temporary=tempfile.TemporaryDirectory();self.root=Path(self.temporary.name)
        self.etc=self.root/'instance/etc/shaper';self.saved=self.root/'instance/saved'
        self.ssh=self.root/'public-ssh';self.cache=self.root/'inner-store'
        for p in (self.etc,self.saved/'maestro/context',self.ssh,self.cache):p.mkdir(parents=True)
        self.tokens=self.root/'instance/operator.tokens.json';self.tokens.write_text('{"caller":"private-test-value"}')
        self.tokens.chmod(0o600)
        (self.etc/'config.json').write_text('{"universe":"new"}');(self.ssh/'authorized_keys').write_text('public-test-key')
        self.grant=fixtures.grant(fixtures.C,str(self.root),600000,800000)
        self.grant['image']=IMAGE;self.grant['mounts']=[
            {'source':str(self.etc),'destination':'/etc/shaper','read_only':False},
            {'source':str(self.saved),'destination':'/var/lib/shaper','read_only':False},
            {'source':str(self.cache),'destination':'/var/lib/containers','read_only':False},
            {'source':str(self.ssh),'destination':'/root/.ssh','read_only':True}]
        # Root custody itself is exercised by the frozen tests; /tmp is writable
        # and therefore deliberately not an administrator deployment path.
        self.custody=patch.object(tool,'protected',side_effect=lambda p,**kw:Path(p));self.custody.start()

    def tearDown(self):self.custody.stop();self.temporary.cleanup()

    def manifest(self):
        entries=[]
        for p in [self.root,*sorted(self.root.rglob('*'))]:
            st=p.lstat();entry={'path':str(p.relative_to(self.root)),'uid':st.st_uid,'gid':st.st_gid,'mode':stat.S_IMODE(st.st_mode),
                'type':'directory'if p.is_dir()else'file'}
            if entry['type']=='file':entry.update(bytes=st.st_size,sha256=tool.sha(p))
            entries.append(entry)
        return {'schema':'podmesh.preconstructed-universe/1','universe_uuid':fixtures.C,'volume_root':str(self.root),
            'image_id':IMAGE,'matrix_sha256':MATRIX,'source_pin':SOURCE,'entries':entries}

    def verify(self,manifest=None):return tool.construction(self.grant,manifest or self.manifest(),IMAGE,MATRIX,SOURCE)

    def test_reviewed_configs_and_exact_host_credentials_are_preserved(self):
        self.verify();self.assertEqual(self.tokens.read_text(),'{"caller":"private-test-value"}')
        manifest=self.manifest();manifest['entries'][0]['path']=''
        with self.assertRaises(tool.Refusal):self.verify(manifest)
        manifest=self.manifest();(self.etc/'config.json').write_text('{"changed":true}')
        with self.assertRaises(tool.Refusal):self.verify(manifest)

    def test_empty_functional_context_may_have_bounded_nonroot_identity(self):
        context=self.saved/'maestro/context';os.chown(context,2104,2104);context.chmod(0o700)
        self.verify();os.chown(context,2**30,2104)
        with self.assertRaises(tool.Refusal):self.verify()

    def test_saved_sql_cache_and_undeclared_credential_files_refuse_even_reviewed(self):
        for p in (self.saved/'sql',self.cache/'blob',self.root/'other.tokens.json',self.ssh/'private-key'):
            p.write_bytes(b'not-allowed')
            with self.subTest(path=p),self.assertRaises(tool.Refusal):self.verify()
            p.unlink()

    def test_credentials_must_be_single_link_private_bounded_object(self):
        for content in ('[]','{"x":1,"x":2}','x'*1048577):
            self.tokens.write_text(content)
            with self.subTest(content_size=len(content)),self.assertRaises((tool.Refusal,ValueError)):self.verify()
        self.tokens.write_text('{}');self.tokens.chmod(0o644)
        with self.assertRaises(tool.Refusal):self.verify()
        self.tokens.chmod(0o600);os.link(self.tokens,self.root/'shared')
        with self.assertRaises(tool.Refusal):self.verify()
        (self.root/'shared').unlink();(self.saved/'link').symlink_to(self.etc,target_is_directory=True)
        with self.assertRaises(tool.Refusal):self.verify()

    def test_wrong_image_source_matrix_or_duplicate_manifest_path_refuse(self):
        for key,value in [('image_id','e'*64),('source_pin','e'*40),('matrix_sha256','e'*64)]:
            manifest=self.manifest();manifest[key]=value
            with self.subTest(key=key),self.assertRaises(tool.Refusal):self.verify(manifest)
        manifest=self.manifest();manifest['entries'].append(manifest['entries'][0])
        with self.assertRaises(tool.Refusal):self.verify(manifest)


def tar_bytes(files):
    output=io.BytesIO()
    with tarfile.open(fileobj=output,mode='w')as archive:
        for name,data in files.items():
            info=tarfile.TarInfo(name);info.size=len(data);archive.addfile(info,io.BytesIO(data))
    return output.getvalue()


class ImageGuards(unittest.TestCase):
    def test_real_oci_config_and_manifest_custody_require_exact_loaded_identity(self):
        config=b'{"architecture":"amd64","os":"linux"}';image=hashlib.sha256(config).hexdigest()
        manifest=enc({'config':{'digest':'sha256:'+image}});digest=hashlib.sha256(manifest).hexdigest()
        oci=tar_bytes({'index.json':enc({'manifests':[{'digest':'sha256:'+digest}]}),
            'blobs/sha256/'+digest:manifest,'blobs/sha256/'+image:config})
        lock={'schema':'shaper.core.matrix/v1','class':'univ-shaper-core','source_pin':SOURCE,'images':{'outer':{'image_id':'sha256:'+image,'manifest_digest':'sha256:'+digest,
            'archive':'outer.oci.tar','archive_sha256':hashlib.sha256(oci).hexdigest()}}}
        matrix=tar_bytes({'cfg-image-lock.json':enc(lock),'outer.oci.tar':oci})
        with tempfile.TemporaryDirectory()as tmp,patch.object(tool,'protected',side_effect=lambda p,**kw:Path(p)):
            path=Path(tmp)/'matrix.tar';path.write_bytes(matrix);hash=tool.sha(path)
            instance=SimpleNamespace(podman=lambda *args:SimpleNamespace(stdout=enc([{'Id':'sha256:'+image}])))
            self.assertEqual(tool.image_proof(instance,path,hash,image,SOURCE)['image_id'],image)
            for wrong_image,wrong_source,wrong_hash in [(IMAGE,SOURCE,hash),(image,'e'*40,hash),(image,SOURCE,'e'*64)]:
                with self.assertRaises(tool.Refusal):tool.image_proof(instance,path,wrong_hash,wrong_image,wrong_source)
            instance.podman=lambda *args:SimpleNamespace(stdout=enc([{'Id':'sha256:'+IMAGE}]))
            with self.assertRaises(tool.Refusal):tool.image_proof(instance,path,hash,image,SOURCE)


# Execute the existing generation/process and exact native target regressions
# against the same immutable functions exported by this separate tool.
class ReusedNativeGuards(unittest.TestCase):
    test_absence_requires_sql_api_and_exact_app_process_in_stopped_generation=fixtures.AppendGuards.test_absence_requires_sql_api_and_exact_app_process_in_stopped_generation
    test_native_phase_service_pid_and_container_process_require_stopped=fixtures.AppendGuards.test_native_phase_service_pid_and_container_process_require_stopped
    test_only_exact_recorded_target_may_omit_service_pid_properties=fixtures.AppendGuards.test_only_exact_recorded_target_may_omit_service_pid_properties
    test_atomic_replace_error_does_not_modify_receipt=fixtures.AppendGuards.test_atomic_replace_error_does_not_modify_receipt


if __name__=='__main__':unittest.main()
