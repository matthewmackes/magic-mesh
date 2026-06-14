# Substrate-correctness audit (SUBAUDIT) — 2026-06-14

Operator question (2026-06-14): *"How were all of these items misconfiguration,
and what should we evaluate to see if other services are misconfiguration or
built?"* — triggered by a run of live-discovered empty panels on .13 (the
bug-discovery workstation): .mesh DNS, Message Bus, Discovered Hosts, Hardware,
https-tunnel, plus the NAV-1 Desktop-in-sidebar regression.

## Finding: none were "unbuilt" — all four are deployed-substrate mismatches

Every feature compiles + has green tests. They failed because tests are
**tempdir-bound** (seed a fake directory/store), so they validate code logic,
not the real deployed substrate. The §7 "runtime-reachable" gate checks for a
code path, not whether it works against the real mount / store / deps / node
role. See [[audit-gap-infra-preconditions]].

### Class A — wrong data source (reads the empty sqlite roster/`nodes`)

The local SQLite roster + `nodes` table are **empty mesh-wide** — enrollment
writes the replicated QNM-Shared `peers/` directory, not sqlite. Code that read
sqlite showed empty in production.

- Confirmed/fixed: `nodes list` + `fleet-status` (→ directory), `mesh_dns` worker
  (→ directory).
- **To audit** — `export_roster` consumers: `bin/mackesd.rs`, `ipc/jobs.rs`,
  `workers/netassess.rs`, `workers/netstate_apply.rs`, `workers/validation_suite.rs`,
  `ipc/directory.rs` (the one legit producer). `list_nodes` consumers:
  `health.rs`, `ipc/nebula.rs`, `ipc/files.rs`, `workers/health_reconciler.rs`,
  `workers/mesh_latency.rs`, `workers/metrics_exporter.rs`, `workers/peer_cap.rs`,
  `bin/mackesd.rs`. Each: is sqlite populated for this path in prod, or switch to
  the directory?

### Class B — path/root mismatch (ignores the env-pinned shared mount)

Code assumed a path ≠ the deployed shared mount: per-HOME bus vs `/run/mde-bus`,
`/mnt/mesh-storage` vs the real mount, phantom `topics/`.

- Confirmed/fixed: workgroup-root single-source, Message Bus (`bus_root` →
  `mde_bus::default_data_dir()` + walk root).
- **To audit** — per-HOME bus paths still in `panels/mesh_federation.rs`,
  `panels/vm_wizard.rs`, `mde-files/{trash,desktop}.rs`. Confirm each honors
  `MDE_BUS_ROOT`.

### Class C — missing runtime dependency (helper binary absent → silent empty)

Workers shell helper binaries; only `nebula` is a hard RPM `Requires`. Absent
binaries degrade to empty with no operator signal.

- Confirmed: **Discovered Hosts** — `nmap` not declared (journal: "could not
  spawn nmap … Requires: nmap in the RPM").
- **To fix** — declare (Requires or Recommends, matching node role) +
  honest-degrade UI: `nmap`, `resolvectl`/systemd-resolved, `firewalld`
  (firewall-cmd), `NetworkManager` (nmcli), `podman`, `lizardfs-client`
  (mfsmount), `rsync`. Already declared: nebula, ansible-core, kamailio,
  rtpengine, libvirt, openssh-clients.

### Class D — gated precondition + phantom-unit panel check

In-process features gated on a precondition, reported by the panel as a
non-existent systemd unit.

- **https-tunnel** — the `:443` `NebulaHttpsListener` runs in-process in mackesd,
  only when a relay cert exists (`/etc/nebula/lighthouse.crt`); skipped on a
  peer/workstation. Mesh Services checks a systemd unit `mackes-nebula-https-tunnel`
  that never existed → "NOT INSTALLED". Operator wants it **on by default on all
  installs + tested**. Fix: self-bootstrap a per-node TLS cert so the listener
  runs everywhere (fp-pinned), or report the in-process capability honestly; and
  correct the Mesh Services check for in-process workers.
- **nebula-lighthouse** — same phantom-unit pattern.

## Go-forward evaluation (run as an audit, not click-by-click)

1. Data-source audit — the 15 sqlite consumers above.
2. Path/root audit — the 3 per-HOME stragglers.
3. Dependency audit — declare the ~7 helper bins; panels degrade honestly.
4. Panel↔reality audit — every Mesh Services unit + panel check vs how it runs.
5. Live end-to-end probe — run each panel's command on .13 + a droplet; diff.
