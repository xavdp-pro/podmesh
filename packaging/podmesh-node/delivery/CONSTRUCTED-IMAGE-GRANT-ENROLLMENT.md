# Append one constructed receiving grant with an approved image

`enroll-constructed-image-grant.py` is a separate root administrator for a
stopped installed Node. It adds one fresh contained receiving grant. If its exact
image is already approved once, the complete `allowed_images` value must remain
byte-identical, including order, prefix spelling and whitespace. Otherwise, it
appends that one explicitly approved, already loaded image. Duplicate approval
of the same image, including a `sha256:` alias, is refused. Both paths require
the same actual loaded-image, matrix/OCI and constructed-tree proofs. This is
not an empty-target enrollment shortcut. It does not replace or weaken the
frozen empty-target administrator `enroll-contained-grant.py` (SHA-256
`6381411f3ffdd335fc25e2e384251bd7c1df4f8c6a7d2d36c83c895a1dde8702`).
The original installed bundle, provider, operation history and old grants remain
unchanged. There are no image pulls, container lifecycle calls, SQL writes or
implicit universe-budget changes. A separate authorized budget update may be
needed before the new grant can be used.

Run this source from a separately pinned root-protected delivery directory
containing the exact frozen administrator beside it. All inputs are explicit:

- Original installed `--bundle`, `--scope`,
  `--expected-bundle-manifest-sha256`, `--expected-policy-sha256` and
  `--expected-receipt-sha256`.
- Reviewed `--new-policy`, `--expected-new-policy-sha256`, `--template-uuid`
  and dedicated `--enrollment-root`. Only the contained-grant append and, for
  a previously unapproved image, the single image append may differ. Existing
  array items retain their exact descriptor bytes. The new
  grant retains the template's containment, network, capabilities, relative
  mount layout and ID-count, with disjoint bounded UID/GID ranges and root.
- `--expected-image-id` is the bare 64-character image ID;
  `--expected-image-source-pin` is its full 40-character source revision.
  `--matrix-file` and `--expected-matrix-sha256` bind the root-reviewed matrix
  TAR. Its `cfg-image-lock.json`, outer OCI archive, manifest and configuration
  blob must agree with both pins. The original Node's private store must already
  contain that exact image. OCI locks use the `sha256:` prefix.
- `--construction-manifest` and
  `--expected-construction-manifest-sha256` bind the complete root-reviewed
  preconstructed target. The manifest is root-owned mode 0600.
- `--freshness-receipt`, `--freshness-sha256` use the frozen administrator's
  exact SQL/API/process evidence schema. Actual SQL count zero, inventory
  absence and executable/generation evidence must all be from the last stopped
  native app/database generation. Provider state and actual scoped containers
  must independently show that this UUID has never been consumed.
- A fresh `--operation-id`, separate `--tool-source-pin` and
  `--expected-tool-sha256` bind the administrator execution.

The construction manifest has exactly these fields:

```json
{
  "schema": "podmesh.preconstructed-universe/1",
  "universe_uuid": "<fresh canonical UUID>",
  "volume_root": "<absolute declared root>",
  "image_id": "<bare image ID>",
  "matrix_sha256": "<matrix TAR SHA-256>",
  "source_pin": "<image source revision>",
  "entries": []
}
```

Every entry has `path`, `type` (`directory` or `file`), numeric `uid`, `gid` and
`mode`; file entries also have `bytes` and `sha256`. Paths are relative to the
volume root; the root entry is **`.`**, not an empty string. Entries cover every
actual path, including empty directories, without duplicates or extra fields.
Links, special files, shared regular inodes, cross-filesystem entries and group
or other writable paths are refused.

Regular files may exist only under the declared `/etc/shaper` mount, as the
read-only SSH mount's `authorized_keys`, or as the exact host-only
`instance/operator.tokens.json` leaf derived from the configuration mount's
`instance/etc/shaper` layout. This credential leaf must be root:root 0600,
single-link, nonempty JSON object of at most 1 MiB; its bytes are checked without
printing their content. No other host-only credential files are accepted.
Saved and inner-store paths remain directory-only: no copied SQL, cache blobs
or even empty regular placeholders. Mount roots and configuration directories
remain root-owned and may use bounded functional group IDs without group-write
permission. Empty functional subdirectories below saved may have
bounded declared UID/GID ownership, with exact modes and ownership in the
reviewed manifest.

The administrator uses the original `Instance.read`, immutable unit checks and
stopped-state guards under `operator.lock`. It preserves versioned preimages,
proposal and a durable intent before the policy/receipt replacements. The only
receipt delta is the truthful policy checksum. Partial transactions are refused
and retained for explicit inspection; they are never silently adopted or
rolled back. Completion verifies original administration readback, all old
mount identities, the new mount identities, provider state and constructed
bytes. `ROOT_CONFIGURATION_ENROLLED` is configuration evidence;
`runtime_qualified` remains false. Native restart, actual fresh clone bootstrap,
restore, handover and renewal require their own exact-artifact qualification.

Build-host regressions (root is required for actual ownership fixtures):

```sh
python3 -m unittest discover -s packaging/podmesh-node/delivery/tests -p test_enroll_constructed_image_grant.py
```

These standard-library tests exercise guard refusals and source-level
transactions. They do not establish VM operation or containment proof.
