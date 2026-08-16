//! E1.2 — role-gated worker subsets.
//!
//! Each `mackesd` worker is tiered to the **minimum deployment role rank** that
//! runs it (`Lighthouse ⊂ Workstation`). `run_serve` resolves the box's rank
//! once via [`resolve_rank`] and gates every `sup.spawn` with [`runs`], so a
//! Lighthouse never starts the fleet/media/voice/desktop workers.
//!
//! **Interpretation (E1.2):** a Lighthouse IS a VPS relay, so it runs Nebula +
//! mde-bus + mesh routing + leader + health. Over-tiering a relay-essential
//! worker would break routing, so the mesh/control plane sits at rank 0; every
//! fleet + voice/media + desktop worker sits at rank 1 (Workstation — a headless
//! box is a Workstation too, its desktop workers idle without a local display).

#[cfg(feature = "async-services")]
pub use crate::workers::RestartPolicy;
#[cfg(not(feature = "async-services"))]
/// Lean-build copy of the worker restart-policy tags used by the role registry.
///
/// The actual supervisor type lives in `workers`, which is available only with
/// `async-services`. The registry remains useful in lean library builds, so the
/// tag shape is mirrored here without pulling in the worker pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Don't restart after the worker returns.
    Never,
    /// Restart only after a failed return.
    OnFailure,
    /// Restart after any return.
    Always,
}
use mackes_mesh_types::worker_runtime as runtime;
use mde_role::{Capability, Role, RoleClass};

const MIB: u64 = 1024 * 1024;

/// WL-ARCH-009 — the six independently supervised process groups.
///
/// This enum is the process-isolation boundary: every registered worker owns
/// exactly one value, and the values map one-to-one to the governed systemd
/// service names. Keeping the mapping typed prevents ad-hoc group strings from
/// drifting between the registry, status projection, and future entrypoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkerGroup {
    /// Coordination, shared configuration, and persistent-state authority.
    Control,
    /// Read-only probes, health reconciliation, and telemetry projection.
    Observation,
    /// Privileged or operator-requested mutation executors.
    Actions,
    /// Replication, durable collections, and bounded data movement.
    Data,
    /// Workload placement, VM/container lifecycle, and VDI brokering.
    Compute,
    /// Optional LAN, device, media, and external-provider adapters.
    Integrations,
}

impl WorkerGroup {
    /// Stable order used by process launchers and status renderers.
    pub const ALL: [Self; 6] = [
        Self::Control,
        Self::Observation,
        Self::Actions,
        Self::Data,
        Self::Compute,
        Self::Integrations,
    ];

    /// Stable wire/config name for this group.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Observation => "observation",
            Self::Actions => "actions",
            Self::Data => "data",
            Self::Compute => "compute",
            Self::Integrations => "integrations",
        }
    }

    /// Governed systemd service that will own this group at the hard cut.
    #[must_use]
    pub const fn service_name(self) -> &'static str {
        match self {
            Self::Control => "mackesd-control.service",
            Self::Observation => "mackesd-observation.service",
            Self::Actions => "mackesd-actions.service",
            Self::Data => "mackesd-data.service",
            Self::Compute => "mackesd-compute.service",
            Self::Integrations => "mackesd-integrations.service",
        }
    }

    /// Typed state namespace owned by the group. A worker's stable `name` is
    /// appended by the runtime projection.
    #[must_use]
    pub const fn state_topic_prefix(self) -> &'static str {
        match self {
            Self::Control => "state/mackesd/control/workers",
            Self::Observation => "state/mackesd/observation/workers",
            Self::Actions => "state/mackesd/actions/workers",
            Self::Data => "state/mackesd/data/workers",
            Self::Compute => "state/mackesd/compute/workers",
            Self::Integrations => "state/mackesd/integrations/workers",
        }
    }

    /// Typed health-key prefix owned by the group.
    #[must_use]
    pub const fn health_key_prefix(self) -> &'static str {
        match self {
            Self::Control => "mackesd.control",
            Self::Observation => "mackesd.observation",
            Self::Actions => "mackesd.actions",
            Self::Data => "mackesd.data",
            Self::Compute => "mackesd.compute",
            Self::Integrations => "mackesd.integrations",
        }
    }

    /// Typed action namespace owned by the group.
    #[must_use]
    pub const fn action_namespace(self) -> &'static str {
        match self {
            Self::Control => "action/mackesd/control",
            Self::Observation => "action/mackesd/observation",
            Self::Actions => "action/mackesd/actions",
            Self::Data => "action/mackesd/data",
            Self::Compute => "action/mackesd/compute",
            Self::Integrations => "action/mackesd/integrations",
        }
    }

    const fn defaults(self) -> GroupDefaults {
        match self {
            Self::Control => GroupDefaults {
                criticality: Criticality::Essential,
                cadence: CadencePolicy::Continuous,
                queue: QueuePolicy::Bounded {
                    max_items: 256,
                    max_bytes: 2 * MIB,
                    overflow: QueueOverflow::RejectNew,
                },
                cache: CachePolicy::Bounded {
                    max_items: 256,
                    max_bytes: 4 * MIB,
                    ttl_secs: 300,
                },
                resources: ResourceBudget {
                    memory_high_bytes: 96 * MIB,
                    memory_max_bytes: 128 * MIB,
                    cpu_millis_per_second: 250,
                    max_tasks: 16,
                },
                cleanup: CleanupPolicy {
                    owner: CleanupOwner::GroupSupervisor,
                    grace_secs: 10,
                    pending: PendingWorkPolicy::Reject,
                },
            },
            Self::Observation => GroupDefaults {
                criticality: Criticality::Important,
                cadence: CadencePolicy::Periodic {
                    min_interval_secs: 5,
                    max_interval_secs: 300,
                },
                queue: QueuePolicy::Bounded {
                    max_items: 32,
                    max_bytes: MIB,
                    overflow: QueueOverflow::LatestWins,
                },
                cache: CachePolicy::Bounded {
                    max_items: 1_024,
                    max_bytes: 16 * MIB,
                    ttl_secs: 300,
                },
                resources: ResourceBudget {
                    memory_high_bytes: 64 * MIB,
                    memory_max_bytes: 96 * MIB,
                    cpu_millis_per_second: 150,
                    max_tasks: 12,
                },
                cleanup: CleanupPolicy {
                    owner: CleanupOwner::Worker,
                    grace_secs: 5,
                    pending: PendingWorkPolicy::Discard,
                },
            },
            Self::Actions => GroupDefaults {
                criticality: Criticality::Essential,
                cadence: CadencePolicy::OnDemand,
                queue: QueuePolicy::Bounded {
                    max_items: 128,
                    max_bytes: 2 * MIB,
                    overflow: QueueOverflow::RejectNew,
                },
                cache: CachePolicy::Disabled,
                resources: ResourceBudget {
                    memory_high_bytes: 128 * MIB,
                    memory_max_bytes: 192 * MIB,
                    cpu_millis_per_second: 500,
                    max_tasks: 32,
                },
                cleanup: CleanupPolicy {
                    owner: CleanupOwner::Worker,
                    grace_secs: 30,
                    pending: PendingWorkPolicy::Reject,
                },
            },
            Self::Data => GroupDefaults {
                criticality: Criticality::Important,
                cadence: CadencePolicy::EventDriven,
                queue: QueuePolicy::Bounded {
                    max_items: 512,
                    max_bytes: 8 * MIB,
                    overflow: QueueOverflow::RejectNew,
                },
                cache: CachePolicy::Bounded {
                    max_items: 4_096,
                    max_bytes: 64 * MIB,
                    ttl_secs: 86_400,
                },
                resources: ResourceBudget {
                    memory_high_bytes: 128 * MIB,
                    memory_max_bytes: 192 * MIB,
                    cpu_millis_per_second: 250,
                    max_tasks: 24,
                },
                cleanup: CleanupPolicy {
                    owner: CleanupOwner::Worker,
                    grace_secs: 30,
                    pending: PendingWorkPolicy::Drain,
                },
            },
            Self::Compute => GroupDefaults {
                criticality: Criticality::Important,
                cadence: CadencePolicy::OnDemand,
                queue: QueuePolicy::Bounded {
                    max_items: 64,
                    max_bytes: 2 * MIB,
                    overflow: QueueOverflow::RejectNew,
                },
                cache: CachePolicy::Bounded {
                    max_items: 256,
                    max_bytes: 16 * MIB,
                    ttl_secs: 900,
                },
                resources: ResourceBudget {
                    memory_high_bytes: 384 * MIB,
                    memory_max_bytes: 512 * MIB,
                    cpu_millis_per_second: 800,
                    max_tasks: 64,
                },
                cleanup: CleanupPolicy {
                    owner: CleanupOwner::GroupSupervisor,
                    grace_secs: 60,
                    pending: PendingWorkPolicy::Reject,
                },
            },
            Self::Integrations => GroupDefaults {
                criticality: Criticality::Optional,
                cadence: CadencePolicy::Periodic {
                    min_interval_secs: 15,
                    max_interval_secs: 900,
                },
                queue: QueuePolicy::Bounded {
                    max_items: 16,
                    max_bytes: 2 * MIB,
                    overflow: QueueOverflow::LatestWins,
                },
                cache: CachePolicy::Bounded {
                    max_items: 1_024,
                    max_bytes: 32 * MIB,
                    ttl_secs: 900,
                },
                resources: ResourceBudget {
                    memory_high_bytes: 128 * MIB,
                    memory_max_bytes: 192 * MIB,
                    cpu_millis_per_second: 300,
                    max_tasks: 24,
                },
                cleanup: CleanupPolicy {
                    owner: CleanupOwner::Worker,
                    grace_secs: 15,
                    pending: PendingWorkPolicy::Discard,
                },
            },
        }
    }

    /// Parse the exact process-group token used by `mackesd serve --group`
    /// and the six governed systemd entrypoints.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "control" => Ok(Self::Control),
            "observation" => Ok(Self::Observation),
            "actions" => Ok(Self::Actions),
            "data" => Ok(Self::Data),
            "compute" => Ok(Self::Compute),
            "integrations" => Ok(Self::Integrations),
            _ => Err(format!(
                "unknown worker group {value:?}; expected control, observation, actions, data, compute, or integrations"
            )),
        }
    }
}

impl std::fmt::Display for WorkerGroup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for WorkerGroup {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Operational importance used for restart-storm and degraded-mode decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criticality {
    /// Required for safe baseline operation; failure degrades the node.
    Essential,
    /// Product function remains available in a clearly degraded state.
    Important,
    /// Optional provider/function may remain unavailable without harming core.
    Optional,
}

/// Capability part of the activation predicate. `AnyNode` is explicit rather
/// than an absent value so every registry row has a complete predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityPredicate {
    /// No capability tag beyond the role-rank gate is required.
    AnyNode,
    /// Activation requires the named, resolved role capability.
    Requires(Capability),
}

/// Configuration part of the activation predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPredicate {
    /// No optional configuration is required.
    Always,
    /// The named environment setting must be present and non-empty.
    EnvironmentPresent(&'static str),
    /// Enabled by default; the named setting can explicitly disable the worker.
    EnvironmentUnlessFalse(&'static str),
    /// A typed runtime/provider record must be available before activation.
    RuntimeAvailable(&'static str),
}

/// Complete capability + configuration activation gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationPolicy {
    /// Capability-tag requirement.
    pub capability: CapabilityPredicate,
    /// Effective-configuration/provider requirement.
    pub config: ConfigPredicate,
}

/// How work reaches a long-running worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CadencePolicy {
    /// Long-lived listener or supervisor loop.
    Continuous,
    /// Work is triggered by a typed bus/file/provider event.
    EventDriven,
    /// Work starts only after an admitted command.
    OnDemand,
    /// Polling cadence constrained to this inclusive range.
    Periodic {
        /// Fastest permitted interval.
        min_interval_secs: u32,
        /// Slowest permitted normal interval, excluding failure backoff.
        max_interval_secs: u32,
    },
}

/// Behavior when a bounded queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueOverflow {
    /// Reject the incoming item and return explicit backpressure.
    RejectNew,
    /// Evict the oldest queued item before accepting the new item.
    DropOldest,
    /// Coalesce queued telemetry to the newest value.
    LatestWins,
}

/// Queue bounds. There is deliberately no unbounded variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePolicy {
    /// Worker accepts no queued messages.
    Disabled,
    /// Queue has explicit item/byte ceilings and overflow behavior.
    Bounded {
        /// Maximum queued messages/items.
        max_items: u32,
        /// Maximum aggregate serialized bytes.
        max_bytes: u64,
        /// Behavior at either ceiling.
        overflow: QueueOverflow,
    },
}

/// Cache bounds. There is deliberately no unbounded variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    /// Worker retains no process-local cache.
    Disabled,
    /// Cache has explicit item, byte, and age ceilings.
    Bounded {
        /// Maximum retained entries.
        max_items: u32,
        /// Maximum aggregate retained bytes.
        max_bytes: u64,
        /// Maximum entry age before eviction.
        ttl_secs: u32,
    },
}

/// Per-worker admission budget; group cgroups may impose a tighter aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudget {
    /// Soft memory-pressure threshold used for warning/profile capture.
    pub memory_high_bytes: u64,
    /// Hard per-worker admission ceiling inside the group budget.
    pub memory_max_bytes: u64,
    /// CPU budget in milliseconds available per wall-clock second.
    pub cpu_millis_per_second: u16,
    /// Maximum worker-owned tasks/threads admitted at once.
    pub max_tasks: u16,
}

/// Component responsible for releasing subscriptions, sockets, child handles,
/// and external leases before the cleanup deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOwner {
    /// The worker must release its own resources before returning.
    Worker,
    /// The group supervisor owns final cancellation and handle reclamation.
    GroupSupervisor,
}

/// What happens to queued work after shutdown begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingWorkPolicy {
    /// Finish already-admitted work within the cleanup grace period.
    Drain,
    /// Reject queued/not-yet-started work with an explicit shutdown result.
    Reject,
    /// Discard replaceable telemetry/provider refresh work.
    Discard,
}

/// Bounded shutdown and cleanup contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupPolicy {
    /// Component accountable for cleanup completion.
    pub owner: CleanupOwner,
    /// Maximum graceful-cleanup interval.
    pub grace_secs: u16,
    /// Treatment of pending work once shutdown starts.
    pub pending: PendingWorkPolicy,
}

/// Typed ownership for the three runtime namespaces and shutdown cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeOwnership {
    /// Group that publishes the worker's state projection.
    pub state: WorkerGroup,
    /// Group that publishes and clears the worker health key.
    pub health: WorkerGroup,
    /// Group that admits typed actions for the worker.
    pub actions: WorkerGroup,
    /// Bounded resource-release responsibility.
    pub cleanup: CleanupPolicy,
}

/// How the production daemon binds a canonical worker registration to its
/// runtime start site. This is part of the registration itself: directly
/// supervised workers and responder threads are not an out-of-band exception
/// to the worker census.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnBinding {
    /// Constructed through `spawn_tiered`, with role gating and policy read from
    /// this registration.
    Tiered,
    /// Constructed directly under the Rust supervisor. The imperative start
    /// site must use the restart policy declared by this registration.
    DirectSupervisor,
    /// Started as a named responder/maintenance thread rather than a
    /// `Supervisor` worker.
    ResponderThread,
    /// Process-local startup/retry/watchdog infrastructure which is not a
    /// `Supervisor` worker but still has exactly one governed group owner.
    ProcessInfrastructure,
    /// Registered by a supervisor helper that returns the worker's runtime
    /// name instead of containing a string-literal spawn call.
    DynamicSupervisor,
}

#[derive(Debug, Clone, Copy)]
struct GroupDefaults {
    criticality: Criticality,
    cadence: CadencePolicy,
    queue: QueuePolicy,
    cache: CachePolicy,
    resources: ResourceBudget,
    cleanup: CleanupPolicy,
}

/// MEDIA-1 — the deployment **class** that gates worker spawns.
///
/// The role rank plus its capability tags. `run_serve` resolves this once and
/// gates every `sup.spawn` through [`runs_in`], so a rank-gated worker checks the
/// tier and a capability-gated worker (the Navidrome media worker — MEDIA-3)
/// additionally requires the matching tag. Keeping rank + tags together is the §9
/// doctrine. Media discovery and client auto-configuration are Workstation-tier
/// behaviors; the legacy hosting capability does not gate them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeployClass {
    /// The role rank (0 lighthouse · 1 workstation).
    pub rank: u8,
    /// Legacy media-hosting marker retained for role-file compatibility. It does
    /// not gate Workstation media discovery or music auto-configuration.
    pub media: bool,
}

impl DeployClass {
    /// A plain rank with no capability tags — the back-compat path for the
    /// rank-only callers (`resolve_rank`).
    #[must_use]
    pub const fn plain(rank: u8) -> Self {
        Self { rank, media: false }
    }

    /// Build the class from a pinned [`RoleClass`].
    #[must_use]
    pub const fn from_role_class(class: &RoleClass) -> Self {
        Self {
            rank: class.role.rank(),
            media: class.is_media_lighthouse(),
        }
    }
}

/// WL-ARCH-004 / WL-ARCH-009 — one declarative runtime contract for each
/// supervised, role-tiered worker. Rank and restart policy remain the live spawn
/// inputs; group, activation, bounded buffers/resources, namespace ownership, and
/// cleanup policy form the first process-isolation contract slice. This single
/// table is the source of truth BOTH the role census AND the `run_serve` spawner
/// derive from, so the two can never drift (the historical BUG-STORAGE-1 / ARCH-5
/// failure mode): [`workers_for_class`] / [`min_rank`] read the gate here, and
/// `spawn_tiered` (bin/mackesd/spawn.rs) reads the policy + gate here for every
/// spawn — the constructor is bound at the spawn site (workers carry
/// heterogeneous, order-sensitive construction), so the entry declares the
/// *what/when/where*, while the site supplies the *how*.
///
/// The census MUST list every role-tiered worker `run_serve` spawns (a unit test
/// pins the count) — a worker missing from the table defaults to rank 0 (runs
/// everywhere), a safe default that never silently drops a worker from a role,
/// but the drift test catches the omission so every tier is a deliberate decision.
#[derive(Debug, Clone, Copy)]
pub struct WorkerSpec {
    /// The worker's stable name — the `worker_names` roster key, the `runs`
    /// gate key, and the `spawn_tiered` registration key.
    pub name: &'static str,
    /// Exact production start-site shape used by the bidirectional drift guard.
    pub spawn_binding: SpawnBinding,
    /// Minimum role rank that runs this worker (0 lighthouse · 1 workstation).
    pub min_rank: u8,
    /// Restart policy the supervisor applies when this worker returns/panics.
    pub policy: RestartPolicy,
    /// Exactly one of the six governed process groups.
    pub group: WorkerGroup,
    /// Importance used for degraded-mode and restart-storm decisions.
    pub criticality: Criticality,
    /// Capability and effective-configuration activation predicate.
    pub activation: ActivationPolicy,
    /// Continuous/event/command/poll cadence contract.
    pub cadence: CadencePolicy,
    /// Explicitly bounded inbound queue policy.
    pub queue: QueuePolicy,
    /// Explicitly bounded retained-cache policy.
    pub cache: CachePolicy,
    /// Per-worker admission budget within the owning group.
    pub resources: ResourceBudget,
    /// State, health, action, and cleanup ownership.
    pub ownership: RuntimeOwnership,
    /// Canonical typed mutations exposed through the Action Console. Empty is
    /// an explicit refusal: the executor may preview no action for this worker.
    pub actions: &'static [WorkerActionSpec],
}

#[derive(Debug, Clone, Copy)]
pub struct WorkerActionSpec {
    pub action: runtime::WorkerAction,
    pub label: &'static str,
    pub arming: runtime::WorkerArmingRequirement,
}

impl WorkerSpec {
    /// A rank-tiered worker registration.
    #[must_use]
    const fn tier(
        name: &'static str,
        min_rank: u8,
        policy: RestartPolicy,
        group: WorkerGroup,
    ) -> Self {
        let defaults = group.defaults();
        Self {
            name,
            spawn_binding: SpawnBinding::Tiered,
            min_rank,
            policy,
            group,
            criticality: defaults.criticality,
            activation: ActivationPolicy {
                capability: CapabilityPredicate::AnyNode,
                config: ConfigPredicate::Always,
            },
            cadence: defaults.cadence,
            queue: defaults.queue,
            cache: defaults.cache,
            resources: defaults.resources,
            ownership: RuntimeOwnership {
                state: group,
                health: group,
                actions: group,
                cleanup: defaults.cleanup,
            },
            actions: &[],
        }
    }

    /// A canonical registration for a worker constructed directly under the
    /// monolithic supervisor during the process-isolation migration.
    #[must_use]
    const fn direct(name: &'static str, policy: RestartPolicy, group: WorkerGroup) -> Self {
        let mut spec = Self::tier(name, 0, policy, group);
        spec.spawn_binding = SpawnBinding::DirectSupervisor;
        spec
    }

    /// A canonical registration for an explicitly named responder or bounded
    /// maintenance thread. `Never` records that the worker supervisor does not
    /// own an independent restart loop for this start site.
    #[must_use]
    const fn responder(name: &'static str, group: WorkerGroup) -> Self {
        let mut spec = Self::tier(name, 0, RestartPolicy::Never, group);
        spec.spawn_binding = SpawnBinding::ResponderThread;
        spec.cadence = CadencePolicy::Continuous;
        spec
    }

    /// A canonical registration for bounded process-local infrastructure.
    #[must_use]
    const fn infrastructure(
        name: &'static str,
        group: WorkerGroup,
        cadence: CadencePolicy,
    ) -> Self {
        let mut spec = Self::tier(name, 0, RestartPolicy::Never, group);
        spec.spawn_binding = SpawnBinding::ProcessInfrastructure;
        spec.cadence = cadence;
        spec
    }

    #[must_use]
    const fn with_spawn_binding(mut self, binding: SpawnBinding) -> Self {
        self.spawn_binding = binding;
        self
    }

    /// Override the group's default activation predicate for an optional
    /// provider without weakening the role/restart behavior.
    #[must_use]
    const fn with_config(mut self, config: ConfigPredicate) -> Self {
        self.activation.config = config;
        self
    }

    /// Override the group's cadence when a long-lived integration is a listener
    /// rather than a polling provider.
    #[must_use]
    const fn with_cadence(mut self, cadence: CadencePolicy) -> Self {
        self.cadence = cadence;
        self
    }
}

const WORKER_REGISTRY: &[WorkerSpec] = &[
    // ── Lighthouse (rank 0) — the relay control plane: Nebula, mde-bus,
    //    mesh routing/discovery, leader, health, security baseline.
    WorkerSpec::tier(
        "nebula_supervisor",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    WorkerSpec::tier(
        "heartbeat",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    WorkerSpec::tier(
        "health_reconciler",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    WorkerSpec::tier(
        "mesh_router",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    WorkerSpec::tier(
        "stun_gather",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    WorkerSpec::tier(
        "mdns_relay",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_cadence(CadencePolicy::Continuous),
    WorkerSpec::tier(
        "mesh_latency",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // MESHMAP-6 — per-link byte-counter collector (nftables accounting on
    // the Nebula iface). A control-plane traffic observer that runs on
    // every node, like mesh_latency.
    WorkerSpec::tier(
        "link-traffic",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    WorkerSpec::tier(
        "mesh_dns",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    WorkerSpec::tier(
        "hardware_probe",
        0,
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::tier(
        "bus_supervisor",
        0,
        RestartPolicy::Always,
        WorkerGroup::Control,
    ),
    WorkerSpec::tier(
        "firewall_preset",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    WorkerSpec::tier(
        "sshd_overlay_bind",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    WorkerSpec::tier(
        "ssh_pubkey_gossip",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    ),
    WorkerSpec::tier(
        "fleet_reconcile",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    WorkerSpec::tier(
        "presence_watch",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    WorkerSpec::tier(
        "etcd_watch",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    )
    .with_cadence(CadencePolicy::Continuous),
    // DEVMGR-8 — the device-control executor: privileged hardware ops
    // (enable/disable, reload module, rescan bus) the Device-Manager surface
    // dispatches to a target node. UNIVERSAL (rank 0); every node can be an
    // action target and drains only its own replicated
    // fleet/device-control/<self> request dir.
    WorkerSpec::tier(
        "device_control",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    // WL-RUN-006 — the router-action executor: the privileged firewall-edit seam
    // the Device-Manager surface dispatches to the node behind a router. UNIVERSAL
    // (rank 0) like device_control — every node can sit behind its own
    // router/firewall and drains ONLY its own replicated `action/router/<self>`
    // dir (typed-confirm + Vyatta commit-confirm auto-revert + hash-chain audit).
    // The live mutation itself is operator-gated (MDE_ROUTER_ACTION_LIVE).
    WorkerSpec::tier(
        "router_action",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    WorkerSpec::tier(
        "reconcile",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    WorkerSpec::tier(
        "netstate_apply",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    WorkerSpec::tier(
        "validation_suite",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    WorkerSpec::tier(
        "metrics_exporter",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // BUG-STORAGE-1 — the E12-20 storage worker: a UNIVERSAL per-node topology
    // mirror (read-only UDisks2 enumerate → `state/storage/<node>`). Pinned at
    // rank 0 so it provably publishes on EVERY role — a Workstation has local
    // disks the seated user manages, and a Lighthouse still publishes an honest
    // (often `backend: Unavailable`) mirror. It previously rode the silent
    // "unknown worker ⇒ rank 0" default, which spawned it at runtime but OMITTED
    // it from this census, so `workers_for_rank` / `mackesd role-workers` wrongly
    // reported the Workstation as NOT running storage. Only the READ/publish path
    // is enabled here; the live UDisks2Executor stays IntegrationGated as-is.
    WorkerSpec::tier(
        "storage",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // EXPLORER-1 — the unit_aggregator worker: the daemon spine of the Hero unit
    // explorer (unit-explorer.md #18). UNIVERSAL (rank 0) like storage: every
    // node folds its OWN unit view (self-first #23) — the mesh mirror it already
    // reads + the union of every node's cloud mirror + its LAN scan — and
    // publishes `state/units/<node>`. There is no leader/center to elect (lock
    // #20: "no center"); a lighthouse publishes an honest units view too. A
    // deliberate rank-0 entry (the BUG-STORAGE-1 lesson), never the silent
    // unknown-worker default.
    WorkerSpec::tier(
        "unit_aggregator",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // WL-FUNC-008 — the service_aggregator worker: the unified service
    // provenance/health view. UNIVERSAL (rank 0) like unit_aggregator/storage
    // — every node folds its OWN mesh-wide merge of the three service
    // sources (published KDC directory + probe inventory + Explorer enrichment) and
    // publishes `state/services/<node>`; there is no center. A deliberate rank-0
    // census entry (the BUG-STORAGE-1 lesson), never the silent unknown-worker default.
    WorkerSpec::tier(
        "service_aggregator",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // CHAT-FIX-2 — the local-notification producer worker: watches this node's
    // OWN event sources (mesh peer join/leave, dnf/platform updates, systemctl
    // --failed, df/SMART, journal WARN+) and publishes typed notifications the
    // Chat surface renders as a timestamped feed + tray badge (the real empty-Chat
    // fix — console-frontdoor.md Q34/46/47). UNIVERSAL (rank 0) like the chat
    // worker it feeds: every node — lighthouse included — has local services /
    // disks / a journal / peers to report on, and its notifications ride the same
    // bus the chat worker folds on every role. A deliberate rank-0 census entry
    // (the BUG-STORAGE-1 lesson), never the silent unknown-worker default.
    WorkerSpec::tier(
        "notify",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // WL-SEC-002 — the federation runtime-enforcement worker. UNIVERSAL (rank 0):
    // a Lighthouse RELAYS cross-mesh traffic so it especially must enforce the
    // cross-mesh boundary (default-deny grant-gated routing + trust-cert lifecycle),
    // and a Workstation enforces its own foreign-mesh ingress too. A deliberate
    // rank-0 census entry (the BUG-STORAGE-1 lesson), never the silent
    // unknown-worker default.
    WorkerSpec::tier(
        "federation_enforcer",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    // The universal System and Mesh Health publisher. Every node contributes
    // typed evidence; workstation rows are roster-folded into the five-seat
    // snapshot, while lighthouse reachability contributes mesh-wide evidence.
    WorkerSpec::tier(
        "node_grade",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // KDC-MESH-3 (kdc-mesh.md #15) — the KDE Connect host is UNIVERSAL (rank 0):
    // it runs on EVERY node incl. lighthouses/headless so the mesh-wide "every
    // node recognizes the phone" (#5) + "all nodes serve the phone at once" (#6)
    // goals actually hold. Safe on a headless/relay node because KDC-MESH-1's
    // transport is overlay-ONLY — it binds 1716 on the Nebula overlay IP, never
    // the public NIC, so `kdc_host` on a lighthouse opens NO public port (the
    // firewall preset opens 1716 on the overlay/trusted zone only; public stays
    // default-deny). Was Workstation-only (rank 1) pre-KDC-MESH-3.
    WorkerSpec::tier(
        "kdc_host",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_cadence(CadencePolicy::Continuous),
    // CHAT-FIX-1 — the mesh chat worker: folds every node's chat/notification
    // traffic off the bus into the Chat surface's feed. UNIVERSAL (rank 0): it
    // ALREADY ran on every node — a lighthouse included — via the silent
    // "unknown worker ⇒ rank 0" default (live-verified on Eagle: boot log
    // `starting worker: chat`), but that default OMITTED it from this census, so
    // `mackesd role-workers` dishonestly failed to list a worker every node runs.
    // A deliberate rank-0 census entry now (the BUG-STORAGE-1 lesson) — same rank
    // it always had, now EXPLICIT + counted. Pairs with `notify` (CHAT-FIX-2), the
    // producer whose events it folds.
    WorkerSpec::tier("chat", 0, RestartPolicy::OnFailure, WorkerGroup::Data),
    // WL-FUNC-011 Phase 2 — the mesh `collab` worker: the live spine that makes the
    // headless mde-collab-core CollabEngine real (drain action/collab/* → sign +
    // project + publish state/collab/* + live events → converge). UNIVERSAL (rank
    // 0) exactly like the `chat` worker it will EVENTUALLY replace (Phase 4; it
    // runs ALONGSIDE chat for now): every node, headless Lighthouse included,
    // participates in the Communications suite. A deliberate rank-0 census entry
    // (the BUG-STORAGE-1 lesson), spawned via spawn_tiered like chat.
    WorkerSpec::tier("collab", 0, RestartPolicy::OnFailure, WorkerGroup::Data),
    // WL-ARCH-001 Phase B — the `cloud` worker: the OpenTofu + Ansible cloud
    // backend that succeeds the deleted OpenStack worker tree. UNIVERSAL (rank 0)
    // like service_aggregator/storage — every node publishes its OWN
    // `state/cloud/<node>` mirror (per-tool backend health + the local libvirt
    // roster, no center); placement-scoped `action/cloud/*` verbs execute only
    // on their explicit node, and live mutations require a short-lived,
    // body-bound, single-use capability. A deliberate rank-0 census entry (the
    // BUG-STORAGE-1 lesson), spawned via spawn_tiered.
    WorkerSpec::tier("cloud", 0, RestartPolicy::OnFailure, WorkerGroup::Compute),
    // Rolling Node — the `vehicle` worker: the workstation-side adapter that
    // SSH/HTTP-polls a mobile Sierra AirLink MG90 gateway and publishes a per-node
    // `state/vehicle/<node>` mirror. UNIVERSAL (rank 0) like `cloud` — every node
    // publishes its own mirror (no center) — but a genuine no-op on the nodes that
    // have no gateway attached (`MDE_VEHICLE_GATEWAY` unset ⇒ the worker idles).
    // A deliberate rank-0 census entry (the BUG-STORAGE-1 lesson), spawned via
    // spawn_tiered.
    WorkerSpec::tier(
        "vehicle",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentPresent("MDE_VEHICLE_GATEWAY")),
    // WL-FUNC-017 S2 — the Workstation weather/map location authority. It is
    // default-on even without a vehicle source because Manual mode and the
    // persisted verified fallback remain usable; a Lighthouse has no Maps seat.
    WorkerSpec::tier(
        "weather_location",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    )
    .with_cadence(CadencePolicy::Periodic {
        min_interval_secs: 1,
        max_interval_secs: 5,
    }),
    // WL-FUNC-017 S3 — daemon-owned official NWS current conditions and
    // effective-location forecast. The provider is keyless and default-on for
    // seated Workstation nodes; all network and projection bounds are internal.
    WorkerSpec::tier(
        "weather_forecast",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_cadence(CadencePolicy::Periodic {
        min_interval_secs: 30,
        max_interval_secs: 10 * 60,
    }),
    // WL-FUNC-017 S4 — keyless official nowCOAST atmospheric map fields.
    WorkerSpec::tier(
        "weather_atmosphere",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_cadence(CadencePolicy::Periodic {
        min_interval_secs: 30,
        max_interval_secs: 10 * 60,
    }),
    // WL-FUNC-017 S6 — node-scoped route and navigation authority. The worker
    // is reachable by default on seated nodes and remains honestly unavailable
    // until an approved production route provider is provisioned.
    WorkerSpec::tier(
        "navigation",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    )
    .with_cadence(CadencePolicy::Periodic {
        min_interval_secs: 1,
        max_interval_secs: 5,
    }),
    // WL-FUNC-022 S2 — local persisted Clock deadline authority. Workstation
    // scoped because this first slice executes local alarms/timers only.
    WorkerSpec::tier(
        "clock",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    )
    .with_cadence(CadencePolicy::Periodic {
        min_interval_secs: 1,
        max_interval_secs: 5,
    }),
    // WL-FUNC-020 S1 — workstation Android catalog trust boundary. The worker
    // remains alive but fail-closed until both local public-key settings exist.
    WorkerSpec::tier(
        "android_catalog",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    )
    .with_config(ConfigPredicate::EnvironmentPresent(
        "MDE_ANDROID_CATALOG_TRUST_KEY_FILE",
    ))
    .with_cadence(CadencePolicy::Periodic {
        min_interval_secs: 1,
        max_interval_secs: 1,
    }),
    // WL-FUNC-018 S2 — workstation Flatpak catalog trust boundary. The worker
    // stays registered but fail-closed until its public trust anchor is present.
    WorkerSpec::tier(
        "app_catalog",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    )
    .with_cadence(CadencePolicy::Periodic {
        min_interval_secs: 1,
        max_interval_secs: 1,
    }),
    // WL-FUNC-012 / MG90 airspace — workstation-side typed scanner mirror.
    // The default worker publishes an explicit no-source state until a proven
    // MG90 survey probe is configured; it never invents a scanner endpoint.
    WorkerSpec::tier(
        "airspace",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentPresent("MDE_VEHICLE_GATEWAY")),
    // WL-FUNC-012 / OVERLAY-10 — keyless USGS earthquake feed adapter.
    // Workstation-tier: external overlay bandwidth stays on the seated adapter
    // host. This zero-cost public feed is default-on and can be explicitly
    // disabled with MDE_OVERLAY_USGS_EARTHQUAKES=0/false/no/off.
    WorkerSpec::tier(
        "earthquake_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentUnlessFalse(
        "MDE_OVERLAY_USGS_EARTHQUAKES",
    )),
    // WL-FUNC-012 / OVERLAY-1 — point-scoped keyless NWS alert adapter with
    // affected-zone geometry fallback. Workstation-tier, explicit opt-in.
    WorkerSpec::tier(
        "nws_alert_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentUnlessFalse(
        "MDE_OVERLAY_NWS_ALERTS",
    )),
    // WL-FUNC-012 / OVERLAY-2 — keyless IEM/NWS animated NEXRAD tiles.
    WorkerSpec::tier(
        "iem_radar_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentUnlessFalse(
        "MDE_OVERLAY_IEM_RADAR",
    )),
    // WL-FUNC-012 / OVERLAY-6 — keyless NIFC WFIGS wildfire perimeters.
    WorkerSpec::tier(
        "wildfire_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentUnlessFalse(
        "MDE_OVERLAY_NIFC_WILDFIRE",
    )),
    // WL-FUNC-012 / OVERLAY-3 — keyless NCDOT TIMS current traffic events.
    WorkerSpec::tier(
        "traffic_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentUnlessFalse(
        "MDE_OVERLAY_NCDOT_TRAFFIC",
    )),
    // WL-FUNC-012 / OVERLAY-7 — credential-gated US EPA AirNow AQI stations.
    // Workstation-tier; a missing sealed key publishes honest unconfigured state.
    WorkerSpec::tier(
        "air_quality_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::RuntimeAvailable("sealed-airnow-api-key")),
    // WL-FUNC-012 / OVERLAY-6 — credential-gated NASA FIRMS hotspot feed.
    // Workstation-tier; a missing sealed key or fresh vehicle fix is explicit.
    WorkerSpec::tier(
        "firms_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::RuntimeAvailable("sealed-firms-api-key")),
    // WL-FUNC-012 / OVERLAY-4 — keyless NWS hourly current/drive-ahead forecast.
    WorkerSpec::tier(
        "nws_forecast_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentUnlessFalse(
        "MDE_OVERLAY_NWS_FORECAST",
    )),
    // WL-FUNC-012 / OVERLAY-8 — point-scoped keyless adsb.lol aircraft feed.
    // Workstation-tier, explicit opt-in, fresh local vehicle fix required.
    WorkerSpec::tier(
        "aircraft_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentUnlessFalse(
        "MDE_OVERLAY_ADSB_LOL",
    )),
    // WL-FUNC-012 / OVERLAY-9 — keyless MBTA GTFS-Realtime transit vehicles.
    WorkerSpec::tier(
        "transit_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentUnlessFalse(
        "MDE_OVERLAY_MBTA_TRANSIT",
    )),
    // WL-FUNC-012 / OVERLAY-5 — keyless Caltrans CWWP2 traffic-camera stills.
    WorkerSpec::tier(
        "caltrans_camera_overlay",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::RuntimeAvailable("caltrans-district")),
    // ── ARCH-5 (drift guard) — universal (rank-0) workers that were spawned in
    //    `run_serve` gated on `worker_role::runs(...)` but OMITTED from this census,
    //    so they silently rode the "unknown worker ⇒ rank 0" default: they DID run
    //    everywhere (correct) but `mackesd role-workers` never listed them — the exact
    //    BUG-STORAGE-1 omission, repeated. The new
    //    `worker_spawns_and_the_census_do_not_drift` reconcile test now REFUSES that
    //    silent default: every `runs(...)`-gated worker must be a deliberate census
    //    entry. Pinned at rank 0 = the rank they already resolved to via the default,
    //    so runtime behavior is UNCHANGED; they are now EXPLICIT + listed. Each spawn
    //    site documents its own "rank-0 / runs-everywhere / universal" intent
    //    (self-marker-gated where relevant).
    // BOOT-STATUS-1 — fabric bring-up snapshot, all roles.
    WorkerSpec::tier(
        "boot_readiness",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // MV-2 — per-node KVM service health, universal virt stack.
    WorkerSpec::tier(
        "kvm_health",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    // WL-ARCH-010 — one journal-backed compute reconciler for VM and Quadlet
    // workloads; Workstation/Server only, never Lighthouse.
    WorkerSpec::tier(
        "workload_compute",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Compute,
    ),
    // MV-5 — placement scheduler (single-actor election), runs everywhere.
    WorkerSpec::tier(
        "scheduler",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Compute,
    ),
    // VDI — session-roster broker, leader-gated internally, runs everywhere.
    WorkerSpec::tier(
        "session_broker",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Compute,
    ),
    // VDI — roaming-session reconciler, runs everywhere.
    WorkerSpec::tier(
        "session_roaming",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    ),
    // Onboarding action engines, leader/address-gated internally.
    WorkerSpec::tier(
        "service_onboard",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    WorkerSpec::tier(
        "spawn_lighthouse_onboard",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    WorkerSpec::tier(
        "onboard_apply",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    // LIGHTHOUSE-8 — per-lighthouse deep-probe lane.
    WorkerSpec::tier(
        "lighthouse_probe",
        0,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    )
    .with_spawn_binding(SpawnBinding::DynamicSupervisor),
    // ── Workstation (rank 1) — everything beyond the relay control plane: the
    //    fleet + mesh storage workers AND voice / clipboard / kdc / remmina /
    //    music. A headless box is a Workstation too (the desktop workers idle
    //    gracefully without a local display).
    WorkerSpec::tier(
        "ansible-pull",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    )
    .with_config(ConfigPredicate::EnvironmentPresent("MDE_ANSIBLE_PULL_URL"))
    .with_cadence(CadencePolicy::Periodic {
        min_interval_secs: 900,
        max_interval_secs: 900,
    }),
    WorkerSpec::tier("app-sync", 1, RestartPolicy::OnFailure, WorkerGroup::Data),
    WorkerSpec::tier(
        "job_exec",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    WorkerSpec::tier(
        "clipboard_sync",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    ),
    // WL-UX-005 — the peer_app_launch executor: drains the shell Front Door's
    // `action/apps/launch` publishes and actually launches the requested app on
    // the target node, allowlisted against that node's own advertised app catalog
    // (never an arbitrary wire command). A desktop feature — you launch apps onto
    // a seat — so Workstation-tier; it idles gracefully on a headless box (no
    // launch requests land) and OnFailure-restarts like the other action executors.
    WorkerSpec::tier(
        "peer_app_launch",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    // BOOKMARKS-2 — the mesh-synced bookmarks worker. A desktop feature (the
    // seated user edits the Bookmarks surface), so Workstation-tier; it idles
    // gracefully on a headless box (no action/bookmarks/* requests) while still
    // replaying peers' Syncthing segments into the shared collection.
    WorkerSpec::tier("bookmarks", 1, RestartPolicy::Always, WorkerGroup::Data),
    // BOOKMARKS-7 — the mesh-wide ad-blocker worker. A desktop feature (it feeds
    // the shared policy engine), so Workstation-tier; it idles gracefully on a
    // headless box (no action/adfilter/* requests)
    // while still replicating peers' filter-store blobs over Syncthing and, when
    // leader, compiling the shared engine blob.
    WorkerSpec::tier("adfilter", 1, RestartPolicy::Always, WorkerGroup::Data),
    // KDC-MESH-6 — phone-as-touchpad/keyboard seat consumer. Drains KDC
    // worker's action/seat/remote-input handoffs and invokes the configured
    // local uinput/seat helper when present. Workstation-tier; idles on
    // headless nodes.
    WorkerSpec::tier(
        "seat_remote_input",
        1,
        RestartPolicy::Always,
        WorkerGroup::Actions,
    )
    .with_config(ConfigPredicate::RuntimeAvailable("seat-input-helper")),
    // FILEMGR-5 — the Files-surface sshfs mesh-mount worker. A desktop feature
    // (the seated user browses peers), so Workstation-tier; it idles gracefully
    // with no mount requests on a headless box.
    WorkerSpec::tier("mesh_mount", 1, RestartPolicy::OnFailure, WorkerGroup::Data),
    // CHOOSER-1 — the desktop-source discovery aggregator behind the Chooser
    // surface. A desktop feature (the seated user picks a desktop to connect
    // to), so Workstation-tier; it idles gracefully on a headless box (the
    // aggregation is cheap and the verbs simply never arrive).
    WorkerSpec::tier(
        "desktop_sources",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    WorkerSpec::tier(
        "remmina-sync",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    ),
    // MEDIA-8 — Workstation music auto-config: a desktop consumer of versioned
    // Media server records published by any participating Media node. It
    // resolves credential refs locally and writes only the seated user's creds.
    WorkerSpec::tier(
        "music_autoconfig",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::RuntimeAvailable("media-server-record")),
    // MEDIA-14 — the mesh media-source discovery aggregator behind the
    // mde-media Sources panel. A desktop feature (the seated user picks a media
    // source to play), so Workstation-tier; it idles gracefully on a headless
    // box (the aggregation is cheap and simply publishes an empty roster).
    WorkerSpec::tier(
        "media_sources",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_cadence(CadencePolicy::EventDriven),
    // MEDIA-15 — the mesh media server + DLNA/UPnP + aggregation (the PRODUCER
    // half MEDIA-14 discovers). A desktop feature (the seated user shares their
    // media folders), so Workstation-tier; it idles gracefully on a headless
    // box (empty share manifest, empty aggregated library).
    WorkerSpec::tier(
        "media_server",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::EnvironmentPresent("MDE_MEDIA_SHARE_DIRS"))
    .with_cadence(CadencePolicy::Continuous),
    // WL-FUNC-014 — the AirSonic/Subsonic gateway proxy responder. A desktop/media
    // gateway feature: it binds the mesh proxy port on a node that has been
    // registered as a LAN AirSonic gateway, resolves the sealed read-only
    // username/password server-side, strips client Subsonic auth, and forwards
    // `/rest/...` without exposing credentials to clients. Workstation-tier like
    // media_sources/media_server; headless workstations can still run it, stock
    // lighthouses do not open the media proxy port.
    WorkerSpec::tier(
        "media_airsonic_proxy",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::RuntimeAvailable("airsonic-gateway-record"))
    .with_cadence(CadencePolicy::Continuous),
    // WL-FUNC-015 — the Jellyfin gateway proxy responder. A desktop/media
    // gateway feature: it binds the mesh proxy port on a node that has been
    // registered as a LAN Jellyfin gateway, resolves the sealed read-only token
    // server-side, and forwards the Jellyfin API without exposing credentials to
    // clients. Workstation-tier like media_sources/media_server; headless
    // workstations can still run it, stock lighthouses do not open the media
    // proxy port.
    WorkerSpec::tier(
        "media_jellyfin_proxy",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Integrations,
    )
    .with_config(ConfigPredicate::RuntimeAvailable("jellyfin-gateway-record"))
    .with_cadence(CadencePolicy::Continuous),
    // TERM-7 — the mesh PTY-broker: opens remote shells on peers over the
    // overlay for the mde-term-egui terminal surface. A desktop feature (the
    // seated user opens a terminal on a mesh node), so Workstation-tier; it
    // idles gracefully on a headless box (no action/pty/* requests arrive).
    WorkerSpec::tier(
        "pty_broker",
        1,
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    // TRANSFERS-1 — the transfers worker: the daemon-owned queue/ledger/verb spine
    // of the Transfers surface (docs/design/transfers-surface.md). A desktop feature
    // fronted by the File Browser (Q1), the sibling of pty_broker/mesh_mount, so
    // Workstation-tier; it idles gracefully on a headless box or a Lighthouse relay
    // (an empty inbox + empty ledger, no transfer.submit verbs arrive). A deliberate
    // census entry (the BUG-STORAGE-1 lesson — a worker absent from the census
    // silently never runs).
    WorkerSpec::tier("transfers", 1, RestartPolicy::OnFailure, WorkerGroup::Data),
    // WL-ARCH-009 — canonical registrations for the workers that the current
    // monolith starts directly while the six process entrypoints are being
    // extracted. These used to live in a test-only NON_TIERED_WORKERS
    // exception list, which meant they had no runtime contract at all.
    WorkerSpec::direct("action", RestartPolicy::Always, WorkerGroup::Actions),
    WorkerSpec::direct(
        "alert_relay",
        RestartPolicy::Always,
        WorkerGroup::Integrations,
    ),
    WorkerSpec::responder("apps_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "apps_installed",
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::direct(
        "apps_running",
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::responder("bus_retention_gc", WorkerGroup::Data),
    WorkerSpec::responder("clipboard_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "compute_expose",
        RestartPolicy::Always,
        WorkerGroup::Compute,
    ),
    WorkerSpec::direct(
        "compute_migrate",
        RestartPolicy::Always,
        WorkerGroup::Compute,
    ),
    WorkerSpec::responder("connect_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "connect_firewall",
        RestartPolicy::OnFailure,
        WorkerGroup::Actions,
    ),
    WorkerSpec::direct("copilot", RestartPolicy::Always, WorkerGroup::Integrations),
    WorkerSpec::direct(
        "cups_sync",
        RestartPolicy::Always,
        WorkerGroup::Integrations,
    ),
    WorkerSpec::direct(
        "datacenter_orchestrator",
        RestartPolicy::Always,
        WorkerGroup::Compute,
    ),
    WorkerSpec::direct("dc_auditor", RestartPolicy::Always, WorkerGroup::Compute),
    WorkerSpec::responder("dc_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct("dc_health", RestartPolicy::Always, WorkerGroup::Observation),
    WorkerSpec::direct("dc_jobs", RestartPolicy::Always, WorkerGroup::Compute),
    WorkerSpec::responder("dc_power_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct("dc_promote", RestartPolicy::Always, WorkerGroup::Compute),
    WorkerSpec::direct(
        "dc_snap_scheduler",
        RestartPolicy::Always,
        WorkerGroup::Compute,
    ),
    WorkerSpec::responder("ddns_bus_responder", WorkerGroup::Actions),
    WorkerSpec::responder("ddns_reconcile", WorkerGroup::Integrations),
    WorkerSpec::responder("directory_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct("dr_scheduler", RestartPolicy::Always, WorkerGroup::Compute),
    WorkerSpec::direct(
        "farm_orchestrator",
        RestartPolicy::Always,
        WorkerGroup::Compute,
    ),
    WorkerSpec::responder("files_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "firewall_monitor",
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::responder("fleet_bus_responder", WorkerGroup::Actions),
    WorkerSpec::responder("host_ops_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "host_state",
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::responder("jobs_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "leader_election",
        RestartPolicy::Always,
        WorkerGroup::Control,
    ),
    WorkerSpec::direct(
        "media_registry",
        RestartPolicy::Always,
        WorkerGroup::Integrations,
    ),
    WorkerSpec::direct("mesh_firewall", RestartPolicy::Always, WorkerGroup::Control),
    WorkerSpec::direct("mirror_syncd", RestartPolicy::Always, WorkerGroup::Data),
    WorkerSpec::direct(
        "navidrome_supervisor",
        RestartPolicy::Always,
        WorkerGroup::Integrations,
    ),
    WorkerSpec::responder("nebula_bus_responder", WorkerGroup::Control),
    WorkerSpec::responder(
        "nebula_control_signal_dispatcher",
        WorkerGroup::Control,
    ),
    WorkerSpec::responder(
        "nebula_observation_signal_dispatcher",
        WorkerGroup::Observation,
    ),
    // WL-ARCH-009 — each isolated process owns one honestly group-scoped
    // supervisor projection. Observation alone folds those six inputs into the
    // node-global Bus/file aggregate.
    WorkerSpec::responder(
        "worker_runtime_status_control_publisher",
        WorkerGroup::Control,
    ),
    WorkerSpec::responder(
        "worker_runtime_status_observation_publisher",
        WorkerGroup::Observation,
    ),
    WorkerSpec::responder(
        "worker_runtime_status_actions_publisher",
        WorkerGroup::Actions,
    ),
    WorkerSpec::responder(
        "worker_runtime_status_data_publisher",
        WorkerGroup::Data,
    ),
    WorkerSpec::responder(
        "worker_runtime_status_compute_publisher",
        WorkerGroup::Compute,
    ),
    WorkerSpec::responder(
        "worker_runtime_status_integrations_publisher",
        WorkerGroup::Integrations,
    ),
    WorkerSpec::responder(
        "worker_runtime_status_aggregate_publisher",
        WorkerGroup::Observation,
    ),
    // WL-ARCH-009 — process-local infrastructure is censused and admitted by
    // one group before it can touch identity state, the network, or systemd.
    WorkerSpec::infrastructure(
        "mesh_service_key_reconciler",
        WorkerGroup::Control,
        CadencePolicy::Periodic {
            min_interval_secs: 30,
            max_interval_secs: 30,
        },
    ),
    WorkerSpec::infrastructure(
        "etcd_startup_probe",
        WorkerGroup::Observation,
        CadencePolicy::OnDemand,
    ),
    WorkerSpec::infrastructure(
        "process_watchdog_control",
        WorkerGroup::Control,
        CadencePolicy::Continuous,
    ),
    WorkerSpec::infrastructure(
        "process_watchdog_observation",
        WorkerGroup::Observation,
        CadencePolicy::Continuous,
    ),
    WorkerSpec::infrastructure(
        "process_watchdog_actions",
        WorkerGroup::Actions,
        CadencePolicy::Continuous,
    ),
    WorkerSpec::infrastructure(
        "process_watchdog_data",
        WorkerGroup::Data,
        CadencePolicy::Continuous,
    ),
    WorkerSpec::infrastructure(
        "process_watchdog_compute",
        WorkerGroup::Compute,
        CadencePolicy::Continuous,
    ),
    WorkerSpec::infrastructure(
        "process_watchdog_integrations",
        WorkerGroup::Integrations,
        CadencePolicy::Continuous,
    ),
    WorkerSpec::direct(
        "nebula_ca_backup",
        RestartPolicy::OnFailure,
        WorkerGroup::Data,
    ),
    WorkerSpec::direct(
        "nebula_csr_watcher",
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    WorkerSpec::direct(
        "nebula_enroll_listener",
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    WorkerSpec::direct(
        "nebula_https_listener",
        RestartPolicy::OnFailure,
        WorkerGroup::Control,
    ),
    WorkerSpec::direct("netassess", RestartPolicy::Always, WorkerGroup::Observation),
    WorkerSpec::direct(
        "netdata_aggregator",
        RestartPolicy::Always,
        WorkerGroup::Integrations,
    ),
    WorkerSpec::direct(
        "peer-cap",
        RestartPolicy::OnFailure,
        WorkerGroup::Observation,
    ),
    WorkerSpec::direct("probe", RestartPolicy::Always, WorkerGroup::Observation),
    WorkerSpec::responder("route_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "router_registry",
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::direct(
        "selinux_monitor",
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::responder("settings_bus_responder", WorkerGroup::Actions),
    WorkerSpec::responder("shell_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "surface_enable",
        RestartPolicy::Always,
        WorkerGroup::Actions,
    ),
    WorkerSpec::direct(
        "surface_firmware",
        RestartPolicy::Always,
        WorkerGroup::Actions,
    ),
    WorkerSpec::direct(
        "surface_verify",
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::direct(
        "surrounding_hosts",
        RestartPolicy::Always,
        WorkerGroup::Observation,
    ),
    WorkerSpec::responder("tofu_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct(
        "upgrade_intent_watcher",
        RestartPolicy::Always,
        WorkerGroup::Actions,
    ),
    WorkerSpec::direct(
        "voice_provision",
        RestartPolicy::Always,
        WorkerGroup::Integrations,
    ),
    WorkerSpec::responder("voip_bus_responder", WorkerGroup::Actions),
    WorkerSpec::direct("voip_rtt", RestartPolicy::Always, WorkerGroup::Observation),
    WorkerSpec::responder("vpn_bus_responder", WorkerGroup::Actions),
];

/// MEDIA-1 — workers that ALSO require a capability tag beyond their rank tier.
///
/// A capability-gated worker runs only on a box that is at (or above) its rank
/// AND carries the tag — so the Navidrome media worker (MEDIA-3) runs on a
/// `Lighthouse_Media` node but never on a stock lighthouse / server / peer
/// (acceptance: "container absent on a non-media node"). The worker is still
/// listed in [`WORKER_REGISTRY`] for the rank floor; this table adds the tag gate.
///
/// `navidrome` is the foundation entry MEDIA-3 spawns onto: a rank-0 (lighthouse
/// tier) worker that additionally requires [`Capability::Media`]. It is wired
/// here now (not at MEDIA-3) so the gate is a single source of truth the worker
/// pool reads — MEDIA-3 adds the spawn, the tier table already refuses it
/// everywhere but a media-lighthouse.
const WORKER_CAPABILITIES: &[(&str, Capability)] = &[("navidrome", Capability::Media)];

/// Lighthouse tier (rank 0) — the rank floor the media worker sits at. The
/// `navidrome` worker is a lighthouse-tier worker that additionally requires the
/// [`Capability::Media`] tag (it never runs on a stock lighthouse).
const MEDIA_WORKER_RANK: u8 = 0;

/// Minimum rank that runs `worker`. Unknown workers default to 0 (Lighthouse).
///
/// NOTE this is the rank floor ONLY — a capability-gated worker (see
/// [`WORKER_CAPABILITIES`]) ALSO needs its tag; use [`runs_in`] for the full gate.
#[must_use]
pub fn min_rank(worker: &str) -> u8 {
    if let Some(rank) = capability_min_rank(worker) {
        return rank;
    }
    WORKER_REGISTRY
        .iter()
        .find(|s| s.name == worker)
        .map_or(0, |s| s.min_rank)
}

/// WL-ARCH-009 — the canonical registration for `worker`, regardless of its
/// current monolith start-site shape.
#[must_use]
pub fn spec(worker: &str) -> Option<&'static WorkerSpec> {
    WORKER_REGISTRY.iter().find(|s| s.name == worker)
}

/// Runtime spellings emitted by older worker implementations while the
/// canonical registry uses underscore-separated identities.  This is an
/// explicit ownership boundary: do not infer aliases by normalizing arbitrary
/// input, because that would let an uncensused runtime name claim a live
/// worker's group and status row.
const WORKER_RUNTIME_ALIASES: &[(&str, &str)] = &[
    ("nebula-supervisor", "nebula_supervisor"),
    ("health-reconciler", "health_reconciler"),
    ("mesh-latency", "mesh_latency"),
    ("mesh-router", "mesh_router"),
    ("nebula-ca-backup", "nebula_ca_backup"),
    ("nebula-csr-watcher", "nebula_csr_watcher"),
    ("nebula-enroll-listener", "nebula_enroll_listener"),
    ("nebula-https-listener", "nebula_https_listener"),
    ("stun-gather", "stun_gather"),
    ("kdc-host", "kdc_host"),
];

/// Resolve a production worker name to its canonical registry row.
///
/// Exact canonical names always win.  Only aliases emitted by a known worker
/// implementation are accepted; unknown kebab/snake transformations fail
/// closed instead of crossing a process-group ownership boundary.
#[must_use]
pub fn runtime_spec(worker: &str) -> Option<&'static WorkerSpec> {
    spec(worker).or_else(|| {
        WORKER_RUNTIME_ALIASES
            .iter()
            .find(|(alias, _)| *alias == worker)
            .and_then(|(_, canonical)| spec(canonical))
    })
}

/// WL-ARCH-009 — the complete runtime-contract registry.
///
/// Process entrypoints and status projections consume this view instead of
/// maintaining group-local worker lists.
#[must_use]
pub const fn worker_specs() -> &'static [WorkerSpec] {
    WORKER_REGISTRY
}

/// WL-ARCH-009 — contracts owned by one governed process group, in stable
/// registry order.
pub fn specs_for_group(group: WorkerGroup) -> impl Iterator<Item = &'static WorkerSpec> + Clone {
    WORKER_REGISTRY
        .iter()
        .filter(move |worker| worker.group == group)
}

/// Admit a named runtime start only in the process group declared by the
/// canonical registry.
///
/// Raw responder and maintenance threads do not pass through the `Supervisor`,
/// so their launcher uses this same fail-closed predicate before opening a Bus
/// cursor, SQLite handle, socket, or OS thread. Unknown names are rejected;
/// they must never become an uncensused seventh process surface.
#[must_use]
pub fn belongs_to_group(worker: &str, group: WorkerGroup) -> bool {
    runtime_spec(worker).is_some_and(|worker| worker.group == group)
}

/// Apply the canonical registry's startup-time configuration predicate.
///
/// Runtime/provider predicates stay admitted because their workers must run to
/// observe records that can appear after daemon startup. Environment predicates
/// are fixed for the process lifetime and are therefore enforced before a
/// constructor can open a queue, socket, subprocess, or task.
#[must_use]
pub fn startup_configuration_allows(worker: &str) -> bool {
    startup_configuration_allows_with(worker, |key| std::env::var_os(key))
}

fn startup_configuration_allows_with<F>(worker: &str, lookup: F) -> bool
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    let Some(worker) = spec(worker) else {
        return false;
    };
    match worker.activation.config {
        ConfigPredicate::Always | ConfigPredicate::RuntimeAvailable(_) => true,
        ConfigPredicate::EnvironmentPresent(key) => {
            lookup(key).is_some_and(|value| !value.is_empty())
        }
        ConfigPredicate::EnvironmentUnlessFalse(key) => lookup(key).is_none_or(|value| {
            !matches!(
                value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        }),
    }
}

/// Project one daemon registry row into the neutral worker-runtime contract.
///
/// The daemon registry remains authoritative for spawn behavior and the exact
/// activation predicate. This projection carries only the bounded, shell-free
/// declaration that the shared contract can represent; it never fabricates a
/// runtime snapshot or a worker state. Any source policy without an admitted
/// neutral equivalent is rejected instead of being silently weakened.
pub fn worker_contract(
    worker: &WorkerSpec,
) -> Result<runtime::WorkerContract, runtime::WorkerRuntimeContractError> {
    let group = runtime_group(worker.group);
    let applicability = runtime_applicability(worker)?;
    let cadence = runtime_cadence(worker.cadence)?;
    let queue = runtime_queue(worker.queue)?;
    let cache = runtime_cache(worker.cache)?;
    let ownership = runtime_ownership(worker)?;

    let mut contract = runtime::WorkerContract::new(worker.name, group, worker.name)?;
    contract.applicability = applicability;
    contract.criticality = match worker.criticality {
        Criticality::Essential => runtime::WorkerCriticality::Essential,
        Criticality::Important => runtime::WorkerCriticality::Important,
        Criticality::Optional => runtime::WorkerCriticality::Optional,
    };
    contract.restart_policy = match worker.policy {
        RestartPolicy::Never => runtime::WorkerRestartPolicy::Never,
        RestartPolicy::OnFailure => runtime::WorkerRestartPolicy::OnFailure,
        RestartPolicy::Always => runtime::WorkerRestartPolicy::Always,
    };
    contract.cadence = cadence;
    contract.queue = queue;
    contract.cache = cache;
    contract.resources = runtime::WorkerResourceBudget {
        memory_high_bytes: worker.resources.memory_high_bytes,
        memory_max_bytes: worker.resources.memory_max_bytes,
        cpu_millis_per_second: worker.resources.cpu_millis_per_second,
        max_tasks: worker.resources.max_tasks,
    };
    contract.ownership = ownership;

    contract.actions = worker
        .actions
        .iter()
        .map(|action| runtime::WorkerActionDescriptor {
            action: action.action,
            label: action.label.to_string(),
            arming: action.arming,
        })
        .collect();
    contract.admitted()
}

/// Project the complete canonical registry in stable registry order.
///
/// The duplicate check is intentionally performed across the whole result;
/// validating each row alone cannot detect two registry entries publishing the
/// same neutral worker identity.
pub fn worker_contracts(
) -> Result<Vec<runtime::WorkerContract>, runtime::WorkerRuntimeContractError> {
    let mut identities = std::collections::BTreeSet::new();
    let mut contracts = Vec::with_capacity(WORKER_REGISTRY.len());
    for worker in worker_specs() {
        let contract = worker_contract(worker)?;
        if !identities.insert(contract.worker_id.clone()) {
            return Err(runtime::WorkerRuntimeContractError::Duplicate(
                "worker_registry.worker_id",
            ));
        }
        contracts.push(contract);
    }
    Ok(contracts)
}

/// Project one named registry row, returning `None` only for an unknown worker.
pub fn worker_contract_for(
    worker: &str,
) -> Result<Option<runtime::WorkerContract>, runtime::WorkerRuntimeContractError> {
    spec(worker).map(worker_contract).transpose()
}

fn runtime_group(group: WorkerGroup) -> runtime::WorkerGroup {
    match group {
        WorkerGroup::Control => runtime::WorkerGroup::Control,
        WorkerGroup::Observation => runtime::WorkerGroup::Observation,
        WorkerGroup::Actions => runtime::WorkerGroup::Actions,
        WorkerGroup::Data => runtime::WorkerGroup::Data,
        WorkerGroup::Compute => runtime::WorkerGroup::Compute,
        WorkerGroup::Integrations => runtime::WorkerGroup::Integrations,
    }
}

fn runtime_applicability(
    worker: &WorkerSpec,
) -> Result<runtime::WorkerApplicability, runtime::WorkerRuntimeContractError> {
    let roles = match worker.min_rank {
        0 => vec![
            runtime::WorkerRole::Lighthouse,
            runtime::WorkerRole::Workstation,
        ],
        1 => vec![runtime::WorkerRole::Workstation],
        _ => {
            return Err(runtime::WorkerRuntimeContractError::InvalidField(
                "worker_spec.min_rank",
            ));
        }
    };

    let capabilities = match worker.activation.capability {
        CapabilityPredicate::AnyNode => Vec::new(),
        CapabilityPredicate::Requires(capability) => {
            let applies_to_declared_role = match capability {
                Capability::Media => false,
            };
            if !applies_to_declared_role {
                return Err(runtime::WorkerRuntimeContractError::InvalidRelationship(
                    "worker_spec.activation.capability",
                ));
            }
            vec![capability.as_str().to_owned()]
        }
    };

    let requires_configuration = match worker.activation.config {
        ConfigPredicate::Always => false,
        ConfigPredicate::EnvironmentPresent(key) | ConfigPredicate::EnvironmentUnlessFalse(key) => {
            if !valid_environment_key(key) {
                return Err(runtime::WorkerRuntimeContractError::InvalidField(
                    "worker_spec.activation.environment",
                ));
            }
            true
        }
        ConfigPredicate::RuntimeAvailable(key) => {
            if !valid_runtime_key(key) {
                return Err(runtime::WorkerRuntimeContractError::InvalidField(
                    "worker_spec.activation.runtime",
                ));
            }
            true
        }
    };

    Ok(runtime::WorkerApplicability {
        roles,
        capabilities,
        requires_configuration,
    })
}

fn runtime_cadence(
    cadence: CadencePolicy,
) -> Result<runtime::WorkerCadence, runtime::WorkerRuntimeContractError> {
    Ok(match cadence {
        CadencePolicy::Continuous => runtime::WorkerCadence::Continuous,
        CadencePolicy::EventDriven => runtime::WorkerCadence::EventDriven,
        CadencePolicy::OnDemand => runtime::WorkerCadence::OnDemand,
        CadencePolicy::Periodic {
            min_interval_secs,
            max_interval_secs,
        } => runtime::WorkerCadence::Periodic {
            min_interval_ms: u64::from(min_interval_secs).checked_mul(1_000).ok_or(
                runtime::WorkerRuntimeContractError::InvalidField(
                    "worker_spec.cadence.periodic.min_interval_secs",
                ),
            )?,
            max_interval_ms: u64::from(max_interval_secs).checked_mul(1_000).ok_or(
                runtime::WorkerRuntimeContractError::InvalidField(
                    "worker_spec.cadence.periodic.max_interval_secs",
                ),
            )?,
        },
    })
}

fn runtime_queue(
    queue: QueuePolicy,
) -> Result<runtime::WorkerQueueContract, runtime::WorkerRuntimeContractError> {
    let QueuePolicy::Bounded {
        max_items,
        max_bytes,
        overflow,
    } = queue
    else {
        return Err(runtime::WorkerRuntimeContractError::InvalidField(
            "worker_spec.queue.disabled",
        ));
    };

    let overflow = match overflow {
        QueueOverflow::RejectNew => runtime::WorkerQueueOverflow::RejectNew,
        QueueOverflow::LatestWins => runtime::WorkerQueueOverflow::LatestWins,
        QueueOverflow::DropOldest => {
            return Err(runtime::WorkerRuntimeContractError::InvalidField(
                "worker_spec.queue.overflow.drop_oldest",
            ));
        }
    };

    Ok(runtime::WorkerQueueContract {
        max_items,
        max_bytes,
        overflow,
    })
}

fn runtime_cache(
    cache: CachePolicy,
) -> Result<runtime::WorkerCachePolicy, runtime::WorkerRuntimeContractError> {
    Ok(match cache {
        CachePolicy::Disabled => runtime::WorkerCachePolicy::Disabled,
        CachePolicy::Bounded {
            max_items,
            max_bytes,
            ttl_secs,
        } => runtime::WorkerCachePolicy::Bounded {
            max_items,
            max_bytes,
            ttl_ms: u64::from(ttl_secs).checked_mul(1_000).ok_or(
                runtime::WorkerRuntimeContractError::InvalidField("worker_spec.cache.ttl_secs"),
            )?,
        },
    })
}

fn runtime_ownership(
    worker: &WorkerSpec,
) -> Result<runtime::WorkerOwnership, runtime::WorkerRuntimeContractError> {
    if worker.ownership.state != worker.group {
        return Err(runtime::WorkerRuntimeContractError::InvalidRelationship(
            "worker_spec.ownership.state_group",
        ));
    }
    if worker.ownership.health != worker.group {
        return Err(runtime::WorkerRuntimeContractError::InvalidRelationship(
            "worker_spec.ownership.health_group",
        ));
    }
    if worker.ownership.actions != worker.group {
        return Err(runtime::WorkerRuntimeContractError::InvalidRelationship(
            "worker_spec.ownership.action_group",
        ));
    }
    if !(1..=60).contains(&worker.ownership.cleanup.grace_secs) {
        return Err(runtime::WorkerRuntimeContractError::InvalidField(
            "worker_spec.cleanup.grace_secs",
        ));
    }

    Ok(runtime::WorkerOwnership {
        state_group: runtime_group(worker.ownership.state),
        health_group: runtime_group(worker.ownership.health),
        action_group: runtime_group(worker.ownership.actions),
        cleanup_owner: match worker.ownership.cleanup.owner {
            CleanupOwner::Worker => runtime::WorkerCleanupOwner::Worker,
            CleanupOwner::GroupSupervisor => runtime::WorkerCleanupOwner::GroupSupervisor,
        },
    })
}

fn valid_environment_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.starts_with("MDE_")
        && key
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_runtime_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// WL-ARCH-004 — the restart policy declared for a role-tiered `worker`. `None`
/// for an absent or directly bound worker; `spawn_tiered` treats either as a
/// hard error, which prevents a direct registration from silently changing its
/// start semantics.
#[must_use]
pub fn policy_for(worker: &str) -> Option<RestartPolicy> {
    spec(worker)
        .filter(|spec| spec.spawn_binding == SpawnBinding::Tiered)
        .map(|spec| spec.policy)
}

/// The rank floor for a capability-gated worker that isn't in [`WORKER_REGISTRY`]
/// (the media worker lives in the capability table, not the rank census, so its
/// rank floor is pinned here). `None` for a plain rank-gated worker.
fn capability_min_rank(worker: &str) -> Option<u8> {
    WORKER_CAPABILITIES
        .iter()
        .find(|(n, _)| *n == worker)
        .map(|(_, cap)| match cap {
            Capability::Media => MEDIA_WORKER_RANK,
        })
}

/// MEDIA-1 — the capability tag `worker` requires (beyond its rank), if any.
#[must_use]
pub fn required_capability(worker: &str) -> Option<Capability> {
    WORKER_CAPABILITIES
        .iter()
        .find(|(n, _)| *n == worker)
        .map(|(_, c)| *c)
}

/// The canonical role name for a resolved `rank`.
///
/// An unknown rank falls back to the top tier, matching the tolerant
/// [`resolve_rank`] posture.
#[must_use]
pub fn role_name(rank: u8) -> &'static str {
    Role::all()
        .into_iter()
        .find(|r| r.rank() == rank)
        .unwrap_or(Role::Workstation)
        .as_str()
}

/// Resolve the deployment rank that gates worker spawns: the pinned role's
/// rank, or **Workstation (1) when unpinned** (a dev tree / pre-role-pin box
/// runs the full set — the desktop workers idle gracefully without a Wayland
/// session), or **Lighthouse (0) when `role.toml` is malformed** (fail closed —
/// run only the relay control plane, never assume a Workstation default).
/// Reads `/var/lib/mde/role.toml` locally; no mesh needed.
#[must_use]
pub fn resolve_rank() -> u8 {
    resolve_class().rank
}

/// MEDIA-1 — resolve the full deployment **class** (rank + capability tags) that
/// gates worker spawns.
///
/// Same fail-soft contract as [`resolve_rank`]: an unpinned box → Workstation (no
/// media tag — the desktop set, never the media worker), a malformed `role.toml`
/// → Lighthouse fail-closed (no media tag). The media tag is only ever set when a
/// valid legacy media-hosting class is pinned. Workstation media discovery does
/// not depend on this marker; it consumes replicated Airsonic server records.
#[must_use]
pub fn resolve_class() -> DeployClass {
    match mde_role::load_class() {
        Ok(class) => DeployClass::from_role_class(&class),
        Err(mde_role::LoadError::NotPinned) => DeployClass::plain(Role::Workstation.rank()),
        Err(_) => DeployClass::plain(Role::Lighthouse.rank()),
    }
}

/// ENT-2 (C3) — the FAIL-CLOSED resolver the worker pool boots
/// through: an unpinned box refuses to start instead of silently
/// running the fattest (Workstation) worker set. Display/diagnostic
/// paths keep the tolerant [`resolve_rank`]; the supervisor uses this.
///
/// # Errors
/// A human-actionable message naming the fix (`mackesd role pin …`).
pub fn resolve_rank_strict() -> Result<u8, String> {
    resolve_class_strict().map(|c| c.rank)
}

/// MEDIA-1 — the fail-closed counterpart to [`resolve_class`] (ENT-2).
///
/// The same refuse-when-unpinned contract as [`resolve_rank_strict`], returning
/// the full [`DeployClass`] so the worker pool gates capability workers (the
/// media worker) as well as rank workers off a single resolved class.
///
/// # Errors
/// A human-actionable message naming the fix (`mackesd role pin …`).
pub fn resolve_class_strict() -> Result<DeployClass, String> {
    match mde_role::load_class() {
        Ok(class) => Ok(DeployClass::from_role_class(&class)),
        Err(mde_role::LoadError::NotPinned) => Err(
            "no deployment role pinned (/var/lib/mde/role.toml absent) — this box refuses to \
             start its worker pool unpinned (ENT-2 fail-closed). Pin one first: \
             `mackesd role pin <lighthouse|workstation>`"
                .to_string(),
        ),
        Err(e) => Err(format!(
            "role.toml unreadable ({e}) — refusing to start the worker pool (ENT-2). \
             Repair or re-pin: `mackesd role pin <role>`"
        )),
    }
}

/// Whether a box at `role_rank` runs `worker` — the **rank-only** gate.
///
/// A capability-gated worker (the media worker) is NOT runnable through this path
/// (it needs its tag too); [`runs`] returns `false` for one, and the full gate
/// lives in [`runs_in`]. Plain rank-gated workers are unaffected.
#[must_use]
pub fn runs(worker: &str, role_rank: u8) -> bool {
    runs_in(worker, DeployClass::plain(role_rank))
}

/// MEDIA-1 — the full spawn gate: whether a box of `class` runs `worker`.
///
/// A worker runs iff the box is at (or above) the worker's rank floor AND — for a
/// capability-gated worker — the box carries the required tag. This is the single
/// predicate `run_serve` gates every `sup.spawn` through, so the media worker
/// remains limited to the legacy hosting worker; Workstation media clients use
/// the rank-gated `music_autoconfig` worker instead.
#[must_use]
pub fn runs_in(worker: &str, class: DeployClass) -> bool {
    // Rank-floor lookup historically defaulted unknown names to Lighthouse.
    // Keep that tolerant diagnostic API, but never let an uncensused name pass
    // an executable spawn gate.
    if spec(worker).is_none() && required_capability(worker).is_none() {
        return false;
    }
    if class.rank < min_rank(worker) {
        return false;
    }
    match required_capability(worker) {
        None => true,
        // The historical media-lighthouse worker class is retired. Keep the
        // capability entry for old callers, but never schedule it on any node.
        Some(Capability::Media) => false,
    }
}

/// Every worker a box at `role_rank` runs — the rank-gated subset (plan §12).
///
/// Capability-gated workers (the media worker) are EXCLUDED here (a rank alone
/// can't satisfy a tag gate); use [`workers_for_class`] for the full set on a
/// tagged box. Order follows the tier census (lowest tier first). This is the
/// static counterpart to `run_serve`'s live `worker_names` listing, surfaced by
/// `mackesd role-workers`.
#[must_use]
pub fn workers_for_rank(role_rank: u8) -> Vec<&'static str> {
    workers_for_class(DeployClass::plain(role_rank))
}

/// MEDIA-1 — every worker a box of `class` runs, including capability-gated ones.
///
/// The legacy capability workers its tags unlock. Rank workers first (tier
/// census order), then the capability workers the box's tags satisfy.
#[must_use]
pub fn workers_for_class(class: DeployClass) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = WORKER_REGISTRY
        .iter()
        .filter(|s| class.rank >= s.min_rank)
        .map(|s| s.name)
        .collect();
    out.extend(
        WORKER_CAPABILITIES
            .iter()
            .filter(|(name, _)| runs_in(name, class))
            .map(|(name, _)| *name),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_inventory_sha256(registry: &[WorkerSpec]) -> String {
        use sha2::{Digest as _, Sha256};

        let mut digest = Sha256::new();
        for worker in registry {
            digest.update(format!("{worker:?}\n"));
        }
        format!("{:x}", digest.finalize())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // WL-ARCH-004 — the (now structural) drift guard.
    //
    // Worker registration used to be split between the role census and a
    // test-only allowlist for directly started workers. WL-ARCH-009 makes
    // [`WORKER_REGISTRY`] the one canonical inventory and records the exact
    // start-site shape on every row.
    //
    // `worker_spawns_and_the_census_do_not_drift` proves that: it asserts the census
    // is derived from the registry (not a parallel list) AND reads the crate
    // source at test time to enforce the registry ⇔ production-start-site
    // identity in both directions. Airgapped-safe: pure source parsing, no live
    // environment and no exception list.

    fn crate_src_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn read_source(rel: &str) -> String {
        let p = crate_src_dir().join(rel);
        std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("ARCH-5 drift guard: cannot read {}: {e}", p.display()))
    }

    /// Every `*.rs` under the crate `src/` tree.
    fn rust_sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let rd = std::fs::read_dir(dir).unwrap_or_else(|e| {
            panic!("ARCH-5 drift guard: cannot read dir {}: {e}", dir.display())
        });
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(rust_sources(&p));
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(p);
            }
        }
        out
    }

    /// Drop whole-line comments so doc/inline mentions of `runs(...)` don't register
    /// as gate sites (e.g. the `//!` module docs in `media_registry.rs`).
    fn strip_line_comments(src: &str) -> String {
        src.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Extract each worker-name literal that immediately follows `needle`
    /// (e.g. `.push("` or `runs("`), reading up to the closing quote. Only lowercase
    /// worker tokens (`[a-z0-9_-]`) are accepted, so multi-word / non-worker strings
    /// are ignored.
    fn scan_names(src: &str, needle: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = src.as_bytes();
        let mut i = 0usize;
        while i < src.len() {
            let Some(pos) = src[i..].find(needle) else {
                break;
            };
            let start = i + pos + needle.len();
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            let tok = &src[start..j];
            if !tok.is_empty()
                && tok
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            {
                out.push(tok.to_string());
            }
            i = j + 1;
        }
        out
    }

    /// Extract the FIRST quoted worker-name literal appearing anywhere after each
    /// `needle` occurrence. Unlike [`scan_names`], the needle need NOT abut the
    /// opening quote — used for `spawn_tiered(…, "name", || …)` sites, where the
    /// name is a later argument and rustfmt may line-break the whole call. Only
    /// lowercase worker tokens (`[a-z0-9_-]`) are kept.
    fn scan_call_names(src: &str, needle: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while let Some(pos) = src[i..].find(needle) {
            let after = i + pos + needle.len();
            if let Some(q1) = src[after..].find('"') {
                let start = after + q1 + 1;
                if let Some(q2) = src[start..].find('"') {
                    let tok = &src[start..start + q2];
                    if !tok.is_empty()
                        && tok.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                        })
                    {
                        out.push(tok.to_string());
                    }
                    i = start + q2 + 1;
                    continue;
                }
            }
            i = after;
        }
        out
    }

    /// Associate each literal `worker_names.push("name")` with the nearest
    /// restart policy since the preceding literal registration. Direct
    /// supervisor starts place `Spawn::new(..., RestartPolicy::X)` immediately
    /// before the push; responder threads contain no restart policy in that
    /// segment. Duplicate start sites must agree.
    fn scan_literal_spawn_policies(
        src: &str,
    ) -> std::collections::BTreeMap<String, Option<RestartPolicy>> {
        use std::collections::BTreeMap;

        let mut out = BTreeMap::new();
        let mut segment_start = 0usize;
        let needle = ".push(\"";
        while let Some(relative) = src[segment_start..].find(needle) {
            let call = segment_start + relative;
            let name_start = call + needle.len();
            let Some(name_end_relative) = src[name_start..].find('"') else {
                break;
            };
            let name_end = name_start + name_end_relative;
            let name = &src[name_start..name_end];
            if valid_worker_name(name) {
                let segment = &src[segment_start..call];
                let candidates = [
                    (segment.rfind("RestartPolicy::Never"), RestartPolicy::Never),
                    (
                        segment.rfind("RestartPolicy::OnFailure"),
                        RestartPolicy::OnFailure,
                    ),
                    (
                        segment.rfind("RestartPolicy::Always"),
                        RestartPolicy::Always,
                    ),
                ];
                let policy = candidates
                    .into_iter()
                    .filter_map(|(position, policy)| position.map(|position| (position, policy)))
                    .max_by_key(|(position, _)| *position)
                    .map(|(_, policy)| policy);
                if let Some(previous) = out.insert(name.to_owned(), policy) {
                    assert_eq!(
                        previous, policy,
                        "WL-ARCH-009: duplicate literal start sites disagree for {name}"
                    );
                }
            }
            segment_start = name_end + 1;
        }
        out
    }

    /// Every worker name passed to a PRODUCTION `worker_role::runs(...)` /
    /// `runs_in(...)` gate anywhere in the crate. Skips this module (its `runs(...)`
    /// calls are test fixtures like `"some-future-worker"`) and comment lines.
    fn collect_gate_names() -> std::collections::BTreeSet<String> {
        let mut set = std::collections::BTreeSet::new();
        let mut n_files = 0usize;
        for path in rust_sources(&crate_src_dir()) {
            if path.file_name().and_then(|s| s.to_str()) == Some("worker_role.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read rs source");
            let code = strip_line_comments(&src);
            for name in scan_names(&code, "runs(\"") {
                set.insert(name);
            }
            for name in scan_names(&code, "runs_in(\"") {
                set.insert(name);
            }
            n_files += 1;
        }
        assert!(
            n_files >= 3,
            "ARCH-5 drift guard: only scanned {n_files} source files — the walker is broken"
        );
        set
    }

    fn valid_worker_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && name
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
    }

    fn valid_runtime_key(key: &str) -> bool {
        !key.is_empty()
            && key.len() <= 64
            && key
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    }

    #[test]
    fn runtime_contract_is_total_unique_and_bounded() {
        use std::collections::BTreeSet;

        let mut names = BTreeSet::new();
        assert_eq!(worker_specs().len(), WORKER_REGISTRY.len());

        for worker in worker_specs() {
            assert!(
                valid_worker_name(worker.name),
                "WL-ARCH-009: invalid stable worker name {:?}",
                worker.name
            );
            assert!(
                names.insert(worker.name),
                "WL-ARCH-009: duplicate worker contract for {}",
                worker.name
            );
            assert!(
                worker.min_rank <= Role::Workstation.rank(),
                "WL-ARCH-009: {} has invalid role rank {}",
                worker.name,
                worker.min_rank
            );

            match worker.activation.capability {
                CapabilityPredicate::AnyNode | CapabilityPredicate::Requires(Capability::Media) => {
                }
            }
            match worker.activation.config {
                ConfigPredicate::Always => {}
                ConfigPredicate::EnvironmentPresent(key)
                | ConfigPredicate::EnvironmentUnlessFalse(key) => assert!(
                    key.starts_with("MDE_")
                        && key.len() <= 64
                        && key.bytes().all(|byte| byte.is_ascii_uppercase()
                            || byte.is_ascii_digit()
                            || byte == b'_'),
                    "WL-ARCH-009: {} has invalid environment gate {key:?}",
                    worker.name
                ),
                ConfigPredicate::RuntimeAvailable(key) => assert!(
                    valid_runtime_key(key),
                    "WL-ARCH-009: {} has invalid runtime gate {key:?}",
                    worker.name
                ),
            }

            if let CadencePolicy::Periodic {
                min_interval_secs,
                max_interval_secs,
            } = worker.cadence
            {
                assert!(
                    min_interval_secs > 0
                        && min_interval_secs <= max_interval_secs
                        && max_interval_secs <= 86_400,
                    "WL-ARCH-009: {} has invalid cadence {:?}",
                    worker.name,
                    worker.cadence
                );
            }

            if let QueuePolicy::Bounded {
                max_items,
                max_bytes,
                ..
            } = worker.queue
            {
                assert!(
                    max_items > 0 && max_items <= 4_096 && max_bytes > 0 && max_bytes <= 64 * MIB,
                    "WL-ARCH-009: {} has invalid queue bound {:?}",
                    worker.name,
                    worker.queue
                );
            }
            if let CachePolicy::Bounded {
                max_items,
                max_bytes,
                ttl_secs,
            } = worker.cache
            {
                assert!(
                    max_items > 0
                        && max_items <= 4_096
                        && max_bytes > 0
                        && max_bytes <= 64 * MIB
                        && ttl_secs > 0
                        && ttl_secs <= 86_400,
                    "WL-ARCH-009: {} has invalid cache bound {:?}",
                    worker.name,
                    worker.cache
                );
            }

            let budget = worker.resources;
            assert!(
                budget.memory_high_bytes > 0
                    && budget.memory_high_bytes <= budget.memory_max_bytes
                    && budget.memory_max_bytes <= 512 * MIB
                    && (1..=1_000).contains(&budget.cpu_millis_per_second)
                    && (1..=64).contains(&budget.max_tasks),
                "WL-ARCH-009: {} has invalid resource budget {:?}",
                worker.name,
                budget
            );
            assert!(
                (1..=60).contains(&worker.ownership.cleanup.grace_secs),
                "WL-ARCH-009: {} has invalid cleanup deadline {:?}",
                worker.name,
                worker.ownership.cleanup
            );

            // A singular enum field makes double-group assignment impossible;
            // these ownership assertions keep every derived namespace on that
            // same process boundary.
            assert_eq!(
                worker.ownership.state, worker.group,
                "{} state owner",
                worker.name
            );
            assert_eq!(
                worker.ownership.health, worker.group,
                "{} health owner",
                worker.name
            );
            assert_eq!(
                worker.ownership.actions, worker.group,
                "{} action owner",
                worker.name
            );
            assert!(worker
                .group
                .state_topic_prefix()
                .starts_with("state/mackesd/"));
            assert!(worker.group.health_key_prefix().starts_with("mackesd."));
            assert!(worker
                .group
                .action_namespace()
                .starts_with("action/mackesd/"));
            assert_eq!(
                spec(worker.name).map(|entry| entry.group),
                Some(worker.group)
            );
        }
    }

    #[test]
    fn neutral_worker_contract_projection_is_deterministic_and_total() {
        let first = worker_contracts().expect("the shipped worker registry projects");
        let second = worker_contracts().expect("the projection remains repeatable");

        assert_eq!(first, second);
        assert_eq!(first.len(), worker_specs().len());
        assert!(first.iter().all(|contract| contract.validate().is_ok()));

        let cloud = worker_contract_for("cloud")
            .expect("cloud projection")
            .expect("cloud is registered");
        assert_eq!(cloud.group, runtime::WorkerGroup::Compute);
        assert_eq!(
            cloud.applicability.roles,
            vec![
                runtime::WorkerRole::Lighthouse,
                runtime::WorkerRole::Workstation
            ]
        );
        assert_eq!(
            cloud.restart_policy,
            runtime::WorkerRestartPolicy::OnFailure
        );

        let airspace = worker_contract_for("airspace")
            .expect("airspace projection")
            .expect("airspace is registered");
        assert!(airspace.applicability.requires_configuration);

        let media_sources = worker_contract_for("media_sources")
            .expect("media_sources projection")
            .expect("media_sources is registered");
        assert_eq!(media_sources.cadence, runtime::WorkerCadence::EventDriven);
        assert!(worker_contract_for("not-registered")
            .expect("unknown lookup")
            .is_none());
    }

    #[test]
    fn neutral_worker_contract_projection_rejects_incomplete_or_hostile_rows() {
        let mut invalid =
            WorkerSpec::tier("invalid", 2, RestartPolicy::OnFailure, WorkerGroup::Control);
        assert_eq!(
            worker_contract(&invalid),
            Err(runtime::WorkerRuntimeContractError::InvalidField(
                "worker_spec.min_rank"
            ))
        );

        invalid = WorkerSpec::tier("invalid", 0, RestartPolicy::OnFailure, WorkerGroup::Control);
        invalid.queue = QueuePolicy::Disabled;
        assert_eq!(
            worker_contract(&invalid),
            Err(runtime::WorkerRuntimeContractError::InvalidField(
                "worker_spec.queue.disabled"
            ))
        );

        invalid.queue = QueuePolicy::Bounded {
            max_items: 1,
            max_bytes: 1,
            overflow: QueueOverflow::DropOldest,
        };
        assert_eq!(
            worker_contract(&invalid),
            Err(runtime::WorkerRuntimeContractError::InvalidField(
                "worker_spec.queue.overflow.drop_oldest"
            ))
        );

        invalid.queue = WorkerGroup::Control.defaults().queue;
        invalid.name = "../worker";
        assert!(worker_contract(&invalid).is_err());

        invalid.name = "invalid";
        invalid.activation.config = ConfigPredicate::EnvironmentPresent("MDE_BAD-NAME");
        assert_eq!(
            worker_contract(&invalid),
            Err(runtime::WorkerRuntimeContractError::InvalidField(
                "worker_spec.activation.environment"
            ))
        );

        invalid.activation.config = ConfigPredicate::Always;
        invalid.ownership.state = WorkerGroup::Observation;
        assert_eq!(
            worker_contract(&invalid),
            Err(runtime::WorkerRuntimeContractError::InvalidRelationship(
                "worker_spec.ownership.state_group"
            ))
        );

        invalid.ownership.state = WorkerGroup::Control;
        invalid.ownership.health = WorkerGroup::Observation;
        assert_eq!(
            worker_contract(&invalid),
            Err(runtime::WorkerRuntimeContractError::InvalidRelationship(
                "worker_spec.ownership.health_group"
            ))
        );

        invalid.ownership.health = WorkerGroup::Control;
        invalid.ownership.actions = WorkerGroup::Observation;
        assert_eq!(
            worker_contract(&invalid),
            Err(runtime::WorkerRuntimeContractError::InvalidRelationship(
                "worker_spec.ownership.action_group"
            ))
        );

        invalid.ownership.actions = WorkerGroup::Control;
        invalid.ownership.cleanup.grace_secs = 0;
        assert_eq!(
            worker_contract(&invalid),
            Err(runtime::WorkerRuntimeContractError::InvalidField(
                "worker_spec.cleanup.grace_secs"
            ))
        );
    }

    #[test]
    fn six_group_coverage_and_spawn_behavior_are_stable() {
        use std::collections::BTreeSet;

        let mut services = BTreeSet::new();
        let mut covered = 0;
        for group in WorkerGroup::ALL {
            let count = specs_for_group(group).count();
            assert!(
                count > 0,
                "WL-ARCH-009: {} group has no registered workers",
                group.as_str(),
            );
            assert!(services.insert(group.service_name()));
            covered += count;
        }
        assert_eq!(
            WorkerGroup::ALL.map(WorkerGroup::as_str),
            [
                "control",
                "observation",
                "actions",
                "data",
                "compute",
                "integrations",
            ]
        );
        assert_eq!(services.len(), WorkerGroup::ALL.len());
        assert_eq!(covered, WORKER_REGISTRY.len());

        let mut on_failure = 0;
        let mut always = 0;
        let mut never = 0;
        for worker in WORKER_REGISTRY {
            match worker.policy {
                RestartPolicy::OnFailure => on_failure += 1,
                RestartPolicy::Always => always += 1,
                RestartPolicy::Never => never += 1,
            }
        }
        assert_eq!(on_failure + always + never, WORKER_REGISTRY.len());
        assert!(on_failure > 0 && always > 0 && never > 0);
        for rank in [0, 1] {
            assert!(WORKER_REGISTRY.iter().any(|worker| {
                matches!(
                    worker.spawn_binding,
                    SpawnBinding::Tiered | SpawnBinding::DynamicSupervisor
                ) && worker.min_rank == rank
            }));
        }
    }

    #[test]
    fn responder_threads_are_admitted_only_by_their_registered_group() {
        for responder in WORKER_REGISTRY
            .iter()
            .filter(|worker| worker.spawn_binding == SpawnBinding::ResponderThread)
        {
            for group in WorkerGroup::ALL {
                assert_eq!(
                    belongs_to_group(responder.name, group),
                    responder.group == group,
                    "WL-ARCH-009 responder process admission drifted for {}",
                    responder.name
                );
            }
        }
        assert!(!belongs_to_group(
            "uncensused_responder",
            WorkerGroup::Control
        ));
    }

    #[test]
    fn runtime_aliases_are_explicit_and_unknown_normalizations_fail_closed() {
        assert_eq!(
            runtime_spec("mesh-router").map(|worker| worker.name),
            Some("mesh_router")
        );
        assert_eq!(
            runtime_spec("mesh_router").map(|worker| worker.name),
            Some("mesh_router")
        );
        assert!(runtime_spec("mesh-router-shadow").is_none());
        assert!(runtime_spec("mesh_router_shadow").is_none());
        assert!(runtime_spec("nebula--supervisor").is_none());
    }

    #[test]
    fn admitted_runtime_aliases_preserve_process_group_ownership() {
        let canonical = spec("mesh_router").expect("canonical worker must be registered");
        assert!(belongs_to_group("mesh-router", canonical.group));
        assert!(!belongs_to_group("mesh-router", WorkerGroup::Observation));
        assert!(!belongs_to_group("mesh-router-extra", canonical.group));
    }

    #[test]
    fn process_group_parser_is_exact_and_round_trips_service_tokens() {
        for group in WorkerGroup::ALL {
            assert_eq!(WorkerGroup::parse(group.as_str()), Ok(group));
            assert_eq!(group.to_string(), group.as_str());
        }
        assert!(WorkerGroup::parse("all").is_err());
        assert!(WorkerGroup::parse("control;systemctl stop mackesd").is_err());
    }

    #[test]
    fn worker_spawns_and_the_census_do_not_drift() {
        use std::collections::BTreeSet;

        let census: BTreeSet<&str> = WORKER_REGISTRY.iter().map(|s| s.name).collect();
        let caps: BTreeSet<&str> = WORKER_CAPABILITIES.iter().map(|(n, _)| *n).collect();
        let registered = |binding| {
            WORKER_REGISTRY
                .iter()
                .filter(|worker| worker.spawn_binding == binding)
                .map(|worker| worker.name)
                .collect::<BTreeSet<_>>()
        };
        let tiered_registry = registered(SpawnBinding::Tiered);
        let direct_registry = registered(SpawnBinding::DirectSupervisor);
        let responder_registry = registered(SpawnBinding::ResponderThread);
        let dynamic_registry = registered(SpawnBinding::DynamicSupervisor);
        let infrastructure_registry = registered(SpawnBinding::ProcessInfrastructure);

        // WL-ARCH-004 — the registry names are unique.
        assert_eq!(
            census.len(),
            WORKER_REGISTRY.len(),
            "WL-ARCH-004: duplicate worker name in WORKER_REGISTRY"
        );

        // WL-ARCH-004 — the census is now DERIVED from WORKER_REGISTRY (both
        // `min_rank`/`workers_for_class` and the spawner read this one table), so
        // the two registries that historically drifted (ARCH-5 / BUG-STORAGE-1)
        // cannot diverge by construction. Retired media state must not extend
        // the registry.
        let derived: BTreeSet<&str> = workers_for_class(DeployClass {
            rank: 1,
            media: true,
        })
        .into_iter()
        .collect();
        let expected = census.clone();
        assert_eq!(
            derived, expected,
            "WL-ARCH-004: workers_for_class no longer derives from WORKER_REGISTRY"
        );

        // The spawn roster, read from the source at test time:
        //  • `tiered`  — every `spawn_tiered(…, \"X\", || …)` site (the sole way a
        //    role-tiered worker is now spawned; policy + gate come from the registry).
        //  • `pushed`  — every remaining `worker_names.push(\"X\")` literal (the
        //    directly supervised and responder-thread bindings keep this shape).
        // Both scanned across run_serve (`bin/mackesd.rs`) and its spawn helpers
        // (`bin/mackesd/spawn.rs`).
        let bin = read_source("bin/mackesd.rs") + &read_source("bin/mackesd/spawn.rs");
        assert!(
            !bin.contains("std::env::var(\"MDE_ANSIBLE_PULL_URL\")"),
            "WL-ARCH-009: ansible-pull startup configuration escaped the canonical registry"
        );
        let tiered: BTreeSet<String> = scan_call_names(&bin, "spawn_tiered(").into_iter().collect();
        let infrastructure: BTreeSet<String> =
            scan_call_names(&bin, "register_process_infrastructure(")
                .into_iter()
                .collect();
        let pushed: BTreeSet<String> = scan_names(&bin, ".push(\"").into_iter().collect();
        let literal_policies = scan_literal_spawn_policies(&bin);
        assert!(
            tiered.len() >= 60,
            "WL-ARCH-004 drift guard: only {} `spawn_tiered(…, \"X\")` sites found — the source \
             scan is broken (expected ~70)",
            tiered.len()
        );
        assert!(
            pushed.len() >= 45,
            "WL-ARCH-004 drift guard: only {} `.push(\"…\")` direct/responder sites found — the source \
             scan is broken (expected ~65)",
            pushed.len()
        );

        // (1) Tiered registry ⇔ spawn_tiered identity.
        let mut census_unspawned: Vec<&str> = tiered_registry
            .iter()
            .copied()
            .filter(|n| !tiered.contains(*n))
            .collect();
        census_unspawned.sort_unstable();
        assert!(
            census_unspawned.is_empty(),
            "WL-ARCH-009 DRIFT: these Tiered registrations are never spawned via spawn_tiered: \
             {census_unspawned:?}"
        );
        let mut tiered_unregistered: Vec<&str> = tiered
            .iter()
            .map(String::as_str)
            .filter(|n| !tiered_registry.contains(n))
            .collect();
        tiered_unregistered.sort_unstable();
        assert!(
            tiered_unregistered.is_empty(),
            "WL-ARCH-004 DRIFT: these workers are spawned via spawn_tiered but are MISSING from \
             WORKER_REGISTRY, so spawn_tiered would panic on the unknown restart policy. Add each \
             to WORKER_REGISTRY with a deliberate tier: {tiered_unregistered:?}"
        );

        let mut infrastructure_unspawned: Vec<&str> = infrastructure_registry
            .iter()
            .copied()
            .filter(|name| !infrastructure.contains(*name))
            .collect();
        infrastructure_unspawned.sort_unstable();
        assert!(
            infrastructure_unspawned.is_empty(),
            "WL-ARCH-009 DRIFT: registered process infrastructure has no production start: \
             {infrastructure_unspawned:?}"
        );
        let mut infrastructure_unregistered: Vec<&str> = infrastructure
            .iter()
            .map(String::as_str)
            .filter(|name| !infrastructure_registry.contains(name))
            .collect();
        infrastructure_unregistered.sort_unstable();
        assert!(
            infrastructure_unregistered.is_empty(),
            "WL-ARCH-009 DRIFT: uncensused process infrastructure start: \
             {infrastructure_unregistered:?}"
        );

        // (2) Literal runtime names are exactly the directly supervised and
        //     responder-thread registrations. No allowlist sits beside the
        //     canonical registry.
        let literal_registry: BTreeSet<&str> = direct_registry
            .union(&responder_registry)
            .copied()
            .collect();
        let mut tiered_pushed_literally: Vec<&str> = pushed
            .iter()
            .map(String::as_str)
            .filter(|n| tiered_registry.contains(n) || dynamic_registry.contains(n))
            .collect();
        tiered_pushed_literally.sort_unstable();
        assert!(
            tiered_pushed_literally.is_empty(),
            "WL-ARCH-004 DRIFT: these role-tiered workers still `worker_names.push(\"…\")` \
             directly instead of via spawn_tiered — finish the conversion: {tiered_pushed_literally:?}"
        );

        // (3) Bidirectional direct binding: every literal production start is
        //     registered, and every direct/responder registration has a start.
        let mut unaccounted: Vec<&str> = pushed
            .iter()
            .map(String::as_str)
            .filter(|n| !literal_registry.contains(n))
            .collect();
        unaccounted.sort_unstable();
        assert!(
            unaccounted.is_empty(),
            "WL-ARCH-009 DRIFT: these literal production starts have no canonical registration: \
             {unaccounted:?}"
        );
        let mut stale: Vec<&str> = literal_registry
            .iter()
            .copied()
            .filter(|n| !pushed.contains(*n))
            .collect();
        stale.sort_unstable();
        assert!(
            stale.is_empty(),
            "WL-ARCH-009 DRIFT: these direct/responder registrations have no literal production \
             start: {stale:?}"
        );
        assert!(
            dynamic_registry == BTreeSet::from(["lighthouse_probe"]),
            "WL-ARCH-009: the runtime-named supervisor binding must stay explicit in the registry"
        );

        for worker in WORKER_REGISTRY {
            match worker.spawn_binding {
                SpawnBinding::DirectSupervisor => assert_eq!(
                    literal_policies.get(worker.name),
                    Some(&Some(worker.policy)),
                    "WL-ARCH-009: direct supervisor policy drift for {}",
                    worker.name
                ),
                SpawnBinding::ResponderThread => assert_eq!(
                    literal_policies.get(worker.name),
                    Some(&None),
                    "WL-ARCH-009: responder acquired or lost a supervisor policy for {}",
                    worker.name
                ),
                SpawnBinding::Tiered
                | SpawnBinding::DynamicSupervisor
                | SpawnBinding::ProcessInfrastructure => {}
            }
        }

        // (4) Any REMAINING literal `runs(\"X\")` / `runs_in(\"X\")` gate in the crate
        //     (the capability gate + a few self-gating workers; the tiered gates now
        //     live inside spawn_tiered) must still name a censused worker, so a stray
        //     gate can never silently resolve `min_rank => 0` (the BUG-STORAGE-1 bug).
        let gated = collect_gate_names();
        let mut gated_uncensused: Vec<&str> = gated
            .iter()
            .map(String::as_str)
            .filter(|n| !census.contains(n) && !caps.contains(n))
            .collect();
        gated_uncensused.sort_unstable();
        assert!(
            gated_uncensused.is_empty(),
            "WL-ARCH-004 DRIFT: these workers are gated on `worker_role::runs(…)` but are MISSING \
             from WORKER_REGISTRY/WORKER_CAPABILITIES, so they silently default to rank 0 (the \
             BUG-STORAGE-1 bug). Add each to the census with a deliberate tier: {gated_uncensused:?}"
        );
    }

    #[test]
    fn canonical_registry_inventory_hash_covers_every_runtime_field() {
        let hash = registry_inventory_sha256(WORKER_REGISTRY);
        assert_eq!(
            hash, "0687e5676b1f5370e4337fe78192607bd39442557a329c583c20f11f7e4fd244",
            "WL-ARCH-009: canonical registration inventory drifted"
        );

        let mut hostile = WORKER_REGISTRY.to_vec();
        hostile[0].ownership.cleanup.grace_secs += 1;
        assert_ne!(registry_inventory_sha256(&hostile), hash);
        hostile[0] = WORKER_REGISTRY[0];
        hostile[0].cadence = CadencePolicy::OnDemand;
        assert_ne!(registry_inventory_sha256(&hostile), hash);
    }

    #[test]
    fn startup_configuration_is_registry_owned_and_fails_closed() {
        let lookup = |key: &str| match key {
            "MDE_ANSIBLE_PULL_URL" => Some(std::ffi::OsString::from("https://fleet.invalid/repo")),
            "MDE_OVERLAY_USGS_EARTHQUAKES" => Some(std::ffi::OsString::from("OFF")),
            _ => None,
        };

        assert!(startup_configuration_allows_with("ansible-pull", lookup));
        assert!(!startup_configuration_allows_with("vehicle", lookup));
        assert!(!startup_configuration_allows_with(
            "earthquake_overlay",
            lookup
        ));
        assert!(!startup_configuration_allows_with("uncensused", lookup));
        assert!(!startup_configuration_allows_with("ansible-pull", |_| {
            Some(std::ffi::OsString::new())
        }));

        let ansible = spec("ansible-pull").expect("ansible-pull registry row");
        assert_eq!(
            ansible.activation.config,
            ConfigPredicate::EnvironmentPresent("MDE_ANSIBLE_PULL_URL")
        );
        assert_eq!(
            ansible.cadence,
            CadencePolicy::Periodic {
                min_interval_secs: 900,
                max_interval_secs: 900,
            }
        );
    }

    #[test]
    fn the_table_is_the_current_role_tiered_worker_census() {
        // Guards against a worker added to run_serve without a deliberate tier
        // (it would silently default to Lighthouse). 31 originally; -1 redundant
        // python `clipboard` (RETIRE-PY.3), -1 broken python
        // `mdns` relay (RETIRE-PY.1), +1 native `mdns_relay` (MESH-MDNS-RELAY,
        // the real Rust cross-segment relay), -1 dead python `fs_sync` GVFS
        // worker (RETIRE-PY.4, mesh storage is Syncthing under SUBSTRATE-V2).
        // -12 sway/desktop workers (E11 'Cosmic owns the desktop' — the
        // labwc/sway worker stack deleted). +1 ssh_pubkey_gossip (SVC-2),
        // +1 fleet_reconcile (PD-9), +1 presence_watch (PD-13),
        // lifecycle_exec (PD-11) was retired by WL-ARCH-010; WorkloadCompute
        // is now the sole VM/container actuator. +1 job_exec (PLANES-9),
        // +1 mesh_dns (PLANES-18), +1 netstate_apply (PLANES-15),
        // +1 validation_suite (PLANES-19), +1 metrics_exporter (EFF-9),
        // +1 hardware_probe (SUBAUDIT-D2 — the PeerProbe producer).
        // +1 clipboard_sync (CLIP-SYNC-1 — the mesh clipboard watcher; it is the
        // SOLE clipboard capturer, spawning `wl-paste --watch` directly. The
        // never-built `mde-clipd` daemon + its `clipd_supervisor` worker were
        // removed in CLIP-SYNC-2: that binary never existed in the workspace).
        // +1 etcd_watch (SUBSTRATE-10 — the coordination-plane WATCH worker that
        // pushes instant peer-down / leader-change alerts off etcd watch streams).
        // +1 music_autoconfig (MEDIA-8 — Workstation music birthright: resolves
        // an Airsonic server record's secret reference into local user creds).
        // +1 link-traffic (MESHMAP-6 — per-link byte-counter collector, rank 0).
        // +1 mesh_mount (FILEMGR-5 — the Files-surface sshfs mesh-mount worker,
        // Workstation-tier: a seated-user desktop feature).
        // +1 bookmarks (BOOKMARKS-2 — the mesh-synced bookmarks worker,
        // Workstation-tier: a seated-user desktop feature).
        // +1 desktop_sources (CHOOSER-1 — the desktop-source discovery
        // aggregator, Workstation-tier: a seated-user desktop feature).
        // +1 media_sources (MEDIA-14 — the mesh media-source discovery
        // aggregator, Workstation-tier: a seated-user desktop feature).
        // +1 media_server (MEDIA-15 — the mesh media server + DLNA + aggregation,
        // the PRODUCER half; Workstation-tier: a seated-user desktop feature).
        // +1 media_airsonic_proxy (WL-FUNC-014 — AirSonic/Subsonic gateway proxy,
        // Workstation-tier media gateway).
        // +1 pty_broker (TERM-7 — the mesh PTY-broker opening remote shells over
        // the overlay, Workstation-tier: a seated-user desktop feature).
        // +1 adfilter (BOOKMARKS-7 — the mesh-wide ad-blocker worker replicating the
        // filter-store blob + leader-compiling the engine, Workstation-tier: a
        // seated-user desktop-governance feature).
        // +1 browser_policy (BOOKMARKS-8 — the mesh-wide browser/ad-blocker POLICY
        // worker: reads the synced fleet policy doc + enforces at the browser
        // launch seam, Workstation-tier: a seated-user desktop-governance feature).
        // +1 browser_session_sync (BROWSER-DD-7 — Browser follow-me/startup-restore
        // session snapshots mirrored onto the Syncthing file plane,
        // Workstation-tier browser feature).
        // +1 browser_read_aloud (BROWSER-DD-11 — Browser read-aloud/TTS owner,
        // Workstation-tier accessibility feature).
        // +1 storage (BUG-STORAGE-1 — the E12-20 universal per-node topology mirror,
        // pinned at rank 0 so it is a deliberate census entry on every role instead
        // of riding the silent unknown-worker default that hid it from role-workers).
        // +1 unit_aggregator (EXPLORER-1 — the Hero unit-explorer daemon spine,
        // pinned at rank 0: every node folds + publishes its OWN unit view
        // (state/units/<node>), no center; the BUG-STORAGE-1 deliberate-entry lesson).
        // +1 notify (CHAT-FIX-2 — the local-notification producer, pinned at rank 0:
        // every node reports its own peer/service/disk/journal events into the Chat
        // feed the chat worker folds; the real empty-Chat fix).
        // KDC-MESH-3 (#15) — kdc_host MOVED from rank 1 → rank 0 (universal KDE
        // Connect host: every node recognizes the phone, overlay-only so no public
        // port opens). A tier move, not an add, so the total is unchanged; the
        // rank split shifts 26/16 → 27/15 (see `tier_counts_match_the_two_role_split`).
        // +1 chat (CHAT-FIX-1 — the universal mesh chat worker, pinned at rank 0:
        // it already ran on every node via the silent unknown-worker default; now
        // it is an EXPLICIT census entry so `mackesd role-workers` lists it. The
        // rank split shifts 27/15 → 28/15, len 42 → 43).
        // +1 node_grade (NODE-GRADE-1 — the universal per-node self-grade worker,
        // pinned at rank 0: every node computes + publishes its own A–F capability
        // grade. The rank split shifts 28/15 → 29/15, len 43 → 44).
        // +1 device_control (DEVMGR-8 — the universal per-node device-control
        // executor, pinned at rank 0: every node can be a device-action target and
        // drains its own fleet/device-control/<self> dir. Split 29/15 → 30/15, len
        // 44 → 45).
        // +1 transfers (TRANSFERS-1 — the Workstation-tier transfers queue/ledger/
        // verb spine, sibling of pty_broker/mesh_mount. Split 30/15 → 30/16, len
        // 45 → 46). +1 browser_session_sync shifts split 30/16 → 30/17, len 46 → 47.
        // +1 browser_read_aloud shifts split 30/17 → 30/18, len 47 → 48.
        // +1 browser_voice_command shifts split 30/18 → 30/19, len 48 → 49.
        // +1 browser_translate shifts split 30/19 → 30/20, len 49 → 50.
        // +1 browser_offline_cache shifts split 30/20 → 30/21, len 50 → 51.
        // +1 browser_security_update shifts split 30/21 → 30/22, len 51 → 52.
        // +1 browser_tab_suspend shifts split 30/22 → 30/23, len 52 → 53.
        // +1 browser_protocol shifts split 30/23 → 30/24, len 53 → 54.
        // +1 browser_share shifts split 30/24 → 30/25, len 54 → 55.
        // +1 seat_remote_input shifts split 30/25 → 30/26, len 55 → 56.
        // +1 browser_passkeys shifts split 30/26 → 30/27, len 56 → 57.
        // ARCH-5 (drift guard) +14 universal rank-0 workers that were riding the
        // silent "unknown worker ⇒ rank 0" default (spawned + `runs(...)`-gated but
        // uncensused → hidden from `mackesd role-workers`, the BUG-STORAGE-1 class):
        // boot_readiness, kvm_health, scheduler, session_broker,
        // session_roaming,
        // service_onboard, spawn_lighthouse_onboard, onboard_apply, lighthouse_probe.
        // All rank 0 (behavior-preserving), so the split shifts 30/27 → 44/27,
        // len 57 → 71. The `worker_spawns_and_the_census_do_not_drift` test now keeps
        // the census + the run_serve spawn sites from silently diverging again.
        // +1 service_aggregator (WL-FUNC-008 — the universal rank-0 unified
        // service-provenance/health view; rank-0 44 → 45). Reconcile: the rank-1
        // census had already drifted +1 — `peer_app_launch` (WL-UX-005) is a rank-1
        // registry entry that was never counted here — so the real split is 45/28,
        // len 73 (this assertion + the tier-split counts below stale-asserted 71/27).
        // +1 router_action (WL-RUN-006 — the universal rank-0 router firewall-edit
        // executor; rank-0 44 → 45, len 72 → 73).
        // +1 federation_enforcer (WL-SEC-002 — the universal rank-0 cross-mesh
        // WL-SEC-002 +1 federation_enforcer; WL-FUNC-011 Phase 2 +1 collab
        // (chat's Phase-4 successor); WL-ARCH-001 -1 openstack (removed) => len 75.
        // WL-ARCH-001 Phase B +1 cloud (the OpenTofu+Ansible backend, the universal
        // rank-0 successor to the removed openstack worker) => len 76.
        // Rolling Node +1 vehicle (the universal rank-0 MG90 vehicle-gateway mirror,
        // a no-op where no gateway is attached) => len 77.
        // WL-FUNC-011 U2 -1 voice_config (the Kamailio/RTPengine VV render-config
        // worker; Q9 retired the dead SIP-proxy stack) => len 76.
        // WL-FUNC-012 +2 workstation-tier keyless adapters: USGS earthquakes
        // (default-on, explicit false opt-out), NWS active alerts, and
        // adsb.lol aircraft (explicit opt-in; unconfigured is idle) => len 79.
        // OVERLAY-9 adds MBTA transit => 80;
        // OVERLAY-4 adds NWS hourly guidance => 81; OVERLAY-5 adds Caltrans
        // traffic cameras => 82; OVERLAY-2 adds IEM NEXRAD radar => 83;
        // OVERLAY-6 adds keyless NIFC WFIGS perimeters => 84; OVERLAY-3 adds
        // keyless NCDOT TIMS events => 85; AirNow AQI => 86. AirSonic and
        // Jellyfin gateway proxies brought the pre-cutover census to 90; the
        // Chromium Browser VM hard cut removed all 11 host Browser workers,
        // and retiring the duplicate VM/container tiers plus the raw console
        // relay leaves 76 role-tiered
        // workers in the current registry.
        assert_eq!(WORKER_REGISTRY.len(), 159);
        assert_eq!(
            WORKER_REGISTRY
                .iter()
                .filter(|worker| matches!(
                    worker.spawn_binding,
                    SpawnBinding::Tiered | SpawnBinding::DynamicSupervisor
                ))
                .count(),
            81
        );
    }

    #[test]
    fn retired_clipboard_authority_has_no_source_or_spawn_surface() {
        let forbidden = [
            ["clipboard", "_bridge"].concat(),
            ["Clipboard", "BridgeWorker"].concat(),
            ["OsClipboard", "Access"].concat(),
            ["action", "/vdi/", "clipboard"].concat(),
        ];
        for path in rust_sources(&crate_src_dir()) {
            let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!(
                    "WL-ARCH-010 S6 guard: cannot read {}: {error}",
                    path.display()
                )
            });
            for retired in &forbidden {
                assert!(
                    !source.contains(retired),
                    "WL-ARCH-010 S6: retired clipboard authority `{retired}` remains in {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn strict_resolver_error_names_the_fix() {
        // ENT-2 — we can't unpin the dev box's real role.toml from a
        // test, but the error contract is pure: both failure arms
        // must name `mackesd role pin`. Pin the strings.
        // (The fail-closed behavior itself is smoked in CI via
        // `mackesd serve` on a roleless container — OBS-2 scope.)
        let unpinned_msg = matches!(
            mde_role::load_from(std::path::Path::new("/nonexistent/ent2/role.toml")),
            Err(mde_role::LoadError::NotPinned)
        );
        assert!(unpinned_msg, "absent file reads NotPinned");
    }

    #[test]
    fn tier_counts_match_the_two_role_split() {
        let count = |rank: u8| {
            WORKER_REGISTRY
                .iter()
                .filter(|s| {
                    matches!(
                        s.spawn_binding,
                        SpawnBinding::Tiered | SpawnBinding::DynamicSupervisor
                    ) && s.min_rank == rank
                })
                .count()
        };
        assert_eq!(
            count(0),
            43,
            "Lighthouse control plane plus universal storage/service/notification/control workers, with retired VM/container lifecycle and raw console relay absent"
        );
        assert_eq!(
            count(1),
            38,
            "Workstation = fleet/actions + desktop data + seat input + media gateways + the WL-FUNC-012 provider adapters; all retired host Browser workers are absent after the Chromium VM cutover"
        );
        // No middle tier in the 2-role model — Workstation is the top rank.
        assert_eq!(
            count(2),
            0,
            "the retired Server/XCP-NG tier (rank 2) is gone"
        );
    }

    #[test]
    fn lighthouse_runs_only_the_control_plane() {
        let r = Role::Lighthouse.rank();
        for w in [
            "nebula_supervisor",
            "heartbeat",
            "health_reconciler",
            "mesh_router",
            "bus_supervisor",
        ] {
            assert!(runs(w, r), "Lighthouse must run {w}");
        }
        // KDC-MESH-3 (#15) — kdc_host is NO LONGER in this list: it is now a
        // universal rank-0 worker that DOES run on a Lighthouse (see
        // `kdc_host_runs_on_every_role`). Overlay-only, so it opens no public port.
        for w in ["ansible-pull", "app-sync"] {
            assert!(!runs(w, r), "Lighthouse must NOT run {w}");
        }
    }

    #[test]
    fn workstation_adds_fleet_and_desktop() {
        // The retired Server tier folded into Workstation: it now runs BOTH the
        // fleet workers AND the desktop stack (a headless box runs them too — the
        // desktop workers idle without a display).
        let r = Role::Workstation.rank();
        for w in ["ansible-pull", "app-sync", "clipboard_sync", "kdc_host"] {
            assert!(runs(w, r), "Workstation must run {w}");
        }
    }

    #[test]
    fn workstation_runs_every_worker() {
        let r = Role::Workstation.rank();
        for spec in WORKER_REGISTRY {
            assert!(runs(spec.name, r), "Workstation must run {}", spec.name);
        }
    }

    #[test]
    fn storage_mirror_publishes_on_every_role_including_workstation() {
        // BUG-STORAGE-1 — the storage worker is a universal per-node topology
        // mirror. It MUST spawn (and thus publish `state/storage/<node>`) on a
        // Workstation — a seated user manages their local disks — and still on a
        // Lighthouse (an honest, often-Unavailable mirror). Pinned at rank 0.
        assert_eq!(
            min_rank("storage"),
            0,
            "storage is a universal (rank-0) worker"
        );
        assert!(
            runs("storage", Role::Workstation.rank()),
            "the storage mirror MUST run on a Workstation (the live BUG-STORAGE-1)"
        );
        assert!(
            runs("storage", Role::Lighthouse.rank()),
            "the storage mirror still runs on a Lighthouse"
        );
        // ...and it is a DELIBERATE census entry now, so the `mackesd role-workers`
        // diagnostic (workers_for_rank) lists it on both roles instead of silently
        // omitting it (the omission that read as "storage doesn't run here").
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"storage"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"storage"));
        // The read/publish eligibility carries no capability gate — a plain rank
        // is enough (the live UDisks2 executor is gated inside the worker, not here).
        assert_eq!(required_capability("storage"), None);
    }

    #[test]
    fn unit_aggregator_runs_on_every_role() {
        // EXPLORER-1 — the Hero unit-explorer daemon spine is universal (#18/#20:
        // every node folds + publishes its OWN unit view, no center). It MUST spawn
        // on every role — a lighthouse publishes an honest units view too — and it
        // is a DELIBERATE rank-0 census entry (the BUG-STORAGE-1 lesson), never the
        // silent unknown-worker default.
        assert_eq!(
            min_rank("unit_aggregator"),
            0,
            "unit_aggregator is a universal (rank-0) worker"
        );
        assert!(runs("unit_aggregator", Role::Workstation.rank()));
        assert!(runs("unit_aggregator", Role::Lighthouse.rank()));
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"unit_aggregator"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"unit_aggregator"));
        // No capability tag — every node runs it.
        assert_eq!(required_capability("unit_aggregator"), None);
    }

    #[test]
    fn notify_producer_runs_on_every_role() {
        // CHAT-FIX-2 — the local-notification producer is universal (rank 0): every
        // node has its own services / disks / journal / peers to report into the
        // Chat feed the chat worker folds. A DELIBERATE rank-0 census entry (the
        // BUG-STORAGE-1 lesson), never the silent unknown-worker default — so
        // `mackesd role-workers` lists it on both roles.
        assert_eq!(
            min_rank("notify"),
            0,
            "notify is a universal (rank-0) worker"
        );
        assert!(runs("notify", Role::Workstation.rank()));
        assert!(runs("notify", Role::Lighthouse.rank()));
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"notify"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"notify"));
        // No capability tag — every node runs it.
        assert_eq!(required_capability("notify"), None);
    }

    #[test]
    fn node_grade_runs_on_every_role() {
        // NODE-GRADE-1 (node-grade.md #11) — the per-node self-grade worker is
        // UNIVERSAL (rank 0): every node computes + publishes its OWN A–F capability
        // grade, so a lighthouse grades itself too (its own headroom/health/reach
        // matters to the dock's grade list). A DELIBERATE rank-0 census entry (the
        // BUG-STORAGE-1 lesson), never the silent unknown-worker default.
        assert_eq!(
            min_rank("node_grade"),
            0,
            "node_grade is a universal (rank-0) worker"
        );
        assert!(runs("node_grade", Role::Workstation.rank()));
        assert!(runs("node_grade", Role::Lighthouse.rank()));
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"node_grade"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"node_grade"));
        // No capability tag — every node runs it.
        assert_eq!(required_capability("node_grade"), None);
    }

    #[test]
    fn kdc_host_runs_on_every_role() {
        // KDC-MESH-3 (kdc-mesh.md #15) — the KDE Connect host is UNIVERSAL (rank 0):
        // it MUST spawn on EVERY node incl. a headless Lighthouse, so the mesh-wide
        // "every node recognizes the phone" (#5) + "all nodes serve at once" (#6)
        // goals hold. It was Workstation-only (rank 1) before; the move is safe
        // because KDC-MESH-1's transport is overlay-only (binds 1716 on the Nebula
        // overlay IP, never the public NIC — so a lighthouse opens no public port).
        assert_eq!(
            min_rank("kdc_host"),
            0,
            "kdc_host is a universal (rank-0) worker"
        );
        assert!(
            runs("kdc_host", Role::Workstation.rank()),
            "a Workstation still runs the KDE Connect host"
        );
        assert!(
            runs("kdc_host", Role::Lighthouse.rank()),
            "a Lighthouse now runs the KDE Connect host too (overlay-only, no public port)"
        );
        // A DELIBERATE census entry, so `mackesd role-workers` lists it on both roles.
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"kdc_host"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"kdc_host"));
        // No capability tag — every node runs it (the overlay-only transport is the
        // gate that keeps it safe on a headless/relay node, not a role tag).
        assert_eq!(required_capability("kdc_host"), None);
    }

    #[test]
    fn chat_runs_on_every_role() {
        // CHAT-FIX-1 — the mesh chat worker is UNIVERSAL (rank 0): it MUST spawn on
        // EVERY node incl. a headless Lighthouse (live-verified on Eagle: boot log
        // `starting worker: chat`). It always ran everywhere via the silent
        // "unknown worker ⇒ rank 0" default; this pins it as an EXPLICIT census
        // entry so `mackesd role-workers` honestly lists it on both roles.
        assert_eq!(min_rank("chat"), 0, "chat is a universal (rank-0) worker");
        assert!(
            runs("chat", Role::Workstation.rank()),
            "a Workstation runs the mesh chat worker"
        );
        assert!(
            runs("chat", Role::Lighthouse.rank()),
            "a Lighthouse runs the mesh chat worker too (it always did, now explicit)"
        );
        // Present in the census table now, not riding the unknown-worker default.
        assert!(WORKER_REGISTRY.iter().any(|s| s.name == "chat"));
        // A DELIBERATE census entry, so `mackesd role-workers` lists it on both roles.
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"chat"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"chat"));
        // No capability tag — every node runs it.
        assert_eq!(required_capability("chat"), None);
    }

    #[test]
    fn role_name_maps_each_rank_to_its_canonical_name() {
        // Keep diagnostics and role-file projections on canonical names.
        assert_eq!(role_name(Role::Lighthouse.rank()), "lighthouse");
        assert_eq!(role_name(Role::Workstation.rank()), "workstation");
        // An out-of-range rank falls back to the top tier (tolerant posture).
        assert_eq!(role_name(9), "workstation");
    }

    #[test]
    fn unknown_worker_rank_is_tolerant_but_the_spawn_gate_fails_closed() {
        assert_eq!(min_rank("some-future-worker"), 0);
        assert!(!runs("some-future-worker", Role::Lighthouse.rank()));
        assert!(!runs("some-future-worker", Role::Workstation.rank()));
    }

    #[test]
    fn music_autoconfig_is_workstation_role_gated_not_media_host_gated() {
        assert!(!runs_in(
            "music_autoconfig",
            DeployClass::plain(Role::Lighthouse.rank())
        ));
        assert!(runs_in(
            "music_autoconfig",
            DeployClass::plain(Role::Workstation.rank())
        ));
        assert!(runs_in(
            "music_autoconfig",
            DeployClass {
                rank: Role::Workstation.rank(),
                media: false,
            }
        ));
    }

    #[test]
    fn workers_for_rank_is_a_growing_superset() {
        let lh = workers_for_rank(Role::Lighthouse.rank());
        let ws = workers_for_rank(Role::Workstation.rank());
        // 30 lighthouse-tier workers (22 control-plane + the BUG-STORAGE-1 universal
        // storage mirror + the EXPLORER-1
        // universal unit_aggregator + the CHAT-FIX-2 universal notify producer + the
        // NODE-GRADE-1 universal node_grade self-grade + the KDC-MESH-3 universal
        // kdc_host + the CHAT-FIX-1 universal chat worker + the DEVMGR-8 universal
        // device_control executor at rank 0); Workstation adds the 22 fleet + desktop
        // workers (incl. the TRANSFERS-1 transfers worker, BROWSER-DD-6
        // browser_passkeys owner, BROWSER-DD-7 browser_session_sync owner,
        // BROWSER-DD-11 browser read-aloud +
        // voice-command owners, and BROWSER-DD-12 browser_protocol +
        // browser_share + browser_translate + browser_offline_cache +
        // browser_security_update + browser_tab_suspend owners, plus the
        // KDC-MESH-6 seat_remote_input consumer) for the full 57 (the retired
        // Server tier folded into Workstation in the 2-role model).
        // ARCH-5 (drift guard) +14 universal rank-0 workers censused (30 → 44),
        // so both roles grow by 14: lh 30 → 44, ws 57 → 71.
        // WL-FUNC-008 +1 rank-0 service_aggregator → lh 45; reconcile +1 rank-1
        // peer_app_launch (WL-UX-005, previously uncounted) → ws = 45 + 28 = 73.
        // WL-SEC-002 +1 federation_enforcer; WL-FUNC-011 Phase 2 +1 collab;
        // WL-ARCH-001 -1 openstack (removed) => lh 47, ws = 47 + 28 = 75.
        // WL-ARCH-001 Phase B +1 rank-0 cloud (the OpenTofu+Ansible backend) → lh 48,
        // ws = 48 + 28 = 76.
        // Rolling Node +1 rank-0 vehicle (the MG90 vehicle-gateway mirror) → lh 49,
        // ws = 49 + 28 = 77.
        // WL-FUNC-011 U2 -1 workstation-tier voice_config (Q9 dead VV stack retired)
        // → lh 49 (rank-0 unchanged), ws = 49 + 27 = 76.
        // WL-FUNC-012 +2 workstation-tier earthquake_overlay + nws_alert_overlay
        // WL-FUNC-012 OVERLAY-8 +1 rank-1 aircraft_overlay
        // → lh 49 (unchanged), ws = 49 + 30 = 79.
        // WL-FUNC-012 OVERLAY-9 +1 rank-1 transit_overlay => ws 80.
        // WL-FUNC-012 OVERLAY-4 +1 rank-1 nws_forecast_overlay => ws 81.
        // WL-FUNC-012 OVERLAY-5 +1 rank-1 caltrans_camera_overlay => ws 82.
        // WL-FUNC-012 OVERLAY-2 +1 rank-1 iem_radar_overlay => ws 83.
        // WL-FUNC-012 OVERLAY-6 +1 rank-1 wildfire_overlay => ws 84.
        // WL-FUNC-012 OVERLAY-3 +1 rank-1 traffic_overlay => ws 85.
        // WL-FUNC-012 OVERLAY-7 +1 rank-1 air_quality_overlay => ws 86.
        // WL-FUNC-012 OVERLAY-6 +1 rank-1 firms_overlay => ws 88. The two media
        // gateway proxies brought the real pre-cutover count to 90. The current
        // canonical roster contains 81 tiered/dynamic registrations, 70 direct
        // supervisors/responders, and eight process-infrastructure rows. The
        // latter are universal census entries but still have one exact group
        // owner at runtime; all retired VM authorities are absent.
        assert_eq!(lh.len(), 121);
        assert_eq!(ws.len(), 159);
        // The universal storage mirror is now a listed census entry on BOTH roles
        // (it previously ran but was omitted from this diagnostic listing).
        assert!(
            lh.contains(&"storage"),
            "Lighthouse lists the storage mirror"
        );
        assert!(
            ws.contains(&"storage"),
            "Workstation lists the storage mirror"
        );
        // Strict superset: every lighthouse worker is also in the workstation set.
        assert!(lh.iter().all(|w| ws.contains(w)));
    }

    // ── Retired media capability gate ──

    #[test]
    fn navidrome_is_disabled_for_all_lighthouse_classes() {
        // Keep the legacy capability declaration for wire compatibility, but
        // never schedule the retired media worker.
        assert_eq!(required_capability("navidrome"), Some(Capability::Media));
        // A legacy media marker does not unlock it.
        let media_lh = DeployClass {
            rank: Role::Lighthouse.rank(),
            media: true,
        };
        assert!(
            !runs_in("navidrome", media_lh),
            "retired media marker must not run navidrome"
        );
        // ...but a stock lighthouse / workstation WITHOUT the tag does NOT
        // (acceptance: container absent on a non-media node), even at higher rank.
        for rank in [Role::Lighthouse.rank(), Role::Workstation.rank()] {
            assert!(
                !runs_in("navidrome", DeployClass::plain(rank)),
                "rank {rank} without the media tag must NOT run navidrome"
            );
        }
        // The rank-only `runs` never starts a capability worker (it has no tag).
        assert!(!runs("navidrome", Role::Workstation.rank()));
    }

    #[test]
    fn retired_media_tag_does_not_extend_the_worker_tier() {
        let media_lh = DeployClass {
            rank: Role::Lighthouse.rank(),
            media: true,
        };
        assert!(runs_in("nebula_supervisor", media_lh), "still a lighthouse");
        assert!(
            !runs_in("ansible-pull", media_lh),
            "media ≠ workstation (fleet) tier"
        );
        let set = workers_for_class(media_lh);
        // = the 30 lighthouse-tier workers (incl. link-traffic MESHMAP-6, the
        // BUG-STORAGE-1 universal storage mirror, the EXPLORER-1 universal
        // unit_aggregator, the CHAT-FIX-2
        // universal notify producer, the NODE-GRADE-1 universal node_grade
        // self-grade, the KDC-MESH-3 universal kdc_host, the CHAT-FIX-1 universal
        // chat worker + the DEVMGR-8 universal device_control executor + the
        // WL-RUN-006 universal router_action executor + the WL-SEC-002 universal
        // federation_enforcer + the ARCH-5 14 universal
        // rank-0 workers + WL-FUNC-008 service_aggregator + WL-FUNC-011 Phase 2
        // collab + WL-ARCH-001 Phase B cloud + Rolling Node vehicle, minus the
        // removed openstack) + navidrome.
        assert!(!set.contains(&"navidrome"));
        assert!(set.contains(&"nebula_supervisor"));
        assert!(!set.contains(&"ansible-pull"));
        // A plain lighthouse class never includes the media worker.
        let plain_lh = DeployClass::plain(Role::Lighthouse.rank());
        assert!(!workers_for_class(plain_lh).contains(&"navidrome"));
    }

    #[test]
    fn deploy_class_from_role_class_drops_the_retired_media_tag() {
        let media = DeployClass::from_role_class(&RoleClass {
            role: Role::Lighthouse,
            media: true,
        });
        assert_eq!(media.rank, 0);
        assert!(!media.media);
        let ws = DeployClass::from_role_class(&RoleClass::plain(Role::Workstation));
        assert_eq!(ws.rank, 1);
        assert!(!ws.media);
    }
}
