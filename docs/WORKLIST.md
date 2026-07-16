# MCNF Worklist — Local-First Red Hat Virtualization and Containers

**Status:** ACTIVE / HARD CUTOVER  
**Decision date:** 2026-07-16  
**Supersedes:** QUASAR-CLOUD and all OpenStack-based VM, storage, networking, identity, image, orchestration, packaging, test, documentation, and UI paths.  
**Primary product:** a local workstation operating system with near-native local VMs, direct DRM/KMS shell integration, standard Red Hat virtualization and container components, and mesh-connected management.

## Governing outcome

A workstation must boot into the MCNF shell, create a local VM from an LVM thin snapshot, run it through libvirt/QEMU-KVM with pinned CPU and fixed memory, and display it through a direct virtio-gpu/DMA-BUF path with performance close to native. OpenStack must not be required for local VM, container, image, networking, update, or lifecycle operations.

## Epic 1 — Remove OpenStack and restore a local-first boundary

- [ ] **LOCAL-1 — Make the workstation the primary product.** Local VM and container operation must remain available without any remote control plane.
- [ ] **LOCAL-2 — Standardize virtualization on libvirt + QEMU/KVM.** Remove alternate VM schedulers and direct all lifecycle operations through supported libvirt interfaces.
- [ ] **LOCAL-3 — Use raw LVM logical volumes for VM disks.** Do not use QCOW2 as the default runtime disk format.
- [ ] **LOCAL-4 — Create one shared LVM thin pool per virtualization host.** All managed VM disks and imported bases live in that pool.
- [ ] **LOCAL-5 — Add automatic thin-pool cleanup.** Delete expired snapshots and unused VM volumes before blocking growth; never silently overrun data or metadata space.
- [ ] **LOCAL-6 — Retain automatic VM snapshots for 24 hours.** Expire them automatically after the retention window.
- [ ] **LOCAL-7 — Snapshot before guest updates and major configuration changes.** Do not snapshot on every start or shutdown.
- [ ] **LOCAL-8 — Do not create automatic VM backups.** Snapshots are local recovery points; VM export remains explicit.
- [ ] **LOCAL-9 — Make VM creation template-first.** Users start from opinionated profiles rather than a raw libvirt form.
- [ ] **LOCAL-10 — Ship the initial broad template set.** Windows 11 Desktop, Fedora Workstation, Fedora Server, a RHEL-compatible server, Ubuntu Desktop, and Ubuntu Server.

## Epic 2 — CPU, memory, NUMA, storage, and near-native performance

- [ ] **PERF-11 — Pin dedicated physical CPU cores to VMs by default.** Avoid shared-vCPU scheduling for managed local VMs.
- [ ] **PERF-12 — Reserve two physical cores for the host.** All remaining eligible cores may be dedicated to VMs.
- [ ] **PERF-13 — Use fixed, non-overcommitted VM memory.** Do not depend on ballooning for normal capacity.
- [ ] **PERF-14 — Use transparent huge pages.** Do not reserve static huge pages globally.
- [ ] **PERF-15 — Configure two adapters on every standard VM.** One adapter serves the local network and one serves the routed mesh-side VM network.
- [ ] **PERF-16 — Route VM mesh traffic through the host’s Nebula connection.** VMs do not receive Nebula certificates and do not become encrypted Nebula peers.
- [ ] **PERF-17 — Give each host a fleet-allocated routed VM subnet.** The host advertises and routes the subnet across the mesh.
- [ ] **PERF-18 — Allocate VM subnets centrally from fleet state.** Prevent overlap and retain allocation history.
- [ ] **PERF-19 — Allocate a /24 VM subnet to each host.** Reserve addressing and gateway conventions centrally.
- [ ] **PERF-20 — Assign fixed VM addresses from fleet state.** Do not use dynamic DHCP for managed VMs.

## Epic 3 — VM addressing, DNS, LAN connectivity, and firewall behavior

- [ ] **NET-21 — Readdress a VM when it moves hosts.** Assign an address from the destination host’s /24 rather than preserving the old address.
- [ ] **NET-22 — Give every VM a stable hostname.** Back mesh DNS with the fleet service directory and update records automatically after movement.
- [ ] **NET-23 — Use systemd-resolved for VM name resolution.** Generate host and mesh routing domains from fleet state.
- [ ] **NET-24 — Treat the authenticated mesh as trusted for VM-to-mesh communication.** Keep the public boundary protected by the host firewall.
- [ ] **NET-25 — Use direct macvtap for the VM’s local-network adapter.** Favor near-native LAN performance.
- [ ] **NET-26 — Fall back when macvtap is unavailable.** Preserve connectivity rather than refusing to start the VM.
- [ ] **NET-27 — Select fallback by interface type.** Use a Linux bridge on Ethernet and routed NAT on Wi-Fi or cellular.
- [ ] **NET-28 — Make direct virtio-gpu/DMA-BUF import the primary local display path.** SPICE, VNC, or RDP may exist only as recovery or remote fallbacks.
- [ ] **NET-29 — Implement tiered guest graphics.** VirGL by default, Venus/Vulkan where supported, and full GPU passthrough for performance templates.
- [ ] **NET-30 — Support dynamic detach of a single host GPU.** The host yields the GPU to a VM and reclaims it safely after shutdown.

## Epic 4 — Passthrough, input, audio, and latency-sensitive workloads

- [ ] **DEVICE-31 — Let the passed-through GPU drive the physical display.** Keep the host remotely manageable over the mesh while its local shell is unavailable.
- [ ] **DEVICE-32 — Use tiered input passthrough.** Prefer a whole USB controller, fall back to individual USB devices, then software forwarding.
- [ ] **DEVICE-33 — Use tiered VM audio.** PipeWire/virtio audio by default, USB passthrough for low latency, and PCIe audio passthrough for the highest-performance profile.
- [ ] **DEVICE-34 — Give low-latency VM audio vCPUs bounded real-time scheduling.** Protect the host from starvation with explicit limits.
- [ ] **DEVICE-35 — Reserve two vCPUs for audio workloads.** Keep those vCPUs dedicated inside the low-latency VM profile.
- [ ] **DEVICE-36 — Benchmark host CPU topology for audio placement.** Do not hard-code SMT sibling or separate-core placement.
- [ ] **DEVICE-37 — Score candidate audio cores by jitter, DSP throughput, cache locality, and IRQ pressure.** Persist the measured result per host.
- [ ] **DEVICE-38 — Run persistent platform containers through system Podman and Quadlet.** Do not introduce Kubernetes or OpenShift as a local dependency.
- [ ] **DEVICE-39 — Run user containers through the same system Podman + Quadlet model.** Keep one lifecycle and supervision path.
- [ ] **DEVICE-40 — Permit images from any OCI-compatible registry.** Do not restrict pulls to Red Hat or an allowlist.

## Epic 5 — Container runtime, networking, storage, backup, and updates

- [ ] **CTR-41 — Do not require image scanning before first run.** Images may start immediately.
- [ ] **CTR-42 — Use Podman bridge networking by default.** Publish inbound ports explicitly.
- [ ] **CTR-43 — Give each host a routed container subnet over Nebula.** Containers ride the host tunnel rather than joining Nebula directly.
- [ ] **CTR-44 — Allocate a /24 container subnet per host.** Manage it separately from the VM subnet.
- [ ] **CTR-45 — Assign fixed persistent-container IPs from fleet state.** Record service ownership and current host.
- [ ] **CTR-46 — Readdress containers after movement.** Allocate from the destination host’s /24 and update DNS/service discovery.
- [ ] **CTR-47 — Use Podman-managed named volumes for persistent container data.** Avoid bespoke volume formats by default.
- [ ] **CTR-48 — Mesh-replicate selected container-volume backups.** Keep backup state and outcomes visible through the existing platform workspaces.
- [ ] **CTR-49 — Select backup destinations automatically.** Choose a healthy peer with sufficient capacity.
- [ ] **CTR-50 — Back up selected container volumes before updates or major changes.** Do not use a fixed hourly or daily schedule.

## Epic 6 — Container update validation and rollback

- [ ] **CTR-51 — Implement automatic rolling container updates.** Back up selected state, pull, restart, validate, and roll back on failure.
- [ ] **CTR-52 — Treat OCI health checks as the primary success gate.** Respect the image-defined check where present.
- [ ] **CTR-53 — Treat sustained systemd/Quadlet activity as success when no OCI health check exists.** Do not require a custom probe.
- [ ] **CTR-54 — Use a two-minute stability window for containers without health checks.** Reset the timer after process restarts.
- [ ] **CTR-55 — Roll back only the container image.** Do not automatically restore persistent volume data after a failed update.
- [ ] **GUEST-56 — Orchestrate supported guest updates from the host.** Track progress and completion in existing management workspaces.
- [ ] **GUEST-57 — Use Red Hat Insights and Remote Host Configuration for supported RHEL-compatible guests.** Keep this as the primary Red Hat guest-management integration.
- [ ] **GUEST-58 — Use Ansible over the routed VM network for Windows, Fedora, and Ubuntu.** Invoke each guest’s native update mechanism.
- [ ] **GUEST-59 — Authenticate with SSH keys on Linux and WinRM certificates on Windows.** Provision credentials per VM.
- [ ] **GUEST-60 — Detect guest updates and require user approval.** Do not apply them silently.

## Epic 7 — Golden images and local VM provisioning

- [ ] **IMG-61 — Maintain one current golden image per OS and archive the previous image.** Avoid an unbounded template catalog.
- [ ] **IMG-62 — Build all golden images with Packer and Ansible.** Use one cross-platform build pipeline.
- [ ] **IMG-63 — Store the authoritative image library on a central image server.** Hosts maintain local caches.
- [ ] **IMG-64 — Allow VM creation from cached images when the image server is unavailable.** Block only uncached image pulls and refreshes.
- [ ] **IMG-65 — Use S3-compatible object storage for the image library.** Store it in the same bucket used by the music server.
- [ ] **IMG-66 — Separate bucket data with top-level prefixes and policies.** At minimum use `music/` and `vm-images/` with separate credentials.
- [ ] **IMG-67 — Verify golden images with SHA-256 before cache import or launch.** Reject mismatches.
- [ ] **IMG-68 — Import each base image once into the host thin pool.** Create VM disks as LVM thin snapshots for rapid provisioning.
- [ ] **IMG-69 — Flatten VM disks only before export or migration.** Retain thin-snapshot efficiency while the VM remains local.
- [ ] **MIG-70 — Make live migration a supported goal.** Use standard libvirt/QEMU migration rather than a custom hypervisor protocol.

## Epic 8 — Migration and hardware compatibility

- [ ] **MIG-71 — Select migration mode using measured link quality.** Use pre-copy block migration on fast stable mesh links and cold migration when thresholds are not met.
- [ ] **MIG-72 — Remove passthrough devices before migration.** Replace GPU, USB, PCI, and audio devices with virtual hardware.
- [ ] **MIG-73 — Install virtio and expected passthrough drivers in every applicable template.** Guests must remain bootable after hardware substitution.
- [ ] **MIG-74 — Use a common named CPU baseline for migratable VMs.** Do not expose host-passthrough CPU features to those VMs.
- [ ] **MIG-75 — Select a stable Red Hat-recommended QEMU/libvirt CPU model.** Record and validate the model in fleet policy.
- [ ] **MIG-76 — Mark hosts that cannot support the baseline as local-only VM hosts.** Their VMs are non-migratable.
- [ ] **MIG-77 — Keep VM vCPUs, memory, and assigned devices NUMA-local whenever possible.** Make placement NUMA-aware.
- [ ] **MIG-78 — Reduce VM memory instead of spanning NUMA nodes.** Preserve locality.
- [ ] **MIG-79 — Permit at most a 25% automatic memory reduction.** Refuse startup if more reduction would be required.
- [ ] **MIG-80 — Use virtio-blk with direct I/O for managed VM disks.** Optimize for low overhead and local performance.

## Epic 9 — Disk I/O, shutdown, crash recovery, permissions, and desired state

- [ ] **OPS-81 — Use `cache=none` with io_uring.** Validate kernel, QEMU, and storage support during host qualification.
- [ ] **OPS-82 — Enable continuous guest discard/TRIM.** Return unused blocks to the thin pool.
- [ ] **OPS-83 — Pause discard during heavy VM storage activity.** Resume reclamation when load falls.
- [ ] **OPS-84 — Rely on guest journaling, QEMU flush semantics, and storage recovery.** Do not make UPS integration mandatory.
- [ ] **OPS-85 — Gracefully shut down all running VMs before host shutdown.** Block host poweroff while the grace period is active.
- [ ] **OPS-86 — Force power-off after a two-minute guest shutdown timeout.** Audit the forced action.
- [ ] **OPS-87 — Automatically restart all VMs that were running before an unexpected host crash.** Persist the prior running set.
- [ ] **OPS-88 — Restart those VMs concurrently.** Do not stagger recovery.
- [ ] **OPS-89 — Permit any authenticated mesh user to manage any VM.** Preserve the current flat workgroup authorization model.
- [ ] **OPS-90 — Require typed VM-name confirmation for deletion.** Prevent accidental destructive actions.

## Epic 10 — Reconciliation, host updates, observability, and hard cutover

- [ ] **GOV-91 — Store VM definitions and policy as fleet-managed declarative YAML or TOML.** Treat that state as authoritative.
- [ ] **GOV-92 — Reconcile approved non-disruptive VM changes immediately.** Generate and apply host libvirt configuration from fleet state.
- [ ] **GOV-93 — Stage restart-required changes and wait for user approval.** Do not reboot guests automatically for configuration changes.
- [ ] **GOV-94 — Apply automatic transactional bootc/OSTree host updates.** Reboot when required and preserve the previous deployment.
- [ ] **GOV-95 — Require full post-update functional validation.** Validate core services, networking, storage, Nebula, mackesd, libvirt, Podman, the DRM shell, a test VM, and a test container.
- [ ] **GOV-96 — Roll back immediately when post-update validation fails.** Boot the previous deployment without leaving the failed image active.
- [ ] **GOV-97 — Present health and management through the existing project workspaces.** Do not create a redundant observability dashboard.
- [ ] **GOV-98 — Use the platform’s existing alerting methods.** Preserve severity handling, audit records, alert hooks, and the established notification/chat path.
- [ ] **GOV-99 — Remove OpenStack completely.** Delete workers, verbs, UI, packages, tests, configuration, deployment logic, and active documentation; do not retain disabled feature flags.
- [ ] **GOV-100 — Perform the hard cutover on `master`.** Remove OpenStack first, then rebuild the direct libvirt, LVM-thin, VM-networking, graphics, Podman, Quadlet, Ansible, Packer, fleet, update, alerting, and recovery paths.

## Required implementation order

1. Remove OpenStack and make the workspace compile without any OpenStack dependency or dead reference.
2. Restore direct libvirt/QEMU-KVM lifecycle verbs and local VM template creation.
3. Provision LVM thin-pool base images and snapshot-backed VM disks.
4. Build the two-NIC VM network path and fleet-allocated VM/container subnets.
5. Deliver local virtio-gpu/DMA-BUF presentation into the DRM-native egui shell.
6. Deliver system Podman + Quadlet lifecycle, networking, fixed addressing, update, backup, and rollback.
7. Reconnect definitions, health, actions, alerts, audit, and DNS to the existing workspaces and platform services.
8. Add passthrough, real-time audio, live migration, image distribution, guest update orchestration, transactional host validation, and automatic recovery.

## Definition of done

The epic is complete only when a clean installed workstation can, without OpenStack:

- boot the direct DRM/KMS egui shell;
- create a VM from an S3-hosted, SHA-256-verified golden image cached into an LVM thin pool;
- launch that VM through libvirt/QEMU-KVM with pinned CPU, fixed memory, NUMA-local placement, virtio-blk, io_uring, dual networking, and fleet-managed addressing;
- display the local VM through the direct virtio-gpu/DMA-BUF shell path;
- run and update system-managed Podman/Quadlet containers on routed fleet-managed addressing;
- manage guest and host updates through the selected Red Hat/Ansible/bootc paths;
- expose all lifecycle, health, alerts, and audit state through existing MCNF workspaces and platform alerting;
- survive update failure, host crash, guest shutdown timeout, image-server loss, and mesh-link degradation with the policies above;
- pass CI, hardware-seat validation, clean-install validation, and a repository-wide audit proving no active OpenStack code or documentation remains.
