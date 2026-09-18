"""What the manager suites share: the generic universe image, and a replica's configuration
given to its universe as a PodMesh secret rather than baked into an image (Codex's finding B2).

The private configurations live in PODMESH_REPLICA_CONFIGS/<alias>/config.json on the workstation
(the replica set's generator writes them; they hold the pair keys and never enter Git or an image).
A suite hands one to a host over root SSH into the state directory's inbox, root-only, and asks
`secret_declare`; the daemon moves it into Podman's store and removes the inbox copy. The universe
is then created from the generic image with the secret mounted at the resident's configuration
path. `remove_replica_config` takes the secret out of Podman's store once the universe is gone.
"""
import json, os

GENERIC_TAG = 'localhost/podmesh-manager-universe:m-u2-generic'
CONFIG_TARGET = '/etc/podmesh-manager/config.json'
ENTRYPOINT = ['/usr/local/bin/manager-universe']


def configs_dir():
    return os.environ['PODMESH_REPLICA_CONFIGS']


def secret_name(alias):
    return f'manager-config-{alias}'


def generic_image(h):
    listing = h.call('podman_run', args=['images', '--no-trunc', '--format', '{{.ID}} {{.Repository}}:{{.Tag}}'])['stdout'].splitlines()
    return next(l.split()[0] for l in listing if l.endswith(' ' + GENERIC_TAG))


def declare_replica_config(h, alias, reference, state_dir):
    """The alias's private configuration into the host's inbox (root, 0600), then declared."""
    import uuid
    name = secret_name(alias)
    content = open(os.path.join(configs_dir(), alias, 'config.json'), 'rb').read()
    h.ssh(f'sudo -n mkdir -p -m 0700 {state_dir}/inbox/secrets && sudo -n install -m 0600 -o root -g root /dev/stdin {state_dir}/inbox/secrets/{name}', input_bytes=content)
    r = h.api({'operation': 'secret_declare', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference, 'name': name, 'source': name})
    assert r.get('ok'), (h.role, 'secret_declare', r)
    return r['data']


def remove_replica_config(h, alias, reference):
    import uuid
    return h.api({'operation': 'secret_remove', 'operation_id': str(uuid.uuid4()), 'authorization_ref': reference, 'name': secret_name(alias)})


def secrets_for(alias):
    return [{'name': secret_name(alias), 'target': CONFIG_TARGET}]


def replica_create(h, u, alias, reference, address, request):
    """The create request of a replica universe: generic image, entrypoint, managed profile at its
    declared address, the configuration as a secret."""
    return h.ok(request('create', u, reference, image=generic_image(h), command=ENTRYPOINT, network_profile='managed',
                        network_address=address, secrets=secrets_for(alias)))


def prove_takeover(rot, hosts, tool, request, reference, resource):
    """The takeover proof an exclusive publication needs, from the tool's rotation: as issued when
    no holder or the same holder came before; upgraded with the previous holder's fence when that
    host is in the set (its supersession delivered, its fence run, the receipt attested); else
    waited for, up to the authority's barrier, on this clock. Whatever the method, the proof is returned
    only once its `eligible_after` is reached on this clock: a rotation carries the barrier of the epoch
    before it into same-holder and fence-receipt documents too, and a node older than 2026-09-18 holds
    only a lease barrier to it (third review of V3-1). Returns the proof and what was done."""
    import json, os, subprocess, tempfile, time, uuid

    def at_barrier(proof, how):
        waited = max(0, int(proof.get('eligible_after') or 0) - int(time.time()))
        while time.time() < (proof.get('eligible_after') or 0):
            time.sleep(1)
        return proof, how + (f', then waited its barrier {waited} s' if waited else '')
    proof = rot['takeover_proof']
    if proof['method'] in ('first', 'same_holder'):
        return at_barrier(proof, proof['method'])
    previous = proof.get('previous_holder')
    holder = next((h for h in hosts.values() if h.identity == previous), None)
    if holder is not None:
        holder.ok(request('activation_supersede', resource, reference, permit=rot['permit']))
        opid = str(uuid.uuid4())
        fence = holder.ok({'operation': 'activation_fence', 'operation_id': opid, 'authorization_ref': reference, 'timeout_seconds': 10})
        with tempfile.NamedTemporaryFile('w', suffix='.json', delete=False, prefix='podmesh-receipt-') as f:
            json.dump({'host': holder.identity, 'operation_id': opid, 'fence': fence}, f)
        attested = tool('attest-fence', '--universe', resource, '--receipt', f.name)
        os.unlink(f.name)
        return at_barrier(attested['takeover_proof'], f'fenced {holder.role}')
    return at_barrier(proof, 'waited the barrier')
