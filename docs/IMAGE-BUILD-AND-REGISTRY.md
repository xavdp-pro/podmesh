# Image build and registry operations

This is the operating direction for container image delivery. It does not claim
that a particular deployment already implements the separation. Deployment
addresses, current capacity and migration evidence belong in the private
maintainers' workshop.

## Separate build, storage and execution

The normal delivery flow is:

1. A build worker compiles the source, builds images and runs the required tests.
2. The worker pushes the accepted images to an OCI-compatible registry and records
   their content digests and source provenance.
3. Runtime hosts pull the selected images by digest and verify the required
   behavior on their target substrate. Publication alone is not qualification.

Podman is the container engine and registry client. An OCI registry stores and
distributes image manifests and layers; it does not compile the application or
host its qualification databases.

Use a dedicated registry service boundary and persistent storage allocation,
separate from build workspaces and caches. A dedicated LXC running the registry
under Podman is one deployment option, not a universal host requirement.
Co-location for a small laboratory is possible, but shared capacity and the
resulting failure coupling must be explicit. A mixed build/registry container
must not be described as a dedicated registry container.

## Own the data and its lifecycle

- **Build worker:** source checkouts, compiler outputs, intermediate images,
  caches and disposable test state. Bound their growth and retire completed
  workspaces after preserving the required delivery artifacts and evidence.
- **Registry:** persistent manifests, layers, tags and service configuration.
  Back up its unique contents and configuration; verify recovery. Keep secrets
  outside published images and manifests.
- **Runtime:** pulled images and separately declared persistent application
  state. Application data and private databases are not image registry contents.

Tags may share a manifest and layers. Neither tag counts nor the sum of image
sizes measures actual registry disk use. Measure the registry storage directory,
the guest filesystem's available space, and any shared host storage pool
separately. A guest's disk allocation is not a reservation of free physical
capacity in a thin pool.

Temporary paths are not a retention policy: build directories may survive a
reboot and grow indefinitely. Identify the owner, active users, reproducibility
and retained evidence before scoped cleanup. Do not use a blanket prune or
delete an old directory merely because of its age. Registry garbage collection
is a separate operation governed by the selected registry implementation and
the retained image references.

## Verify delivery and changes

Declare the registry endpoint, access policy and storage in the deployment
configuration; never hard-code laboratory values as product defaults. Verify
both a real image push and a pull by digest from an intended runtime host.
An API health response or catalog listing alone does not prove the delivery path.

If separating an existing mixed deployment, inventory and preserve all required
images and references, copy and verify them at the new endpoint, then update
clients within the authorized scope. Retain the original service and data until
the replacement is accepted and rollback is available. Creating a replacement
does not itself authorize deleting the original or switching unrelated clients.

The standard build-to-registry workflow is also described in
[CNCF Distribution's registry overview](https://distribution.github.io/distribution/about/).
