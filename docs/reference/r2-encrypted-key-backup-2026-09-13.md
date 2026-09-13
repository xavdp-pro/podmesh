<!-- Relocated into the repository on 2026-09-14 from /tmp/podmesh-claude/, where it was
     cited as the basis for decision X4. Checked before relocation: it records that no R2
     access key was found and states that no secret value may be committed; it contains none. -->

# PodMesh Backup Server — Cloudflare R2 and encrypted recovery keys

**Date:** 2026-09-13  
**Status:** design and environment inspection; no production backup has been uploaded  
**Audience:** Claude Code, Codex, and the root human-agent tandem

## Objective

Use Cloudflare R2 through its S3-compatible API as an independent off-site storage target for PodMesh Backup Server. R2 may store backup objects, manifests, proofs, and encrypted key envelopes. It must never become the only place from which its own decryption or recovery authority can be recovered.

The recovery workflow is agent-first. An agent inventories, validates, restores into quarantine, and emits evidence. The human retains control over sensitive authority changes, recovery-key use, identity replacement, and activation of a restored manager.

## Verified environment state

The central REMOTE3 environment currently declares:

- a Cloudflare account identifier;
- an active general Cloudflare API token;
- an R2 account identifier;
- an R2 S3 endpoint;
- an intended R2 bucket name;
- vault-related variables.

Read-only verification produced these results:

| Capability | Result |
| --- | --- |
| Verify the Cloudflare token | HTTP 200; active |
| List accessible zones | HTTP 200 |
| List Cloudflare tunnels | HTTP 200 |
| List R2 buckets | HTTP 403, Cloudflare code 10000 |
| Find an R2 S3 Access Key ID and Secret Access Key in the inspected environment/vault inventory | Not found |

The current token is therefore usable for existing zone and tunnel operations, but it is not verified or authorized for R2. No R2 bucket, upload, download, retention rule, or restore has been proven in this inspection.

Do not print, copy into reports, or commit any secret value found in REMOTE3.

## Required credential separation

Cloudflare R2 S3 access requires a dedicated R2 Access Key ID and Secret Access Key. The general Cloudflare token used for zones and tunnels must not be reused as the backup data-plane credential.

Create separate, bucket-scoped credentials:

1. **Backup writer** — Object Read & Write on the single PodMesh backup bucket. It uploads immutable objects and verifies them. It is present only on authorized backup producers. It must not manage buckets or lock rules.
2. **Restore reader** — Object Read only on the same bucket. It is used by the external recovery tool and does not permit mutation.
3. **Retention administrator** — administrative access used only to configure bucket-lock and lifecycle rules. It must not be present on ordinary PodMesh hosts or inside the Backup Server universe.

Cloudflare R2 currently does not expose a bucket-scoped write-only permission: Object Read & Write also permits reads and listing. PodMesh must therefore obtain deletion resistance from bucket-lock rules and credential isolation, not from an invented write-only policy.

Store credential values in the external secret store and inject them at runtime. Git may carry only variable names, credential purpose, fingerprints, and rotation metadata.

## Key hierarchy

Use envelope encryption:

- A unique **data-encryption key (DEK)** encrypts each backup or bounded backup generation.
- A **key-encryption key (KEK)** wraps each DEK.
- R2 stores ciphertext, manifests, proofs, and wrapped DEKs.
- The unwrapped KEK or root recovery secret is never stored in the same R2 security domain as the ciphertext it unlocks.

Maintain at least two independently recoverable copies of the root recovery material, with at least one copy offline or in a separate provider/security domain. The exact custody mechanism is an operator decision. A secret vault is acceptable only if its own recovery does not depend on the PodMesh manager or the Backup Server being restored.

Each signed manifest must bind:

- backup and universe UUIDs;
- parent backup/generation when incremental;
- object hashes and sizes;
- encryption algorithm and format version;
- DEK envelope identifier and KEK identifier;
- nonce and authenticated metadata where required by the algorithm;
- capture consistency state;
- creation time as evidence, never as sole authority;
- producer identity and signature;
- required PodMesh and restore-tool compatibility versions.

Key rotation must preserve the ability to decrypt retained historical backups. Rewrapping a DEK creates a new signed envelope; it must not silently rewrite the backup manifest or ciphertext.

## R2 object layout

R2 is a flat object store; prefixes are naming conventions. Use content-addressed immutable objects:

```text
podmesh/v1/chunks/<sha256>
podmesh/v1/manifests/<universe-uuid>/<backup-uuid>.json
podmesh/v1/envelopes/<backup-uuid>/<envelope-version>.json
podmesh/v1/proofs/<backup-uuid>/<proof-kind>.json
podmesh/v1/catalog/checkpoints/<catalog-generation>.json
```

Never overwrite a manifest or proof in place. Publish a new signed object and reference it from a later catalog checkpoint.

## Retention and deletion resistance

Cloudflare R2 bucket-lock rules can prevent deletion and overwriting for a duration, until a date, or indefinitely. They take precedence over lifecycle deletion rules. Configure lock rules only after proving the complete workflow in a disposable bucket or prefix, because an incorrect retention rule can make test data undeletable for its retention period.

Suggested first policy for a disposable qualification prefix:

- lock uploaded backup objects for a short declared test duration;
- keep lifecycle deletion disabled during qualification;
- prove that overwrite and deletion are rejected;
- prove that reads and checksum verification still succeed;
- then choose production retention based on restoration objectives and storage cost.

Age alone never authorizes PodMesh to delete backup evidence. The collector may propose deletion only when the declared retention contract, replacement proof, open-authorization checks, incident preservation rules, and operator mandate all permit it. R2 lifecycle automation must not bypass that decision model.

## External bootstrap and self-recovery

PodMesh Backup Server must not depend on itself for recovery. Provide a minimal, independently installable `podmesh-recovery` package for a clean Debian/Linux host. It must be usable without ShaperOS, while ShaperOS remains the preferred operating environment.

Recovery sequence:

1. Install the pinned recovery package on a clean host.
2. Inject the R2 read-only credential from the external secret store.
3. Fetch signed catalog checkpoints, manifests, envelopes, chunks, and proofs.
4. Verify signatures, hashes, format compatibility, and completeness before decryption.
5. Obtain the separately held root recovery material through the authorized tandem workflow.
6. Restore the Backup Server universe into an isolated network and identity quarantine.
7. Rebuild and verify its catalog from immutable objects.
8. Demonstrate a byte-exact restore of a known backup and record external evidence.
9. Prove exclusion of the old active identity and obtain explicit activation authority.
10. Only then expose the restored service and rotate runtime credentials.

The same quarantine rule applies when restoring a manager replica. Restoring data does not grant current authority.

## Qualification checklist

- [ ] Create or locate a dedicated R2 S3 credential scoped to the intended bucket.
- [ ] Store it in the external vault; do not add it to Git or ordinary reports.
- [ ] Confirm the bucket and jurisdiction-specific endpoint match.
- [ ] Upload a harmless random canary through the S3 API.
- [ ] Read it back and verify its exact length and SHA-256 hash.
- [ ] Repeat with the read-only restore credential.
- [ ] Confirm the restore credential cannot upload, overwrite, or delete.
- [ ] Configure a short bucket-lock rule on a disposable prefix.
- [ ] Prove overwrite and deletion rejection under the lock.
- [ ] Upload an encrypted synthetic backup and its signed manifest/envelope.
- [ ] Recover it on a separate clean host using separately held recovery material.
- [ ] Verify byte-exact restored content and persisted evidence from outside the producer.
- [ ] Simulate loss of the Backup Server universe and rebuild its catalog from R2.
- [ ] Rotate the KEK or envelope version and prove old retained backups remain restorable.
- [ ] Simulate unavailable R2 and prove local PodMesh operation fails safely without corrupting state.
- [ ] Record cost, storage growth, transfer time, and restore time.

## Immediate conclusion

R2 is a suitable candidate for off-site encrypted backup objects. The configured general Cloudflare token cannot currently qualify that path: it can read zones and tunnels, but R2 bucket listing is forbidden, and the required S3 key pair was not found. The next concrete step is to provision or locate bucket-scoped R2 S3 credentials, store them outside Git, and execute the harmless canary qualification before any real key material is backed up.

## Official references

- Cloudflare R2 authentication and permissions: https://developers.cloudflare.com/r2/api/tokens/
- Cloudflare R2 S3 setup: https://developers.cloudflare.com/r2/get-started/s3/
- Cloudflare R2 bucket locks: https://developers.cloudflare.com/r2/buckets/bucket-locks/
- Cloudflare R2 object lifecycle rules: https://developers.cloudflare.com/r2/buckets/object-lifecycles/
