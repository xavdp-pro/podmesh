#!/usr/bin/env python3
"""Make a replica set vote and decide (V3-4, V3-5): add the `votes` section to each replica's configuration
that `generate-replica-set.py` wrote, and the scopes a replica votes and proposes in.

    add-votes.py --dir <replica set> \
      --key lab-a:<key_id>:<public key> --key lab-b:... --key lab-c:... \
      --resource <uuid>:<lease seconds>:<takeover margin seconds>:<renewal not_after> [--resource ...] \
      [--baseline <uuid>:<epoch>:<holder host uuid>:<eligible_after>] \
      [--max-certificate-life 300] [--voter-interval-ms 1000] [--operator-uid 0]

Each `--key` names one replica's key by its alias: its identifier and its public half only, as
`vote-key.py` printed it on that replica's own host (the seed never leaves the host). The policy is the
2-of-3 quorum of the three keys at serial 0, the nodes are the replica set's three hosts, the operator's
evidence directory is the host state's `/run/podmesh-host/evidence`, and every replica decides the
resources named, with the barrier rules' lease, margin and renewal bound (`renewal_not_after`: the
latest `not_after` of any follow mandate standing on the hosts, 0 when none renews by itself). A
`--baseline` records where a resource moving from the gate starts: the gate's last epoch, its holder and
its barrier. The nodes' policies must name the same quorum (`activation_require` with
`authority_quorum`, the digest printed here). Refuses a set that already votes.
"""
import argparse, hashlib, json, os, sys

p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
p.add_argument('--dir', required=True)
p.add_argument('--key', action='append', required=True, help='alias:key_id:public_key')
p.add_argument('--resource', action='append', required=True, help='uuid:lease:margin:renewal_not_after')
p.add_argument('--baseline', action='append', default=[], help='uuid:epoch:holder:eligible_after')
p.add_argument('--authority-id', default='replicas')
p.add_argument('--max-certificate-life', type=int, default=300)
p.add_argument('--voter-interval-ms', type=int, default=1000)
p.add_argument('--operator-uid', type=int, default=0)
a = p.parse_args()

with open(os.path.join(a.dir, 'replica-set.json')) as f:
    manifest = json.load(f)
aliases = {r['alias']: r for r in manifest['replicas']}
keys = {}
for spec in a.key:
    alias, key_id, public = spec.split(':')
    if alias not in aliases or alias in keys:
        sys.exit(f'add-votes: {alias} is not a replica of the set, or is named twice')
    if len(public) != 64 or any(c not in '0123456789abcdef' for c in public):
        sys.exit(f'add-votes: the public key of {alias} is not 64 lowercase hex characters')
    keys[alias] = (key_id, public)
if set(keys) != set(aliases):
    sys.exit('add-votes: name one key for each of the three replicas')
quorum = {'threshold': 2, 'keys': sorted(({'key_id': k, 'public_key': pub} for k, pub in keys.values()), key=lambda k: k['key_id'])}
policy = dict(quorum, form='podmesh-authority-quorum/1', authority_id=a.authority_id, single_key=False, serial=0)
digest = hashlib.sha256(json.dumps(policy, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
nodes = [r['host_id'] for r in manifest['replicas']]
baselines = {}
for spec in a.baseline:
    uuid, epoch, holder, eligible = spec.split(':')
    baselines[uuid] = {'epoch': int(epoch), 'holder': holder, 'eligible_after': int(eligible)}
resources = []
for spec in a.resource:
    uuid, lease, margin, renewal = spec.split(':')
    rules = {'resource': uuid, 'lease_seconds': int(lease), 'takeover_margin_seconds': int(margin), 'renewal_not_after': int(renewal)}
    if uuid in baselines:
        rules['baseline'] = baselines.pop(uuid)
    resources.append(rules)
if baselines:
    sys.exit(f'add-votes: a baseline for a resource not named: {sorted(baselines)}')
for alias, r in aliases.items():
    path = os.path.join(a.dir, alias, 'config.json')
    with open(path) as f:
        config = json.load(f)
    if 'votes' in config:
        sys.exit(f'add-votes: {path} already votes; nothing was changed')
    grants = config['network']['manager']['grants']
    for o in manifest['replicas']:
        for prefix in ('votes', 'proposals'):
            grants.append({'scope': f"{prefix}/{o['replica_id']}", 'owner_replica_id': o['replica_id']})
    config['votes'] = {
        'key_id': keys[alias][0], 'authority_id': a.authority_id, 'authority_quorum': quorum, 'authority_serial': 0,
        'replica_keys': {o['replica_id']: keys[o['alias']][0] for o in manifest['replicas']},
        'nodes': nodes, 'evidence_dir': '/run/podmesh-host/evidence', 'operator_uid': a.operator_uid,
        'max_certificate_life_seconds': a.max_certificate_life,
        'decisions': {'voter_interval_ms': a.voter_interval_ms, 'resources': resources},
    }
    with open(path + '.tmp', 'w') as f:
        json.dump(config, f, indent=2)
        f.write('\n')
    os.chmod(path + '.tmp', 0o600)
    os.replace(path + '.tmp', path)
manifest['votes'] = {'authority_id': a.authority_id, 'authority_quorum': quorum, 'policy_digest': digest, 'nodes': nodes,
                     'keys': {alias: k for alias, (k, _) in keys.items()}}
with open(os.path.join(a.dir, 'replica-set.json'), 'w') as f:
    json.dump(manifest, f, indent=2)
    f.write('\n')
print(json.dumps({'policy_digest': digest, 'authority_quorum': quorum, 'nodes': nodes}))
