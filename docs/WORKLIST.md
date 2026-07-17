# MCNF Worklist — Local-First Red Hat Virtualization and Containers

**Status:** ACTIVE / HARD CUTOVER  
**Decision date:** 2026-07-16  
**Supersedes:** QUASAR-CLOUD and all OpenStack-based VM, storage, networking, identity, image, orchestration, packaging, test, documentation, and UI paths.  
**Primary product:** a local workstation operating system with near-native local VMs, direct DRM/KMS shell integration, standard Red Hat virtualization and container components, and mesh-connected management.

## Governing outcome

A workstation must boot into the MCNF shell, create a local VM from an LVM thin snapshot, run it through libvirt/QEMU-KVM with pinned CPU and fixed memory, and display it through a direct virtio-gpu/DMA-BUF path with performance close to native. OpenStack must not be required for local VM, container, image, networking, update, or lifecycle operations.

## Epic 1 — Remove OpenStack and restore a local-first boundary

- [ ] **