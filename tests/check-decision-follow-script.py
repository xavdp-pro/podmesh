#!/usr/bin/env python3
"""The decision follow tick (packaging/podmesh-decision-follow, V3-5), against a stubbed node and a stubbed
resident that answer as the real ones would and record every request. It refuses without a mandate and
with a malformed one; a certificate naming this host is acquired, and not again once held; one naming
another host is delivered as a supersession above the screen only; a certificate below the screen, a
decision that cannot be read, a conflict, a node that cannot be read and a policy that names no quorum
deliver nothing and the tick exits 3; a refusal of the node is reported and the tick carries on; the
publisher is started with the certificate as its takeover proof only when declared, eligible and idle;
the door (manager_decision) is used when the mandate names the manager universe; and the script opens no
listener. Purely local: no daemon, no host. The same tick against real residents and a real node is the
web tree's end-to-end test. Run: python3 -B tests/check-decision-follow-script.py"""
import json, os, shutil, socket, subprocess, sys, tempfile, threading, time

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'packaging', 'podmesh-decision-follow')
R = '91eeb6bf-5489-405b-b77a-53105b0aff7a'
R2 = '0a6a0a36-5c4b-4a3f-9f3e-7d9c1b2a3e4f'
HERE = '0a0a0a0a-0000-4000-8000-000000000000'
THERE = '1b1b1b1b-1111-4111-8111-111111111111'
UNIVERSE = '5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10'
checks = []


class Stub:
    """A Unix socket that answers each request with `answer(request)` and records it. The node's protocol
    is one JSON line per connection; the resident's is one JSON document read to the end of the stream."""

    def __init__(self, path, answer, line):
        self.path, self.answer, self.line, self.calls = path, answer, line, []
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.bind(path)
        self.sock.listen(8)
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            with conn:
                data = b''
                while True:
                    chunk = conn.recv(65536)
                    if not chunk:
                        break
                    data += chunk
                    if self.line and data.endswith(b'\n'):
                        break
                request = json.loads(data)
                self.calls.append(request)
                reply = self.answer(request)
                conn.sendall(reply if isinstance(reply, bytes) else (json.dumps(reply) + ('\n' if self.line else '')).encode())

    def close(self):
        self.sock.close()


def certificate(epoch, holder, previous=HERE, method='same_holder', resource=R):
    return {'kind': 'podmesh-takeover-proof/quorum-ed25519', 'authority_id': 'replicas', 'policy_digest': 'd' * 64,
            'resource': resource, 'new_epoch': epoch, 'previous_epoch': epoch - 1, 'new_holder': holder,
            'previous_holder': previous, 'holder_boot_id': 'boot', 'grant_id': f'g{epoch}', 'method': method,
            'eligible_after': 1, 'issued_at': 1, 'expires_at': 2,
            'signatures': [{'key_id': 'replica-a', 'signature': 'a' * 128}, {'key_id': 'replica-b', 'signature': 'b' * 128}]}


def decision(cert, resource=R, conflicts=()):
    return {'decision': {'resource': resource, 'view': {'epoch': cert['new_epoch'] if cert else 0},
                         'current': {'epoch': cert['new_epoch'], 'certificate': cert} if cert else None,
                         'pending': [], 'conflicts': list(conflicts)}}


class World:
    """What the stubs answer: the node's activation state per resource, its publisher, the replica's decision."""

    def __init__(self, td):
        self.status = {R: self.activation()}
        self.decisions = {R: decision(certificate(3, HERE))}
        self.publisher = {'declared': False}
        self.refuse = {}
        self.broken_node = False
        self.node = Stub(os.path.join(td, 'node.sock'), self.answer_node, True)
        self.resident = Stub(os.path.join(td, 'resident.sock'), self.answer_resident, False)

    @staticmethod
    def activation(screen=2, holder=HERE, epoch=2, live=True, quorum=True):
        return {'this_host_uuid': HERE, 'highest_epoch_seen': screen, 'holder_host_uuid': holder, 'epoch': epoch,
                'live': live, 'superseded': False, 'generation': 1, 'expires_at': 100,
                'authority_quorum': {'threshold': 2, 'keys': []} if quorum else None}

    def answer_node(self, request):
        op = request['operation']
        if self.broken_node:
            return b'not json\n'
        if op in self.refuse:
            return {'ok': False, 'error': self.refuse[op]}
        if op == 'activation_status':
            return {'ok': True, 'data': self.status[request['universe_uuid']]}
        if op == 'manager_decision':
            return {'ok': True, 'data': {'resident_reply': self.decisions[request['resource']]}}
        if op == 'publisher_status':
            return {'ok': True, 'data': self.publisher}
        return {'ok': True, 'data': {}}

    def answer_resident(self, request):
        assert request == {'operation': 'decision_read', 'resource': request['resource']}, request
        return self.decisions[request['resource']]

    def calls(self):
        return [c['operation'] for c in self.node.calls]

    def reset(self):
        self.node.calls.clear()
        self.resident.calls.clear()


def tick(td, mandate, expect):
    path = os.path.join(td, 'mandate')
    with open(path, 'w') as f:
        f.write(mandate)
    p = subprocess.run(['python3', '-B', SCRIPT], capture_output=True, text=True,
                       env={**os.environ, 'PODMESH_DECISION_FOLLOW_MANDATE': path, 'PODMESH_SOCKET': os.path.join(td, 'node.sock')})
    assert p.returncode == expect, (p.returncode, p.stdout, p.stderr)
    return [json.loads(line) for line in p.stdout.splitlines()], p.stderr


def main():
    td = tempfile.mkdtemp(prefix='podmesh-decision-follow-', dir=os.environ.get('TMPDIR'))
    w = World(td)
    socket_mandate = f'authorization_ref=test\nresident_socket={td}/resident.sock\nresource={R}\n'

    # The mandate: absent, malformed, ambiguous.
    p = subprocess.run(['python3', '-B', SCRIPT], capture_output=True, text=True,
                       env={**os.environ, 'PODMESH_DECISION_FOLLOW_MANDATE': '/no/such/mandate', 'PODMESH_SOCKET': w.node.path})
    assert p.returncode == 3 and 'no mandate' in p.stderr, p
    for bad, why in [(f'authorization_ref=test\nresident_socket=/x\n', 'resource'),
                     (f'authorization_ref=test\nresource=not-a-uuid\nresident_socket=/x\n', 'resource'),
                     (f'authorization_ref=test\nresource={R}\n', 'exactly one'),
                     (f'authorization_ref=test\nresource={R}\nresident_socket=/x\nmanager_universe={UNIVERSE}\n', 'exactly one'),
                     (f'authorization_ref=te st\nresource={R}\nresident_socket=/x\n', 'authorization_ref'),
                     (f'authorization_ref=test\nresource={R}\nresident_socket=/x\npublisher=yes\n', 'publisher'),
                     (f'authorization_ref=test\nresource={R}\nresident_socket=/x\nrenew=1\n', 'unknown keys')]:
        _, err = tick(td, bad, 3)
        assert why in err, (bad, err)
    assert w.calls() == [], w.calls()
    checks.append('refuses without a mandate, and with one that names no resource, both or neither source, a bad reference, a bad publisher flag or an unknown key; nothing is asked')

    # A certificate naming this host, above the screen: acquired with it, under a deterministic ID.
    out, _ = tick(td, socket_mandate, 0)
    acquire = [c for c in w.node.calls if c['operation'] == 'activation_acquire']
    assert len(acquire) == 1 and acquire[0]['certificate'] == w.decisions[R]['decision']['current']['certificate'], w.node.calls
    assert acquire[0]['universe_uuid'] == R and acquire[0]['authorization_ref'] == 'test'
    assert acquire[0]['operation_id'].startswith('decision-acquire-3-') and len(acquire[0]['operation_id']) <= 80
    assert out[0]['action'] == 'activation_acquire' and out[0]['result'] == 'delivered', out
    first_id = acquire[0]['operation_id']
    w.reset()
    tick(td, socket_mandate, 0)
    assert [c['operation_id'] for c in w.node.calls if c['operation'] == 'activation_acquire'] == [first_id]
    checks.append('a certificate naming this host above its screen is delivered to activation_acquire, the same request ID when the situation is the same')

    # Held at that epoch: nothing more.
    w.reset()
    w.status[R] = w.activation(screen=3, epoch=3)
    out, _ = tick(td, socket_mandate, 0)
    assert w.calls() == ['activation_status'], w.calls()
    assert out[0]['action'].startswith('none: this host holds'), out
    checks.append('a lease held here at the certificate\'s epoch is left alone: a repeated tick delivers nothing twice')

    # A lapsed lease at the epoch, the certificate still decided: acquired again, under another request ID.
    w.reset()
    w.status[R] = dict(w.activation(screen=3, epoch=3, live=False), expires_at=200)
    tick(td, socket_mandate, 0)
    again = [c for c in w.node.calls if c['operation'] == 'activation_acquire']
    assert len(again) == 1 and again[0]['operation_id'] != first_id, w.node.calls
    checks.append('a lease of this host lapsed at the epoch is acquired again with the certificate, under a new request ID')

    # A certificate below the screen: nothing.
    w.reset()
    w.status[R] = w.activation(screen=5, holder=THERE, epoch=5)
    out, _ = tick(td, socket_mandate, 0)
    assert w.calls() == ['activation_status'] and 'below' in out[0]['action'], (w.calls(), out)
    checks.append('a certificate below the node\'s screen is not delivered')

    # A certificate naming another host: a supersession above the screen, nothing at or below it.
    w.reset()
    w.status[R] = w.activation(screen=3, epoch=3)
    w.decisions[R] = decision(certificate(4, THERE, method='lease_barrier'))
    out, _ = tick(td, socket_mandate, 0)
    sup = [c for c in w.node.calls if c['operation'] == 'activation_supersede']
    assert len(sup) == 1 and sup[0]['certificate']['new_holder'] == THERE and 'activation_acquire' not in w.calls(), w.node.calls
    assert sup[0]['operation_id'].startswith('decision-supersede-4-'), sup
    w.reset()
    w.status[R] = w.activation(screen=4, epoch=3)
    out, _ = tick(td, socket_mandate, 0)
    assert w.calls() == ['activation_status'] and 'seen the epoch' in out[0]['action'], (w.calls(), out)
    checks.append('a certificate naming another host is delivered to activation_supersede above the screen, and not at it')

    # A refusal of the node: reported, the tick carries on.
    w.reset()
    w.status[R] = w.activation(screen=3, epoch=3)
    w.decisions[R] = decision(certificate(4, HERE))
    w.refuse['activation_acquire'] = "The certificate's barrier is at 99, 9 seconds from now on this clock; refusing before it"
    out, _ = tick(td, socket_mandate, 0)
    assert out[0]['result'] == 'refused' and 'barrier' in out[0]['error'], out
    del w.refuse['activation_acquire']
    checks.append('a refusal of the node (a barrier not reached) is reported, and retried at the next tick')

    # Fail closed: an unreadable decision, a conflict, a decision for another resource, an unreadable node,
    # a policy without a quorum, a certificate without a holder: nothing is delivered, exit 3.
    cases = [
        ('the replica answers an error', lambda: w.decisions.__setitem__(R, {'error': 'decision_refused', 'code': 'vote_store_unreadable'})),
        ('the replica reads a conflict', lambda: w.decisions.__setitem__(R, decision(certificate(4, HERE), conflicts=[4]))),
        ('the replica answers for another resource', lambda: w.decisions.__setitem__(R, decision(certificate(4, HERE, resource=R2), resource=R2))),
        ('the certificate names no holder', lambda: w.decisions.__setitem__(R, decision({**certificate(4, HERE), 'new_holder': None}))),
        ('the node is not readable', lambda: setattr(w, 'broken_node', True)),
        ('the policy names no quorum', lambda: w.status.__setitem__(R, w.activation(quorum=False))),
        ('activation_status is refused', lambda: w.refuse.__setitem__('activation_status', 'no such universe')),
    ]
    for why, arrange in cases:
        w.reset()
        w.status[R] = w.activation(screen=3, epoch=3)
        w.decisions[R] = decision(certificate(4, HERE))
        w.broken_node = False
        w.refuse.clear()
        arrange()
        out, err = tick(td, socket_mandate, 3)
        assert not {'activation_acquire', 'activation_supersede', 'publisher_start'} & set(w.calls()), (why, w.calls())
        assert out[0].get('unreadable') and 'nothing delivered' in err, (why, out, err)
    w.broken_node = False
    w.refuse.clear()
    # The resident's socket absent altogether.
    out, err = tick(td, f'authorization_ref=test\nresident_socket={td}/absent.sock\nresource={R}\n', 3)
    assert out[0].get('unreadable'), out
    checks.append('fails closed, delivering nothing and exiting 3: a replica that answers an error, reads a conflict, answers for another resource or is absent, a certificate without a holder, a node that cannot be read, a policy without a quorum')

    # Two resources: one unreadable does not stop the other.
    w.reset()
    w.status[R2] = w.activation(screen=0, holder=None, epoch=None)
    w.decisions[R2] = decision(certificate(1, THERE, previous=None, method='first', resource=R2), resource=R2)
    w.status[R] = w.activation(screen=3, epoch=3)
    w.decisions[R] = {'error': 'decision_refused'}
    out, _ = tick(td, f'authorization_ref=test\nresident_socket={td}/resident.sock\nresource={R}\nresource={R2}\n', 3)
    assert [c['universe_uuid'] for c in w.node.calls if c['operation'] == 'activation_supersede'] == [R2], w.node.calls
    checks.append('each resource is followed on its own: one that cannot be read does not stop another\'s delivery')

    # The publisher: started with the certificate as the takeover proof when declared, eligible and idle.
    pub_mandate = socket_mandate + 'publisher=1\n'
    w.decisions[R] = decision(certificate(4, HERE))
    w.status[R] = w.activation(screen=4, epoch=4)
    for publisher, started in [({'declared': False, 'publisher_eligible': False}, False),
                               ({'declared': {}, 'publisher_eligible': True, 'unit': {'state': 'active'}}, False),
                               ({'declared': {}, 'publisher_eligible': True, 'unit': {'state': 'inactive'}, 'transition': {'state': 'starting'}}, False),
                               ({'declared': {}, 'publisher_eligible': False, 'unit': {'state': 'inactive'}, 'reasons': ['no effective exclusive route']}, False),
                               ({'declared': {}, 'publisher_eligible': True, 'unit': {'state': 'inactive'},
                                 'lease': {'generation': 1, 'acquired_at': 1789700000}}, True)]:
        w.reset()
        w.publisher = publisher
        out, _ = tick(td, pub_mandate, 0)
        starts = [c for c in w.node.calls if c['operation'] == 'publisher_start']
        assert len(starts) == int(started), (publisher, w.node.calls)
        if started:
            assert starts[0]['takeover_proof'] == w.decisions[R]['decision']['current']['certificate'] and starts[0]['resource'] == R
    # Two ticks while the connector is not yet visible (no active unit, no transition): one operation,
    # keyed on the certificate and the lease, never on the clock (review of V3-5, finding 5).
    w.reset()
    w.publisher = {'declared': {}, 'publisher_eligible': True, 'unit': {'state': 'inactive'},
                   'lease': {'generation': 3, 'acquired_at': 1789800000}}
    tick(td, pub_mandate, 0)
    time.sleep(1.1)
    tick(td, pub_mandate, 0)
    ids = [c['operation_id'] for c in w.node.calls if c['operation'] == 'publisher_start']
    assert len(ids) == 2 and len(set(ids)) == 1 and ids[0].endswith('-3-1789800000'), ids
    w.reset()
    w.publisher['lease'] = {'generation': 4, 'acquired_at': 1789800900}
    tick(td, pub_mandate, 0)
    assert [c['operation_id'] for c in w.node.calls if c['operation'] == 'publisher_start'] != ids[:1], w.node.calls
    w.reset()
    w.publisher = {'declared': {}, 'publisher_eligible': True, 'unit': {'state': 'inactive'}}
    tick(td, socket_mandate, 0)
    assert 'publisher_status' not in w.calls(), w.calls()
    checks.append('publisher_start carries the certificate as its takeover proof only when the mandate says so, a publisher is declared, eligible, and nothing runs or is recorded; two ticks while the connector is not yet visible send one operation ID, another acquisition of the lease another one')

    # Through the door: manager_decision on the node, naming the manager universe and the resource.
    w.reset()
    w.status[R] = w.activation(screen=3, epoch=3)
    w.decisions[R] = decision(certificate(4, THERE, method='lease_barrier'))
    tick(td, f'authorization_ref=test\nmanager_universe={UNIVERSE}\nresource={R}\n', 0)
    door = [c for c in w.node.calls if c['operation'] == 'manager_decision']
    assert door == [{'operation': 'manager_decision', 'universe_uuid': UNIVERSE, 'resource': R, 'authorization_ref': 'test'}], door
    assert w.resident.calls == [] and 'activation_supersede' in w.calls()
    checks.append('with manager_universe the decision is read through the node\'s door (manager_decision), and the resident\'s socket is not touched')

    # No listener: the script connects and never binds, listens or accepts.
    text = open(SCRIPT).read()
    for word in ('.bind(', '.listen(', '.accept(', 'socketserver', 'http.server', 'AF_INET'):
        assert word not in text, word
    checks.append('the script opens no listener: it binds, listens and accepts nothing, and speaks no network family')

    w.node.close()
    w.resident.close()
    shutil.rmtree(td, ignore_errors=True)
    for c in checks:
        print('PASS', c)
    print(f'{len(checks)} checks passed')


if __name__ == '__main__':
    main()
