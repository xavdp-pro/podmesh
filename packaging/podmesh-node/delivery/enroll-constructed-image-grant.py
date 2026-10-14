#!/usr/bin/env python3
"""Root-only stopped-instance append of one constructed grant and optional image.

The frozen empty-target enroller remains unchanged. This separate administrator
admits only exact preconstructed configuration bytes from root-reviewed custody;
no SQL/cache data, provider change, lifecycle operation or unreviewed image pull.
"""
import argparse
import copy
import fcntl
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import stat
import tarfile
import time

BASE_GUARD_SHA='6381411f3ffdd335fc25e2e384251bd7c1df4f8c6a7d2d36c83c895a1dde8702'
base_path=Path(__file__).with_name('enroll-contained-grant.py')
with base_path.open('rb')as stream:
    if hashlib.file_digest(stream,'sha256').hexdigest()!=BASE_GUARD_SHA:
        raise RuntimeError('frozen stopped-instance guard source differs')
spec=importlib.util.spec_from_file_location('frozen_contained_enrollment',base_path)
base=importlib.util.module_from_spec(spec);spec.loader.exec_module(base)
Refusal=base.Refusal
require=base.require
sha=base.sha
decode=base.decode
protected=base.protected
directory_identities=base.directory_identities
check_stopped=base.check_stopped
inventory_guard=base.inventory_guard
validate_freshness=base.validate_freshness
state_fingerprint=base.state_fingerprint
write_new=base.write_new
replace_from=base.replace_from


def spans(data):
    text=data.decode('utf8');decoder=json.JSONDecoder(object_pairs_hook=base.unique)
    def white(index):
        while index<len(text)and text[index].isspace():index+=1
        return index
    index=white(0);require(text[index]=='{','policy object required');index+=1;found={}
    while True:
        index=white(index)
        if text[index]=='}':break
        name,index=decoder.raw_decode(text,index);index=white(index)
        require(text[index]==':','policy field separator differs');index=white(index+1)
        start=index;value,index=decoder.raw_decode(text,index)
        require(name not in found,'duplicate policy field')
        found[name]=(start,index,value)
        index=white(index)
        if text[index]==',':index+=1
        else:require(text[index]=='}','policy object separator differs');break
    return text,found


def unchanged_arrays(before,after):
    old_text,old=spans(before);new_text,new=spans(after)
    def masked(text,fields):
        out=[];cursor=0
        for name in sorted(('allowed_images','allowed_contained_nested'),key=lambda n:fields[n][0]):
            start,end,_=fields[name];out.extend((text[cursor:start],'<'+name+'>'));cursor=end
        out.append(text[cursor:]);return ''.join(out)
    require(masked(old_text,old)==masked(new_text,new),'policy bytes outside two append arrays changed')
    return old_text,old,new_text,new


def append_only(before,after,template_uuid,image):
    old=decode(before);new=decode(after)
    old_text,old_spans,new_text,new_spans=unchanged_arrays(before,after)
    require(re.fullmatch('[0-9a-f]{64}',image),'one canonical approved image digest required')
    original=old['allowed_images'];proposed=new['allowed_images']
    require(isinstance(original,list)and isinstance(proposed,list)
        and all(isinstance(value,str)and re.fullmatch(r'(?:sha256:)?[0-9a-f]{64}',value)for value in original),
        'canonical existing approved image identities required')
    matches=[value for value in original if value.removeprefix('sha256:')==image]
    # Raw items and punctuation/whitespace surrounding old values stay intact.
    old_array=old_text[old_spans['allowed_images'][0]:old_spans['allowed_images'][1]]
    new_array=new_text[new_spans['allowed_images'][0]:new_spans['allowed_images'][1]]
    def item_bytes(array):
        # Treat this array as the frozen guard's existing grant-array reader.
        text,(_,_,items)=base.array_span(('{"allowed_contained_nested":'+array+'}').encode())
        return items
    if matches:
        require(len(matches)==1 and proposed==original and new_array==old_array,
                'already approved image requires the exact unchanged image list, without duplicate approval')
    else:
        require(len(proposed)==len(original)+1 and proposed[:-1]==original and proposed[-1]==image,
                'exactly one new approved image must be appended')
        require(item_bytes(new_array)[:-1]==item_bytes(old_array),'old allowed image item bytes changed')
    _,(_,_,raw_grants)=base.array_span(after)
    grant=new['allowed_contained_nested'][-1]
    require(grant['image']==image,'new grant must use only the explicitly approved image')
    templates=[g for g in old['allowed_contained_nested']if g['universe_uuid']==template_uuid]
    require(len(templates)==1,'one existing explicit template grant required')
    template=templates[0]
    # Delegate all original grant/network/capability/map/layout restrictions to
    # the unchanged exact guard. Only the separately reviewed image delta is
    # normalized for that read-only validation; committed bytes are untouched.
    raw=raw_grants[-1];_,fields=spans(raw.encode());start,end,_=fields['image']
    normalized_raw=raw[:start]+json.dumps(template['image'])+raw[end:]
    array_start,array_end,_=new_spans['allowed_contained_nested']
    grant_array=new_text[array_start:array_end]
    require(grant_array.count(raw)==1,'ambiguous last grant bytes')
    grant_array=grant_array.replace(raw,normalized_raw,1)
    edits=[(new_spans['allowed_images'][0],new_spans['allowed_images'][1],old_array),(array_start,array_end,grant_array)]
    normalized=new_text
    for start,end,replacement in sorted(edits,reverse=True):normalized=normalized[:start]+replacement+normalized[end:]
    base.append_only(before,normalized.encode(),template_uuid)
    return old,new,grant


def construction(grant,manifest,image,matrix_hash,source_pin):
    require(set(manifest)=={'schema','universe_uuid','volume_root','image_id','matrix_sha256','source_pin','entries'}
        and manifest['schema']=='podmesh.preconstructed-universe/1'
        and manifest['universe_uuid']==grant['universe_uuid']and manifest['volume_root']==grant['volume_root']
        and manifest['image_id']==image and manifest['matrix_sha256']==matrix_hash and manifest['source_pin']==source_pin,
        'reviewed construction/image/source binding differs')
    root=protected(grant['volume_root'],directory=True);mounts={m['destination']:m for m in grant['mounts']}
    require('/etc/shaper'in mounts and '/var/lib/shaper'in mounts,'configuration/saved bindings required')
    etc=Path(mounts['/etc/shaper']['source'])
    saved=Path(mounts['/var/lib/shaper']['source'])
    require(etc.name=='shaper'and etc.parent.name=='etc','declared configuration layout differs')
    tokens=etc.parent.parent/'operator.tokens.json'
    require(tokens.is_relative_to(root),'operator credential leaf outside declared root')
    def bounded_identity(meta):
        return (0<=meta.st_uid<grant['id_count']or grant['uid_start']<=meta.st_uid<grant['uid_start']+grant['id_count'])and (0<=meta.st_gid<grant['id_count']or grant['gid_start']<=meta.st_gid<grant['gid_start']+grant['id_count'])
    ssh=Path(mounts['/root/.ssh']['source'])if '/root/.ssh'in mounts else None
    if ssh:require(mounts['/root/.ssh']['read_only']is True,'public SSH mount must remain read-only')
    actual=[]
    for path in [root,*sorted(root.rglob('*'))]:
        meta=path.lstat();require(meta.st_dev==root.stat().st_dev and not stat.S_ISLNK(meta.st_mode)
            and not meta.st_mode&0o022,'constructed path custody/type/write access differs')
        relative=str(path.relative_to(root));entry={'path':relative,'uid':meta.st_uid,'gid':meta.st_gid,'mode':stat.S_IMODE(meta.st_mode)}
        if stat.S_ISDIR(meta.st_mode):
            require(meta.st_uid==0 and bounded_identity(meta)or path!=saved and path.is_relative_to(saved)and bounded_identity(meta),
                'nonroot directory permitted only inside empty saved subtree with bounded identity')
            entry['type']='directory'
        else:
            require(stat.S_ISREG(meta.st_mode)and meta.st_nlink==1,'constructed special/link/shared file refused')
            require(path==tokens or path.is_relative_to(etc)or ssh is not None and path.is_relative_to(ssh),
                'saved SQL/cache/undeclared constructed file refused')
            if ssh:
                require(not path.is_relative_to(ssh)or path==ssh/'authorized_keys','public SSH may contain only authorized_keys')
            require(bounded_identity(meta),'constructed file owner/group outside declared identity range')
            if path==tokens:
                require(meta.st_uid==0 and meta.st_gid==0 and stat.S_IMODE(meta.st_mode)==0o600
                    and 0<meta.st_size<=1048576,'operator credential leaf custody/bound differs')
                require(isinstance(decode(path.read_bytes()),dict),'operator credential leaf must be bounded JSON object')
            entry.update(type='file',bytes=meta.st_size,sha256=sha(path))
        actual.append(entry)
    require(isinstance(manifest['entries'],list)and len({entry['path']for entry in manifest['entries']})==len(manifest['entries'])
        and sorted(actual,key=lambda entry:entry['path'])==sorted(manifest['entries'],key=lambda entry:entry['path']),
        'actual complete constructed tree differs from reviewed manifest')
    return actual


def image_proof(instance,matrix_file,matrix_hash,image,source_pin):
    matrix=protected(matrix_file);require(sha(matrix)==matrix_hash,'approved matrix bytes differ')
    require(re.fullmatch('[0-9a-f]{40}',source_pin),'exact approved image source pin required')
    with tarfile.open(matrix,'r:*')as archive:
        members=archive.getmembers();require(len(members)<=100000 and len({m.name for m in members})==len(members)
            and all(m.isfile()and not Path(m.name).is_absolute()and '..'not in Path(m.name).parts for m in members),
            'approved matrix structure differs')
        member=archive.getmember('cfg-image-lock.json');require(member.size<=1048576,'matrix lock too large')
        lock=decode(archive.extractfile(member).read());require(lock.get('schema')=='shaper.core.matrix/v1'
            and lock.get('class')=='univ-shaper-core'and lock['source_pin']==source_pin,'approved matrix source/type differs')
        outer=lock['images']['outer'];require(outer['image_id']=='sha256:'+image,'approved matrix outer image differs')
        require(re.fullmatch('sha256:[0-9a-f]{64}',outer['manifest_digest']),'canonical outer manifest digest required')
        member=archive.getmember(outer['archive'])
        with archive.extractfile(member)as stream:require(hashlib.file_digest(stream,'sha256').hexdigest()==outer['archive_sha256'],'matrix outer OCI checksum differs')
        with archive.extractfile(member)as stream,tarfile.open(fileobj=stream,mode='r:*')as oci:
            entries=oci.getmembers();require(len(entries)<=100000 and len({m.name for m in entries})==len(entries)
                and all((m.isfile()or m.isdir())and not Path(m.name).is_absolute()and '..'not in Path(m.name).parts for m in entries),
                'outer OCI member custody differs')
            index_member=oci.getmember('index.json');require(index_member.isfile()and index_member.size<=1048576,'OCI index bound differs')
            index=decode(oci.extractfile(index_member).read());require(len(index['manifests'])==1
                and index['manifests'][0]['digest']==outer['manifest_digest'],'approved outer OCI manifest differs')
            raw=oci.extractfile('blobs/sha256/'+outer['manifest_digest'][7:]).read(1048577)
            require(len(raw)<=1048576 and hashlib.sha256(raw).hexdigest()==outer['manifest_digest'][7:],'outer manifest bytes differ')
            config=decode(raw)['config'];require(config['digest']=='sha256:'+image,'outer image configuration identity differs')
            blob=oci.extractfile('blobs/sha256/'+image).read(1048577)
            require(len(blob)<=1048576 and hashlib.sha256(blob).hexdigest()==image,'outer configuration bytes differ')
    actual=decode(instance.podman('image','inspect',image).stdout)
    require(len(actual)==1 and actual[0]['Id'].removeprefix('sha256:')==image,'approved image is not already loaded in actual private store')
    return {'matrix_sha256':matrix_hash,'source_pin':source_pin,'image_id':image,
        'manifest_digest':outer['manifest_digest'],'outer_oci_sha256':outer['archive_sha256'],'actual_image_id':actual[0]['Id']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--scope', required=True)
    parser.add_argument('--expected-bundle-manifest-sha256', required=True)
    parser.add_argument('--expected-policy-sha256', required=True)
    parser.add_argument('--expected-receipt-sha256', required=True)
    parser.add_argument('--new-policy', required=True, type=Path)
    parser.add_argument('--expected-new-policy-sha256', required=True)
    parser.add_argument('--template-uuid', required=True)
    parser.add_argument('--enrollment-root', required=True, type=Path)
    parser.add_argument('--operation-id', required=True)
    parser.add_argument('--tool-source-pin', required=True)
    parser.add_argument('--expected-tool-sha256', required=True)
    parser.add_argument('--freshness-receipt', required=True, type=Path,
                        help='actual root-observed private SQL/inventory absence, collected before native stop')
    parser.add_argument('--freshness-sha256', required=True)
    parser.add_argument('--expected-image-id',required=True)
    parser.add_argument('--expected-image-source-pin',required=True)
    parser.add_argument('--matrix-file',required=True,type=Path)
    parser.add_argument('--expected-matrix-sha256',required=True)
    parser.add_argument('--construction-manifest',required=True,type=Path)
    parser.add_argument('--expected-construction-manifest-sha256',required=True)
    args = parser.parse_args()
    require(os.geteuid() == 0, 'separate root administrator required')
    os.umask(0o077)
    require(re.fullmatch('[a-zA-Z0-9_-]{1,96}', args.operation_id), 'bounded safe enrollment operation ID required')
    require(re.fullmatch('[0-9a-f]{40}', args.tool_source_pin), 'separate full tool source pin required')
    require(sha(Path(__file__).resolve()) == args.expected_tool_sha256, 'pinned administrator bytes differ')
    protected(base_path);require(sha(base_path)==BASE_GUARD_SHA,'frozen base administrator changed')
    bundle_manifest = protected(args.bundle / 'bundle.json')
    require(sha(bundle_manifest) == args.expected_bundle_manifest_sha256, 'original installed bundle manifest differs')
    bundle = decode(bundle_manifest.read_bytes())
    instance_path = protected(args.bundle / 'instance.py')
    require(sha(instance_path) == bundle['payload_sha256']['instance.py'], 'original administration payload differs before import')
    spec = importlib.util.spec_from_file_location('original_installed_node', instance_path)
    original = importlib.util.module_from_spec(spec);spec.loader.exec_module(original)
    instance = original.Instance(args.scope)
    protected(instance.root, directory=True, mode=0o700)
    fd = os.open(instance.root / 'operator.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'r+') as lock:
        protected(instance.root / 'operator.lock', mode=0o600)
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        instance.read();check_stopped(instance)
        policy_path = protected(instance.root / 'policy.json', mode=0o600)
        receipt_path = protected(instance.receipt, mode=0o600)
        require(sha(policy_path) == args.expected_policy_sha256 and sha(receipt_path) == args.expected_receipt_sha256,
                'expected stopped preimages changed')
        before_policy, before_receipt = policy_path.read_bytes(), receipt_path.read_bytes()
        proposal = protected(args.new_policy, mode=0o600);require(sha(proposal) == args.expected_new_policy_sha256, 'reviewed append proposal changed')
        after_policy = proposal.read_bytes();old, new, grant = append_only(before_policy, after_policy, args.template_uuid,args.expected_image_id)
        approved_root = protected(args.enrollment_root, directory=True)
        require(len(approved_root.parts) >= 4, 'dedicated enrollment subtree required')
        new_root = Path(grant['volume_root'])
        require(new_root != approved_root and new_root.is_relative_to(approved_root)
                and not new_root.is_relative_to(instance.root) and not new_root.is_relative_to(args.bundle), 'new root outside explicit dedicated enrollment subtree')
        old_inodes = directory_identities(old['allowed_contained_nested'])
        new_inodes = directory_identities([grant])
        manifest_path=protected(args.construction_manifest,mode=0o600)
        require(sha(manifest_path)==args.expected_construction_manifest_sha256,'root-reviewed construction manifest differs')
        manifest=decode(manifest_path.read_bytes())
        constructed_before=construction(grant,manifest,args.expected_image_id,args.expected_matrix_sha256,args.expected_image_source_pin)
        approved_image=image_proof(instance,args.matrix_file,args.expected_matrix_sha256,args.expected_image_id,args.expected_image_source_pin)
        require(not { (i['device'], i['inode']) for i in old_inodes.values()} &
                    { (i['device'], i['inode']) for i in new_inodes.values()}, 'new directories reuse retained inode identity')
        inventory_guard(instance, grant)
        freshness_path = protected(args.freshness_receipt, mode=0o600)
        require(sha(freshness_path) == args.freshness_sha256, 'actual fresh receiving proof changed')
        freshness = decode(freshness_path.read_bytes())
        evidence = freshness.get('evidence')
        require(isinstance(evidence, dict) and set(evidence) == {'sql', 'inventory', 'process'},
                'actual SQL/API/process snapshots required, not absence booleans alone')
        snapshots = {}
        for kind, reference in evidence.items():
            proof_path = protected(reference['path'], mode=0o600)
            require(sha(proof_path) == reference['sha256'], 'paired absence evidence checksum differs')
            snapshots[kind] = decode(proof_path.read_bytes())
        validate_freshness(freshness, snapshots, instance, grant, args.expected_policy_sha256)
        provider_before = state_fingerprint(instance.root / 'provider-state')
        enrollment_parent = instance.root / 'enrollments'
        enrollment_parent.mkdir(mode=0o700, exist_ok=True);protected(enrollment_parent, directory=True, mode=0o700)
        for previous in enrollment_parent.iterdir():
            protected(previous, directory=True, mode=0o700)
            completed = protected(previous / 'completed.json', mode=0o600)
            require(decode(completed.read_bytes()).get('status') == 'ROOT_CONFIGURATION_ENROLLED',
                    'prior enrollment incomplete; preserve and inspect before another append')
        transaction = enrollment_parent / args.operation_id
        require(not transaction.exists(), 'enrollment exists/partial: preserve and inspect it; no replay/adoption')
        transaction.mkdir(mode=0o700)
        for directory in (instance.root, enrollment_parent):
            directory_fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            os.fsync(directory_fd);os.close(directory_fd)
        intent = {'schema':'podmesh.constructed-image-grant-enrollment/1','operation_id':args.operation_id,
            'scope':args.scope,'bundle':instance.r['bundle'],'host_machine_id':instance.r['host_machine_id'],
            'original_bundle_manifest_sha256':args.expected_bundle_manifest_sha256,
            'tool_source_pin':args.tool_source_pin,'tool_sha256':args.expected_tool_sha256,'time':time.time(),
            'before_policy_sha256':args.expected_policy_sha256,'before_receipt_sha256':args.expected_receipt_sha256,
            'new_policy_sha256':args.expected_new_policy_sha256,'freshness_sha256':args.freshness_sha256,
            'added_uuid':grant['universe_uuid'],'old_directory_identities':old_inodes,'new_directory_identities':new_inodes,
            'provider_state_sha256':provider_before,'policy_change':
                'append-one-constructed-contained-grant-with-existing-approved-image'if new['allowed_images']==old['allowed_images']else
                'append-one-approved-image-and-one-constructed-contained-grant',
            'image_proof':approved_image,'construction_manifest_sha256':args.expected_construction_manifest_sha256,
            'base_guard_sha256':BASE_GUARD_SHA}
        write_new(transaction / 'intent.json', (json.dumps(intent,indent=2,sort_keys=True)+'\n').encode())
        write_new(transaction / 'policy.before.json', before_policy)
        write_new(transaction / 'instance.before.json', before_receipt)
        write_new(transaction / 'policy.after.json', after_policy)
        after = decode(before_receipt);after['configuration_sha256']['policy.json'] = args.expected_new_policy_sha256
        after_receipt = (json.dumps(after,indent=2,sort_keys=True)+'\n').encode()
        write_new(transaction / 'instance.after.json', after_receipt)
        # The receipt's only semantic change is the actual new policy checksum.
        expected = copy.deepcopy(instance.r);expected['configuration_sha256']['policy.json'] = args.expected_new_policy_sha256
        require(after == expected, 'unexpected instance receipt delta')
        write_new(transaction / 'policy.commit', after_policy)
        write_new(transaction / 'instance.commit', after_receipt)
        check_stopped(instance);inventory_guard(instance,grant)
        require(construction(grant,manifest,args.expected_image_id,args.expected_matrix_sha256,args.expected_image_source_pin)==constructed_before,
            'constructed bytes changed before commit')
        require(image_proof(instance,args.matrix_file,args.expected_matrix_sha256,args.expected_image_id,args.expected_image_source_pin)==approved_image,
            'approved image changed before commit')
        require(sha(policy_path) == args.expected_policy_sha256 and sha(receipt_path) == args.expected_receipt_sha256
                and old_inodes == directory_identities(old['allowed_contained_nested'])
                and new_inodes == directory_identities([grant]), 'preimages/directory custody changed before commit')
        replace_from(transaction / 'policy.commit', policy_path)
        replace_from(transaction / 'instance.commit', receipt_path)
        instance.read();check_stopped(instance)
        require(instance.r == after and old_inodes == directory_identities(old['allowed_contained_nested'])
                and new_inodes == directory_identities([grant]) and provider_before == state_fingerprint(instance.root / 'provider-state'),
                'post-enrollment custody differs; preserve versioned intent and do not start')
        require(construction(grant,manifest,args.expected_image_id,args.expected_matrix_sha256,args.expected_image_source_pin)==constructed_before,
            'constructed bytes changed after enrollment; preserve intent and do not start')
        complete = {**intent,'status':'ROOT_CONFIGURATION_ENROLLED','after_receipt_sha256':sha(receipt_path),
            'original_instance_read_passed':True,'native_phase':'stopped','existing_grants_preserved':True,'existing_images_preserved':True,'constructed_bytes_preserved':True,
            'sql_or_lifecycle_effects':False,'runtime_qualified':False}
        write_new(transaction / 'completed.json', (json.dumps(complete,indent=2,sort_keys=True)+'\n').encode())
        print(json.dumps({'status':complete['status'],'added_uuid':grant['universe_uuid'],'record':str(transaction),
                          'runtime_qualified':False}))


if __name__ == '__main__':
    main()
