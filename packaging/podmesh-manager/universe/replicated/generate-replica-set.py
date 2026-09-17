#!/usr/bin/env python3
"""Generate the configurations of one logical manager replicated as three universes on the managed
network: one logical manager UUID, three replica UUIDs bound to three host UUIDs, three owned
scopes, three pair keys (one per pair, distinct, 32 random bytes each) and explicit authenticated
endpoints -- nothing resolves a name. The output splits public topology from private material:
`replica-set.json` names the logical manager, the replicas, their hosts, addresses and scopes and
may be shared; each `<alias>/config.json` holds that replica's pair keys, stays out of Git and out
of every image, and reaches its host only as a PodMesh secret (`secret_declare`, from a root-only
inbox copy) mounted into the universe at creation -- never baked into a layer (Codex, B2).

    generate-replica-set.py --out <dir> \
      --replica lab-a:<host-uuid>:10.86.1.10 --replica lab-b:<host-uuid>:10.86.2.10 --replica lab-c:<host-uuid>:10.86.3.10

Each replica listens on every address of its universe (`bind` 0.0.0.0, port 9443): its own managed
address, and the logical manager's service address when PodMesh gives it to the active manager's replica
as an alias (docs/UNIVERSE-NETWORK-CONTRACT.md). Its peers are the two others, by explicit address.
The scopes are `m-u2/<alias>/observations`, owned by that alias's replica. The entrypoint
appends one boot fact per start in the replica's own scope; convergence is then three facts,
one per scope, on every replica.
"""
import argparse, json, os, secrets, uuid

p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
p.add_argument('--out', required=True)
p.add_argument('--replica', action='append', required=True, help='alias:host-uuid:managed-address')
p.add_argument('--port', type=int, default=9443)
p.add_argument('--incoming-workers', type=int, default=2)
a = p.parse_args()
replicas = []
for spec in a.replica:
    alias, host_uuid, address = spec.split(':')
    replicas.append({'alias': alias, 'host_id': host_uuid, 'address': address, 'replica_id': str(uuid.uuid4())})
if len(replicas) != 3:
    raise SystemExit('exactly three replicas')
logical = str(uuid.uuid4())
keys = {}
for i in range(3):
    for j in range(i + 1, 3):
        keys[(i, j)] = secrets.token_hex(32)
os.makedirs(a.out, mode=0o700, exist_ok=True)
manifest = {'logical_manager_id': logical, 'replicas': [{'alias': r['alias'], 'replica_id': r['replica_id'], 'host_id': r['host_id'], 'address': r['address']} for r in replicas],
            'scopes': {r['alias']: f"m-u2/{r['alias']}/observations" for r in replicas}}
for i, r in enumerate(replicas):
    peers = []
    for j, o in enumerate(replicas):
        if j == i:
            continue
        peers.append({'replica_id': o['replica_id'], 'endpoint': f"{o['address']}:{a.port}", 'shared_key_hex': keys[(min(i, j), max(i, j))]})
    config = {
        'network': {
            'replica_id': r['replica_id'],
            'database_path': '/var/lib/podmesh-manager/manager.sqlite',
            'manager': {
                'logical_manager_id': logical,
                'replicas': [{'replica_id': o['replica_id'], 'host_id': o['host_id']} for o in replicas],
                'grants': [{'scope': f"m-u2/{o['alias']}/observations", 'owner_replica_id': o['replica_id']} for o in replicas],
            },
            'bind': f'0.0.0.0:{a.port}',
            'peers': peers,
        },
        'control_socket': '/run/podmesh-manager/control.sock',
        'observation_writer_uid': 0,
        'interval_ms': 1000,
        'max_backoff_ms': 30000,
        'incoming_workers': a.incoming_workers,
    }
    d = os.path.join(a.out, r['alias']); os.makedirs(d, mode=0o700, exist_ok=True)
    with open(os.path.join(d, 'config.json'), 'w') as f:
        json.dump(config, f, indent=2); f.write('\n')
    os.chmod(os.path.join(d, 'config.json'), 0o600)
with open(os.path.join(a.out, 'replica-set.json'), 'w') as f:
    json.dump(manifest, f, indent=2); f.write('\n')
print(json.dumps({'logical_manager_id': logical, 'replicas': [(r['alias'], r['address']) for r in replicas], 'out': a.out}))
