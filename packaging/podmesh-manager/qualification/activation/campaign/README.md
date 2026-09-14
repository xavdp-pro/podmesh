# Repeatable three-host campaign driver

`run-campaign.sh campaign` runs a whole evidence-v3 campaign from the workstation: private
parent directories on every host, activation in the frozen host order, one owned observation
per host, a bounded wait for three equal canonical digests, the converged captures sealed into
the ledgers, the typed graceful shutdowns, preservation of every raw store at rest with an
inspection derived from the copy, fetch with sidecar verification, the comparator, the
derivation check, and a public summary. It stops at the first failure, rolls back whatever it
activated, and still writes the summary; `finish` resumes after the live phases when a later
phase failed for a reason of the driver's. Every phase is also invocable alone.

What it preserves, per host, under the campaign directory (identical path on host and
workstation, so the sidecars stay valid): the four stage files and their sidecars, the
activation ledger, `preserved-store/manager.sqlite` with `SHA256SUMS`, and
`derived-inspection.json`; and, for the campaign, `comparison.json`, `derivation.json`,
`steps.json` and `campaign-summary.json`. The preserved stores and the derived inspections hold
raw identifiers and stay private; the summary carries aliases, hashes, counts and verdicts only.

Derivation: the frozen candidate is run as `--inspect-store` on the host against the preserved
copy, under a configuration rebuilt from the installed one with only the database path changed
(the candidate refuses a database outside its declared state directory); the digests and counts
it reports must equal what the live post-cleanup capture projected, on every host.

The verdict is the comparator's and the derivation's over live phases that all succeeded; a
driver phase that failed and was repaired is listed under `driver_incidents`, never hidden and
never counted against the candidate; a failed live phase makes the campaign FAIL.

Environment (all private, none in Git): `PODMESH_CAMPAIGN_DIR`, `PODMESH_CAMPAIGN_HOSTS`
(alias to SSH target), `PODMESH_CAMPAIGN_KNOWN_HOSTS`, `PODMESH_CAMPAIGN_REMOTE_DIR`,
`PODMESH_CAMPAIGN_HARNESS` (default `harness6`), `PODMESH_CAMPAIGN_CANDIDATE`. The plan
(`campaign-plan.json`) carries the frozen operation IDs, scopes and the convergence rule.
