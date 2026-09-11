# Optional LVM2 storage validation

Status: authorized next lab stage, not executed.

After completing the current lifecycle tests, shut down the three PodMesh lab VMs cleanly, add one new 60 GB virtual disk to each, then restart them. Verify host storage capacity before allocation. Identify each new disk by its device identity and size, confirm it contains no existing data, and never initialize the operating-system disk.

Create an LVM2 physical volume and volume group on each new disk. Test ordinary logical volumes and a thin pool with thin logical volumes, filesystem mounting, snapshots and independent restoration. Choose pool sizing with free space reserved for extension and metadata; document actual provisioned and consumed capacity. Monitor both thin-pool data and metadata usage. Do not silently overcommit physical capacity.

LVM2 is an optional storage backend, not a PodMesh prerequisite. Thin provisioning allocates physical pool space as blocks are written. Thin snapshots initially share blocks and consume additional space as data diverges. Neither snapshots nor thin provisioning replicate data to another host or capture process memory.

Test application-consistent snapshots, clone independence, backup restoration, pool-capacity alerts and controlled failure handling on disposable workloads. A snapshot on the same disk is not an independent backup. Keep lab virtualization details separate from generic Linux host preparation instructions.

## Sequential storage comparison (operator decision)

Use one additional 60 GB disposable disk per lab VM, reused across these stages:

1. LVM2: test conventional logical volumes and thin provisioning.
2. After saving the LVM evidence and completing its tests, remove the disposable LVM setup and initialize ZFS on the same additional disk on each host.
3. After saving the ZFS evidence and completing its tests, remove the disposable ZFS setup and initialize Btrfs on the same additional disk on each host.

The operator authorizes replacement of these lab storage formats in this sequence. This does not authorize erasing OS disks, existing host pools or unrelated data. Before each replacement, stop dependent test workloads, unmount the test filesystem, verify the exact additional disk identity and preserve scripts, results and required recovery artifacts outside that disk.

Use comparable workloads and measure installation effort, snapshot and clone behavior, consumed space, transfer volume and time where supported, restoration correctness, failure recovery and manual interventions. Include non-recursive Btrfs snapshot coverage for nested subvolumes. Select the most practical backend based on evidence, not a presumed winner. These formats remain optional deployment capabilities.
