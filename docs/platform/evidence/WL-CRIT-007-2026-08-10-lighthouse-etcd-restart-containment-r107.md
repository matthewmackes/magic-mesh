# WL-CRIT-007 — lighthouse etcd restart-amplification containment

Date: 2026-08-10

## Failure established

Read-only inspection of the three DigitalOcean lighthouse members found only
444 MiB usable RAM per member, approximately 99% CPU pressure, 92–97% memory
pressure, and substantial I/O pressure. Members `.2` and `.3` had roughly
85–90 MiB of etcd memory swapped out. Lighthouse `.1` also had a 75 MB etcd
database, a full `/run`, and 80%-used root storage.

On `.1`, timed-out proposal probes caused the minute health watchdog to restart
etcd while the prior stop/start transaction was still unresolved. The stop then
exceeded its timeout, systemd's watchdog aborted etcd, and the resource-starved
member restarted slowly from its database. `.2` and `.3` remained active but
showed multi-second ReadIndex delays, leader loss/elections, and raft timeouts.
The observed failure is resource starvation amplified by watchdog restarts, not
a certificate or basic Nebula-overlay failure.

The diagnosis used identity/access checks, systemd unit/environment comparison,
bounded endpoint status/member reads, journals, raft and socket state,
filesystem/database metadata, cgroup limits, memory/swap use, PSI pressure, and
watchdog history. No lighthouse was mutated. A write-bearing `endpoint health`
probe was deliberately not used during diagnosis.

## Corrected-forward policy

`install-helpers/mesh-health-check.sh` no longer treats failed etcd proposal
probes as unconditional restart authority for a configured local voter. Before
one recovery attempt it now requires all of these facts:

- `etcd.service` is in a stable `active`, `inactive`, or `failed` state; an
  `activating`, `deactivating`, `reloading`, unknown, or unreadable state is a
  hard refusal;
- CPU and memory PSI `some.avg10` values are available, valid, and below the
  80% severe-pressure boundary;
- a strict majority of distinct configured endpoints answers the read-only
  `endpoint status` verb with distinct member IDs and one matching nonzero
  cluster ID, leader ID, and Raft term; and
- no watchdog etcd restart was recorded during the preceding 1,800 seconds.

The restart stamp is written before invoking systemd, so a failed or timed-out
restart cannot be amplified on the next minute tick. If `/run` cannot persist
that stamp, recovery is refused. A successful proposal probe clears the stamp.
Every refusal leaves health degraded and emits an
actionable journal status naming the transition, pressure readings, visible
quorum count, or remaining cooldown and the corresponding operator check.
Client-only workstations retain the existing no-local-restart behavior.

This preserves recovery for a stably failed or wedged member only when remote
majority visibility and host headroom make that action safe. It fails closed
when those preconditions cannot be established.

## Deterministic hostile verification

Farm machine 9 (`172.20.0.50`), isolated slot
`crit007-etcd-containment-r107`:

```text
./install-helpers/test-mesh-health-etcd-containment.sh
mesh-health etcd containment hostile regression: passed
```

The fixture proves:

- activating and deactivating members receive zero restart attempts;
- unavailable PSI telemetry receives zero restart attempts;
- 99% CPU / 92% memory pressure suppresses recovery before any additional
  read-only status requests;
- one responder in a three-member configuration is insufficient restart
  authority;
- two responders that disagree on consensus identity are insufficient restart
  authority;
- an unwritable restart stamp receives zero restart attempts;
- two responders plus bounded pressure admit exactly one recovery attempt;
- a repeated timer pass cannot restart again during the 300-second fixture
  cooldown, while expiry permits one later bounded attempt; and
- a restored successful commit probe clears the containment stamp and returns
  health to `ok`.

Local `bash -n` and `git diff --check` also passed for the exact helper and
fixture. No live lighthouse mutation, restart, package installation, or probe
that commits an etcd proposal was performed for this implementation.

## Residual operations

The patch contains restart amplification; it does not create capacity. The
three 444 MiB lighthouses still need a separately governed capacity/storage
correction and coordinated live acceptance. Until this helper is delivered in
a signed package, installed watchdogs do not carry this containment policy.
