# PodMesh operator console

Experimental React/Vite/Tailwind/Lucide console for human–agent tandems. It calls the same PodMesh daemon API as agents through a loopback Express gateway. It does not implement manager HA.

## Run

Use Node.js 22 or later. In this directory run `npm ci`, `npm run build`, copy `config.example.json` to `config.local.json`, and run `npm start`. Open http://127.0.0.1:4175. `PODMESH_WEB_CONFIG` can select an external configuration file.

Each host requires a stable local identifier, display name, and exactly one transport: an absolute Unix `socket` path or an `ssh` destination. Remote hosts require Python 3, access to the daemon socket through non-interactive sudo, and a verified SSH host key. An optional `knownHostsFile` selects a trusted key file. Never commit host credentials or private infrastructure configuration.

Actions default to disabled. `allowActions` enables console actions for that target; this is a gateway restriction, not a read-only host credential. The current laboratory SSH credential has broader host authority. Production use requires a restricted remote wrapper, scoped credentials and authentication; do not publish this loopback console directly.

## Feature inventory and qualification

| ID | Feature | Current scope |
| --- | --- | --- |
| WEB-01 | Overview and hosts | Real inventories read from three laboratory hosts |
| WEB-02 | Universes | Search, host filters, observed state, raw inventory detail |
| WEB-03 | Lifecycle actions | Create, clone, start, non-escalating stop, delete; browser lifecycle passed on a disposable Alpine universe |
| WEB-04 | Operations | Latest non-polling observations, not completion verdicts; polling reads remain in the daemon journal |
| WEB-05 | Manager | Explicit placeholder; no HA or takeover claim |
| WEB-06 | Resource metrics | Host RAM, CPU/load and graph-root filesystem capacity implemented; unsupported, failed, stale and partial states remain explicit |
| WEB-07 | Browser presentation | Six views exercised in Chromium; desktop/mobile screenshots captured |
| WEB-08 | Migration and restore | Not exposed by this console yet |
| WEB-09 | Nested explorer | Ordinary and nested Podman detail, with a distinct ShaperOS presentation hint; no logical-parent inference |
| WEB-10 | Fractal forest | Read-only model, filters, conflict/cycle handling and partial statistics tested with fixtures; live manager relationships unavailable |

The landing view shows six matching inventory entries; the Universes view is currently capped at 100. Inventory labels identify candidates only: the daemon verifies ownership. Collection time is not the time of an authoritative state change.

## Action contract

Targets are configured by the operator, never supplied as arbitrary SSH commands by the browser. Creation requires explicit target selection. Requests preserve operation and universe UUIDs. The gateway checks origin, session token, configured action access, supported fields and advertised daemon capabilities. Stop always requests ten seconds with `leave_running`; no forced escalation is exposed.

A transport error can occur after execution. An unknown outcome disables retry in that dialog: inspect the daemon journal and actual workload before proceeding. A successful API response alone is not independent proof of the resulting workload state.

## Verification and remaining work

`npm test` passes 32 gateway, metrics and fractal-model tests. `npm run build` succeeds and the production dependency audit reports no known vulnerability. Browser navigation over real read-only inventories and live resource metrics from three dedicated observer sockets produced no page errors or mobile horizontal overflow. The browser lifecycle returned successful API results for create, clone, start, graceful stop and both deletions. An independent Podman inventory comparison confirmed unchanged pre-existing container IDs/states and absence of disposable containers afterward. Intermediate application behavior and HA recovery are not covered.

Claude Code Opus completed a source-only counter-review. Corrections applied include strict action fields, explicit transport selection, cache generation invalidation, UTF-8 buffering, frame protection, explicit create target and unknown-outcome retry disabling. Findings are not a production approval. Cache-race and split-UTF-8 regression tests now pass. Restricted remote credentials and further accessibility qualification remain outside this local operator release.

## Execution checklist

- [x] Build six truthful views over the existing API.
- [x] Connect three laboratory hosts with verified SSH keys.
- [x] Run gateway tests, production build and browser navigation.
- [x] Obtain Claude source counter-review and apply primary corrections.
- [x] Complete regression coverage for cache races and split UTF-8.
- [x] Qualify create/clone/start/stop/delete using only disposable workloads.
- [x] Recheck the metrics and fractal changes independently with Claude Opus and apply the findings.
- [x] Publish and qualify the metrics-enabled observer package upgrade on the three hosts.
- [ ] Connect manager relationships and migration only when their API contracts are available.

## Three-pass closeout

1. Governance: this console adds no manager authority. Each mutation carries an explicit authorization reference and operation UUID; the existing runtime remains responsible for ownership and reservations.
2. Operator experience: host selection is explicit for creation, unknown metrics and unqualified HA remain visible, and ambiguous outcomes cannot be retried in the same dialog. Keyboard Escape and mobile navigation were exercised after the UI corrections.
3. Runtime: 32 automated tests cover gateway boundaries, dedicated observer routing and exclusion, action-endpoint isolation, optional failure handling, UTF-8 chunking, cache invalidation, host-metric aggregation and the fixture-only fractal model. The isolated browser lifecycle succeeded; external inventory comparison verified no change to pre-existing container identities/states. The metrics-enabled observer package was upgraded through signed APT on all three hosts without changing lifecycle daemon PIDs or workload inventories. Claude provided independent source counter-views; Codex assessed the findings and the affected suites were rerun after correction.

Verdict: coherent with corrections for a local experimental operator console. Production exposure remains OPEN pending scoped credentials and authentication. No Shaper canon was modified. WEB-01 through WEB-10 are reconciled in the feature table above. Live manager relationships, migration and manager HA remain explicit omissions. The metrics source is installed and qualified on the three laboratory hosts.

## Nested universe explorer

Click a container name to inspect it. Configuration shows CPU limits, memory limits, networking mode and mount count; metric samples show CPU, RAM, network and block I/O when available. Disk layer sizes exclude external volumes and are not free-space measurements. Sensitive environment, command and mount contents are omitted.

Running parents can expose their default rootful nested Podman inventory. Click children and use breadcrumbs to navigate up to four container levels. Stopped containers are never started for inspection. Ordinary containers retain their regular detail presentation. A nested inventory with observed `org.shaper.brick` labels receives the ShaperOS-oriented presentation; this is a presentation hint, not certification or a logical parentage assertion. Nested observations come from software inside the parent and are identified as such.

The gateway requires `container_details` capability. During laboratory qualification, an isolated observation-only daemon is selected with the operator-only `detailsSocket` setting; ordinary actions continue to use the existing installed PodMesh daemon. `PODMESH_READ_ONLY=1` rejects mutations on the observer, which uses its own socket/state directory. No existing package or workload is replaced. The observer is packaged as a persistent service; each later package-version upgrade remains a separate qualification gate.

The explorer enforces full container IDs, bounds command duration/output and allows one detail request per host at a time. Abandoning a browser request does not cancel an observation already executing inside a parent; its command timeout bounds it. Refresh details explicitly for another sample.

Qualification: disposable nested parent and child navigation passed in Chromium; observed CPU/RAM/I/O and configuration displayed. Mutation rejection was verified on all three observer endpoints. Claude Opus reviewed the initial explorer source; corrections included explicit root scope, optional size sampling, bounded output, observation concurrency, schema handling and provenance. A final source-only Opus review reported no remaining blocker for the local experimental observer. Its two residual observations, client timeout recovery and consistent lowercase ID gates, were then corrected.

See [the fractal forest plan](FRACTAL-VIEW-PLAN.md) for logical trees and per-fractal statistics. Those depend on the relationship registry and are not synthesized from container nesting.
