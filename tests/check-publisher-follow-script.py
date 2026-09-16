#!/usr/bin/env python3
"""The follow tick refuses without a mandate and does not invent a proof (docs/PUBLISHER-FOLLOW-LAB.md)."""
import os, pathlib, subprocess, tempfile

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'packaging', 'podmesh-publisher-follow')
checks = []

def run(env, expect):
    p = subprocess.run(['python3', '-B', SCRIPT], env={**os.environ, **env}, capture_output=True, text=True)
    assert p.returncode == expect, (p.returncode, p.stdout, p.stderr)
    return p

p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': '/no/such/mandate'}, 3)
assert 'no mandate' in p.stderr
checks.append('refuses without a mandate')

td = tempfile.mkdtemp(prefix='podmesh-follow-')
bad = pathlib.Path(td) / 'mandate'
bad.write_text('authorization_ref=ok\nresource=not-a-uuid\nproof=/tmp/x\n')
p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': str(bad)}, 3)
assert 'resource UUID' in p.stderr
checks.append('refuses a mandate that names no resource UUID')

bad.write_text('authorization_ref=ok\nresource=91eeb6bf-5489-405b-b77a-53105b0aff7a\nproof=/tmp/x\nrenew=yes\n')
p = run({'PODMESH_PUBLISHER_FOLLOW_MANDATE': str(bad)}, 3)
assert "renew must be 0 or 1" in p.stderr
checks.append('refuses a mandate whose renew is not 0 or 1')

print('PASS')
for c in checks:
    print('-', c)
