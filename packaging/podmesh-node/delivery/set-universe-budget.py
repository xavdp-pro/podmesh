#!/usr/bin/env python3
"""Explicit stopped private NODE administration: change only max_universes.

No service stop/start, provider record/history deletion, new grant, SQL write,
resource operation, installed bundle change or identity adoption. The root caller
supplies reviewed old/new policy bytes, bounded budget and required new capacity.
Original installed Instance/read/unit checks and separately pinned enroller
stopped/custody/atomic-commit guards are reused. Two-file interrupted commits
remain durable and refused; operator reconciliation is explicit, never automatic.
"""
import argparse
import copy
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import time
import uuid


class Refusal(RuntimeError):pass


def require(value, reason):
    if not value:raise Refusal(reason)


def unique(pairs):
    result={}
    for key,value in pairs:
        require(key not in result,'duplicate JSON field');result[key]=value
    return result


def decode(data):return json.loads(data,object_pairs_hook=unique)


def scalar_span(data):
    """Locate the one top-level scalar while retaining every outside byte."""
    text=data.decode('utf8');decoder=json.JSONDecoder(object_pairs_hook=unique)
    def whitespace(index):
        while index<len(text) and text[index].isspace():index+=1
        return index
    index=whitespace(0);require(index<len(text) and text[index]=='{','policy object required');index+=1
    found=None
    while True:
        index=whitespace(index);require(index<len(text),'truncated policy object')
        if text[index]=='}':break
        key,index=decoder.raw_decode(text,index);index=whitespace(index)
        require(text[index]==':','invalid policy object');index=whitespace(index+1)
        start=index;value,index=decoder.raw_decode(text,index)
        if key=='max_universes':
            require(found is None and type(value) is int,'one integer universe budget required');found=(start,index)
        index=whitespace(index)
        if text[index]==',':index+=1
        else:require(text[index]=='}','invalid policy separator');break
    require(found is not None,'max_universes scalar missing');return text,found


def budget_only(before,after,expected_old,expected_new,retained_records,required_new_identities):
    require(all(type(value) is int for value in (expected_old,expected_new,retained_records,required_new_identities)),
        'typed integer budget/counts required')
    require(1<=expected_old<expected_new<=256 and retained_records>=0
        and 1<=required_new_identities<=256,'explicit bounded budget increase required')
    old,new=decode(before),decode(after)
    require(old['max_universes']==expected_old and new['max_universes']==expected_new,'expected scalar values differ')
    old_text,(a,b)=scalar_span(before);new_text,(c,d)=scalar_span(after)
    require(old_text[:a]==new_text[:c] and old_text[b:]==new_text[d:],'nonbudget policy bytes changed')
    expected=copy.deepcopy(old);expected['max_universes']=expected_new
    require(new==expected,'policy changed beyond universe budget')
    approved=[]
    for grant in old.get('allowed_nested',[])+old.get('allowed_contained_nested',[]):
        identifier=grant['universe_uuid'];require(str(uuid.UUID(identifier))==identifier,'approved UUID is not canonical')
        approved.append(identifier)
    require(len(set(approved))==len(approved) and expected_new<=len(approved),'new budget exceeds distinct root-approved identities')
    require(retained_records+required_new_identities<=expected_new,'budget cannot fit retained history plus requested new identities')
    return old,new


def receipt_after(before,old_policy_sha256,new_policy_sha256):
    receipt=decode(before)
    require(receipt['configuration_sha256']['policy.json']==old_policy_sha256,'receipt old policy hash differs')
    result=copy.deepcopy(receipt);result['configuration_sha256']['policy.json']=new_policy_sha256
    return result


def provider_identity_stem(stem):
    # f761 lifecycle::is_uuid: ASCII hexadecimal, exact separators; no version/variant normalization.
    return re.fullmatch(r'[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}',stem) is not None


def provider_records(enroller,root):
    enroller.protected(root,directory=True,mode=0o700);count=0;manifest={}
    for path in sorted(root.rglob('*')):
        item=path.lstat();require(not stat.S_ISLNK(item.st_mode) and item.st_uid==0 and not item.st_mode & 0o022,
            'provider history custody differs')
        require(stat.S_ISREG(item.st_mode) or stat.S_ISDIR(item.st_mode),'provider history special file refused')
        # Match host_adapter read_dir/file_stem counting, independent of extension/type.
        # Intent, unknown and malformed file bytes remain preserved in the manifest.
        if path.parent==root and provider_identity_stem(path.stem):count+=1
        if stat.S_ISREG(item.st_mode):
            enroller.protected(path);manifest[str(path.relative_to(root))]={'sha256':enroller.sha(path),
                'uid':item.st_uid,'gid':item.st_gid,'mode':stat.S_IMODE(item.st_mode),'device':item.st_dev,'inode':item.st_ino}
    return count,manifest


def preflight_original(enroller,instance,expected_policy_sha256,expected_receipt_sha256):
    instance.read();enroller.check_stopped(instance)
    policy=enroller.protected(instance.root/'policy.json',mode=0o600)
    receipt=enroller.protected(instance.receipt,mode=0o600)
    require(enroller.sha(policy)==expected_policy_sha256 and enroller.sha(receipt)==expected_receipt_sha256,
        'stopped policy/receipt preimages differ')
    require(instance.r['configuration_sha256']['policy.json']==expected_policy_sha256,'native receipt policy binding differs')
    return policy,receipt


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('bundle','enroller-file','new-policy'):
        parser.add_argument('--'+name,required=True,type=Path)
    for name in ('scope','operation-id','tool-source-pin','expected-tool-sha256','expected-enroller-sha256',
        'expected-bundle-manifest-sha256','expected-policy-sha256','expected-receipt-sha256','expected-new-policy-sha256'):
        parser.add_argument('--'+name,required=True)
    for name in ('expected-old-budget','new-budget','required-new-identities'):
        parser.add_argument('--'+name,required=True,type=int)
    args=parser.parse_args();require(os.geteuid()==0,'explicit root administrator required');os.umask(0o077)
    require(re.fullmatch('[A-Za-z0-9_-]{1,96}',args.operation_id) and re.fullmatch('[0-9a-f]{40}',args.tool_source_pin),
        'bounded operation and full source pin required')
    def digest(path):
        with Path(path).open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()
    require(digest(Path(__file__).resolve())==args.expected_tool_sha256,'pinned budget administrator differs')
    # Check the separately frozen enroller before importing any of its root code.
    for path in (args.enroller_file,*args.enroller_file.parents):
        item=path.lstat();require(path.is_absolute() and item.st_uid==0 and not stat.S_ISLNK(item.st_mode)
            and not item.st_mode & 0o022,'enroller root custody differs')
    leaf=args.enroller_file.lstat();require(stat.S_ISREG(leaf.st_mode) and leaf.st_nlink==1,'one protected enroller file required')
    require(digest(args.enroller_file)==args.expected_enroller_sha256,'pinned enroller differs')
    spec=importlib.util.spec_from_file_location('frozen_contained_enroller',args.enroller_file)
    enroller=importlib.util.module_from_spec(spec);spec.loader.exec_module(enroller)
    manifest=enroller.protected(args.bundle/'bundle.json');require(enroller.sha(manifest)==args.expected_bundle_manifest_sha256,'installed immutable manifest differs')
    bundle=decode(manifest.read_bytes());original_path=enroller.protected(args.bundle/'instance.py')
    require(enroller.sha(original_path)==bundle['payload_sha256']['instance.py'],'original Instance bytes differ')
    spec=importlib.util.spec_from_file_location('original_budget_instance',original_path);original=importlib.util.module_from_spec(spec);spec.loader.exec_module(original)
    instance=original.Instance(args.scope);enroller.protected(instance.root,directory=True,mode=0o700)
    fd=os.open(instance.root/'operator.lock',os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'r+') as lock:
        enroller.protected(instance.root/'operator.lock',mode=0o600);fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        policy_path,receipt_path=preflight_original(enroller,instance,args.expected_policy_sha256,args.expected_receipt_sha256)
        before_policy,before_receipt=policy_path.read_bytes(),receipt_path.read_bytes()
        proposal=enroller.protected(args.new_policy,mode=0o600)
        require(enroller.sha(proposal)==args.expected_new_policy_sha256,'reviewed scalar proposal differs');after_policy=proposal.read_bytes()
        count,history=provider_records(enroller,instance.root/'provider-state')
        old,new=budget_only(before_policy,after_policy,args.expected_old_budget,args.new_budget,count,args.required_new_identities)
        identities_grants=old.get('allowed_contained_nested',[])+[g for g in old.get('allowed_nested',[]) if 'volume_root' in g and 'mounts' in g]
        directories=enroller.directory_identities(identities_grants)
        after=receipt_after(before_receipt,args.expected_policy_sha256,args.expected_new_policy_sha256)
        expected=copy.deepcopy(instance.r);expected['configuration_sha256']['policy.json']=args.expected_new_policy_sha256
        require(after==expected,'unexpected native receipt delta')
        after_receipt=(json.dumps(after,indent=2,sort_keys=True)+'\n').encode()
        previous_enrollments=instance.root/'enrollments'
        if previous_enrollments.exists():
            enroller.protected(previous_enrollments,directory=True,mode=0o700)
            for entry in previous_enrollments.iterdir():
                enroller.protected(entry,directory=True,mode=0o700)
                record=enroller.protected(entry/'completed.json',mode=0o600)
                require(decode(record.read_bytes()).get('status')=='ROOT_CONFIGURATION_ENROLLED',
                    'prior contained enrollment incomplete; preserve and reconcile')
        parent=instance.root/'budget-adjustments';parent.mkdir(mode=0o700,exist_ok=True);enroller.protected(parent,directory=True,mode=0o700)
        for previous in parent.iterdir():
            enroller.protected(previous,directory=True,mode=0o700)
            completed=enroller.protected(previous/'completed.json',mode=0o600)
            require(decode(completed.read_bytes()).get('status')=='ROOT_UNIVERSE_BUDGET_CONFIGURED','prior budget transaction incomplete; preserve and reconcile')
        transaction=parent/args.operation_id;require(not transaction.exists(),'operation already exists; no replay/adoption');transaction.mkdir(mode=0o700)
        intent={'schema':'podmesh.root-universe-budget/v1','operation_id':args.operation_id,'scope':args.scope,
            'source_pin':args.tool_source_pin,'tool_sha256':args.expected_tool_sha256,'enroller_sha256':args.expected_enroller_sha256,
            'time':time.time(),'old_budget':args.expected_old_budget,'new_budget':args.new_budget,'retained_provider_records':count,
            'required_new_identities':args.required_new_identities,'before_policy_sha256':args.expected_policy_sha256,
            'after_policy_sha256':args.expected_new_policy_sha256,'before_receipt_sha256':args.expected_receipt_sha256,
            'provider_records':history,'directory_identities':directories,'policy_change':'only-max_universes-scalar'}
        for name,data in (('intent.json',(json.dumps(intent,sort_keys=True,indent=2)+'\n').encode()),
            ('policy.before.json',before_policy),('instance.before.json',before_receipt),('policy.after.json',after_policy),
            ('instance.after.json',after_receipt),('policy.commit',after_policy),('instance.commit',after_receipt)):
            enroller.write_new(transaction/name,data)
        preflight_original(enroller,instance,args.expected_policy_sha256,args.expected_receipt_sha256)
        require(provider_records(enroller,instance.root/'provider-state')==(count,history)
            and directories==enroller.directory_identities(identities_grants),
            'retained provider history or source mount identities changed before commit')
        enroller.replace_from(transaction/'policy.commit',policy_path)
        enroller.replace_from(transaction/'instance.commit',receipt_path)
        instance.read();enroller.check_stopped(instance)
        require(instance.r==after and provider_records(enroller,instance.root/'provider-state')==(count,history)
            and directories==enroller.directory_identities(identities_grants),
            'postcommit preservation differs; keep transaction and do not start')
        complete={**intent,'status':'ROOT_UNIVERSE_BUDGET_CONFIGURED','after_receipt_sha256':enroller.sha(receipt_path),
            'native_phase':'stopped','original_instance_read_passed':True,'sql_or_lifecycle_effects':False,'runtime_qualified':False}
        enroller.write_new(transaction/'completed.json',(json.dumps(complete,sort_keys=True,indent=2)+'\n').encode())
        print(json.dumps({'status':complete['status'],'scope':args.scope,'old_budget':args.expected_old_budget,
            'new_budget':args.new_budget,'record':str(transaction),'runtime_qualified':False}))


if __name__=='__main__':main()
