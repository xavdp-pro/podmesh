# Local API contract

The experimental daemon accepts one newline-terminated JSON request per Unix socket connection and returns one JSON response line. The default endpoint is `/run/podmesh/api.sock`, restricted to root. The CLI is a client of this same endpoint.

## Discover before acting

```sh
sudo podmesh capabilities
sudo podmesh identity
sudo podmesh inventory
sudo podmesh observations
```

Capabilities describe the installed build. Do not infer that planned operations are available from the roadmap.

## Typed mutation requests

Pass a JSON request file as the second CLI argument:

```sh
sudo podmesh create request.json
```

A creation request contains:

```json
{
  "operation_id": "an-explicit-unique-operation-id",
  "universe_uuid": "a-valid-new-universe-uuid",
  "authorization_ref": "operator-approved-lab-work",
  "image": "sha256:FULL_LOCAL_IMAGE_ID",
  "command": ["sleep", "300"]
}
```

The UUID and image above are placeholders, not executable examples. The CLI sets the operation field from its first argument. The initial creation operation produces a stopped container with networking disabled; it does not start the application. Image pulling is not implicit.

Clone requests additionally identify `source_uuid` and use a new target `universe_uuid`. Deletion identifies the target universe and must not be used to imply a stop operation. Consult installed capabilities and the release's tested scope for exact restrictions.

## Results and retries

Check both CLI exit status and JSON `ok`. Keep the same operation ID and byte-equivalent semantic request when retrying. Reusing an operation ID for a different request fails. A completed operation may return its persisted original result marked `replayed`; this is historical evidence, not a fresh claim that the resource still exists or is unchanged. Use inventory or independent inspection for current state.

A dropped connection is an uncertain outcome, not proof that no action occurred. Preserve the request and inspect or retry it according to the operation contract. Do not generate a new operation ID blindly after a timeout.

## Authority and scope

`authorization_ref` records provenance; it is not a verified authorization token. The root-only local endpoint is the present access boundary. Remote authentication, tenant policy and ShaperOS integration remain separate work. Direct Podman commands may be used for independent verification and test fixtures, but do not count as a successful PodMesh operation when its API is absent.
