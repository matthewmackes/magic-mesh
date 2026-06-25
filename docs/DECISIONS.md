# Decision Log (ADR-style, append-only)

Every change to a governance lock (`AI_GOVERNANCE.md` §0–§10) — and any other reopened
design lock — requires an entry here: the **symptom** that justified reopening, the
**superseding decision**, and the **date**. Newest wins (§10). Append only — never edit or
delete a prior entry; supersede it with a newer one.

---

## ADR-0005 — compute inventory bus publish: on-change + slow heartbeat (2026-06-25)

- **Supersedes:** the VIRT-1 (`v5.0.0-compute.md` §1/§3) lock that `compute_registry`
  **publishes the inventory snapshot to `compute/inventory/<peer>` every 10 s tick**.
- **Symptom:** That every-tick publish predates the cross-node transport split. Today the
  `compute/inventory/<peer>` bus topic has exactly **one** consumer — *this* node's own
  Workloads source (`ipc::apps::read_local_inventory`, wired in `mackesd.rs`). Peers read
  the fleet-wide view from the **replicated `compute-inventory.json`** file on the
  QNM-Shared plane (`mde-workbench` compute panel, `probe_nmap`, `ipc::apps`), *not* from
  the bus — there is no federation subscriber, so the bus topic is per-node by design. The
  consumer only ever wants the **latest** doc, yet the worker republished a byte-identical
  body every 10 s, appending **~8 640 redundant messages per peer per day at idle** to the
  append-only Persist log (BUS-1.9 retention then has to prune them).
- **Decision:** Keep the **10 s poll** cadence — VIRT-21 state-transition events and the
  replicated `compute-inventory.json` file must stay timely, and those are cheap local
  ops. **Change only the bus publish** to **publish-on-change plus a 60 s heartbeat**: the
  body is serialized once per tick and published when it differs from the last published
  body, or when ≥60 s have elapsed since the last publish (so a freshly-pruned topic or a
  late subscriber still finds a recent doc). First tick always publishes. A running VM's
  live `cpu_pct` delta naturally keeps an *active* node publishing each tick, while an
  *idle* fleet (the common case) goes quiet between heartbeats.
- **Scope:** `crates/mesh/mackesd` `compute_registry` worker only. The bus body shape and
  the QNM-Shared transport are unchanged, so every consumer is untouched. Cadence policy is
  a pure, unit-tested helper (`should_publish`) so it stays verifiable.
