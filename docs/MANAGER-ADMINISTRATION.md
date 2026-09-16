# The manager's administrators

Status: **built and run on the three laboratory hosts, 2026-09-16**. Owner: Xavier de Poorter.
What is built is stated at the end; nothing above it is a claim about code.

The manager universe answers on its origin, and a publishing connector carries that origin to a
public hostname while this replica is the governor (`MANAGER-PUBLISHER-CONTRACT.md`). This
document says who may administer it, where an administrator comes from, and what the surface
refuses.

## An administrator is a replicated fact

Subject `admin.user.<login>` in the writing replica's own granted scope, value
`scrypt.<n>.<r>.<p>.<salt hex>.<hash hex>`. A companion subject `admin.flag.<login>` carries
`must_change` while the account still holds the password a deployment gave it. The password
itself is never stored, never journaled, never replicated and never written to a command line.

Each replica may write only in the scope it owns, so an administrator created on the governor
lives in the governor's scope and reaches the others by replication, read-only. A login written
in two scopes is a **conflict the page shows**; nothing picks a winner behind the operator's
back. Inside one scope the current state of a login is its highest revision, and a revocation is
simply the revision that says `revoked`.

## Where the first one comes from

**Not from the page.** A page reachable from the Internet that can mint authority is a front
door to power. The first administrator is written from the host that carries a replica, as root,
through PodMesh's typed control door (`manager_observe`), by `tools/manager-admin.py`.

**A deployment creates it, and it is not a default account.** `tools/manager-admin.py bootstrap`
runs as part of deploying the manager (`tools/arm-publisher-follow.py` calls it): a manager that
is up has an administrator from its first minute, so nobody has to remember to make one. It
draws a fresh random password from the operating system, prints it **once** on that root-only
channel, and marks the account `must_change`. A default password is a published password; there
is none here, and two deployments never share one. Running bootstrap again on a manager that
already has an administrator does nothing.

## What the surface refuses

- Every path, administration included, answers **503 without the governor mark** — the same
  fail-closed rule as `/ready`. A replica that is not the governor administers nothing.
- With **no administrator on record**, the page says so and names the host door. It does not
  offer to create one.
- **Creating an administrator requires a session**; an administrator is named by an
  administrator.
- An account that still carries its **deployment password** opens only the page that replaces
  it, and may name nobody until it does.
- A login is 1 to 64 characters of `a-z 0-9 . - _`; a password is at least 12 characters, is not
  the login, and on a change is not the previous one. A login already taken is refused.
- Five failed sign-ins for one login close it for a minute.
- The session cookie is `HttpOnly`, `Secure`, `SameSite=Strict`, and the session lives one hour
  in the responder's memory only: a restarted replica signs everyone out.

## What this does not decide

The manager's own web application beyond administration. Roles or permissions beyond
"administrator". Password recovery — there is none: an administrator who loses a password is
revoked and recreated through the root door. Durability: a replica keeps its store in its
container, with **no volume**, so a replica deleted and recreated starts empty and re-imports
from its peers; recreating all replicas at once would lose every fact, administrators included.
That is the managed-network universe contract's open item (`docs/README.md`, P09), not a
property of this surface.

## What is built

`packaging/podmesh-manager/universe/origin.py` in the web tree (the origin responder, which now
carries `/admin`), `tools/manager-admin.py` (bootstrap, create, list, revoke through the control
door), and the bootstrap call in `tools/arm-publisher-follow.py`.

`tests/check-manager-admin-origin.py` in the web tree runs the responder against a stub resident
and a stub control socket and exercises fifteen gates: everything 503 without the mark; `/ready`
unchanged; the no-administrator refusal and that a creation without a session appends nothing;
the sign-in page naming nobody; a wrong password refused; the cookie's three flags; the
administration page; a bad login, a short password, a password equal to its login and a login
already taken, each refused with nothing appended; the created administrator written as one
observation in the replica's own scope with the password absent from it; the deployment account
opening only the change page and naming nobody; a wrong current password, two differing entries,
a short one and the old one again, each refused; the change writing the new hash and clearing
the flag; a login in two scopes shown as a conflict; signing out; and the mark removed closing
everything again.

On the laboratory, 2026-09-16: the image rebuilt on the three hosts, the three replicas
recreated, the role held by lab-a at epoch 149, the deployment administrator created through the
root door, and a sign-in through the **public hostname** answering 303 with a session cookie and
then the forced change page. The replicas were recreated one host at a time and the fact set
survived through replication, which is why the administrator created before the rebuild was
still there afterwards.
