# Counter-review — PodMesh web console

Source-only review. No tools run, no tests executed. Findings are ordered by how much authority they leak.

---

## Blockers

### B1 — `createApp` → `POST /api/hosts/:id/actions` forwards the request body verbatim
The handler validates five fields and then does `const result=await call(host,p)` with `p === req.body`. Every other key survives: `{operation:'stop', …, on_timeout:'kill', timeout_seconds:0}`, `{operation:'create', …, privileged:true, network:'host', mounts:[…]}`, `{operation:'delete', …, purge_volumes:true}`. The capability precheck only tests `p.operation`, so it does not constrain the extra keys at all. The remote side runs as root (`sudo -n python3`), so this is the widest authority path in the system — and the UI's own promise ("Waits 10 seconds after the stop signal. No forced kill escalation.") is unenforceable.

Fix: build the forwarded object rather than passing the body through.

```js
const fields={
 create:['operation','universe_uuid','operation_id','authorization_ref','image','command'],
 clone :['operation','universe_uuid','source_uuid','operation_id','authorization_ref'],
 start :['operation','universe_uuid','operation_id','authorization_ref'],
 delete:['operation','universe_uuid','operation_id','authorization_ref'],
 stop  :['operation','universe_uuid','operation_id','authorization_ref','timeout_seconds','on_timeout'],
};
const allow=fields[p.operation];
if(Object.keys(req.body).some(k=>!allow.includes(k)))return res.status(400).json({error:'Unexpected field'});
const call_body=Object.fromEntries(allow.filter(k=>k in req.body).map(k=>[k,req.body[k]]));
```
Then type-check the ones currently unchecked server-side: `image` against `/^sha256:[a-f0-9]{64}$/`, `command` as an array of ≤64 strings, `on_timeout==='leave_running'`, `timeout_seconds` an integer in 1..300. Right now `image` and `command` reach the runtime with zero gateway validation — the client-side `JSON.parse` check in `submit` is not a control.

### B2 — `createApp` host validation permits a "remote" host that silently targets the local socket
The config loop checks `h.id` and `h.ssh` but never requires either `ssh` or `socket`. In `request`, a host with neither falls to `host.socket||'/run/podmesh/api.sock'` — the **local** API. A typo'd or dropped `ssh` field produces a card labelled with the remote host's `name`, showing local containers, and `delete` executes locally while the modal says `Host: <remote name>`. That is target confusion on a destructive path.

Fix, in the same validation loop:
```js
if(!!h.ssh===!!h.socket)throw Error('Each host needs exactly one of ssh or socket');
```
Also validate `h.name` is a non-empty string and unique (it is used as the React key in `feed` and as the only operator-visible target identifier).

### B3 — `POST /api/hosts/:id/actions` catch block leaves the snapshot cache intact after an unknown outcome
`cached=null` is only on the success path. On the `catch` — i.e. transport timeout, SSH failure, truncated response — the cache survives, so the `await refresh()` in `submit` returns pre-action inventory for up to 10s. The operator is told "Outcome unknown — reconcile before retry" and is simultaneously shown evidence that nothing changed. The unknown-outcome case is exactly the one where stale evidence is dangerous.

Fix: invalidate unconditionally.
```js
try{ … const result=await call(host,call_body);res.json(result);}
catch(e){res.status(502).json({error:e.message,operation_id:p.operation_id,outcome:'unknown'});}
finally{cached=null;}
```

### B4 — `GET /api/snapshot` in-flight collection overwrites the invalidation
Even with B3 fixed, `loading ??= Promise.all(...).then(rows=>cached={…})` will write a pre-action snapshot into `cached` if a collection was already running when the action completed. Setting `cached=null` does not cancel it.

Fix: guard with a generation counter incremented on every action.
```js
let generation=0;               // bump in the action handler's finally
…
const g=generation;
loading??=Promise.all(…).then(rows=>{const snap={receivedAt:Date.now(),hosts:rows};if(g===generation)cached=snap;return snap;}).finally(()=>loading=null);
res.json(await loading);
```
(Note `res.json(cached)` today can also send `null` if a concurrent invalidation lands between `await loading` and the send.)

### B5 — `request` in `transport.mjs` corrupts multi-byte UTF-8 across chunk boundaries
`const read=b=>{…output+=b.toString();}` decodes each 64 KiB chunk independently. Any non-ASCII byte sequence straddling a chunk boundary becomes U+FFFD, which for a large inventory yields `Invalid PodMesh response` — a host with non-ASCII container names or image labels becomes intermittently unreadable, and the failure mode looks like a transport fault. It affects both the SSH and Unix-socket paths.

Fix: buffer, then decode once.
```js
const chunks=[];
const read=b=>{size+=b.length;if(size>MAX)return finish(new Error('Response too large'));chunks.push(b);};
// in finish(): resolve(JSON.parse(Buffer.concat(chunks).toString('utf8')))
```

### B6 — `allowActions:false` is a UI/gateway flag presented as a host property
The drawer renders "This host is configured for observation only." but the only mechanism is `if(!host.allowActions)` in the gateway, reading a local JSON file. The SSH credential still holds `sudo -n python3 -c <arbitrary>` on that host — unrestricted root. So the read-only assurance describes the console's intent, not the host's posture; anyone who reaches the gateway process or the key has full write authority on every configured host.

Corrections: (a) change the copy to "Actions disabled in this console's configuration" so it stops asserting a host-side property; (b) constrain the remote sudoers rule to a fixed wrapper script rather than `python3`, and use a distinct key per host with `command=` restrictions in `authorized_keys` for observation-only hosts. Until (b), this is a single-credential root-on-all-hosts design, which should be stated in the Manager view alongside the existing HA disclaimer.

### B7 — No `frame-ancestors` / `X-Frame-Options` on any response
The middleware sets `no-store`, `nosniff`, `Referrer-Policy` but no framing control. The Host check does not help here: a browser framing `http://127.0.0.1:4175` sends `Host: 127.0.0.1:4175`, and the framed app is same-origin to itself, so it holds the session token and satisfies the Origin check. The required `authorization_ref` text input raises the bar for a pure click-hijack, but the console is otherwise fully driveable inside a hostile frame.

Fix, in the existing middleware:
```js
res.set('Content-Security-Policy',"default-src 'self';frame-ancestors 'none';base-uri 'none';form-action 'none'");
res.set('X-Frame-Options','DENY');
```

### B8 — `action()` in `main.jsx` picks the target host implicitly for `create`
`hostId: c?.host.id || hosts.find(h=>h.allowActions)?.id`. With up to 16 configured hosts, "Create universe" silently binds to whichever action-enabled host appears first in the config array. The modal's `<label>Host: …</label>` is display-only and appears after the operator already committed to the action.

Fix: for `create`, render a required `<select>` of action-enabled hosts with no preselected value, and block submit until chosen. A destructive/creative operation should not have a default target.

---

## Medium (fix, but not authority-bypass)

- **`feed` render crashes the whole app on unexpected host data.** `o.operation.replaceAll('_',' ')` throws if `operation` is absent, and there is no error boundary — one malformed observation from one host blanks the console. Guard (`String(o.operation||'unknown')`) and wrap `<App/>` in an error boundary.
- **Per-host errors are invisible on the Universes view.** `rows`/`shown` drop failed hosts silently; the only banner is `error`, which is set solely by a failed `/api/snapshot` fetch. An operator can see "No containers in this view" while a host is unreachable, then act on an incomplete picture. Render each host's `errors` as a row in that panel, and disable its filter chip.
- **`shown.slice(0,100)`** truncates with no indication. Show "100 of N matching".
- **`Node http requestTimeout` (300s default) is shorter than the transport timeout (395s).** Long actions will be cut at the socket before `finish` fires, so the browser sees a network error rather than the crafted `outcome:'unknown'` body. Either set `server.requestTimeout`/`headersTimeout` above 395s in `index.mjs`, or drop the transport default to ~240s.
- **`process.stderr.resume()` discards all diagnostics.** `sudo: a password is required`, host-key mismatch, and `Permission denied (publickey)` all collapse into "SSH transport failed; verify host connection and permissions". Capture the last ~2 KiB of stderr and append it to the error.
- **"Retry same request" is enabled on `outcome:'unknown'`,** directly contradicting the adjacent "reconcile before retry". Require an explicit acknowledgement checkbox before re-enabling submit in that state.
- **`host.knownHostsFile` is unvalidated** (unlike `h.ssh`). No shell injection — it is a separate `spawn` argv element — but validate it is an absolute path and `fs.accessSync`-readable at startup so misconfiguration fails loudly rather than per-request.
- **Token comparison `req.headers['x-podmesh-token']!==token`** is not constant-time. Low risk at localhost with no timing oracle, but `crypto.timingSafeEqual` on fixed-length hex is free.
- **`feed` key `o.host+o.id`** uses the host *name*, which is not validated unique (see B2).

---

## On the test suite

`tests/gateway.test.mjs` is tight on the boundaries it covers (origin, token, Host rebinding, capability gating, read-only hosts, transport framing). Two gaps matter:

1. `test('action forwards exact identity through capability check')` asserts `assert.deepEqual(g.calls,[{operation:'capabilities'},payload])` — it **encodes B1 as intended behavior**. After the whitelist fix, add a negative case asserting that `{...payload, on_timeout:'kill'}` is rejected 400 and `calls` stays empty.
2. Nothing covers B3/B4. Add: action → 502 from `call` → subsequent `/api/snapshot` must re-collect; and a concurrent-collection test asserting an in-flight snapshot cannot repopulate `cached` after an action.

No test exercises the SSH argv path or a partially-failing host in `/api/snapshot` (the `break` in the operation loop). Both are worth fixtures.

---

Confirmed sound, for the record: SSH host-key verification is genuinely enforced (`StrictHostKeyChecking=yes` plus an explicit `UserKnownHostsFile`, `BatchMode=yes` preventing prompt fallback); the remote payload travels on stdin with the script itself constant, so there is no command injection through `quote()`; `h.ssh` is validated against leading `-` and argv smuggling; the Host-header check does defeat DNS rebinding; and React's escaping means host-supplied inventory strings are not an XSS vector.