# Manager stress qualification map

The current executable is a local observation-store experiment, not a manager
service. Tests below separate that foundation from required distributed behavior.

| ID | Scenario | Current evidence boundary |
| --- | --- | --- |
| ST-01 | Restart preserves identity/history | Local SQLite reopen and wrong mesh refusal |
| ST-02 | Three replicas catch up | Three persisted files, 256 events, shuffled gaps and duplicates; no network |
| ST-03 | Sudden manager death | Actual child writer SIGKILL after 1/10/100 acknowledgments, externally checked rows and replay; no failover |
| ST-04 | Host power loss | Pending actual deployed service and test-host campaign |
| ST-05 | Network partition and disjoint write authority | Pending authenticated transport and grant/placement reducer |
| ST-06 | Reconnection with conflicts | Local graph quarantine tested; deployed reconciliation pending |
| ST-07 | Old snapshot return | Pending writer incarnation recovery and trusted snapshot protocol |
| ST-08 | Storage exhaustion | SQLite allocation refusal and recovery injected; physical storage failure pending |
| ST-09 | Load and concurrent requests | 256-event catch-up, bounded request quota, four duplicate writers with explicit retry; sustained production load pending |
| ST-10 | All managers lost | Pending independently recoverable bootstrap/backup and authority reconstruction |

No test changes an installed PodMesh service or shuts down a host. Fixtures live
in generated temporary directories and are removed after success; failed evidence
is retained for diagnosis. The child writer is killed/reaped by its owning test.

Acceptance for HA requires ST-04 through ST-07 and ST-10 on a real replicated
service, with zero unauthorized duplicate activation, quantified acknowledged
data loss, takeover time, and proof from outside the producer. A reachable copy,
SQLite persistence, or deterministic conflict detection alone does not meet it.

Source review found two proof/performance weaknesses: request mappings needed
independent checks before replay, and full-history rescan repeated work for reversed
chains. Both were corrected with regression coverage. Further tests must preserve
these boundaries rather than replace missing distributed proof with local counts.
