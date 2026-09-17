# The manager's administrators

Status: **built and run on the three laboratory hosts, 2026-09-16**. Owner: Xavier de Poorter.
What is built is stated at the end; nothing above it is a claim about code.

The manager universe answers on its origin, and a publishing connector carries that origin to a
public hostname while this replica is the active manager (`MANAGER-PUBLISHER-CONTRACT.md`). This
document says who may administer it, where an administrator comes from, and what the surface
refuses.

## An administrator is a replicated fact

Subject `admin.user.<login>` in the writing replica's own granted scope, value
`scrypt.<n>.<r>.<p>.<salt hex>.<hash hex>`. A companion subject `admin.flag.<login>` carries
`must_change` while the account still holds the password a deployment gave it. The password
itself is never stored, never journaled, never replicated and never written to a command line.

Each replica may write only in the scope it owns, so an administrator created on the active manager
lives in the active manager's scope and reaches the others by replication, read-only. A login
written in two scopes is a **conflict the page shows**; nothing picks a winner behind the operator's
back. Inside one scope the current state of a login is its highest revision, and a revocation is
simply the revision that says `revoked`.

## Where the first one comes from

**Not from the page.** A page reachable from the Internet that can mint authority is a front
door to power. The first administrator is written from the host that carries a replica, as root,
through PodMesh's typed control door (`manager_observe`), by `tools/manager-admin.py`.

**A deployment creates it, with a default password changed at the first sign-in.**
`tools/manager-admin.py bootstrap` runs as part of deploying the manager
(`tools/arm-publisher-follow.py` calls it): a manager that is up has an administrator from its
first minute, so nobody has to remember to make one. The operator decided on 2026-09-16 that the
password is a simple default — `admin` / `podmesh` — replaced at the first sign-in, rather than a
random one printed once. The account is marked `must_change`, so the only page it opens is the
one that replaces it and it may name nobody until it does.

The consequence is stated rather than hidden, because a default password is known in advance:
between the deployment and that first sign-in, whoever reaches the page can take the account.
The forced change is what closes that window, so a deployment and its first sign-in belong in the
same breath. `--password`, or `PODMESH_DEFAULT_ADMIN_PASSWORD`, gives a deployment its own
instead. Running bootstrap again on a manager that already has an administrator does nothing.

## What the surface refuses

- Every path, administration included, answers **503 without the active manager's mark** — the same
  fail-closed rule as `/ready`. A replica that is not the active manager administers nothing.
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
That is the managed-network universe contract's open item (product requirement P09), not a
property of this surface.

## What is built

**The operator's stack, 2026-09-16.** The origin is `packaging/podmesh-manager/universe/origin` in
the web tree: an Express server (`server/`, with helmet, express-rate-limit and jsonwebtoken) that
answers `/ready`, the human page and the administration API under `/admin/api`, and serves the
administration app built by Vite from `src/` — React 19, Tailwind 4, lucide-react, framer-motion,
react-router-dom, zustand, axios, react-hot-toast, trilingual (fr, en, es). The same stack as the
operator's other applications, with three differences chosen on purpose: the session is a JWT in an
HttpOnly, Secure, SameSite=Strict cookie, never in browser storage; helmet's content security policy
is on (`script-src 'self'`, `form-action 'none'`); and the production files are served by the origin
itself, not by a development server. Passwords stay scrypt through node's own crypto: the store
already holds that format, and a bcrypt string does not fit the resident's token alphabet. The
image adds Alpine's nodejs; `build-origin.sh` stages the built files, the server and its production
dependencies from the lockfile, so the image build needs no network.

The four web rules of `INTENT.md` are built in: every action is a JSON call from script and the
API refuses anything else (415 for a form body, 405 for a post outside the API); refusals and
confirmations are the page's own notices, a modal (`ConfirmModal`) for a revocation, never a browser
dialog; every list is the styled `Select`, searchable with a clearing cross past four entries; the
password fields carry the eye.

`tests/app.test.mjs` (vitest, supertest) exercises twenty-one gates against stubs: everything 503
without the mark, `/ready` unchanged, the policy, the form refusals, the no-administrator state, sign-in
and its cookie, the lockout after five failures, a forged or revoked session, sign-out, the rules of
creation, the observation written with the password absent, the two-scope conflict, revocation (never
the signed-in account, never the last one), and the deployment password's forced change.
`tests/check-manager-admin-browser.py` drives the built app in a headless browser: rendered from the
API, a wrong password refused in place with the fields kept, the eye, Enter signing in, the change
view forced, forms without method or action, a session ended mid-way sent back to sign-in with the
reason, no navigation, every post a JSON call, and words when JavaScript is off.
`web/tests/ui-rules.test.mjs` fails when any web source of the manager posts a form, opens a browser
dialog or draws a system select.

On the laboratory, 2026-09-16: the image rebuilt on the three hosts, the three replicas recreated
one host at a time (the fact set, administrators included, surviving through replication), the role
held by lab-a, the deployment administrator created through the root door, and the app driven
from the public hostname by the same browser check.
