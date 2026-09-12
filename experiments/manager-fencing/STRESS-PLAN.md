# Fencing qualification plan

All tests require disposable targets and independently read effect evidence.
Local tests below use temporary SQLite files only. A green model is not host HA.

| Stage | Scenario | Acceptance condition | Current state |
| --- | --- | --- | --- |
| Local | Three replicas, 600 seeded message attempts | At least 200 accepts, 100 refusals, 50 stale attempts and ten multi-effect epochs; one owner per resource/epoch; effect epochs never regress; counters equal durable unique receipts | Executable |
| Local | Two concurrent candidate transfers | One CAS succeeds and one is refused | Executable with separate processes |
| Local | Old effect requests after new-owner commit | Post-transfer barrier forces 32 stale requests, all refused; one old effect precedes 33 new effects | Executable with separate processes |
| Local | Kill maker after external effect commit, before local commit | Gate receipt persists; identical retry adds no effect | Executable with real SIGKILL |
| Local | SQL failure after receipt insertion | Entire gate transaction rolls back | Executable injection; not physical disk-full |
| Local | Restart, stale maker return, malformed/fabricated grant, quotas | Current gate remains authoritative and refusals do not mutate effects | Executable |
| Local | Busy writer, wrong store role/version, corrupt/open/broken schema, worker exception | Public storage failures are typed; no implicit retry or rebuild; fixture errors reach the parent explicitly | Executable |
| Negative control | Clone an authority database and use both copies | Demonstrate that two copies can accept different owners; reject this deployment design | Executable counterexample |
| Adapter | Bind authenticated channels to enrolled replica incarnations | Forged, revoked, wrong-host, replayed and unknown principals are refused | Not implemented |
| Adapter | Enforce fencing at a real resource, including in-flight work | Independent resource trace shows no old-owner effect after new-owner activation | Not implemented |
| Hosts | Partition all pairs and partition one host from gate separately | Allowed local work continues; every exclusive effect follows its declared gate policy | Not run |
| Hosts | Pause old owner before/after authorization and resume after takeover | The delayed start/write/route advertisement is mechanically refused | Not run |
| Hosts | Abruptly stop owner, restart it with stale data, reconnect | Replacement only after valid exclusion; old instance cannot regain effects | Not run |
| Authority | Gate outage, WAL loss, old backup, duplicate gate, power loss | Either continuity of authoritative ordering is proven, or activation fails closed pending explicit recovery | Not implemented |
| HA | Repeat host-loss/recovery for each of three hosts | Measure time to verified recovery, lost work, manual interventions, and every ambiguous outcome | Not run |

For the real adapter, record source commit, package versions, enrolled identities,
network fault commands, independent timestamps, effect log and Logger correlation.
Measure both safety and availability: refusing every request is safe but does not
meet recovery goals. Report successful ongoing work in unaffected scopes separately
from blocked exclusive work. Never convert a negative control into a success claim
about production fencing.
