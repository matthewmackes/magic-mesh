//! `mackesd` — CLI entry point for the Mesh control plane.
//!
//! Subcommands land alongside their backing Phase 12 substeps. Today
//! only `mackesd migrate` ships (Phase 12.2 store + migrations); the
//! rest follow as substeps complete. We deliberately do NOT register
//! stub commands here — every `mackesd X` either does X or is absent.

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "mackesd",
    version,
    about = "Magic Mesh control plane — secure no-fixed-center workgroup mesh on Fedora-Cosmic"
)]
struct Cli {
    /// Override the default `SQLite` store path (defaults to
    /// `$MACKESD_HOME/mackesd.db` or `/var/lib/mackesd/mackesd.db`).
    #[arg(long, env = "MACKESD_DB")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Apply every pending `SQLite` migration against the store.
    ///
    /// Idempotent — running `mackesd migrate` against an up-to-date
    /// store is a no-op that exits 0.
    Migrate,

    /// Print store status: applied-migration count + db path.
    Status,

    /// Print the live `HealthReport` as a JSON line (Phase 12.1.3).
    ///
    /// Same shape as `mackesd_core::health::HealthReport` so the
    /// panel + the CLI consume identical data.
    Healthz,

    /// MESH-A-7 (v5.0.0) — resolve the connect-action for a host:port
    /// from the 12 well-known service mappings (R8-Q50). Prints
    /// `<service>\t<launch argv>` (e.g. port 22 → `ssh <ip>`, port 80
    /// → `xdg-open http://<ip>`) for the operator / host-card UI to
    /// run. Exits 0 when the port maps to a known service, 1 otherwise.
    Connect {
        /// Target host IP — a mesh-peer overlay IP or a LAN neighbor.
        #[arg(value_name = "IP")]
        ip: String,
        /// TCP port the service listens on.
        #[arg(value_name = "PORT")]
        port: u16,
    },

    /// MESH-A-4.a (v5.0.0) — classify a surrounding host into one of
    /// the 14 R8-Q9 types from discovery signals. Repeatable
    /// `--mdns` / `--port` flags + an optional `--vendor`; prints the
    /// resolved kebab-case type name. The MESH-A-4.b collectors will
    /// gather these signals from the wire; this surfaces the
    /// classifier for manual checks + smoke tests.
    ClassifyHost {
        /// mDNS service type advertised (repeatable), e.g. `_ipp._tcp`.
        #[arg(long = "mdns", value_name = "SERVICE")]
        mdns: Vec<String>,
        /// Open TCP port observed (repeatable).
        #[arg(long = "port", value_name = "PORT")]
        port: Vec<u16>,
        /// MAC-OUI vendor string.
        #[arg(long = "vendor", value_name = "VENDOR", default_value = "")]
        vendor: String,
        /// Hostname (feeds the console hostname hint, MESH-A-4.b.2).
        #[arg(long = "hostname", value_name = "HOSTNAME", default_value = "")]
        hostname: String,
        /// MAC address — its OUI resolves the vendor via the system OUI
        /// table when --vendor is not given (MESH-A-4.b.3).
        #[arg(long = "mac", value_name = "MAC", default_value = "")]
        mac: String,
    },

    /// MESH-A-4.b.1 (v5.0.0) — browse the LAN for mDNS services
    /// (`avahi-browse -aprt`), group them by host, classify each, and
    /// print one `SurroundingHost` JSON line per discovered host.
    /// Empty output when `avahi-browse` is absent.
    DiscoverMdns,

    /// MESH-A-4.c.4 (v5.0.0) — print the mesh-wide surrounding-host
    /// view: the union of every peer's latest snapshot, coalesced into
    /// one card per host (MAC identity) with sighting count + roaming
    /// IPs. One `CoalescedHost` JSON line per host.
    SurroundingList,

    /// MESH-A-4.d (v5.0.0) — set a surrounding host's operator trust
    /// (the Trust / Block card actions, R8-Q11). KEY is the host's MAC
    /// (preferred) or IP; STATE is `trusted` | `blocked` | `unknown`
    /// (`unknown` clears the override). Persists to the mesh-synced
    /// `surrounding/trust.json`.
    SurroundingTrust {
        /// Host identity key — MAC (preferred) or IP.
        #[arg(value_name = "KEY")]
        key: String,
        /// Trust state: `trusted` | `blocked` | `unknown`.
        #[arg(value_name = "STATE")]
        state: String,
    },

    /// MESH-A-5.1 (v5.0.0) — print the mesh-coordinated firewall DROP
    /// plan: a firewalld source-DROP rich-rule for every IP a Blocked
    /// host (operator trust = blocked) was seen at, roaming-aware. The
    /// A-5.2 worker applies these via firewall-cmd; this prints them.
    MeshFirewallPlan,

    /// MESH-A-6.1 (v5.0.0) — scan the ARP/neighbour table for spoofing
    /// suspects (R8-Q53): a MAC bound to 2+ IPv4 addresses. Prints one
    /// `<mac>\t<ip,ip,…>` line per suspect (empty when clean).
    ArpSpoofCheck,

    /// MESH-A-6.2 (v5.0.0) — broadcast a DHCP discover
    /// (`nmap --script broadcast-dhcp-discover`) + list the responding
    /// DHCP servers (R8-Q54). Prints one server IP per line; warns +
    /// exits 1 when 2+ respond (rogue DHCP).
    RogueDhcpCheck,

    /// MESH-A-6.4 (v5.0.0) — probe for a captive portal (R8-Q31): a
    /// `generate_204` check returning non-204 means a portal intercepted
    /// it. Prints the portal URL (for the UI to open) + exits 1 when
    /// captive; silent + exit 0 when clear.
    CaptivePortalCheck,

    /// VOIP-4 (v5.0.0) — measure this peer's Vitelity-link RTT (TCP-connect
    /// to `out.vitelity.net:5061`) + publish it to `voip/link-rtt/<peer>`.
    /// Prints the RTT in ms (or "unreachable").
    VoipRtt,

    /// E1.2 — list the mackesd workers a deployment role runs (the role-gated
    /// worker subset per plan §12). With no ROLE, prints all three tiers. This
    /// is the static counterpart to the live `worker_names` listing `serve`
    /// builds, so an operator/installer can preview a role before pinning it.
    /// ENT-2 / PKG-4 — pin this box's deployment role (upgrade-only;
    /// a downgrade is refused). The installer/role-chooser calls this;
    /// operators can upgrade a box in place.
    RolePin {
        /// `lighthouse` | `server` | `workstation`.
        role: String,
    },

    /// PLANES-3 (W82/83) — view or set a node's capability tags
    /// (hop|execution|headless). Any enrolled box may set any
    /// target's tags; the change is audit-logged. With no `--set`,
    /// prints the target's current tags.
    Tag {
        /// Target hostname (defaults to this box).
        #[arg(long)]
        host: Option<String>,
        /// Comma-separated tag set to write (replaces the existing
        /// set). Omit to just show.
        #[arg(long)]
        set: Option<String>,
    },

    /// Show which mackesd workers each deployment role runs (the
    /// Lighthouse ⊂ Server ⊂ Workstation tier table).
    RoleWorkers {
        /// lighthouse | server | workstation (default: all three tiers).
        role: Option<String>,
    },

    /// PLANES-17 (W72/W73) — advertise this node as a hop: the underlay
    /// subnets it routes for the fleet. Every other peer installs a
    /// `tun.unsafe_routes` edge through this node's overlay IP. Pass
    /// `0.0.0.0/0` (or `--exit`) to offer a full exit — but the exit edge
    /// only activates fleet-wide once a validation run passes (W73).
    HopAdvertise {
        /// Comma-separated subnets in CIDR form (e.g. `192.168.50.0/24`).
        #[arg(long, value_name = "CIDRS")]
        subnets: Option<String>,
        /// Shorthand for advertising the `0.0.0.0/0` full-exit route.
        #[arg(long)]
        exit: bool,
    },

    /// PLANES-17 — import an external VPN *client* profile (WireGuard /
    /// OpenVPN) into the replicated store. These reach external networks;
    /// they are never the mesh transport (§1 — Nebula is the only overlay).
    VpnImport {
        /// Profile name (the stored filename stem).
        #[arg(long)]
        name: String,
        /// `wireguard` | `openvpn`.
        #[arg(long)]
        kind: String,
        /// Path to the profile config file to import.
        #[arg(long)]
        file: std::path::PathBuf,
    },

    /// E1.3 — systemd `ExecCondition` role gate. Exits 0 when the box's pinned
    /// deployment rank is at least `--min-rank`, else exits non-zero (systemd
    /// then *skips* the unit rather than failing it) after logging the conflict
    /// to the journal. The role-gated units use it so a forbidden service
    /// refuses to start: `mde-session`/`greetd` require rank 2 (Workstation),
    /// `lizardfs`/`ansible-pull.timer` require rank 1 (Server+).
    RoleGate {
        /// Minimum deployment rank the calling unit requires (0/1/2).
        #[arg(long = "min-rank", value_name = "RANK")]
        min_rank: u8,
    },

    /// MESH-A-6.5 (v5.0.0) — detect a DNS leak (R8-Q41): a configured
    /// /etc/resolv.conf resolver not in `--expected` (the mesh resolver
    /// set). Prints leaked resolvers + exits 1 when any.
    DnsLeakCheck {
        /// Expected mesh resolver IP (repeatable).
        #[arg(long = "expected", value_name = "IP")]
        expected: Vec<String>,
    },

    /// MESH-A-6.3 (v5.0.0) — scan WiFi for evil-twin APs (R8-Q60): a
    /// known SSID advertised by a BSSID not in the learned baseline
    /// (`surrounding/wifi-baseline.json`). Prints `<ssid>\t<bssid>` per
    /// suspect + exits 1; learns the current scan into the baseline.
    EvilTwinCheck,

    /// MESH-A-6.8 (v5.0.0) — record a persistent-attack hit from SOURCE
    /// (R8-Q74): coalesces into one accumulating alert per source +
    /// auto-acks alerts quiet > 24h. Prints the source's current alert.
    /// Persists to `surrounding/persistent-alerts.json`.
    RecordAttack {
        /// Attack source — IP or host identity.
        #[arg(value_name = "SOURCE")]
        source: String,
    },

    /// MESH-A-9 (v5.0.0) — write a network-state-change audit entry
    /// (R8-Q80): a `kind="audit"` activity record at
    /// `mde/activity/audit/<iso>-<hash>.json`. Prints the written path.
    AuditLog {
        /// The audited event (e.g. `host-blocked`, `arp-spoof-detected`).
        #[arg(value_name = "EVENT")]
        event: String,
        /// Optional context detail.
        #[arg(long = "detail", value_name = "TEXT", default_value = "")]
        detail: String,
    },

    /// MESH-A-8.1 (v5.0.0) — list LAN MDE-peer pairing candidates
    /// (R8-Q90): mDNS hosts advertising an MDE service. One
    /// `<ip>\t<hostname>` line per candidate.
    DiscoverMdePeers,

    /// EPIC-MESH-PROBE — run the nmap probe engine (MESH-PROBE-2).
    Probe {
        #[command(subcommand)]
        action: ProbeCmd,
    },

    /// MESHFS-1.2 (v5.0.0) — pre-flight check for the LizardFS
    /// mesh-storage rollout. Walks the user's XDG dirs, sums
    /// on-disk bytes, queries `/var/lib/mde/meshfs` free space,
    /// prints a one-line OK / WARN / NoDataDir verdict + emits
    /// the full structured report as a JSON line to stdout.
    /// Exits 0 on OK, 1 on WARN / NoDataDir. Operators can run
    /// this before an upgrade to v5.0.0; the Workbench Mesh
    /// Storage panel (MESHFS-13.1) will surface the same report
    /// as a banner once it lands.
    PreflightMeshFsHeadroom {
        /// Override the LizardFS data parent dir. Defaults to
        /// `/var/lib/mde/meshfs` per the design lock.
        #[clap(long, default_value = "/var/lib/mde/meshfs")]
        data_dir: std::path::PathBuf,
        /// Override `$HOME` (used to locate the five XDG
        /// dirs). Defaults to the env-var `$HOME`.
        #[clap(long)]
        home: Option<std::path::PathBuf>,
    },

    /// MESHFS-13.1 (v5.0.0) — query the active LizardFS master
    /// and emit a fleet status JSON blob (peers, goal, quota cap,
    /// limiting peer). Printed to stdout as a single JSON line.
    /// Exits 0 when the master is reachable, 1 otherwise.
    MeshFsStatus {
        /// Floating overlay VIP the active master listens on.
        #[clap(long, default_value = "10.42.0.1")]
        vip: String,
        /// `mfsadmin` binary name (must be on PATH).
        #[clap(long, default_value = "mfsadmin")]
        admin_binary: String,
    },

    /// MESHFS-8.1 (v5.0.0) — list files recoverable from the LizardFS
    /// `.trash` virtual directory. Emits a JSON array of `TrashEntry`
    /// objects to stdout. Returns an empty array `[]` when the mount
    /// path is absent or `.trash` is empty.
    MeshFsTrashList {
        /// Mount path of the LizardFS client (must be mounted).
        #[clap(long, default_value = "/mnt/mesh-storage")]
        mount_path: String,
    },

    /// MESHFS-8.1 (v5.0.0) — restore one file from the LizardFS trash
    /// by invoking `mfsadmin <vip> TRASH-RECOVER <path>`. `--path` is
    /// the full trash entry path (as emitted by `meshfs-trash-list`).
    /// Exits 0 on success, 1 on failure.
    MeshFsUndelete {
        /// Full path of the `.trash` entry to recover.
        #[clap(long)]
        path: String,
        /// Floating overlay VIP the active master listens on.
        #[clap(long, default_value = "10.42.0.1")]
        vip: String,
        /// `mfsadmin` binary name (must be on PATH).
        #[clap(long, default_value = "mfsadmin")]
        admin_binary: String,
    },

    /// MESHFS-11.1 (v5.0.0) — archive a `.conflict-*` file to
    /// `~/Local/conflict-archive/<ts>/`, completing the Resolve flow
    /// in mde-files. The mde-files UI calls this with either the
    /// original or the conflict sibling, depending on which the
    /// operator chose to discard.
    MeshFsResolveConflict {
        /// Full path of the conflict file to archive (the loser).
        #[clap(long)]
        path: String,
    },

    /// MESHFS-14.1 (v5.0.0) — restore the Nebula CA + (when
    /// present) the LizardFS mesh-storage snapshot from an
    /// armored `state-backup.enc` bundle. CA rows go straight
    /// into the local SQLite store via
    /// `ca::backup::restore_to_store`; the optional LizardFS
    /// artifacts land at `<recovery-dir>/` for the operator
    /// to apply with `mfsmaster --import-metadata`. Automatic
    /// volume replay is intentionally out of scope — replaying
    /// a metadata dump against a live cluster requires careful
    /// peer-by-peer reconciliation that's an operator-driven
    /// step, not a silent CLI action.
    StateRestore {
        /// Path to the armored `state-backup.enc` bundle.
        bundle: std::path::PathBuf,
        /// EFF-28 — dry-run: decode + unseal + report the bundle's
        /// contents WITHOUT touching the store or writing recovery
        /// artifacts. Exit 0 = the bundle is restorable with this
        /// passphrase. Use in DR drills + before a real restore.
        #[clap(long)]
        verify: bool,
        /// Passphrase env-var. Defaults to
        /// `MDE_BACKUP_PASSPHRASE` (same as the daily backup
        /// worker's env).
        #[clap(long, default_value = "MDE_BACKUP_PASSPHRASE")]
        passphrase_env: String,
        /// Directory to write the LizardFS recovery artifacts
        /// for the operator-side manual replay. Created if
        /// missing. Default `/var/lib/mackesd/restore/meshfs`.
        #[clap(long, default_value = "/var/lib/mackesd/restore/meshfs")]
        recovery_dir: std::path::PathBuf,
    },

    /// Generate a fresh 16-char URL-safe passcode (Phase 12.10.1).
    /// Prints the passcode. With `--store` (EPIC-SEC-PASSCODE-CREDS),
    /// also encrypts it to the cred file via `systemd-creds` instead
    /// of printing the libsecret hint.
    GeneratePasscode {
        /// EPIC-SEC-PASSCODE-CREDS — encrypt the generated passcode
        /// to the cred file via `systemd-creds encrypt`.
        #[arg(long, default_value_t = false)]
        store: bool,
        /// Override the cred-file path (defaults to
        /// `/var/lib/mackesd/mesh-passcode.cred`).
        #[arg(long, value_name = "PATH")]
        cred_path: Option<PathBuf>,
    },

    /// OBS-5 (W15) — append a structured log record to this node's
    /// replicated log (`<workgroup>/logs/<host>.jsonl`), where the
    /// PLANES-14 Fleet-logs-search panel reads it. Scripts + workers emit
    /// fleet-visible structured logs through this.
    LogEmit {
        /// `error` | `warn` | `info` | `debug` | `trace`.
        #[arg(long, default_value = "info")]
        level: String,
        /// Emitting subsystem/target.
        #[arg(long, default_value = "")]
        target: String,
        /// The log message.
        #[arg(long)]
        message: String,
    },

    /// PLANES-20 / ENT-8 — fleet rollup: the roster grouped by role with
    /// each group's member count + worst health. `--json` for the
    /// Fleet-rollup panel; otherwise a short table.
    FleetStatus {
        /// Emit the rollup as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// PLANES-4 (W25) — print this node's signing-key fingerprint + its
    /// word-pair (the out-of-band verbal-comparison rendering). `--json`
    /// emits both for the Registration panel.
    Identity {
        /// Emit `{fingerprint, word_pair}` as JSON instead of the
        /// human two-line form.
        #[arg(long)]
        json: bool,
    },

    /// Walk the `events` table forward and verify every row's hash
    /// (Phase 12.10.3). Exits 0 on Intact / Empty, 1 on Break.
    AuditVerify {
        /// PLANES-12 — emit the event timeline (72 h rolling window) +
        /// the verify outcome as JSON for the Audit panel, instead of the
        /// human summary.
        #[arg(long)]
        json: bool,
    },

    /// Rotate the shared mesh passcode (Phase 12.10.2). Prints a
    /// freshly-generated passcode. With `--store`, encrypts it to
    /// the cred file via `systemd-creds`. Peers pick up the new
    /// passcode on their next heartbeat once the reconcile loop runs.
    RotatePasscode {
        /// EPIC-SEC-PASSCODE-CREDS — encrypt the rotated passcode to
        /// the cred file via `systemd-creds encrypt`.
        #[arg(long, default_value_t = false)]
        store: bool,
        /// Override the cred-file path.
        #[arg(long, value_name = "PATH")]
        cred_path: Option<PathBuf>,
    },

    /// EPIC-SEC-PASSCODE-CREDS — decrypt + print the mesh passcode
    /// stored via `systemd-creds` (the inverse of
    /// `generate-passcode --store`). Reads the cred file, runs
    /// `systemd-creds decrypt`, prints the plaintext to stdout.
    ShowPasscode {
        /// Override the cred-file path.
        #[arg(long, value_name = "PATH")]
        cred_path: Option<PathBuf>,
    },

    /// Explain why a given peer is expected to peer with each of
    /// its neighbors (Phase 12.4.4). Reads `topology::calculate`'s
    /// reason chain for the named node.
    PeersWhy {
        /// Stable node id (e.g. `peer:anvil`).
        #[arg(value_name = "NODE_ID")]
        node_id: String,
    },

    /// Dry-run apply (Phase 12.7.4). Runs the validation +
    /// reconcile-plan pipeline without mutating anything; prints
    /// the diff + would-be event log as JSON. Useful in CI to
    /// catch config issues before a real apply.
    Apply {
        /// Skip mutation; print the plan only.
        #[arg(long)]
        dry_run: bool,
    },

    /// Enroll this peer against the mesh. Two flows:
    ///
    /// **Pre-v2.5 (passcode):** Phase 12.3.1 v1.x flow — generates
    /// an Ed25519 keypair + bearer token, prints a signed
    /// `EnrollmentRequest` JSON the leader ingests.
    ///
    /// **v2.5 Nebula (token):** NF-3.6.a — parses the
    /// `mesh:<id>@<ip>:<port>#<bearer>` join token, publishes a
    /// pending-enroll CSR to QNM-Shared, waits up to 30 s for the
    /// lighthouse to sign + write the bundle back. The
    /// `nebula_supervisor` worker materializes /etc/nebula/ once
    /// the bundle lands.
    ///
    /// `--passcode` and `--token` are mutually exclusive; exactly
    /// one must be set.
    Enroll {
        /// 16-character URL-safe shared passcode (v1.x flow).
        /// EFF-21: prefer `--passcode-stdin` — argv is visible in
        /// /proc/<pid>/cmdline and shell history.
        #[arg(long, conflicts_with_all = ["token", "token_stdin", "passcode_stdin"])]
        passcode: Option<String>,
        /// EFF-21 — read the passcode as one line from stdin.
        #[arg(long, conflicts_with_all = ["token", "token_stdin", "passcode"])]
        passcode_stdin: bool,
        /// v2.5 Nebula join token —
        /// `mesh:<mesh_id>@<lighthouse_ip>:<port>#<bearer>`.
        /// EFF-21: prefer `--token-stdin` — the bearer rides argv
        /// otherwise.
        #[arg(long, conflicts_with_all = ["passcode", "passcode_stdin", "token_stdin"])]
        token: Option<String>,
        /// EFF-21 — read the join token as one line from stdin.
        #[arg(long, conflicts_with_all = ["passcode", "passcode_stdin", "token"])]
        token_stdin: bool,
        /// Optional display name; defaults to the system hostname.
        #[arg(long)]
        name: Option<String>,
        /// Override the workgroup root (`$QNM_SHARED_ROOT` env or the
        /// `~/QNM-Shared` default fallback, per EPIC-RETIRE-QNM Phase C).
        /// v2.5 token flow locates the CSR + signed bundle here.
        #[arg(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
    },

    /// Decommission a peer (Phase 12.3.4). Soft-deletes the node
    /// row; preserves history. `--force` skips the unreachable
    /// confirmation.
    Decommission {
        /// Stable node id to retire.
        node_id: String,
        /// Force decommission even when the peer is unreachable.
        #[arg(long)]
        force: bool,
    },

    /// Re-enroll an existing node (Phase 12.3.5). Issues fresh
    /// credentials against the existing row, preserving history.
    Reenroll {
        /// Stable node id to refresh.
        node_id: String,
    },

    /// Force this peer into leadership (Phase 12.1.1b operator
    /// override). Bumps the lease epoch.
    TakeLeadership {
        /// Stable node id to install as leader.
        #[arg(long)]
        as_node: String,
    },

    /// Import legacy mesh state into the `mackesd` store (Phase
    /// 12.13.2). Walks the prior 2.x JSON/TOML caches and emits
    /// a JSON plan that the operator can review before applying.
    ImportLegacy {
        /// Print the plan only; don't write anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Inventory legacy on-disk state (Phase 12.13.1). Walks the
    /// three canonical roots (`~/.config/mackes-shell/`,
    /// `~/.qnm-sync/`, `~/.cache/mackes/`) and prints a catalog of
    /// every JSON / TOML / cache file found, classified by kind and
    /// flagged with whether the filename hints at mesh data. This
    /// is the *inspection* step — `mackesd import-legacy` is what
    /// actually moves data into the store.
    InventoryLegacy {
        /// Only emit artifacts whose filename matches the
        /// mesh-related heuristic.
        #[arg(long)]
        mesh_only: bool,
        /// Emit the full inventory as a JSON array. Without this
        /// flag a human-readable table prints to stdout.
        #[arg(long)]
        json: bool,
    },

    /// Run the reconcile worker (Phase 12.5 wiring). Default mode
    /// loops forever on the foreground thread, ticking every
    /// `RECONCILE_INTERVAL_S` seconds (30 s per the 12.5.1 lock).
    /// This is the entry point systemd's `mackesd.service` invokes.
    ///
    /// The worker reads peer heartbeats + link telemetry from
    /// `QNM_SHARED_ROOT/<peer>/mackesd/{heartbeat,links}.json`,
    /// compares them against the latest applied `desired_config`
    /// snapshot, and routes the resulting drift rows through
    /// `reconcile::plan_tick`. Auto-repairable rows land in the
    /// audit-log with the `intent` field marking that take-action
    /// is gated on the connectivity layer (12.14+); manual-review
    /// rows are surfaced via `tracing::warn` for the GUI inbox.
    ///
    /// SIGTERM / SIGINT trigger a graceful exit: the current tick
    /// finishes, then the loop returns. Cleanly handles systemd's
    /// `TimeoutStopSec`.
    Reconcile {
        /// Run one tick, print the resulting `TickOutcome` as a
        /// pretty-printed JSON object, and exit. No background
        /// thread, no signal handler — for CI smoke tests + the
        /// dry-run loop the operator runs by hand.
        #[arg(long)]
        once: bool,
        /// Override the QNM-Shared root (defaults to
        /// `$QNM_SHARED_ROOT` or `~/QNM-Shared`). Useful for tests.
        #[arg(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
        /// Override the stable node id (defaults to
        /// `peer:<hostname>`). Recorded as the `actor` field on
        /// every emitted audit event.
        #[arg(long)]
        node_id: Option<String>,
    },

    /// v2.0.0 Phase F.12 — desired_config revision management. Read
    /// every revision (`list`), diff two revisions (`diff a b`), or
    /// roll a prior revision forward as a new applied row
    /// (`rollback id`).
    Revisions {
        #[command(subcommand)]
        cmd: RevisionsCmd,
    },

    /// ENT-5 — voluntarily exit the mesh: evict own cert from the
    /// data plane, leave the roster, wipe /etc/nebula + keys, and
    /// unpin the role (back to fail-closed). No ban — re-enroll
    /// stays a clean fresh join.
    Leave {
        /// Required confirmation — this wipes local mesh state.
        #[arg(long)]
        yes: bool,
    },

    /// ENT-4 — bootstrap THIS box as the mesh's founding lighthouse:
    /// pin the role (if unpinned), mint the CA, self-sign + write the
    /// bundle, and print the first peer's single-use join token.
    MeshInit {
        /// Mesh id (e.g. `home-mesh`).
        #[arg(long)]
        mesh_id: String,
        /// This lighthouse's externally-dialable address. Peers'
        /// static_host_map points here.
        #[arg(long)]
        external_addr: String,
        /// Role to pin when unpinned (lighthouse|server|workstation).
        #[arg(long, default_value = "lighthouse")]
        role: String,
    },

    /// ENT-1 — mint a single-use 256-bit enrollment bearer on this
    /// lighthouse and print the join token a new box runs
    /// `mackesd enroll --token <…>` with. The ledger records only the
    /// bearer's hash; the raw value is shown once, here.
    EnrollToken {
        /// Mesh id to embed in the token (e.g. `home-mesh`).
        #[arg(long)]
        mesh_id: String,
        /// Lighthouse address the joining box dials. Defaults to the
        /// published overlay-ip + :4242.
        #[arg(long)]
        lighthouse: Option<String>,
        /// Operator note recorded beside the issued hash.
        #[arg(long, default_value = "")]
        note: String,
    },

    /// ONBOARD-4 — **the Magic founding verb.** Stand up THIS box as
    /// the mesh's first lighthouse in one command: mesh-init (pin role,
    /// mint CA, self-sign, write bundle), generate the self-signed
    /// `/enroll` endpoint identity, and print a ready-to-paste
    /// `mackesd join` line whose token embeds the endpoint's cert
    /// fingerprint + enroll port (token v3). The `nebula-enroll-listener`
    /// activates on the next `mackesd serve` (the endpoint cert now
    /// exists).
    Found {
        /// Mesh id (e.g. `home-mesh`).
        mesh_id: String,
        /// This lighthouse's externally-dialable IPv4. `auto` (default)
        /// detects the primary outbound IP — operators behind NAT pass
        /// the public IP explicitly.
        #[arg(long, default_value = "auto")]
        external_addr: String,
        /// Role to pin when unpinned (lighthouse|server|workstation).
        #[arg(long, default_value = "lighthouse")]
        role: String,
        /// Override the `/enroll` HTTPS port the token advertises.
        #[arg(long)]
        enroll_port: Option<u16>,
    },

    /// ONBOARD-4 — **the Magic joining verb.** Join an existing mesh in
    /// one command: pin the role, network-enroll against the
    /// lighthouse's `/enroll` endpoint over TLS pinned to the token's
    /// fingerprint (fixes MESH-1 — no QNM-Shared pre-mount), and
    /// materialize `/etc/nebula`. Run `mackesd serve` afterwards to
    /// bring up the overlay. With no token, launches the enrollment TUI
    /// (ONBOARD-5).
    Join {
        /// The v3 join token printed by `mackesd found`
        /// (`mesh:<id>@<ip>:<port>#<bearer>?fp=<sha256>`). Omit to
        /// launch the TUI.
        token: Option<String>,
        /// Role to pin when unpinned (lighthouse|server|workstation).
        #[arg(long, default_value = "workstation")]
        role: String,
        /// Optional display name; defaults to the system hostname.
        #[arg(long)]
        name: Option<String>,
        /// Override the workgroup root (`$QNM_SHARED_ROOT` or
        /// `~/QNM-Shared`).
        #[arg(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
    },

    /// PD-1 (Q23/W27) — the joined peer directory: every known peer
    /// with presence tier, health, version, overlay ip/role, and
    /// revision currency — the same record `action/mesh/directory`
    /// serves the GUIs. Table by default; `--json` for the raw rows.
    Peers {
        /// Emit the raw directory JSON instead of the table.
        #[arg(long)]
        json: bool,
    },

    /// PLANES-11 — the remediation layer (W41/W42). `mded remediate
    /// match --json` evaluates the policy core pack (PLANES-13) against
    /// the live directory and pairs each violation with the plan that
    /// remediates it; `mded remediate fire --plan <p> --peer <h>`
    /// enqueues that plan's signed job bundle against the drifted peer
    /// (W21/W32 — no push-SSH). The Controller ▸ Remediation panel
    /// consumes the `match` JSON.
    Remediate {
        #[command(subcommand)]
        cmd: RemediateCmd,
    },

    /// PLANES-13 — the policy engine surface. `mded policy list --json`
    /// emits every loaded policy (the W50 core pack + any TOML in
    /// `<root>/policies/`) with the peers that currently violate it,
    /// evaluated against the live directory. The Controller ▸ Policy
    /// panel consumes the JSON.
    Policy {
        #[command(subcommand)]
        cmd: PolicyCmd,
    },

    /// PLANES-15 — the netstate engine surface. `mded netstate diff
    /// --json` compares the elected fleet revision's desired nmstate
    /// (W67 BaselineSpec) against the box's live interfaces (read via
    /// nmstatectl), reporting per-interface in-sync status (W68). The
    /// Network ▸ Interfaces panel consumes the JSON.
    Netstate {
        #[command(subcommand)]
        cmd: NetstateCmd,
    },

    /// PLANES-18 — the mesh DNS surface. `mded dns list --json` emits
    /// the flat `<host>.mesh → overlay-ip` record set the mesh_dns
    /// worker feeds to systemd-resolved (W74/W75), built from the live
    /// roster. The Network ▸ Mesh DNS panel consumes the JSON.
    #[cfg(feature = "async-services")]
    Dns {
        #[command(subcommand)]
        cmd: DnsCmd,
    },

    /// PLANES-19 — the overlay-reachability validation surface. `mded
    /// validate status --json` reports the newest run's directed-edge
    /// verdict (W79/W80); `mded validate run` requests a fresh run (the
    /// leader mints it). The Network ▸ Routing panel consumes the JSON.
    Validate {
        #[command(subcommand)]
        cmd: ValidateCmd,
    },

    /// PLANES-3/W82 — the fleet capability-tag census. `mded tags
    /// --json` emits, for each v1 tag (hop / execution / headless), the
    /// roster nodes that carry it. The Fleet ▸ Capability Tags panel
    /// consumes the JSON. (Per-node view/set is `mded tag <host>`.)
    Tags {
        /// Emit the JSON array instead of the table.
        #[arg(long)]
        json: bool,
    },

    /// PLANES-21 — the install-profile catalog. `mded profiles list
    /// --json` emits every profile (the per-role core pack + any TOML in
    /// `<root>/profiles/`): role pin, capability tags, kickstart
    /// fragments, and the auto-join slot (W56/W60). The Provisioning ▸
    /// Install Profiles panel consumes the JSON. `--set <name> --role <r>`
    /// writes/overwrites a profile TOML (W56 form-edit write side);
    /// `--rm <name>` deletes an on-disk profile (core profiles revert).
    Profiles {
        /// Emit the JSON array instead of the table.
        #[arg(long)]
        json: bool,
        /// Write/overwrite the named profile (with --role + optional fields).
        #[arg(long, value_name = "NAME")]
        set: Option<String>,
        /// Delete the named on-disk profile (no-op for a core profile).
        #[arg(long, value_name = "NAME")]
        rm: Option<String>,
        /// Role for --set (lighthouse|server|workstation).
        #[arg(long)]
        role: Option<String>,
        /// Description for --set.
        #[arg(long, default_value = "")]
        description: String,
        /// Capability tag for --set (repeatable: hop|execution|headless).
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Kickstart fragment id for --set (repeatable).
        #[arg(long = "ks-fragment")]
        ks_fragments: Vec<String>,
        /// Bake the firstboot auto-join slot for --set (W60).
        #[arg(long)]
        auto_join: bool,
    },

    /// PLANES-24 — the package-mirror catalog. `mded mirrors --json`
    /// emits every mirror (the `magic-mesh` GitHub-RPM core pack + any
    /// TOML in `<root>/mirrors/`): upstream, the `file://` baseurl every
    /// node serves itself from (W62), and the last-sync freshness (W63).
    /// The Provisioning ▸ Mirrors panel consumes the JSON. `--sync <name>`
    /// (or `--sync-all`) runs the W63 one-puller: `dnf reposync` the
    /// upstream into the LizardFS mirror dir, `createrepo_c` the metadata,
    /// then stamp `.last-sync` (LizardFS replicates it to every node).
    Mirrors {
        /// Emit the JSON array instead of the table.
        #[arg(long)]
        json: bool,
        /// Sync just this mirror (by name) instead of listing.
        #[arg(long, value_name = "NAME")]
        sync: Option<String>,
        /// Sync every enabled mirror instead of listing.
        #[arg(long)]
        sync_all: bool,
        /// Write each mirror's dnf `.repo` (local `file://` first, upstream
        /// fallback) into `--repo-dir`, flipping this node to self-serve (W62).
        #[arg(long)]
        write_repo: bool,
        /// Where `--write-repo` lands the `.repo` files (default /etc/yum.repos.d).
        #[arg(long, value_name = "DIR")]
        repo_dir: Option<std::path::PathBuf>,
    },

    /// PLANES-22 — the image catalog. `mded images --json` emits the
    /// four buildable kinds (ISO / VM / container / USB, W53) each with
    /// the versioned builds present on LizardFS (W55). The Provisioning
    /// ▸ Images panel consumes the JSON. `--record --name --kind --version`
    /// registers a completed build's manifest (W55 — the write side a
    /// build job calls when its output lands). `--build --kind --name
    /// --version` runs the actual build (W54) on this node then records it.
    Images {
        /// Emit the JSON array instead of the table.
        #[arg(long)]
        json: bool,
        /// Record a completed build's manifest (with --name/--kind/--version).
        #[arg(long)]
        record: bool,
        /// W54 — build the image now (with --kind/--name/--version), then
        /// record its manifest. Runs the real per-kind tool; meant to run
        /// as a job on an execution-tagged node.
        #[arg(long)]
        build: bool,
        /// Image name for --record.
        #[arg(long)]
        name: Option<String>,
        /// Image kind for --record (iso|vm|container|usb).
        #[arg(long)]
        kind: Option<String>,
        /// Version for --record.
        #[arg(long)]
        version: Option<String>,
        /// Output size in bytes for --record.
        #[arg(long)]
        size_bytes: Option<u64>,
        /// Install profile baked into the image, for --record.
        #[arg(long)]
        profile: Option<String>,
    },

    /// PLANES-7 (W28) — coordinate a fleet upgrade. `--coordinate` writes
    /// an upgrade-intent on the replicated volume; every peer's
    /// upgrade-intent watcher then upgrades to repo-latest behind the
    /// quorum + grace barrier (the best-practice typed update path — not a
    /// raw GUI dnf). `--version` is an optional coordination label.
    Upgrade {
        /// Publish the coordinated-upgrade intent.
        #[arg(long)]
        coordinate: bool,
        /// Coordination label for the intent (default `latest`).
        #[arg(long)]
        version: Option<String>,
    },

    /// CB-1.5.a — fleet node roster. `mded nodes list --json` emits
    /// every row from the `nodes` table as a JSON array; the Iced
    /// inventory panel (in `crates/mde-workbench/src/panels/
    /// inventory.rs`) consumes the same shape. Without `--json` the
    /// command prints a human-readable table.
    Nodes {
        #[command(subcommand)]
        cmd: NodesCmd,
    },

    /// CB-1.5.c follow-up — ansible-pull run history. `mded
    /// ansible-history list --json` walks
    /// `$QNM_SHARED_ROOT/.qnm-sync/ansible-runs/<peer>/*.json`
    /// and emits the union as a sorted (timestamp DESC) JSON
    /// array. The Iced run-history panel reads the same
    /// filesystem source directly today — this CLI alternative
    /// exists for headless / leader-aggregated views where the
    /// reader peer doesn't have QNM-Sync replicated locally.
    AnsibleHistory {
        #[command(subcommand)]
        cmd: AnsibleHistoryCmd,
    },

    /// CB-1.5.b follow-up — curated playbook surface. `mded
    /// playbooks list --json` enumerates every role under
    /// `$QNM_SHARED_ROOT/.qnm-sync/playbooks/roles/` with the
    /// Phase 1.3.0 curated description if recognised. `mded
    /// playbooks run <name>` shells out to `ansible-pull
    /// --tags <name> site.yml` locally — same shape as the
    /// Iced playbooks panel's Run button, but headless-
    /// friendly (no GUI dependency).
    Playbooks {
        #[command(subcommand)]
        cmd: PlaybooksCmd,
    },

    /// CB-1.8 mesh_history follow-up — audit-log viewer
    /// surface. `mded events list --json` emits the entire
    /// hash-chained `events` table as a JSON array. The Iced
    /// mesh_history panel consumes this. Headless callers
    /// (audit scripts) get the same shape.
    Events {
        #[command(subcommand)]
        cmd: EventsCmd,
    },

    /// v2.0.0 Phase G.4 — push a settings revision to a peer
    /// selection. Writes a new `desired_config` row, records one
    /// `fleet_settings_apply_log` row per (peer, key) target, and
    /// prints the JSON plan. The reconcile worker on each named
    /// peer picks up the revision on its next tick.
    ///
    /// `--peers` accepts a comma-separated list of node ids, or the
    /// literal token `all` for the full healthy set.
    #[cfg(feature = "async-services")]
    FleetPushSetting {
        /// Dot-notated setting key (e.g. `theme.accent`).
        key: String,
        /// JSON-encoded value payload. The string itself is taken
        /// verbatim — quote it for the shell as appropriate.
        value: String,
        /// Comma-separated peer ids, or `all`.
        #[arg(long, default_value = "all")]
        peers: String,
        /// Override the revision author tag (defaults to
        /// `peer:<hostname>`).
        #[arg(long)]
        author: Option<String>,
        /// Print the plan but don't write to the store.
        #[arg(long)]
        dry_run: bool,
    },

    /// v2.0.0 Phase B.12 — the unified meta-daemon entry point.
    /// Replaces the legacy `migrate && status` ExecStart on the
    /// systemd unit. Boots the tokio runtime, spawns the worker
    /// supervisor + every registered worker, and blocks on
    /// SIGTERM/SIGINT.
    ///
    /// Phase A.2 ships the supervisor surface; Phase B fills in the
    /// individual workers (`heartbeat`, `mesh_router`, ...).
    /// Today `serve` registers the existing reconcile loop as the
    /// single worker so the unit's behavior matches the current
    /// `mackesd reconcile` invocation while the rest of Phase B lands.
    ///
    /// Requires the `async-services` cargo feature.
    #[cfg(feature = "async-services")]
    Serve {
        /// Override the QNM-Shared root (defaults to
        /// `$QNM_SHARED_ROOT` or `~/QNM-Shared`).
        #[arg(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
        /// Override the stable node id (defaults to `peer:<hostname>`).
        #[arg(long)]
        node_id: Option<String>,
    },

    // AUD3 S-3 (2026-06-12): `PeerCard` (PC-3.a) removed — it spawned
    // the `mde-peer-card` modal, deleted in the E11 pivot.
    /// NF-2.6 (v2.5) — Nebula CA management subcommands.
    /// Mint / rotate / list / dump-ca the mesh-CA artifacts.
    Ca {
        /// Sub-subcommand selector — see `CaCmd` below.
        #[command(subcommand)]
        sub: CaCmd,
    },

    /// NF-18.x (v2.5) — Nebula peer + roster operations.
    /// Operator-facing reads against the live nebula_peer_certs
    /// + nodes tables.
    Nebula {
        #[command(subcommand)]
        sub: NebulaCmd,
    },

    /// VV-1 / VV-1.5 (v4.1.0) — Voice/Video stack operations.
    /// Today only `render-config` ships; VV-2 adds policy-driven
    /// reload, VV-14 adds Vitelity `uac.reg_dump`, etc.
    Voice {
        #[command(subcommand)]
        sub: VoiceCmd,
    },

    /// DEAD-2.5 (v5.1) + NF-21.2 (v1.0/1.1) — Wake-on-LAN.
    ///
    /// Default mode: fires the magic packet at the local broadcast
    /// address (works within one LAN segment). `--via-lighthouse <ip>`
    /// instead sends the magic packet as unicast over the Nebula
    /// overlay to a lighthouse, which de-encapsulates and re-broadcasts
    /// on the target's LAN — the "WoL across LANs" capability the v2.5
    /// cut enables.
    ///
    /// Replaces `mackes/mesh_wol.py::wake_peer` + `mesh_nebula.py::wol_via_lighthouse`.
    WakePeer {
        /// Target MAC in any canonical form: `aa:bb:cc:dd:ee:ff`,
        /// `aa-bb-cc-dd-ee-ff`, or `aabbccddeeff`.
        mac: String,
        /// Broadcast address to fire at. Defaults to the limited
        /// broadcast. Ignored when `--via-lighthouse` is set.
        #[clap(long, default_value = "255.255.255.255")]
        broadcast: String,
        /// Send via this lighthouse's overlay IP as unicast. The
        /// lighthouse-side relay re-broadcasts on the target LAN.
        /// Mutually exclusive with `--broadcast` (when both set,
        /// lighthouse mode wins).
        #[clap(long)]
        via_lighthouse: Option<String>,
        /// Destination UDP port. Standard ports are 7 + 9; 9 is the
        /// historical default and what every mainboard expects.
        #[clap(long, default_value_t = 9)]
        port: u16,
    },

    /// Portal-18.d (v6.0 R12, 2026-05-27) — fire `swaymsg exec <cmd>`
    /// for every entry in a preset tag's `launch_bundle`. The runtime
    /// entry point for Portal-18.d until Portal-17 Hub's tag-card
    /// click handler lands; operators (or Hub callbacks) invoke this
    /// to launch the bundle.
    ///
    /// Prints `launched <N>/<M>` summary; non-zero exit when any
    /// individual exec fails.
    PresetLaunch {
        /// Name of the preset tag to launch. Must exist in
        /// `<XDG_DATA_HOME>/mde/tags.json` with `TagFlavor::Preset`.
        tag: String,
    },
}

/// EPIC-MESH-PROBE — `mackesd probe <sub>` subcommands (MESH-PROBE-2).
/// The scheduled two-tier worker (MESH-PROBE-4) reuses the same
/// `probe_nmap` engine; this is the operator-facing manual surface.
#[derive(Subcommand)]
enum ProbeCmd {
    /// Run a one-shot nmap scan against `targets` and print the
    /// resulting inventory cards as JSON lines. Requires `nmap`
    /// (RPM `Requires: nmap`, MESH-PROBE-3); a missing binary prints
    /// nothing + exits 0 (graceful-degrade).
    Scan {
        /// Hosts / CIDRs to scan (e.g. `10.42.0.5`). At least one.
        #[clap(required = true)]
        targets: Vec<String>,
        /// Deep `-sV`/NSE identification pass (default: fast pass).
        #[clap(long)]
        deep: bool,
        /// Discovery source tag recorded on each host card:
        /// `mesh` (default) / `lan` / `arbitrary`.
        #[clap(long, default_value = "mesh")]
        source: String,
        /// Bundled-NSE script dir for the deep pass (MESH-PROBE-3).
        #[clap(long, default_value = "/usr/share/mde/nmap")]
        nse_dir: String,
    },
    /// Manual refresh (MESH-PROBE-4): run one deep probe cycle against
    /// the resolved mesh peers + write this peer's probe-inventory.json
    /// + announce probe/changed. Same engine the scheduled worker runs.
    Refresh {
        /// Mesh-home root (defaults to `$QNM_SHARED_ROOT` / `~/QNM-Shared`).
        #[clap(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
        /// This peer's node-id (defaults to the daemon default).
        #[clap(long)]
        node_id: Option<String>,
        /// Bundled-NSE script dir for the deep pass.
        #[clap(long, default_value = "/usr/share/mde/nmap")]
        nse_dir: String,
    },
    /// List the merged mesh-wide probe inventory (MESH-PROBE-6): the
    /// union of every peer's `probe-inventory.json`. With `--service`,
    /// list only the hosts running that service kind.
    List {
        /// Mesh-home root (defaults to `$QNM_SHARED_ROOT` / `~/QNM-Shared`).
        #[clap(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
        /// Filter to hosts running this service kind (e.g. `jellyfin`).
        #[clap(long)]
        service: Option<String>,
    },
}

/// VV-1 / VV-1.5 — `mackesd voice <sub>` subcommands.
#[derive(Subcommand)]
enum VoiceCmd {
    /// Regenerate the four kamailio-mde + rtpengine-mde config
    /// files (`kamailio.cfg`, `dispatcher.list`, `uacreg.list`,
    /// `rtpengine.conf`) from the current policy snapshot.
    ///
    /// Invoked by both `kamailio-mde.service` and
    /// `rtpengine-mde.service` as their `ExecStartPre=` hook on
    /// every (re)start, so the on-disk config is always coherent
    /// with the latest approved `voice_mesh` / `voice_public`
    /// policy revision.
    ///
    /// VV-1 ships the minimal generator: no peer routing, no
    /// Vitelity, just enough to boot Kamailio + `RTPengine`. VV-2
    /// wires the generator to mackesd's policy store so peer
    /// AORs (via `dispatcher.list`) + Vitelity sub-accounts (via
    /// `uacreg.list`) flow from approved `voice_mesh` /
    /// `voice_public` revisions.
    RenderConfig {
        /// Override the kamailio-mde output directory (defaults
        /// to `/etc/kamailio-mde/`). Used by tests + dry-runs.
        #[arg(long, value_name = "DIR", default_value = "/etc/kamailio-mde")]
        kamailio_dir: PathBuf,
        /// Override the rtpengine-mde output directory.
        #[arg(long, value_name = "DIR", default_value = "/etc/rtpengine-mde")]
        rtpengine_dir: PathBuf,
        /// VV-2 — JSON file containing a serialized `VoiceDesired`
        /// document. When the file is missing, render-config
        /// falls back to `VoiceDesired::boot_default(node_id)` and
        /// emits the minimal SIP-OPTIONS-keepalive-only config.
        /// The voice_config worker writes to this path on every
        /// policy change; operators can hand-edit during
        /// development by dropping a JSON document at the
        /// default path.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "/var/lib/mackesd/voice-desired.json"
        )]
        desired_json: PathBuf,
        /// Skip the desired_json file entirely and use
        /// `boot_default` — useful for testing the bootstrap
        /// path in isolation.
        #[arg(long)]
        boot_default: bool,
        /// Print each generated file to stdout instead of
        /// writing to disk. Useful for diff'ing across policy
        /// revisions.
        #[arg(long)]
        dry_run: bool,
    },
}

/// NF-2.6 — `mackesd ca <sub>` subcommands.
#[derive(Subcommand)]
enum CaCmd {
    /// Idempotent CA mint at epoch 0. No-op when an active
    /// CA already exists for the named mesh.
    Mint {
        /// Mesh id (defaults to `mesh-<hostname>`).
        #[arg(long, value_name = "MESH_ID")]
        mesh_id: Option<String>,
    },

    /// Bump the CA epoch — retires the active CA, mints a
    /// fresh one at epoch+1, re-signs every active peer
    /// cert under the new epoch.
    Rotate {
        /// Mesh id (defaults to `mesh-<hostname>`).
        #[arg(long, value_name = "MESH_ID")]
        mesh_id: Option<String>,
        /// SEC-2 — read the operator passphrase from stdin instead
        /// of $MDE_CA_PASSPHRASE.
        #[arg(long)]
        passphrase_stdin: bool,
    },

    /// SEC-2 — set (or change) the CA-rotation passphrase. Reads the
    /// new phrase from $MDE_CA_PASSPHRASE (changing additionally
    /// requires the current one in $MDE_CA_PASSPHRASE_CURRENT).
    SetPassphrase,

    /// Print one row per CA epoch — mesh_id, epoch,
    /// created_at, retired_at (or "active" when NULL).
    List,

    /// Print the public CA cert PEM to stdout. Used by
    /// peer-bootstrap flows that need the CA chain to
    /// validate inbound TLS.
    DumpCa {
        /// Mesh id (defaults to `mesh-<hostname>`).
        #[arg(long, value_name = "MESH_ID")]
        mesh_id: Option<String>,
    },

    /// NF-18.1 (v2.5) — export the CA + every peer cert into a
    /// passphrase-encrypted ASCII-armored bundle on stdout (or
    /// to `--output <path>`). Use for off-cluster disaster
    /// recovery — `import` reverses. Passphrase read from
    /// `MDE_BACKUP_PASSPHRASE` env var (operator must export
    /// before invoking) so it never lands in shell history.
    Export {
        /// Mesh id (defaults to `mesh-<hostname>`).
        #[arg(long, value_name = "MESH_ID")]
        mesh_id: Option<String>,
        /// EFF-21 — read the passphrase as one line from stdin
        /// (preferred: the env form is visible in /proc/<pid>/environ
        /// and inherited by children).
        #[arg(long)]
        passphrase_stdin: bool,
        /// Where to write the armored bundle. Default: stdout.
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Sealed CA key path (defaults to
        /// `/var/lib/mackesd/nebula-ca/ca.key`).
        #[arg(long, value_name = "PATH")]
        ca_key: Option<PathBuf>,
    },

    /// NF-18.1 (v2.5) — import an exported bundle and restore
    /// the CA + peer certs into the local store. Reads the
    /// armored bundle from stdin (or `--input <path>`).
    /// Passphrase via `MDE_BACKUP_PASSPHRASE`.
    Import {
        /// Where to read the armored bundle from. Default:
        /// stdin.
        #[arg(long, value_name = "PATH")]
        input: Option<PathBuf>,
        /// EFF-21 — read the passphrase as one line from stdin
        /// (requires `--input`, since the default bundle source is
        /// stdin). Preferred over the env form.
        #[arg(long, requires = "input")]
        passphrase_stdin: bool,
    },

    /// NF-3.6.b (v2.5) — sign a peer's pending-enroll CSR.
    /// Reads `QNM-Shared/<peer-id>/mackesd/pending-enroll.json`,
    /// signs the cert under the active CA, writes the
    /// `nebula-bundle.json` back so the peer's nebula_supervisor
    /// can materialize `/etc/nebula/`. Idempotent — re-running
    /// re-signs at the current epoch + allocates a fresh
    /// overlay IP.
    SignCsr {
        /// Peer's stable node-id (e.g. `peer:anvil`). Must match
        /// a pending-enroll.json under QNM-Shared.
        node_id: String,
        /// Override QNM-Shared root (defaults to
        /// `$QNM_SHARED_ROOT` or `~/QNM-Shared`).
        #[arg(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
        /// Mesh id (defaults to `mesh-<hostname>`).
        #[arg(long, value_name = "MESH_ID")]
        mesh_id: Option<String>,
        /// CA cert path (defaults to `/etc/nebula/ca.crt`).
        #[arg(long, value_name = "PATH")]
        ca_crt: Option<PathBuf>,
        /// Sealed CA key path (defaults to
        /// `/var/lib/mackesd/nebula-ca/ca.key`).
        #[arg(long, value_name = "PATH")]
        ca_key: Option<PathBuf>,
        /// Scratch dir for intermediate peer cert/key files
        /// (defaults to `/var/lib/mackesd/nebula-ca/scratch`).
        #[arg(long, value_name = "PATH")]
        scratch_dir: Option<PathBuf>,
        /// Lighthouse public reachable address baked into the
        /// bundle's roster (form `host:port`). Defaults to
        /// `<hostname>:4242`; operators on multi-NIC or
        /// public-IP-different-from-hostname boxes should
        /// override.
        #[arg(long, value_name = "HOST:PORT")]
        lighthouse_addr: Option<String>,
        /// TUNE-11 — bypass the 8-peer cap (Q3 + Q22 lock). The
        /// override engages an audit-log entry. Document any
        /// real use in `docs/design/cap-overrides.md`.
        #[arg(long, default_value_t = false)]
        override_cap: bool,
    },
    /// INST-7 prerequisite (v2.7) — revoke a peer's Nebula cert.
    /// Marks every active row for `<node-id>` in `nebula_peer_certs`
    /// as revoked, adds the node-id to the local ban list (so the
    /// identity can't re-enroll even after a CA rotation), and fires
    /// a best-effort Bus event `ca/revoke/<node-id>`.
    ///
    /// This is the CLI replacement for the originally-planned
    /// `dev.mackes.MDE.Ca.Revoke` D-Bus method. D-Bus retires by 1.0
    /// per AI_GOVERNANCE §3.3; the wipe sequence in `mde-install`
    /// shells this command instead.
    ///
    /// Exits 0 on success (0 rows marked is still success — the ban
    /// list write happens regardless). Exits non-zero on DB or
    /// ban-list I/O failure.
    Revoke {
        /// Node-id to revoke (e.g. `peer:anvil`).
        node_id: String,
        /// Override QNM-Shared / mesh-home root (defaults to
        /// `$QNM_SHARED_ROOT` or `~/QNM-Shared`).
        #[clap(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<std::path::PathBuf>,
        /// This peer's own node-id (used to locate the local
        /// ban-list file). Defaults to reading `/etc/mde/node-id`.
        #[clap(long)]
        self_node_id: Option<String>,
    },

    /// EPIC-SEC-BANLIST (Q53) — add a node-id to this peer's ban
    /// list. A banned node-id is refused enrollment mesh-wide, even
    /// with a valid passcode + across a CA rotation. GFS replication
    /// propagates the ban to every peer.
    Ban {
        /// Node-id to ban (e.g. `peer:stolen`).
        node_id: String,
        /// Override QNM-Shared / mesh-home root (defaults to
        /// `$QNM_SHARED_ROOT` or `~/QNM-Shared`).
        #[arg(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
    },
    /// EPIC-SEC-BANLIST (Q53) — remove a node-id from this peer's
    /// ban list. Only lifts the entry THIS peer set; a ban another
    /// peer set must be lifted there (the gate checks the union).
    Unban {
        /// Node-id to unban.
        node_id: String,
        /// Override QNM-Shared / mesh-home root.
        #[arg(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
    },
    /// EPIC-SEC-BANLIST (Q53) — print the union of every peer's ban
    /// list (the set the enrollment gate enforces).
    BanList {
        /// Override QNM-Shared / mesh-home root.
        #[arg(long, env = "QNM_SHARED_ROOT")]
        workgroup_root: Option<PathBuf>,
    },
}

/// NF-18.x — `mackesd nebula <sub>` subcommands.
#[derive(Subcommand)]
enum NebulaCmd {
    /// NF-18.2 — emit a JSON array of every active peer cert
    /// (one row per active row in nebula_peer_certs, joined
    /// with the nodes table for the role field). Useful for
    /// off-cluster audit and as a human-readable backup record
    /// that complements the encrypted `ca export` bundle.
    ExportRoster,
}

/// Subcommands for `mackesd ansible-history`. CB-1.5.c
/// follow-up.
#[derive(Subcommand)]
enum AnsibleHistoryCmd {
    /// List every ansible-pull run record across the mesh.
    /// `--json` emits a sorted (timestamp DESC) JSON array.
    List {
        /// Emit a JSON array of `{peer, playbook, timestamp,
        /// exit_code, changed, ok, failed, triggered_by, ...}`
        /// rows.
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `mackesd events`. CB-1.8 mesh_history
/// follow-up.
#[derive(Subcommand)]
enum EventsCmd {
    /// List every row from the `events` table. `--json`
    /// emits a JSON array of every audit-log row in seq
    /// order.
    List {
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `mackesd playbooks`. CB-1.5.b follow-up.
#[derive(Subcommand)]
enum PlaybooksCmd {
    /// List every role under the curated playbooks root.
    /// `--json` emits `[{name, description}, ...]`.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Run a playbook locally via `ansible-pull --tags <name>
    /// site.yml`. Streams stdout to this process's stdout.
    Run {
        /// Role / tag name (matches a directory under the
        /// curated playbooks root).
        name: String,
    },
}

/// Subcommands for `mackesd remediate`. PLANES-11 (W41/W42).
#[derive(Subcommand)]
enum RemediateCmd {
    /// List the loaded remediation plans (the core pack + any TOML in
    /// `<root>/remediation/`). `--json` for the raw plan objects.
    Plans {
        /// Emit a JSON array of `RemediationPlan` rows.
        #[arg(long)]
        json: bool,
    },
    /// Evaluate the policies against the live directory and pair each
    /// violation with its remediation plan. `--json` emits the
    /// `MatchedDrift` array the Remediation panel consumes.
    Match {
        /// Emit the JSON array instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Fire a plan against a drifted peer — enqueues the plan's job
    /// template as a signed bundle the target runs locally (W32). The
    /// fire is loud: the launch reply (run id + targets) prints here.
    Fire {
        /// The remediation plan name (`mded remediate plans`).
        #[arg(long)]
        plan: String,
        /// The drifted peer hostname to remediate.
        #[arg(long)]
        peer: String,
    },
}

/// Subcommands for `mackesd dns`. PLANES-18 (W74/W75).
#[cfg(feature = "async-services")]
#[derive(Subcommand)]
enum DnsCmd {
    /// List the `<host>.mesh → overlay-ip` records built from the live
    /// roster. `--json` emits the array the Mesh DNS panel consumes.
    List {
        /// Emit the JSON array instead of the table.
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `mackesd validate`. PLANES-19 (W79/W80).
#[derive(Subcommand)]
enum ValidateCmd {
    /// Report the newest validation run's directed-edge reachability
    /// verdict. `--json` emits the object the Routing panel consumes.
    Status {
        /// Emit the JSON object instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Request a fresh overlay-reachability run (drops a `runnow`
    /// marker; the FPG leader mints the run).
    Run,
}

/// Subcommands for `mackesd netstate`. PLANES-15 (W65–W68).
#[derive(Subcommand)]
enum NetstateCmd {
    /// Desired (elected revision) vs actual (live nmstate) interface
    /// diff. `--json` emits the array the Interfaces panel consumes.
    Diff {
        /// Emit the JSON array instead of the table.
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `mackesd policy`. PLANES-13 (W46–W51).
#[derive(Subcommand)]
enum PolicyCmd {
    /// List every loaded policy with the peers that currently violate
    /// it (evaluated against the live directory). `--json` emits the
    /// array the Controller ▸ Policy panel consumes.
    List {
        /// Emit the JSON array instead of the table.
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `mackesd nodes`. CB-1.5.a.
#[derive(Subcommand)]
enum NodesCmd {
    /// List every row from the `nodes` table. Without `--json` the
    /// output is a human-readable table with one peer per line.
    List {
        /// Emit a JSON array of `{node_id, name, public_key, role,
        /// health, region}` rows — consumed by the Workbench
        /// Fleet → Inventory panel.
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `mackesd revisions`. Phase F.12.
#[derive(Subcommand)]
enum RevisionsCmd {
    /// List every revision in the `desired_config` table, newest
    /// first. `--json` for machine-readable output (consumed by the
    /// Workbench Fleet → Revisions panel).
    List {
        /// Emit a JSON array of `{revision_id, author, state,
        /// created_at, summary}` rows.
        #[arg(long)]
        json: bool,
    },
    /// Diff two revisions' spec_json payloads. Prints the keys
    /// added / removed / changed (uses `mackesd_core::revisions::diff`
    /// via a thin SQL adapter).
    Diff {
        /// "From" revision id.
        from: String,
        /// "To" revision id.
        to: String,
    },
    /// Roll back to a prior revision by writing its payload as a
    /// fresh applied revision (immutable history per 12.2.2).
    Rollback {
        /// Revision id to restore.
        target_id: String,
        /// Author tag for the new rollback revision (defaults to
        /// `peer:<hostname>`).
        #[arg(long)]
        author: Option<String>,
        /// Peer selector — `all` or comma-list. Today the rollback
        /// only writes the new row centrally; the per-peer apply
        /// happens via the existing reconcile loop. The selector
        /// is recorded in the rollback row's summary for audit.
        #[arg(long, default_value = "all")]
        peers: String,
    },
}

/// EFF-21 — read one secret line from stdin (trailing newline
/// stripped). Used by the `--*-stdin` flags so secrets never ride
/// argv (`/proc/<pid>/cmdline`) or the inherited environment.
fn read_secret_line(ctx: &str) -> anyhow::Result<String> {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| anyhow::anyhow!("{ctx}: reading secret from stdin: {e}"))?;
    let secret = line.trim_end_matches(['\r', '\n']).to_string();
    if secret.is_empty() {
        anyhow::bail!("{ctx}: empty secret on stdin");
    }
    Ok(secret)
}

fn main() -> anyhow::Result<()> {
    // EFF-10 — structured JSON logs when running non-interactively (under
    // systemd/journald, the daemon case) so they're machine-grep-able + ship to
    // a log aggregator; human-readable text when attached to a TTY (interactive
    // CLI use). Force either with MDE_LOG_FORMAT=json|text.
    use std::io::IsTerminal;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let json = match std::env::var("MDE_LOG_FORMAT").as_deref() {
        Ok("json") => true,
        Ok("text") => false,
        _ => !std::io::stderr().is_terminal(),
    };
    if json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_target(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .init();
    }

    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(mackesd_core::default_db_path);

    match cli.cmd {
        Cmd::Migrate => {
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let n = mackesd_core::store::applied_migration_count(&conn)?;
            tracing::info!("store at {} migrated (n={})", db_path.display(), n);
            println!("{n} migrations applied");
        }
        Cmd::Status => {
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let n = mackesd_core::store::applied_migration_count(&conn)?;
            println!("db:                 {}", db_path.display());
            println!("migrations applied: {n}");
        }
        Cmd::Healthz => {
            // EFF-8 — live report off the store: real node counts +
            // health buckets + audit-chain status (was a hardcoded
            // `empty()` baseline). On a fresh peer whose store hasn't
            // migrated yet this still degrades to the zero-node report.
            // (`is_leader`/`applied_revision` remain at defaults pending
            // the leader-lease + applied-revision query plumbing.)
            let report = match mackesd_core::store::open(&db_path) {
                Ok(conn) => mackesd_core::health::HealthReport::from_store(&conn),
                Err(_) => mackesd_core::health::HealthReport::empty(),
            };
            println!("{}", report.to_json_line()?);
        }
        Cmd::Connect { ip, port } => match mackesd_core::connect_actions::connect_argv(&ip, port) {
            Some((service, argv)) => {
                println!("{service}\t{}", argv.join(" "));
            }
            None => {
                eprintln!("error: no known connect-action for port {port}");
                std::process::exit(1);
            }
        },
        Cmd::ClassifyHost {
            mdns,
            port,
            vendor,
            hostname,
            mac,
        } => {
            // Derive the vendor from the MAC's OUI when not given directly.
            let oui_vendor = if vendor.is_empty() && !mac.is_empty() {
                mackesd_core::surrounding_hosts::load_system_oui()
                    .vendor_for(&mac)
                    .unwrap_or_default()
            } else {
                vendor
            };
            let sig = mackesd_core::surrounding_hosts::HostSignals {
                mdns_services: mdns,
                open_ports: port,
                oui_vendor,
                hostname,
            };
            let ty = mackesd_core::surrounding_hosts::classify(&sig);
            println!("{}", ty.wire_name());
        }
        Cmd::DiscoverMdns => {
            use mackesd_core::surrounding_hosts::{
                arp_neigh_map, classify, collect_mdns, enrich_hosts, hosts_from_mdns,
                load_system_oui, refine_unknown_with_http, refine_unknown_with_nmap_os,
                reverse_dns, HostSignals,
            };
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let records = collect_mdns("avahi-browse");
            let mut hosts = hosts_from_mdns(&records, now_ms);
            for host in &mut hosts {
                // Fill a missing hostname via reverse-DNS, then let the
                // console hostname-hint re-refine the type.
                if host.hostname.is_empty() {
                    if let Some(name) = reverse_dns(&host.ip) {
                        host.hostname = name;
                        let sig = HostSignals {
                            mdns_services: host.services.clone(),
                            hostname: host.hostname.clone(),
                            ..Default::default()
                        };
                        host.host_type = classify(&sig);
                    }
                }
            }
            // MESH-A-4.c.1 — ARP-MAC + OUI-vendor enrichment over the
            // local neighbour table, re-typing mDNS-less hosts.
            let mut hosts = enrich_hosts(hosts, &arp_neigh_map(), &load_system_oui());
            // MESH-A-4.c.3 — HTTP-banner refine for still-Unknown hosts.
            refine_unknown_with_http(&mut hosts);
            // MESH-A-4.c.3.b — active nmap -O fingerprint, last-resort
            // refine for hosts still Unknown after the HTTP banner.
            refine_unknown_with_nmap_os(&mut hosts);
            for host in &hosts {
                println!("{}", serde_json::to_string(host)?);
            }
        }
        Cmd::SurroundingList => {
            use mackesd_core::surrounding_hosts::read_all_surrounding;
            if let Some(data_dir) = dirs::data_dir() {
                let base = data_dir.join("mde").join("surrounding");
                for ch in read_all_surrounding(&base) {
                    println!("{}", serde_json::to_string(&ch)?);
                }
            }
        }
        Cmd::SurroundingTrust { key, state } => {
            use mackesd_core::surrounding_hosts::{set_host_trust, TrustState};
            let ts = match state.to_ascii_lowercase().as_str() {
                "trusted" => TrustState::Trusted,
                "blocked" => TrustState::Blocked,
                "unknown" => TrustState::Unknown,
                other => {
                    eprintln!(
                        "error: unknown trust state '{other}' (want trusted|blocked|unknown)"
                    );
                    std::process::exit(1);
                }
            };
            let Some(data_dir) = dirs::data_dir() else {
                eprintln!("error: no XDG data dir");
                std::process::exit(1);
            };
            let path = data_dir.join("mde").join("surrounding").join("trust.json");
            match set_host_trust(&path, &key, ts) {
                Ok(_) => println!("{key}\t{}", ts.wire_name()),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::MeshFirewallPlan => {
            use mackesd_core::surrounding_hosts::{
                blocked_ips, drop_rich_rule_body, read_all_surrounding,
            };
            if let Some(data_dir) = dirs::data_dir() {
                let base = data_dir.join("mde").join("surrounding");
                for ip in blocked_ips(&read_all_surrounding(&base)) {
                    println!("{}", drop_rich_rule_body(&ip));
                }
            }
        }
        Cmd::ArpSpoofCheck => {
            use mackesd_core::surrounding_hosts::{arp_neigh_map, arp_spoof_suspects};
            for (mac, ips) in arp_spoof_suspects(&arp_neigh_map()) {
                println!("{mac}\t{}", ips.join(","));
            }
        }
        Cmd::RogueDhcpCheck => {
            use mackesd_core::surrounding_hosts::detect_dhcp_servers;
            let servers = detect_dhcp_servers();
            for s in &servers {
                println!("{s}");
            }
            if servers.len() >= 2 {
                eprintln!(
                    "ROGUE-DHCP: {} DHCP servers responding (expected 1)",
                    servers.len()
                );
                std::process::exit(1);
            }
        }
        Cmd::CaptivePortalCheck => {
            use mackesd_core::surrounding_hosts::{detect_captive_portal, CAPTIVE_PROBE_URL};
            if let Some(portal) = detect_captive_portal(CAPTIVE_PROBE_URL) {
                if portal.is_empty() {
                    eprintln!("CAPTIVE-PORTAL: detected (splash intercept; no redirect URL)");
                } else {
                    println!("{portal}");
                    eprintln!("CAPTIVE-PORTAL: redirected to {portal}");
                }
                std::process::exit(1);
            }
        }
        Cmd::VoipRtt => {
            use mackesd_core::voip_rtt::{
                own_nebula_ip, publish_link_rtt, rtt_topic, sample_link_rtt, VITELITY_PROXY_HOST,
                VITELITY_PROXY_PORT,
            };
            let peer = own_nebula_ip().unwrap_or_default();
            let sample = sample_link_rtt(&peer);
            match sample.rtt_ms {
                Some(ms) => {
                    println!(
                        "voip-link-rtt: {ms} ms ({VITELITY_PROXY_HOST}:{VITELITY_PROXY_PORT})"
                    );
                }
                None => {
                    println!(
                        "voip-link-rtt: unreachable ({VITELITY_PROXY_HOST}:{VITELITY_PROXY_PORT})"
                    );
                }
            }
            if peer.is_empty() {
                eprintln!("voip-rtt: no nebula1 overlay IP — measured but not published");
            } else {
                publish_link_rtt(&sample);
                eprintln!("voip-rtt: published to {}", rtt_topic(&peer));
            }
        }
        Cmd::Tag { host, set } => {
            let root = mackesd_core::default_qnm_shared_root();
            let target = host.unwrap_or_else(|| {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "unknown".to_string())
            });
            use mackes_mesh_types::cap_tags::{read_tags, write_tags, CapabilityTag, NodeTags};
            if let Some(spec) = set {
                let mut tags = NodeTags::default();
                for tok in spec.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                    match CapabilityTag::parse(tok) {
                        Some(t) => {
                            tags.tags.insert(t);
                        }
                        None => anyhow::bail!(
                            "unknown capability tag `{tok}` — expected hop|execution|headless"
                        ),
                    }
                }
                write_tags(&root, &target, &tags)?;
                // W83 — audit the change (security-relevant fleet edit).
                tracing::info!(
                    target: "mackesd::audit",
                    event = "cap_tags.set",
                    host = %target,
                    tags = %spec,
                    "PLANES-3: capability tags updated"
                );
                println!("tags for {target}: {}", spec);
            } else {
                let tags = read_tags(&root, &target);
                let names: Vec<&str> = tags.tags.iter().map(|t| t.as_str()).collect();
                println!(
                    "tags for {target}: {}",
                    if names.is_empty() {
                        "(none)".to_string()
                    } else {
                        names.join(", ")
                    }
                );
            }
            return Ok(());
        }
        Cmd::HopAdvertise { subnets, exit } => {
            use mackesd_core::nebula_topology::{write_advert, HopAdvert, EXIT_ROUTE};
            let root = mackesd_core::default_qnm_shared_root();
            let host = local_hostname();
            let overlay_ip = local_overlay_ip().ok_or_else(|| {
                anyhow::anyhow!("no overlay IP on nebula1 — is this node enrolled and up?")
            })?;
            let mut nets: Vec<String> = subnets
                .as_deref()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if exit && !nets.iter().any(|s| s == EXIT_ROUTE) {
                nets.push(EXIT_ROUTE.to_string());
            }
            if nets.is_empty() {
                anyhow::bail!("nothing to advertise — pass --subnets <cidr,...> and/or --exit");
            }
            let advert = HopAdvert {
                hop: host.clone(),
                overlay_ip,
                subnets: nets.clone(),
            };
            write_advert(&root, &advert)?;
            tracing::info!(
                target: "mackesd::audit",
                event = "topology.hop_advertise",
                host = %host,
                subnets = %nets.join(","),
                "PLANES-17: hop advertisement updated"
            );
            println!("hop {host} now advertises: {}", nets.join(", "));
            return Ok(());
        }
        Cmd::VpnImport { name, kind, file } => {
            use mackesd_core::nebula_topology::{write_vpn_profile, VpnKind, VpnProfile};
            let root = mackesd_core::default_qnm_shared_root();
            let kind = match kind.to_ascii_lowercase().as_str() {
                "wireguard" | "wg" => VpnKind::Wireguard,
                "openvpn" | "ovpn" => VpnKind::Openvpn,
                other => anyhow::bail!("unknown VPN kind `{other}` — expected wireguard|openvpn"),
            };
            let config = std::fs::read_to_string(&file)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", file.display()))?;
            let path = write_vpn_profile(
                &root,
                &VpnProfile {
                    name: name.clone(),
                    kind,
                    config,
                },
            )?;
            println!("imported VPN client profile `{name}` → {}", path.display());
            let all = mackesd_core::nebula_topology::list_vpn_profiles(&root);
            println!("stored client profiles ({}):", all.len());
            for (n, k) in all {
                println!("  - {n} ({k:?})");
            }
            return Ok(());
        }
        Cmd::RolePin { role } => {
            let parsed: mde_role::Role = role.parse().map_err(|_| {
                anyhow::anyhow!("unknown role `{role}` — expected lighthouse|server|workstation")
            })?;
            match mde_role::pin(parsed) {
                Ok(outcome) => {
                    println!("role pinned: {outcome:?}");
                    return Ok(());
                }
                Err(e) => anyhow::bail!("role pin refused: {e}"),
            }
        }
        Cmd::RoleWorkers { role } => {
            let show = |r: mde_role::Role| {
                let mut names = mackesd_core::worker_role::workers_for_rank(r.rank());
                names.sort_unstable();
                println!("{} (rank {}) runs {} workers:", r, r.rank(), names.len());
                for n in names {
                    println!("  {n}");
                }
            };
            match role {
                Some(s) => match s.parse::<mde_role::Role>() {
                    Ok(r) => show(r),
                    Err(e) => {
                        eprintln!("mackesd role-workers: {e}");
                        std::process::exit(1);
                    }
                },
                None => {
                    for r in mde_role::Role::all() {
                        show(r);
                    }
                }
            }
        }
        Cmd::RoleGate { min_rank } => {
            let rank = mackesd_core::worker_role::resolve_rank();
            if rank < min_rank {
                let role = mde_role::load()
                    .map(|r| r.to_string())
                    .unwrap_or_else(|_| "unpinned".to_string());
                eprintln!(
                    "mackesd role-gate: role conflict — this {role} box (rank {rank}) does not \
                     satisfy the unit's required min-rank {min_rank}; refusing to start the service"
                );
                std::process::exit(1);
            }
            // rank >= min_rank: the gate is satisfied; the unit may start (exit 0).
        }
        Cmd::DnsLeakCheck { expected } => {
            use mackesd_core::surrounding_hosts::{dns_leak, parse_resolv_nameservers};
            let content = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
            let leaked = dns_leak(&parse_resolv_nameservers(&content), &expected);
            for ip in &leaked {
                println!("{ip}");
            }
            if !leaked.is_empty() {
                eprintln!(
                    "DNS-LEAK: {} resolver(s) outside the expected mesh set",
                    leaked.len()
                );
                std::process::exit(1);
            }
        }
        Cmd::EvilTwinCheck => {
            use mackesd_core::surrounding_hosts::{
                evil_twin_suspects, learn_wifi, load_wifi_baseline, save_wifi_baseline,
                scan_wifi_bssids,
            };
            let scan = scan_wifi_bssids();
            let suspects = if let Some(data_dir) = dirs::data_dir() {
                let path = data_dir
                    .join("mde")
                    .join("surrounding")
                    .join("wifi-baseline.json");
                let mut baseline = load_wifi_baseline(&path);
                let suspects = evil_twin_suspects(&scan, &baseline);
                learn_wifi(&mut baseline, &scan); // detect-then-learn
                let _ = save_wifi_baseline(&path, &baseline);
                suspects
            } else {
                Vec::new()
            };
            for (ssid, bssid) in &suspects {
                println!("{ssid}\t{bssid}");
            }
            if !suspects.is_empty() {
                eprintln!(
                    "EVIL-TWIN: {} known SSID(s) on unexpected BSSIDs",
                    suspects.len()
                );
                std::process::exit(1);
            }
        }
        Cmd::RecordAttack { source } => {
            use mackesd_core::surrounding_hosts::{
                accumulate_alert, auto_ack, load_alert_store, save_alert_store,
            };
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if let Some(data_dir) = dirs::data_dir() {
                let path = data_dir
                    .join("mde")
                    .join("surrounding")
                    .join("persistent-alerts.json");
                let mut store = load_alert_store(&path);
                auto_ack(&mut store, now_ms);
                accumulate_alert(&mut store, &source, now_ms);
                let _ = save_alert_store(&path, &store);
                if let Some(a) = store.get(&source) {
                    println!(
                        "{}\tcount={}\tfirst_seen_ms={}\tlast_seen_ms={}",
                        a.source, a.count, a.first_seen_ms, a.last_seen_ms
                    );
                }
            }
        }
        Cmd::AuditLog { event, detail } => {
            use mackesd_core::audit_log::write_audit_event;
            if let Some(data_dir) = dirs::data_dir() {
                let activity_root = data_dir.join("mde").join("activity");
                match write_audit_event(&activity_root, &event, &detail) {
                    Ok(path) => println!("{}", path.display()),
                    Err(e) => {
                        eprintln!("error: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Cmd::DiscoverMdePeers => {
            use mackesd_core::surrounding_hosts::{
                collect_mdns, hosts_from_mdns, mde_peer_candidates,
            };
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let hosts = hosts_from_mdns(&collect_mdns("avahi-browse"), now_ms);
            for (ip, hostname) in mde_peer_candidates(&hosts) {
                println!("{ip}\t{hostname}");
            }
        }
        Cmd::Probe { action } => match action {
            ProbeCmd::Scan {
                targets,
                deep,
                source,
                nse_dir,
            } => {
                use mackesd_core::probe_nmap::{scan, Profile};
                use mde_card::probe::HostSource;
                let src = match source.as_str() {
                    "lan" => HostSource::Lan,
                    "arbitrary" => HostSource::Arbitrary,
                    _ => HostSource::Mesh,
                };
                let profile = if deep { Profile::Deep } else { Profile::Fast };
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let cards = scan("nmap", profile, &targets, &[], &nse_dir, src, now);
                // One JSON line per host card (each carries its service
                // children). Empty output = no hosts found / nmap absent.
                for card in &cards {
                    println!("{}", serde_json::to_string(card)?);
                }
            }
            ProbeCmd::Refresh {
                workgroup_root,
                node_id,
                nse_dir,
            } => {
                // MESH-PROBE-4 manual refresh — one deep cycle that
                // writes probe-inventory.json + announces probe/changed.
                let workgroup_root =
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                let node_id = node_id.unwrap_or_else(default_node_id);
                let home =
                    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
                let n = mackesd_core::probe_nmap::run_probe_cycle(
                    &workgroup_root,
                    &node_id,
                    &home,
                    "nmap",
                    &nse_dir,
                    true,
                );
                println!("probe refresh: {n} host(s) in inventory");
            }
            ProbeCmd::List {
                workgroup_root,
                service,
            } => {
                // MESH-PROBE-6 — read the merged mesh-wide inventory.
                let workgroup_root =
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                match service {
                    Some(kind) => {
                        for hs in
                            mackesd_core::probe_nmap::peers_with_service(&workgroup_root, &kind)
                        {
                            println!(
                                "{}\t{}\t{}:{}",
                                hs.host.ip,
                                hs.service.service_kind,
                                hs.host.hostname,
                                hs.service.port
                            );
                        }
                    }
                    None => {
                        for card in &mackesd_core::probe_nmap::inventory(&workgroup_root) {
                            println!("{}", serde_json::to_string(card)?);
                        }
                    }
                }
            }
        },
        Cmd::PresetLaunch { tag } => {
            // Portal-18.d (v6.0 R12, 2026-05-27) — preset launch-
            // bundle expansion. Loads the tag store, finds the
            // named preset, fires `swaymsg exec <cmd>` for each
            // entry in `launch_bundle`. Prints a one-line summary;
            // non-zero exit when any exec fails.
            let store = mackes_mesh_types::TagStore::load_default()
                .with_context(|| "loading tag store for preset-launch")?;
            let Some(tag_entry) = store.find_by_name(&tag) else {
                eprintln!("error: tag '{tag}' not found in tag store");
                std::process::exit(1);
            };
            let launch_bundle = match &tag_entry.flavor {
                mackes_mesh_types::TagFlavor::Preset { launch_bundle } => launch_bundle.clone(),
                other => {
                    eprintln!("error: tag '{tag}' is not a preset (flavor: {:?})", other);
                    std::process::exit(1);
                }
            };
            if launch_bundle.is_empty() {
                eprintln!("error: tag '{tag}' has an empty launch_bundle");
                std::process::exit(1);
            }
            let total = launch_bundle.len();
            let mut launched = 0usize;
            for cmd_str in &launch_bundle {
                let escaped = cmd_str.replace('\\', "\\\\").replace('"', "\\\"");
                let swayipc_cmd = format!("exec \"{escaped}\"");
                let status = std::process::Command::new("swaymsg")
                    .arg(&swayipc_cmd)
                    .status();
                match status {
                    Ok(s) if s.success() => launched += 1,
                    Ok(s) => {
                        eprintln!("warn: swaymsg exit {s} for '{cmd_str}'");
                    }
                    Err(e) => {
                        eprintln!("warn: swaymsg spawn failed for '{cmd_str}': {e}");
                    }
                }
            }
            println!("launched {launched}/{total} from preset '{tag}'");
            if launched != total {
                std::process::exit(1);
            }
        }
        Cmd::StateRestore {
            bundle,
            verify,
            passphrase_env,
            recovery_dir,
        } => {
            // MESHFS-14.1 — bundle decode + CA restore + LizardFS
            // metadata extraction. We deliberately do NOT replay
            // the LizardFS volume config automatically; operator
            // runs `mfsmaster --import-metadata` against the
            // extracted dump.
            let passphrase = std::env::var(&passphrase_env).with_context(|| {
                format!(
                    "passphrase env-var {passphrase_env} unset — \
                     export it before running state restore",
                )
            })?;
            let armored = std::fs::read_to_string(&bundle)
                .with_context(|| format!("reading bundle {}", bundle.display()))?;
            let sealed =
                mackesd_core::ca::backup::dearmor(&armored).context("ASCII-armor decode")?;
            let plaintext = mackesd_core::ca::backup::unseal(&passphrase, &sealed)
                .context("AEAD unseal — wrong passphrase OR tampered bundle")?;

            // EFF-28 — --verify: report + stop before any mutation.
            if verify {
                eprintln!(
                    "[state-restore --verify] bundle OK: mesh '{}' · exported_at unix:{} · \
                     {} CA cert(s) · {} peer cert(s) · meshfs snapshot: {}",
                    plaintext.mesh_id,
                    plaintext.exported_at,
                    plaintext.ca_certs.len(),
                    plaintext.peer_certs.len(),
                    if plaintext.meshfs_snapshot.is_some() {
                        "present"
                    } else {
                        "absent"
                    },
                );
                eprintln!(
                    "[state-restore --verify] dry-run complete — nothing was written. \
                     Re-run without --verify to restore."
                );
                return Ok(());
            }

            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            mackesd_core::ca::backup::restore_to_store(&conn, &plaintext)
                .context("restoring CA + peer rows to store")?;
            eprintln!(
                "[state-restore] CA: {ca_n} cert(s) + {peer_n} peer cert(s) restored",
                ca_n = plaintext.ca_certs.len(),
                peer_n = plaintext.peer_certs.len(),
            );

            // MESHFS-14.1 — extract LizardFS snapshot if present.
            match plaintext.meshfs_snapshot.as_ref() {
                None => {
                    eprintln!(
                        "[state-restore] bundle has no meshfs snapshot (CA-only, pre-MESHFS, or Gluster-only bundle) — skipping LizardFS step",
                    );
                }
                Some(snap) => {
                    std::fs::create_dir_all(&recovery_dir)
                        .with_context(|| format!("mkdir {}", recovery_dir.display()))?;
                    let mut wrote = 0usize;
                    // Write metadata dump for `mfsmaster --import-metadata`.
                    if let Some(dump) = snap.metadata_dump.as_deref() {
                        let path = recovery_dir.join("metadata.mfs.dump");
                        std::fs::write(&path, dump)
                            .with_context(|| format!("writing {}", path.display()))?;
                        wrote += 1;
                        eprintln!(
                            "[state-restore] meshfs: wrote {} ({} bytes)",
                            path.display(),
                            dump.len(),
                        );
                    }
                    // Write exports config for re-application.
                    if let Some(cfg) = snap.exports_config.as_deref() {
                        let path = recovery_dir.join("mfsexports.cfg");
                        std::fs::write(&path, cfg)
                            .with_context(|| format!("writing {}", path.display()))?;
                        wrote += 1;
                        eprintln!(
                            "[state-restore] meshfs: wrote {} ({} bytes)",
                            path.display(),
                            cfg.len(),
                        );
                    }
                    // Write CS list for topology reference.
                    if let Some(cs) = snap.cs_list.as_deref() {
                        let path = recovery_dir.join("cs-list.txt");
                        std::fs::write(&path, cs)
                            .with_context(|| format!("writing {}", path.display()))?;
                        wrote += 1;
                        eprintln!(
                            "[state-restore] meshfs: wrote {} ({} bytes)",
                            path.display(),
                            cs.len(),
                        );
                    }
                    if wrote == 0 {
                        eprintln!(
                            "[state-restore] meshfs snapshot present but every section was empty — nothing to apply",
                        );
                    } else {
                        let goal_hint = snap.goal.map_or_else(
                            || "N (re-count enrolled peers)".to_owned(),
                            |g| g.to_string(),
                        );
                        eprintln!(
                            "[state-restore] meshfs: {wrote} file(s) at {dir}; restore steps:\n\
                             1. cp {dir}/mfsexports.cfg /etc/mfs/mfsexports.cfg\n\
                             2. mfsmaster --import-metadata {dir}/metadata.mfs.dump\n\
                             3. mfsmaster start  # starts the active master\n\
                             4. mfssetgoal -r {goal_hint} /mnt/mesh-storage\n\
                             (see docs/help/mesh-recovery.md)",
                            dir = recovery_dir.display(),
                        );
                    }
                }
            }
        }
        Cmd::PreflightMeshFsHeadroom { data_dir, home } => {
            // MESHFS-1.2 — pre-flight headroom check for the
            // LizardFS mesh-storage rollout. Mirrors the gluster
            // headroom CLI; operator runs this before upgrading
            // to v5.0.0 or before mesh-storage bootstrap.
            let home_dir = home
                .clone()
                .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
                .context("no HOME env var; pass --home <dir>")?;
            let xdg = mackesd_core::meshfs::headroom::default_xdg_dirs(&home_dir);
            let report = mackesd_core::meshfs::headroom::check(&data_dir, &xdg);
            eprintln!("{}", report.summary());
            println!(
                "{}",
                serde_json::to_string(&report).context("encode report")?
            );
            match report.verdict {
                mackesd_core::meshfs::headroom::Verdict::Ok => {}
                mackesd_core::meshfs::headroom::Verdict::Warn
                | mackesd_core::meshfs::headroom::Verdict::NoDataDir => std::process::exit(1),
            }
        }
        Cmd::MeshFsStatus { vip, admin_binary } => {
            let report =
                mackesd_core::workers::meshfs_worker::meshfs_status_report(&admin_binary, &vip);
            let reachable = report.master_reachable;
            println!(
                "{}",
                serde_json::to_string(&report).context("encode meshfs status")?
            );
            if !reachable {
                std::process::exit(1);
            }
        }
        Cmd::MeshFsTrashList { mount_path } => {
            let entries = mackesd_core::workers::meshfs_worker::list_trash_entries(&mount_path);
            println!(
                "{}",
                serde_json::to_string(&entries).context("encode trash list")?
            );
        }
        Cmd::MeshFsUndelete {
            vip,
            path,
            admin_binary,
        } => {
            let argv = vec![
                admin_binary.clone(),
                vip.clone(),
                "TRASH-RECOVER".to_owned(),
                path.clone(),
            ];
            let ok = std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                anyhow::bail!("TRASH-RECOVER failed for {path}");
            }
        }
        Cmd::MeshFsResolveConflict { path } => {
            mackesd_core::workers::meshfs_worker::resolve_conflict_to_archive(&path)
                .map_err(|e| anyhow::anyhow!("resolve-conflict failed: {e}"))?;
        }
        Cmd::GeneratePasscode { store, cred_path } => {
            let code = mackesd_core::passcode::generate();
            println!("{code}");
            if store {
                let path =
                    cred_path.unwrap_or_else(mackesd_core::passcode_creds::default_cred_path);
                mackesd_core::passcode_creds::store(
                    &code,
                    &path,
                    mackesd_core::passcode_creds::CRED_NAME,
                )
                .map_err(|e| anyhow::anyhow!("generate-passcode --store: {e}"))?;
                eprintln!(
                    "stored (encrypted via systemd-creds) at {}. Share the code \
                     above with peers; the plaintext is not on disk.",
                    path.display()
                );
            } else {
                eprintln!(
                    "(encrypt at rest with: mackesd generate-passcode --store, \
                     or save to libsecret manually)"
                );
            }
        }
        Cmd::LogEmit {
            level,
            target,
            message,
        } => {
            let root = mackesd_core::default_qnm_shared_root();
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let record = magic_fleet::structured_log::LogRecord {
                ts_ms: now_ms,
                host: local_hostname(),
                level,
                target,
                message,
                fields: std::collections::BTreeMap::new(),
            };
            magic_fleet::structured_log::append(&root, &record)
                .map_err(|e| anyhow::anyhow!("log-emit append: {e}"))?;
            return Ok(());
        }
        Cmd::FleetStatus { json } => {
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let nodes = mackesd_core::store::list_nodes(&conn).context("listing nodes")?;
            let pairs: Vec<(String, String)> = nodes
                .iter()
                .map(|n| (n.role.clone(), n.health.clone()))
                .collect();
            let groups = mackesd_core::fleet_rollup::rollup(&pairs);
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "total": nodes.len(), "groups": groups })
                );
            } else if groups.is_empty() {
                println!("fleet empty (no enrolled nodes)");
            } else {
                println!("{:<14} {:>5}  {:<12}", "ROLE", "TOTAL", "WORST HEALTH");
                for g in &groups {
                    println!("{:<14} {:>5}  {:<12}", g.role, g.total, g.worst_health);
                }
            }
        }
        Cmd::Identity { json } => {
            // Load (or first-create) this node's signing key, fingerprint
            // it, and render the W25 word-pair.
            let key_path = std::path::PathBuf::from(mackesd_core::node_key::DEFAULT_KEY_PATH);
            let signing = mackesd_core::node_key::load_or_create(&key_path)
                .with_context(|| format!("loading node key at {}", key_path.display()))?;
            let node = mackesd_core::identity::NodeKey::from_bytes(signing.to_bytes());
            let fingerprint = node.fingerprint();
            let word_pair = mackesd_core::identity::fingerprint_word_pair(&fingerprint);
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "fingerprint": fingerprint, "word_pair": word_pair })
                );
            } else {
                println!("fingerprint: {fingerprint}");
                println!("word-pair:   {word_pair}");
            }
        }
        Cmd::AuditVerify { json } => {
            // Reads every row from the `events` table (ordered by
            // `seq` ASC) and walks the SHA-256 hash chain.
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let rows =
                mackesd_core::store::load_audit_rows(&conn).context("loading events from store")?;
            let outcome = mackesd_core::audit::verify(&rows);
            if json {
                // PLANES-12 — the Audit panel's data: the verify verdict
                // plus the 72 h rolling window of events (W44/W45).
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                let window_ms: i64 = 72 * 3600 * 1000;
                let timeline: Vec<serde_json::Value> = rows
                    .iter()
                    .filter(|r| now_ms.saturating_sub(r.timestamp_ms) <= window_ms)
                    .map(|r| {
                        serde_json::json!({
                            "event_id": r.event_id,
                            "timestamp_ms": r.timestamp_ms,
                            "payload": String::from_utf8_lossy(&r.payload),
                            "hash": r.hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                        })
                    })
                    .collect();
                let (status, detail) = match &outcome {
                    mackesd_core::audit::VerifyOutcome::Empty => ("empty", String::new()),
                    mackesd_core::audit::VerifyOutcome::Intact { verified, .. } => {
                        ("intact", format!("{verified} events"))
                    }
                    mackesd_core::audit::VerifyOutcome::Break { at_event, .. } => {
                        ("break", format!("at event {at_event}"))
                    }
                };
                println!(
                    "{}",
                    serde_json::json!({
                        "verify": status,
                        "detail": detail,
                        "total_events": rows.len(),
                        "retained_72h": timeline.len(),
                        "timeline": timeline,
                    })
                );
                if status == "break" {
                    std::process::exit(1);
                }
            } else {
                match outcome {
                    mackesd_core::audit::VerifyOutcome::Empty => {
                        println!("audit chain empty (no events yet)");
                    }
                    mackesd_core::audit::VerifyOutcome::Intact { verified, .. } => {
                        println!("verified {verified} events  ·  chain intact");
                    }
                    mackesd_core::audit::VerifyOutcome::Break { at_event, .. } => {
                        eprintln!("audit chain BREAK at event {at_event}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Cmd::RotatePasscode { store, cred_path } => {
            // Phase 12.10.2 — generate fresh passcode; peer
            // redistribution wires through the reconcile loop (12.5).
            let code = mackesd_core::passcode::generate();
            println!("{code}");
            if store {
                let path =
                    cred_path.unwrap_or_else(mackesd_core::passcode_creds::default_cred_path);
                mackesd_core::passcode_creds::store(
                    &code,
                    &path,
                    mackesd_core::passcode_creds::CRED_NAME,
                )
                .map_err(|e| anyhow::anyhow!("rotate-passcode --store: {e}"))?;
                eprintln!(
                    "rotation: stored (encrypted via systemd-creds) at {}; \
                     peers refresh their bearer tokens on next heartbeat.",
                    path.display()
                );
            } else {
                eprintln!(
                    "rotation: encrypt at rest with `mackesd rotate-passcode \
                     --store`; peers refresh their bearer tokens on next \
                     heartbeat."
                );
            }
        }
        Cmd::ShowPasscode { cred_path } => {
            // EPIC-SEC-PASSCODE-CREDS — decrypt + print the stored
            // passcode. The inverse of generate/rotate --store.
            let path = cred_path.unwrap_or_else(mackesd_core::passcode_creds::default_cred_path);
            let code =
                mackesd_core::passcode_creds::load(&path, mackesd_core::passcode_creds::CRED_NAME)
                    .map_err(|e| anyhow::anyhow!("show-passcode: {e}"))?;
            println!("{code}");
        }
        Cmd::PeersWhy { node_id } => {
            // Phase 12.4.4 — explanation surface. Loads the node
            // roster from the store, runs `topology::calculate`,
            // and walks the resulting edge set + route table to
            // emit a per-edge reason chain for the named peer.
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let nodes =
                mackesd_core::store::list_nodes(&conn).context("listing nodes from store")?;
            let report = explain_peer(&node_id, &nodes);
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Cmd::Apply { dry_run } => {
            if dry_run {
                // Phase 12.7.4 — run validation against an empty
                // snapshot today; once the store wires the
                // serialized desired-config row in, the dry-run
                // path returns the real diff + event-log preview.
                let snapshot = mackesd_core::topology::DesiredSnapshot::default();
                let errors = mackesd_core::validation::validate(&snapshot);
                let report = serde_json::json!({
                    "dry_run": true,
                    "validation_errors": errors.len(),
                    "would_apply_revisions": 0,
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                eprintln!(
                    "mackesd: non-dry-run apply requires the reconcile loop \
                     (Phase 12.5) — use `mackesd apply --dry-run` for the \
                     validation + plan preview."
                );
                std::process::exit(2);
            }
        }
        Cmd::Enroll {
            passcode,
            passcode_stdin,
            token,
            token_stdin,
            name,
            workgroup_root,
        } => {
            // EFF-21 — stdin intake keeps the secret out of
            // /proc/<pid>/cmdline + shell history. clap's conflict
            // rules guarantee at most one source is set.
            let passcode = if passcode_stdin {
                Some(read_secret_line("enroll --passcode-stdin")?)
            } else {
                passcode
            };
            let token = if token_stdin {
                Some(read_secret_line("enroll --token-stdin")?)
            } else {
                token
            };
            let display = name.unwrap_or_else(|| {
                std::env::var("HOSTNAME").unwrap_or_else(|_| {
                    std::process::Command::new("hostname")
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map_or_else(|| "unknown".to_owned(), |s| s.trim().to_owned())
                })
            });
            match (passcode, token) {
                (Some(_), Some(_)) => {
                    // `conflicts_with` should catch this at parse
                    // time, but belt-and-braces.
                    eprintln!(
                        "mackesd enroll: --passcode and --token are mutually \
                         exclusive; pass exactly one."
                    );
                    std::process::exit(2);
                }
                (None, None) => {
                    eprintln!(
                        "mackesd enroll: pass either --passcode (v1.x flow) or \
                         --token (v2.5 Nebula flow)."
                    );
                    std::process::exit(2);
                }
                (Some(pc), None) => {
                    // Phase 12.3.1 — v1.x build identity + signed request.
                    let identity = mackesd_core::enrollment::build_identity();
                    match mackesd_core::enrollment::build_request(&identity, &pc, &display) {
                        Some(req) => {
                            println!("{}", serde_json::to_string_pretty(&req)?);
                            eprintln!(
                                "enrollment request emitted — drop into the leader's \
                                 pending inbox (Phase 12.8.2)."
                            );
                        }
                        None => {
                            eprintln!(
                                "mackesd enroll: passcode failed validation (must be \
                                 16 URL-safe characters)."
                            );
                            std::process::exit(2);
                        }
                    }
                }
                (None, Some(tok)) => {
                    // NF-3.6.a — v2.5 Nebula join-token flow.
                    let workgroup_root =
                        workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                    let node_id = default_node_id();
                    eprintln!(
                        "mackesd enroll: publishing CSR + waiting up to {} s \
                         for the lighthouse to sign…",
                        mackesd_core::nebula_enroll::ENROLL_WAIT_TIMEOUT.as_secs(),
                    );
                    match mackesd_core::nebula_enroll::enroll_with_token(
                        &workgroup_root,
                        &node_id,
                        &display,
                        &tok,
                    ) {
                        Ok(outcome) => {
                            println!(
                                "enrolled into mesh '{}' as {} (overlay {}) after {} s.",
                                outcome.mesh_id,
                                node_id,
                                outcome.overlay_ip,
                                outcome.waited.as_secs(),
                            );
                            eprintln!(
                                "nebula_supervisor will materialize /etc/nebula/ \
                                 from the bundle on its next reconcile tick."
                            );
                        }
                        Err(e) => {
                            eprintln!("mackesd enroll: {e}");
                            std::process::exit(2);
                        }
                    }
                }
            }
        }
        Cmd::Decommission { node_id, force } => {
            // Phase 12.3.4 — soft-delete the node row and emit a
            // hash-chained Lifecycle event so the audit trail
            // records the operator action. `--force` only changes
            // the audit kind label; the SQL effect is identical
            // (CHECK constraint enforces the same allowed roles).
            let mut conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let updated = mackesd_core::store::set_node_role(&conn, &node_id, "decommissioned")?;
            if updated == 0 {
                eprintln!("mackesd decommission: no node row matches {node_id}");
                std::process::exit(2);
            }
            let payload = serde_json::json!({
                "kind":  if force { "forced" } else { "soft" },
                "node":  node_id,
                "event": "decommission",
            })
            .to_string();
            mackesd_core::store::insert_event(
                &mut conn,
                "lifecycle",
                &default_node_id(),
                &payload,
            )?;
            let report = serde_json::json!({
                "decommission":     node_id,
                "kind":             if force { "forced" } else { "soft" },
                "history_retained": true,
                "audit_logged":     true,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Cmd::Reenroll { node_id } => {
            // Phase 12.3.5 — mint a fresh keypair and write its
            // hex public key into the existing node row. Lifecycle
            // event records the old fingerprint so a forensic
            // walker can correlate before/after.
            let mut conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let prior = mackesd_core::store::list_nodes(&conn)?
                .into_iter()
                .find(|n| n.node_id == node_id);
            let new_identity = mackesd_core::enrollment::build_identity();
            let new_fp = new_identity.key.fingerprint();
            let updated = mackesd_core::store::refresh_node_credentials(&conn, &node_id, &new_fp)?;
            if updated == 0 {
                eprintln!("mackesd reenroll: no node row matches {node_id}");
                std::process::exit(2);
            }
            let payload = serde_json::json!({
                "event":           "reenroll",
                "node":            node_id,
                "old_fingerprint": prior.map(|p| p.public_key),
                "new_fingerprint": &new_fp,
            })
            .to_string();
            mackesd_core::store::insert_event(
                &mut conn,
                "lifecycle",
                &default_node_id(),
                &payload,
            )?;
            let report = serde_json::json!({
                "reenroll":         node_id,
                "new_fingerprint":  new_fp,
                "history_retained": true,
                "audit_logged":     true,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Cmd::TakeLeadership { as_node } => {
            // Phase 12.1.1b — operator-forced leadership bump.
            let lock_path = mackesd_core::default_qnm_shared_root().join(".mackesd-leader.lock");
            let lease = mackesd_core::leader::force_take(&lock_path, &as_node)
                .with_context(|| format!("rewriting {}", lock_path.display()))?;
            println!(
                "leader: {} (epoch {}) — lease renewed at {}",
                lease.node_id, lease.epoch, lease.renewed_at_s
            );
        }
        Cmd::ImportLegacy { dry_run } => {
            // Phase 12.13.2 — inventory the legacy caches under the
            // three canonical roots, then either preview the plan
            // (dry-run, default) or write desired-state rows into
            // the store. The importer is conservative: it only
            // creates node rows for mesh-related artifacts whose
            // filename carries an obvious peer identifier; it never
            // overwrites an existing row.
            let roots = mackesd_core::legacy_inventory::default_roots();
            let artifacts = mackesd_core::legacy_inventory::inventory(&roots);
            let mesh_artifacts: Vec<_> = artifacts.iter().filter(|a| a.mesh_data).collect();
            let candidate_node_names = derive_legacy_node_names(&mesh_artifacts);
            if dry_run {
                let report = serde_json::json!({
                    "import_legacy_dry_run": true,
                    "candidate_paths":       roots
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>(),
                    "artifacts_found":       artifacts.len(),
                    "mesh_artifacts":        mesh_artifacts.len(),
                    "would_import_records":  candidate_node_names.len(),
                    "would_insert_nodes":    &candidate_node_names,
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                let mut conn = mackesd_core::store::open(&db_path)
                    .with_context(|| format!("opening store at {}", db_path.display()))?;
                let existing: std::collections::BTreeSet<String> =
                    mackesd_core::store::list_nodes(&conn)?
                        .into_iter()
                        .map(|n| n.node_id)
                        .collect();
                let mut inserted = Vec::new();
                let mut skipped = Vec::new();
                for name in &candidate_node_names {
                    let node_id = format!("peer:{name}");
                    if existing.contains(&node_id) {
                        skipped.push(node_id);
                        continue;
                    }
                    mackesd_core::store::upsert_node(
                        &conn,
                        &node_id,
                        name,
                        // Placeholder key — a subsequent enrollment
                        // will replace this with the real Ed25519
                        // public-key fingerprint.
                        "legacy-import",
                        None,
                    )?;
                    inserted.push(node_id);
                }
                let payload = serde_json::json!({
                    "event":    "import_legacy",
                    "inserted": &inserted,
                    "skipped":  &skipped,
                })
                .to_string();
                mackesd_core::store::insert_event(
                    &mut conn,
                    "lifecycle",
                    &default_node_id(),
                    &payload,
                )?;
                let report = serde_json::json!({
                    "import_legacy_dry_run": false,
                    "artifacts_found":       artifacts.len(),
                    "mesh_artifacts":        mesh_artifacts.len(),
                    "inserted_nodes":        inserted,
                    "skipped_nodes":         skipped,
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        }
        Cmd::Reconcile {
            once,
            workgroup_root,
            node_id,
        } => {
            // Phase 12.5 wiring — the reconcile worker thread.
            let workgroup_root =
                workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let node_id = node_id.unwrap_or_else(default_node_id);

            if once {
                // Single-tick dry-run path: useful for CI smoke
                // tests + operator inspection. No background
                // thread, no signal handler.
                let outcome = mackesd_core::worker::tick(&workgroup_root, &node_id, &db_path)
                    .with_context(|| format!("one-shot reconcile tick on {}", db_path.display()))?;
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                // Long-running path: spawn the worker, install a
                // SIGTERM/SIGINT handler that flips the shutdown
                // flag, then block until the worker exits.
                use std::sync::atomic::{AtomicBool, Ordering};
                use std::sync::Arc;
                let shutdown = Arc::new(AtomicBool::new(false));
                install_signal_handlers(Arc::clone(&shutdown))?;
                let handle = mackesd_core::worker::spawn_reconcile_worker(
                    workgroup_root,
                    node_id,
                    db_path,
                    Arc::clone(&shutdown),
                );
                // Wait for either the worker to exit (DB went away,
                // panic — we don't panic by design) or the signal
                // handler to flip shutdown. JoinHandle::join blocks
                // until the thread returns either way.
                if let Err(e) = handle.join() {
                    eprintln!("mackesd reconcile: worker thread panicked: {e:?}");
                    std::process::exit(1);
                }
                // If we exited because the worker thread itself
                // crashed unexpectedly (e.g. someone moved the db
                // file out from under us), the loop logged the
                // error before returning. Either way: exit 0 on a
                // clean shutdown-flag path.
                if !shutdown.load(Ordering::Relaxed) {
                    // Worker exited but no shutdown was requested.
                    // Treat as a soft failure.
                    eprintln!("mackesd reconcile: worker exited without shutdown request");
                    std::process::exit(1);
                }
            }
        }
        Cmd::InventoryLegacy { mesh_only, json } => {
            // Phase 12.13.1 — read-only walk of the three legacy
            // roots. Operator runs this before `import-legacy` to
            // see what's on disk.
            let roots = mackesd_core::legacy_inventory::default_roots();
            let mut artifacts = mackesd_core::legacy_inventory::inventory(&roots);
            if mesh_only {
                artifacts.retain(|a| a.mesh_data);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&artifacts)?);
            } else {
                print_inventory_table(&artifacts);
            }
        }
        #[cfg(feature = "async-services")]
        Cmd::Serve {
            workgroup_root,
            node_id,
        } => {
            // v2.0.0 Phase B.12 — unified meta-daemon entry point.
            // Boots the tokio runtime, registers the worker pool +
            // the existing reconcile worker, blocks on SIGTERM.
            run_serve(workgroup_root, node_id, db_path)?;
        }
        Cmd::Ca { sub } => {
            // NF-2.6 (v2.5) — mackesd ca {mint, rotate, list,
            // dump-ca} subcommands. Operator surface backing the
            // CA module.
            let mut conn = mackesd_core::store::open(&db_path)?;
            let default_mesh = format!("mesh-{}", default_node_id());
            match sub {
                CaCmd::Mint { mesh_id } => {
                    let mesh = mesh_id.unwrap_or(default_mesh);
                    match mackesd_core::ca::mint::mint_ca(
                        &mackesd_core::ca::SubprocessBackend,
                        &conn,
                        &mesh,
                        None,
                        None,
                    ) {
                        Ok(mackesd_core::ca::mint::MintOutcome::Created { .. }) => {
                            println!("CA minted at epoch 0 for mesh '{mesh}'.");
                        }
                        Ok(mackesd_core::ca::mint::MintOutcome::AlreadyMinted {
                            epoch, ..
                        }) => {
                            println!(
                                "CA for mesh '{mesh}' already exists at epoch {epoch} (no-op)."
                            );
                        }
                        Err(mackesd_core::ca::CaError::BinaryMissing) => {
                            return Err(anyhow::anyhow!(
                                "nebula-cert not on PATH. Install the Fedora `nebula` package + retry."
                            ));
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("mint: {e}"));
                        }
                    }
                }
                CaCmd::SetPassphrase => {
                    let root = mackesd_core::default_qnm_shared_root();
                    let new = std::env::var("MDE_CA_PASSPHRASE").map_err(|_| {
                        anyhow::anyhow!("set-passphrase: export MDE_CA_PASSPHRASE first")
                    })?;
                    if new.len() < 8 {
                        anyhow::bail!("set-passphrase: at least 8 characters (SEC-2)");
                    }
                    use mackesd_core::ca::rotation_gate::{verify, GateCheck};
                    if verify(&root, "") != GateCheck::NotSet {
                        let current =
                            std::env::var("MDE_CA_PASSPHRASE_CURRENT").unwrap_or_default();
                        if verify(&root, &current) != GateCheck::Ok {
                            anyhow::bail!(
                                "set-passphrase: a gate exists — export the current phrase \
                                 in MDE_CA_PASSPHRASE_CURRENT to change it"
                            );
                        }
                    }
                    mackesd_core::ca::rotation_gate::set_passphrase(&root, &new)?;
                    println!("CA-rotation passphrase set (SEC-2 gate armed).");
                    return Ok(());
                }
                CaCmd::Rotate {
                    mesh_id,
                    passphrase_stdin,
                } => {
                    // SEC-2 — the gate, before any rotation work.
                    let root = mackesd_core::default_qnm_shared_root();
                    let phrase = if passphrase_stdin {
                        let mut line = String::new();
                        std::io::stdin().read_line(&mut line)?;
                        line.trim_end_matches('\n').to_string()
                    } else {
                        std::env::var("MDE_CA_PASSPHRASE").unwrap_or_default()
                    };
                    let check = mackesd_core::ca::rotation_gate::verify(&root, &phrase);
                    if let Some(msg) = mackesd_core::ca::rotation_gate::refusal_message(check) {
                        anyhow::bail!("{msg}");
                    }
                    let mesh = mesh_id.unwrap_or(default_mesh);
                    match mackesd_core::ca::epoch::bump_epoch(
                        &mackesd_core::ca::SubprocessBackend,
                        &mut conn,
                        &mesh,
                        None,
                        None,
                    ) {
                        Ok(o) => {
                            println!(
                                "CA rotated for mesh '{mesh}': epoch {} → {} ({} peer certs re-signed).",
                                o.retired_epoch
                                    .map(|e| e.to_string())
                                    .unwrap_or_else(|| "none".into()),
                                o.new_epoch,
                                o.re_signed,
                            );
                        }
                        Err(mackesd_core::ca::CaError::BinaryMissing) => {
                            return Err(anyhow::anyhow!(
                                "nebula-cert not on PATH. Install the Fedora `nebula` package + retry."
                            ));
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("rotate: {e}"));
                        }
                    }
                }
                CaCmd::List => {
                    let mut stmt = conn.prepare(
                        "SELECT mesh_id, epoch, created_at, retired_at \
                         FROM nebula_ca ORDER BY mesh_id, epoch DESC",
                    )?;
                    let rows = stmt.query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, Option<i64>>(3)?,
                        ))
                    })?;
                    println!(
                        "{:<24} {:>6} {:>12} {:>12}",
                        "MESH_ID", "EPOCH", "CREATED", "RETIRED"
                    );
                    let mut count = 0;
                    for row in rows {
                        let (mesh, epoch, created, retired) = row?;
                        let retired_disp = match retired {
                            Some(t) => t.to_string(),
                            None => "active".to_string(),
                        };
                        println!("{mesh:<24} {epoch:>6} {created:>12} {retired_disp:>12}",);
                        count += 1;
                    }
                    if count == 0 {
                        println!("(no CAs minted yet — run `mackesd ca mint`)");
                    }
                }
                CaCmd::DumpCa { mesh_id } => {
                    let mesh = mesh_id.unwrap_or(default_mesh);
                    match mackesd_core::ca::mint::current_ca(&conn, &mesh) {
                        Ok(Some((_epoch, pem))) => {
                            print!("{pem}");
                        }
                        Ok(None) => {
                            return Err(anyhow::anyhow!("no active CA for mesh '{mesh}'"));
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("dump-ca: {e}"));
                        }
                    }
                }
                CaCmd::Export {
                    mesh_id,
                    passphrase_stdin,
                    output,
                    ca_key,
                } => {
                    // NF-18.1 — encrypted CA backup. EFF-21: prefer
                    // --passphrase-stdin (env is environ-visible +
                    // child-inherited); env stays the fallback.
                    let mesh = mesh_id.unwrap_or(default_mesh);
                    let passphrase = if passphrase_stdin {
                        read_secret_line("export")?
                    } else {
                        std::env::var("MDE_BACKUP_PASSPHRASE").map_err(|_| {
                            anyhow::anyhow!(
                                "export: pass --passphrase-stdin (preferred) or set \
                                 MDE_BACKUP_PASSPHRASE before invoking"
                            )
                        })?
                    };
                    let key_path = ca_key.unwrap_or_else(|| {
                        mackesd_core::nebula_enroll::SignCsrPaths::production_defaults().ca_key
                    });
                    let ca_key_pem =
                        mackesd_core::ca::seal::read_sealed(&key_path).map_err(|e| {
                            anyhow::anyhow!("export: read CA key {}: {e}", key_path.display(),)
                        })?;
                    let ca_key_pem_str = String::from_utf8(ca_key_pem)
                        .map_err(|e| anyhow::anyhow!("export: CA key not UTF-8: {e}"))?;
                    let plaintext = mackesd_core::ca::backup::assemble_from_store(
                        &conn,
                        &mesh,
                        &ca_key_pem_str,
                    )
                    .map_err(|e| anyhow::anyhow!("export: assemble: {e}"))?;
                    let sealed = mackesd_core::ca::backup::seal(&passphrase, &plaintext)
                        .map_err(|e| anyhow::anyhow!("export: seal: {e}"))?;
                    let armored = mackesd_core::ca::backup::armor(&sealed, plaintext.exported_at);
                    match output {
                        Some(path) => {
                            std::fs::write(&path, &armored)
                                .with_context(|| format!("write {}", path.display()))?;
                            eprintln!(
                                "exported {} CA rows + {} peer certs → {} ({} bytes armored)",
                                plaintext.ca_certs.len(),
                                plaintext.peer_certs.len(),
                                path.display(),
                                armored.len(),
                            );
                        }
                        None => {
                            print!("{armored}");
                        }
                    }
                }
                CaCmd::Import {
                    input,
                    passphrase_stdin,
                } => {
                    // NF-18.1 — encrypted CA bundle restore. EFF-21:
                    // --passphrase-stdin preferred (requires --input,
                    // since the default bundle source is stdin).
                    let passphrase = if passphrase_stdin {
                        read_secret_line("import")?
                    } else {
                        std::env::var("MDE_BACKUP_PASSPHRASE").map_err(|_| {
                            anyhow::anyhow!(
                                "import: pass --passphrase-stdin with --input \
                                 (preferred) or set MDE_BACKUP_PASSPHRASE"
                            )
                        })?
                    };
                    let armored = match input {
                        Some(path) => std::fs::read_to_string(&path)
                            .with_context(|| format!("read {}", path.display()))?,
                        None => {
                            use std::io::Read;
                            let mut s = String::new();
                            std::io::stdin().read_to_string(&mut s)?;
                            s
                        }
                    };
                    let sealed = mackesd_core::ca::backup::dearmor(&armored)
                        .map_err(|e| anyhow::anyhow!("import: dearmor: {e}"))?;
                    let plaintext = mackesd_core::ca::backup::unseal(&passphrase, &sealed)
                        .map_err(|e| anyhow::anyhow!("import: {e}"))?;
                    mackesd_core::ca::backup::restore_to_store(&conn, &plaintext)
                        .map_err(|e| anyhow::anyhow!("import: restore: {e}"))?;
                    eprintln!(
                        "imported {} CA rows + {} peer certs for mesh '{}' \
                         (exported_at = unix:{}); restart mackesd to pick up \
                         the new CA + the operator should re-write \
                         /etc/nebula/{{ca.crt,ca.key}} from the bundle.",
                        plaintext.ca_certs.len(),
                        plaintext.peer_certs.len(),
                        plaintext.mesh_id,
                        plaintext.exported_at,
                    );
                }
                CaCmd::SignCsr {
                    node_id,
                    workgroup_root,
                    mesh_id,
                    ca_crt,
                    ca_key,
                    scratch_dir,
                    lighthouse_addr,
                    override_cap,
                } => {
                    // NF-3.6.b — sign the peer's pending-enroll
                    // CSR + write the bundle back to QNM-Shared.
                    let workgroup_root =
                        workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                    let mesh = mesh_id.unwrap_or(default_mesh);
                    let mut paths =
                        mackesd_core::nebula_enroll::SignCsrPaths::production_defaults();
                    if let Some(p) = ca_crt {
                        paths.ca_crt = p;
                    }
                    if let Some(p) = ca_key {
                        paths.ca_key = p;
                    }
                    if let Some(p) = scratch_dir {
                        paths.scratch_dir = p;
                    }
                    // Bug #6: the joining peer must dial the lighthouse's
                    // REAL external address. Resolution order:
                    //   1. an explicit `--lighthouse-addr` override, else
                    //   2. inherit the lighthouse's own roster (the real
                    //      overlay_ip + external_addr mesh-init recorded)
                    //      from its own bundle, else
                    //   3. last-resort hostname guess (NOT DNS-resolvable
                    //      for the peer — the old default that broke joins).
                    let local_id = default_node_id();
                    let lighthouses = if let Some(addr) = lighthouse_addr {
                        vec![mackesd_core::ca::bundle::LighthouseEntry {
                            node_id: local_id.clone(),
                            overlay_ip: "10.42.0.1".to_string(),
                            external_addr: addr,
                        }]
                    } else {
                        let self_bundle = mackesd_core::ca::bundle::read_bundle(
                            &mackesd_core::ca::bundle::bundle_path(&workgroup_root, &local_id),
                        );
                        match self_bundle {
                            Ok(b) if !b.lighthouses.is_empty() => b.lighthouses,
                            _ => {
                                let host = std::fs::read_to_string("/etc/hostname")
                                    .ok()
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or_else(default_node_id);
                                eprintln!(
                                    "mackesd ca sign-csr: no lighthouse bundle at {} — \
                                     falling back to hostname '{host}:4242', which the peer \
                                     may not resolve. Pass --lighthouse-addr <public-ip>:4242.",
                                    mackesd_core::ca::bundle::bundle_path(
                                        &workgroup_root,
                                        &local_id
                                    )
                                    .display()
                                );
                                vec![mackesd_core::ca::bundle::LighthouseEntry {
                                    node_id: local_id.clone(),
                                    overlay_ip: "10.42.0.1".to_string(),
                                    external_addr: format!("{host}:4242"),
                                }]
                            }
                        }
                    };
                    match mackesd_core::nebula_enroll::sign_pending_csr(
                        &mackesd_core::ca::SubprocessBackend,
                        &conn,
                        &workgroup_root,
                        &node_id,
                        &mesh,
                        &paths,
                        lighthouses,
                        override_cap,
                    ) {
                        Ok(outcome) => {
                            if override_cap {
                                eprintln!(
                                    "TUNE-11 OVERRIDE ENGAGED: signed {} past the {}-peer cap. \
                                     Audit-log entry written to the journal under \
                                     `mackesd::cap_override`. Document the exception in \
                                     docs/design/cap-overrides.md.",
                                    outcome.peer_id,
                                    mackesd_core::ca::sign::MAX_PEER_CAP,
                                );
                            }
                            println!(
                                "signed {} into mesh '{}' at epoch {} (overlay {}); bundle at {}.",
                                outcome.peer_id,
                                mesh,
                                outcome.epoch,
                                outcome.overlay_ip,
                                outcome.bundle_path.display(),
                            );
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("sign-csr: {e}"));
                        }
                    }
                }
                CaCmd::Revoke {
                    node_id,
                    workgroup_root,
                    self_node_id,
                } => {
                    // INST-7 prerequisite — revoke a peer's cert +
                    // ban the identity. CLI surface replaces the
                    // originally-planned D-Bus method (D-Bus retires
                    // by 1.0 per AI_GOVERNANCE §3.3).
                    let workgroup_root =
                        workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                    let self_id = self_node_id.unwrap_or_else(default_node_id);
                    let rows = mackesd_core::ca::revoke::revoke_peer(
                        &conn,
                        &workgroup_root,
                        &self_id,
                        &node_id,
                    )
                    .context("ca revoke")?;
                    println!(
                        "revoked '{node_id}': {rows} cert row(s) marked revoked; \
                         added to ban list at {self_id}'s QNM-Shared entry."
                    );
                }
                CaCmd::Ban {
                    node_id,
                    workgroup_root,
                } => {
                    // EPIC-SEC-BANLIST (Q53) — add node-id to this
                    // peer's ban list. GFS replication propagates it.
                    let workgroup_root =
                        workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                    let self_id = default_node_id();
                    match mackesd_core::ca::ban_list::add_banned(
                        &workgroup_root,
                        &self_id,
                        &node_id,
                    ) {
                        Ok(true) => println!(
                            "banned '{node_id}' (recorded in {}'s ban list; \
                             propagates to every peer via mesh-storage).",
                            self_id
                        ),
                        Ok(false) => println!("'{node_id}' was already banned (no-op)."),
                        Err(e) => return Err(anyhow::anyhow!("ca ban: {e}")),
                    }
                }
                CaCmd::Unban {
                    node_id,
                    workgroup_root,
                } => {
                    // EPIC-SEC-BANLIST (Q53) — lift a ban THIS peer
                    // set. Bans set on other peers must be lifted
                    // there (the gate enforces the union).
                    let workgroup_root =
                        workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                    let self_id = default_node_id();
                    match mackesd_core::ca::ban_list::remove_banned(
                        &workgroup_root,
                        &self_id,
                        &node_id,
                    ) {
                        Ok(true) => println!("unbanned '{node_id}' from {self_id}'s ban list."),
                        Ok(false) => {
                            // Still surface the union state so the
                            // operator knows if another peer banned it.
                            if mackesd_core::ca::ban_list::is_banned(&workgroup_root, &node_id) {
                                println!(
                                    "'{node_id}' isn't in {self_id}'s ban list, but ANOTHER \
                                     peer still bans it — unban it on that peer too."
                                );
                            } else {
                                println!("'{node_id}' isn't banned (no-op).");
                            }
                        }
                        Err(e) => return Err(anyhow::anyhow!("ca unban: {e}")),
                    }
                }
                CaCmd::BanList { workgroup_root } => {
                    // EPIC-SEC-BANLIST (Q53) — print the enforced
                    // union across every peer's ban list.
                    let workgroup_root =
                        workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
                    let union = mackesd_core::ca::ban_list::load_union(&workgroup_root);
                    if union.is_empty() {
                        println!("ban list empty (no node-ids banned across the mesh).");
                    } else {
                        println!("Banned node-ids (mesh-wide union, {} total):", union.len());
                        for id in &union {
                            println!("  {id}");
                        }
                    }
                }
            }
        }
        Cmd::Nebula { sub } => {
            // NF-18.x — mackesd nebula <sub> operator surface.
            let conn = mackesd_core::store::open(&db_path)?;
            match sub {
                NebulaCmd::ExportRoster => {
                    // NF-18.2 — JSON array of (node_id, name,
                    // overlay_ip, cert_pem, epoch, created_at,
                    // expires_at, groups). `groups` is sourced
                    // from nodes.role since the Nebula cert
                    // groups are encoded in the cert PEM body
                    // and we want a flat queryable shape.
                    let rows = mackesd_core::nebula_roster::export_roster(&conn)
                        .map_err(|e| anyhow::anyhow!("export-roster: {e}"))?;
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                }
            }
        }
        Cmd::Voice { sub } => {
            // VV-1 / VV-1.5 / VV-2 (v4.1.0) — voice stack operator
            // surface. `render-config` is invoked by both
            // `kamailio-mde.service` and `rtpengine-mde.service` as
            // their ExecStartPre hook; the voice_config worker
            // writes the JSON input file when policy changes and
            // triggers `systemctl reload` to re-run this command.
            match sub {
                VoiceCmd::RenderConfig {
                    kamailio_dir,
                    rtpengine_dir,
                    desired_json,
                    boot_default,
                    dry_run,
                } => {
                    let desired =
                        load_voice_desired(&desired_json, boot_default, &default_node_id())?;
                    let set = mde_voice_config::generate(&desired);
                    let kamailio_files = [
                        ("kamailio.cfg", &set.kamailio_cfg),
                        ("dispatcher.list", &set.dispatcher_list),
                        ("uacreg.list", &set.uacreg_list),
                    ];
                    let rtpengine_files = [("rtpengine.conf", &set.rtpengine_conf)];
                    if dry_run {
                        for (name, body) in kamailio_files {
                            println!(
                                "# ---- {} (would write under {}) ----",
                                name,
                                kamailio_dir.display()
                            );
                            print!("{body}");
                        }
                        for (name, body) in rtpengine_files {
                            println!(
                                "# ---- {} (would write under {}) ----",
                                name,
                                rtpengine_dir.display()
                            );
                            print!("{body}");
                        }
                    } else {
                        write_voice_config_files(&kamailio_dir, &kamailio_files)?;
                        write_voice_config_files(&rtpengine_dir, &rtpengine_files)?;
                        println!(
                            "voice render-config: wrote {} files under {} + {} under {}",
                            kamailio_files.len(),
                            kamailio_dir.display(),
                            rtpengine_files.len(),
                            rtpengine_dir.display(),
                        );
                    }
                }
            }
        }
        Cmd::WakePeer {
            mac,
            broadcast,
            via_lighthouse,
            port,
        } => {
            // DEAD-2.5 + NF-21.2 — wire mackesd_core::workers::wol so
            // the Rust port has a runtime entry point. Replaces the
            // retired Python `mesh_wol.wake_peer` for the MAC-already-
            // known case; hostname resolution is the operator's job
            // until a PeerStore lookup helper lands. `--via-lighthouse`
            // routes through a lighthouse's overlay IP for WoL-across-
            // LANs (NF-21.2).
            let Some(mac_bytes) = mackesd_core::workers::wol::normalize_mac(&mac) else {
                anyhow::bail!("wake-peer: could not parse MAC {mac:?}");
            };
            if let Some(lighthouse_ip) = via_lighthouse.as_deref() {
                mackesd_core::workers::wol::wake_via_lighthouse(mac_bytes, lighthouse_ip, port)
                    .context("wake-peer: send magic packet via lighthouse")?;
                println!(
                    "wake-peer: sent magic packet for {mac} via lighthouse \
                     {lighthouse_ip}:{port}"
                );
            } else {
                mackesd_core::workers::wol::wake(mac_bytes, &broadcast, port)
                    .context("wake-peer: send magic packet")?;
                println!("wake-peer: sent magic packet to {mac} via {broadcast}:{port}");
            }
        }
        // AUD3 S-3 (2026-06-12): `Cmd::PeerCard` arm removed with the
        // peer_join module (targeted the deleted mde-peer-card modal).
        #[cfg(feature = "async-services")]
        Cmd::FleetPushSetting {
            key,
            value,
            peers,
            author,
            dry_run,
        } => {
            // v2.0.0 Phase G.4 — fleet push-setting CLI. Writes the
            // matching desired_config row + fleet_settings_apply_log
            // entries, then prints the JSON plan.
            let mut conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let author = author.unwrap_or_else(default_node_id);
            let plan = mackesd_core::fleet::plan_push(&key, &value, &peers, &author);
            if !dry_run {
                mackesd_core::fleet::record_push(&mut conn, &plan)
                    .context("recording fleet push")?;
            }
            let report = serde_json::json!({
                "fleet_push_setting": {
                    "key":          &plan.key,
                    "value":        &plan.value,
                    "peers":        &plan.peers,
                    "author":       &plan.author,
                    "revision_id":  &plan.revision_id,
                    "dry_run":      dry_run,
                }
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Cmd::Revisions { cmd } => {
            // v2.0.0 Phase F.12 — desired_config revision management.
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            match cmd {
                RevisionsCmd::List { json } => {
                    let rows = list_revisions(&conn)?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&rows)?);
                    } else {
                        print_revisions_table(&rows);
                    }
                }
                RevisionsCmd::Diff { from, to } => {
                    let a = load_revision_payload(&conn, &from)?;
                    let b = load_revision_payload(&conn, &to)?;
                    let report = serde_json::json!({
                        "from":     from,
                        "to":       to,
                        "from_len": a.len(),
                        "to_len":   b.len(),
                        // Surface the raw payloads so the operator + the
                        // Workbench panel can diff them visually.
                        "from_payload": a,
                        "to_payload":   b,
                    });
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
                RevisionsCmd::Rollback {
                    target_id,
                    author,
                    peers,
                } => {
                    let payload = load_revision_payload(&conn, &target_id)?;
                    let author = author.unwrap_or_else(default_node_id);
                    let summary = format!("Rollback to {target_id} (peers={peers})");
                    let mut conn = conn;
                    let now = chrono::Utc::now().to_rfc3339();
                    let revision_id = mackesd_core::store::with_transaction(&mut conn, |tx| {
                        tx.execute(
                            "INSERT INTO desired_config \
                                 (author, message, spec_json, state, created_at) \
                                 VALUES (?, ?, ?, 'approved', ?)",
                            (&author, &summary, &payload, &now),
                        )
                        .map_err(|e| anyhow::anyhow!("inserting rollback revision: {e}"))?;
                        Ok(tx.last_insert_rowid())
                    })?;
                    let report = serde_json::json!({
                        "rollback":      target_id,
                        "new_revision":  revision_id,
                        "author":        author,
                        "peers":         peers,
                    });
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
            }
        }
        Cmd::Leave { yes } => {
            if !yes {
                anyhow::bail!(
                    "leave wipes this box's mesh state (cert, keys, role). \
                     Re-run with --yes to confirm."
                );
            }
            let root = mackesd_core::default_qnm_shared_root();
            let hostname = std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let node_id = format!("peer:{hostname}");
            let report = mackesd_core::leave::leave(
                &root,
                &hostname,
                &node_id,
                std::path::Path::new("/etc/nebula"),
                std::path::Path::new("/var/lib/mde/role.toml"),
            );
            let _ = std::process::Command::new("systemctl")
                .args(["stop", "nebula.service"])
                .status();
            println!("left the mesh: {report:#?}");
            println!("re-join later with: mackesd enroll --token <fresh token from a lighthouse>");
            return Ok(());
        }
        Cmd::MeshInit {
            mesh_id,
            external_addr,
            role,
        } => {
            let parsed: mde_role::Role = role.parse().map_err(|_| {
                anyhow::anyhow!("unknown role `{role}` — expected lighthouse|server|workstation")
            })?;
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            mackesd_core::store::migrate(&conn).context("migrating store")?;
            let root = mackesd_core::default_qnm_shared_root();
            // Bed fix #10: use the SAME node-id resolution `serve` uses
            // (MACKESD_NODE_ID → HOSTNAME → `hostname` → peer:unknown). The
            // old code here shelled ONLY `hostname` (falling back to
            // "founder") and ignored MACKESD_NODE_ID + the HOSTNAME env — so
            // on a box where those disagree (a container with no `hostname`
            // binary, or an operator-set MACKESD_NODE_ID), mesh-init wrote the
            // founding bundle under one id while the next `serve`'s
            // nebula-supervisor looked under a DIFFERENT id, never found it,
            // and the founding lighthouse's overlay never came up. Caught by
            // the OBS-1 container E2E.
            let node_id = default_node_id();
            let report = mackesd_core::mesh_init::mesh_init(
                &mackesd_core::ca::SubprocessBackend,
                &conn,
                &root,
                &node_id,
                &mesh_id,
                &external_addr,
                std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.crt"),
                std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.key"),
                std::path::Path::new("/var/lib/mackesd/nebula-ca/scratch"),
                parsed,
            )?;
            // Best-effort unit starts — the supervisor (next serve)
            // also materializes + starts; containerized test envs
            // without systemd still get a complete on-disk state.
            let _ = std::process::Command::new("systemctl")
                .args(["start", "nebula.service"])
                .status();
            println!(
                "mesh `{}` initialized — lighthouse {} ({})",
                report.mesh_id, node_id, report.overlay_ip
            );
            if let Some(r) = &report.pinned_role {
                println!("role pinned: {r}");
            }
            println!("bundle: {}", report.bundle_path.display());
            println!(
                "\nfirst peer joins with:\n  mackesd enroll --token '{}'",
                report.join_token
            );
            return Ok(());
        }
        Cmd::EnrollToken {
            mesh_id,
            lighthouse,
            note,
        } => {
            let root = mackesd_core::default_qnm_shared_root();
            let bearer = mackesd_core::bearer_ledger::issue(&root, &note)
                .map_err(|e| anyhow::anyhow!("minting bearer: {e}"))?;
            let lh = lighthouse.unwrap_or_else(|| {
                let ip = std::fs::read_to_string("/var/lib/mackesd/nebula/overlay-ip")
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "<lighthouse-ip>".to_string());
                format!("{ip}:4242")
            });
            println!("mesh:{mesh_id}@{lh}#{bearer}");
            eprintln!(
                "single-use token minted (ENT-1) — run on the joining box:\n  mackesd enroll --token 'mesh:{mesh_id}@{lh}#{bearer}'"
            );
            return Ok(());
        }
        Cmd::Found {
            mesh_id,
            external_addr,
            role,
            enroll_port,
        } => {
            return cmd_found(&db_path, &mesh_id, &external_addr, &role, enroll_port);
        }
        Cmd::Join {
            token,
            role,
            name,
            workgroup_root,
        } => {
            return cmd_join(token, &role, name, workgroup_root);
        }
        Cmd::Peers { json } => {
            // PD-1 — the joined directory, CLI face.
            let root = mackesd_core::default_qnm_shared_root();
            let svc =
                mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let dir = svc.build_directory(now);
            if json {
                println!("{dir}");
            } else {
                let head = dir["head"]
                    .as_u64()
                    .map_or("-".to_string(), |v| v.to_string());
                println!("fleet head: {head}");
                println!(
                    "{:<16} {:<8} {:<10} {:<12} {:<15} {:<8}",
                    "PEER", "PRESENCE", "HEALTH", "VERSION", "OVERLAY IP", "REVISION"
                );
                for p in dir["peers"].as_array().into_iter().flatten() {
                    println!(
                        "{:<16} {:<8} {:<10} {:<12} {:<15} {:<8}",
                        p["hostname"].as_str().unwrap_or("-"),
                        p["presence"].as_str().unwrap_or("-"),
                        p["health"].as_str().unwrap_or("-"),
                        p["mde_version"].as_str().unwrap_or("-"),
                        p["overlay_ip"].as_str().unwrap_or("-"),
                        p["revision"]["currency"].as_str().unwrap_or("-"),
                    );
                }
            }
            return Ok(());
        }
        Cmd::Remediate { cmd } => {
            // PLANES-11 — the remediation layer. Wires PLANES-13's
            // policy engine (which had no caller) to the job system:
            // evaluate policies → match plans → fire signed bundles.
            use mackesd_core::{policy_engine, remediation};
            let root = mackesd_core::default_qnm_shared_root();
            match cmd {
                RemediateCmd::Plans { json } => {
                    let plans = remediation::load_plans(&root);
                    if json {
                        println!("{}", serde_json::to_string(&plans)?);
                    } else {
                        println!(
                            "{:<22} {:<20} {:<22} {:<5}",
                            "PLAN", "POLICY", "TEMPLATE", "AUTO"
                        );
                        for p in &plans {
                            println!(
                                "{:<22} {:<20} {:<22} {:<5}",
                                p.name, p.policy, p.template, p.auto
                            );
                        }
                    }
                    return Ok(());
                }
                RemediateCmd::Match { json } => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as u64);
                    let svc = mackesd_core::ipc::directory::DirectoryService::new(
                        &root,
                        Some(db_path.clone()),
                    );
                    let dir = svc.build_directory(now);
                    let peers: Vec<(String, serde_json::Value)> = dir["peers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p["hostname"].as_str().map(|h| (h.to_string(), p.clone())))
                        .collect();
                    let policies = policy_engine::load_policies(&root);
                    let violations = policy_engine::evaluate(&policies, &peers);
                    let plans = remediation::load_plans(&root);
                    let matched = remediation::match_all(&plans, &violations);
                    if json {
                        println!("{}", serde_json::to_string(&matched)?);
                    } else if matched.is_empty() {
                        println!("no drift — every policy holds across {} peers", peers.len());
                    } else {
                        println!(
                            "{:<14} {:<20} {:<8} {:<22} {:<5}",
                            "PEER", "POLICY", "SEV", "PLAN", "AUTO"
                        );
                        for m in &matched {
                            println!(
                                "{:<14} {:<20} {:<8} {:<22} {:<5}",
                                m.violation.peer,
                                m.violation.policy,
                                m.violation.severity,
                                m.plan.as_deref().unwrap_or("(none)"),
                                m.auto
                            );
                        }
                    }
                    return Ok(());
                }
                RemediateCmd::Fire { plan, peer } => {
                    let plans = remediation::load_plans(&root);
                    let Some(p) = plans.iter().find(|x| x.name == plan) else {
                        anyhow::bail!("no remediation plan named '{plan}' (mded remediate plans)");
                    };
                    // Bind the event vars from a synthesized violation
                    // for this (policy, peer) — the operator-fire path.
                    let v = policy_engine::Violation {
                        policy: p.policy.clone(),
                        peer: peer.clone(),
                        severity: "warn".into(),
                        detail: format!("operator fire of '{plan}'"),
                    };
                    let vars = remediation::bind_vars(p, &v);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as u64);
                    let run_id = format!("rem-{now}");
                    let body = serde_json::json!({
                        "playbook": p.template,
                        "targets": { "peers": [peer] },
                        "vars": vars,
                    });
                    let jobs_svc =
                        mackesd_core::ipc::jobs::JobsService::new(&root, Some(db_path.clone()));
                    let reply = mackesd_core::ipc::jobs::build_reply(
                        &jobs_svc,
                        "launch",
                        Some(&body.to_string()),
                        &run_id,
                    );
                    // Loud (W42): the launch reply — run id + resolved
                    // targets — prints for the operator / audit trail.
                    println!("{reply}");
                    return Ok(());
                }
            }
        }
        Cmd::Policy { cmd } => {
            // PLANES-13 — the policy engine surface. Evaluates the loaded
            // policies (core pack + on-disk TOML) against the live
            // directory and reports per-policy compliance.
            use mackesd_core::policy_engine;
            let PolicyCmd::List { json } = cmd;
            let root = mackesd_core::default_qnm_shared_root();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let svc =
                mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
            let dir = svc.build_directory(now);
            let peers: Vec<(String, serde_json::Value)> = dir["peers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["hostname"].as_str().map(|h| (h.to_string(), p.clone())))
                .collect();
            let policies = policy_engine::load_policies(&root);
            // For each policy, the peers that currently violate it.
            let rows: Vec<serde_json::Value> = policies
                .iter()
                .map(|pol| {
                    let violated: Vec<&str> = peers
                        .iter()
                        .filter(|(_, rec)| !pol.holds(rec))
                        .map(|(h, _)| h.as_str())
                        .collect();
                    serde_json::json!({
                        "name": pol.name,
                        "description": pol.description,
                        "field": pol.field,
                        "op": pol.op,
                        "expected": pol.expected,
                        "severity": pol.severity,
                        "violated_peers": violated,
                    })
                })
                .collect();
            if json {
                println!("{}", serde_json::to_string(&rows)?);
            } else {
                println!(
                    "{:<22} {:<8} {:<24} {:<8}",
                    "POLICY", "SEVERITY", "ASSERTION", "STATUS"
                );
                for (pol, row) in policies.iter().zip(&rows) {
                    let n = row["violated_peers"].as_array().map_or(0, Vec::len);
                    let status = if n == 0 {
                        "ok".to_string()
                    } else {
                        format!("{n} violating")
                    };
                    println!(
                        "{:<22} {:<8} {:<24} {:<8}",
                        pol.name,
                        pol.severity.as_str(),
                        format!("{} {:?} {}", pol.field, pol.op, pol.expected),
                        status
                    );
                }
            }
            return Ok(());
        }
        Cmd::Netstate { cmd } => {
            // PLANES-15 — desired (elected revision) vs actual (live
            // nmstate) interface diff (W68).
            use magic_fleet::netstate::{IpConfig, NetInterface, NetOps, SystemNetOps};
            let NetstateCmd::Diff { json } = cmd;
            let root = mackesd_core::default_qnm_shared_root();
            let desired = magic_fleet::store::elect_head(&magic_fleet::store::revisions_dir(&root))
                .map(|h| h.spec.netstate)
                .unwrap_or_default();
            let actual = SystemNetOps.read_actual();

            // Compact one-line IPv4 summary for an interface.
            fn ipv4_summary(cfg: Option<&IpConfig>) -> String {
                match cfg {
                    None => "—".to_string(),
                    Some(c) if !c.enabled => "disabled".to_string(),
                    Some(c) if c.dhcp => "dhcp".to_string(),
                    Some(c) if c.addresses.is_empty() => "no-addr".to_string(),
                    Some(c) => c
                        .addresses
                        .iter()
                        .map(magic_fleet::netstate::IpAddress::cidr)
                        .collect::<Vec<_>>()
                        .join(", "),
                }
            }
            fn find<'a>(set: &'a [NetInterface], name: &str) -> Option<&'a NetInterface> {
                set.iter().find(|i| i.name == name)
            }
            // The union of managed (desired) + observed (actual) names,
            // desired first so the managed interfaces lead.
            let mut names: Vec<String> =
                desired.interfaces.iter().map(|i| i.name.clone()).collect();
            for i in &actual.interfaces {
                if !names.contains(&i.name) {
                    names.push(i.name.clone());
                }
            }
            let rows: Vec<serde_json::Value> = names
                .iter()
                .map(|name| {
                    let d = find(&desired.interfaces, name);
                    let a = find(&actual.interfaces, name);
                    let managed = d.is_some();
                    let in_sync = match (d, a) {
                        (Some(d), Some(a)) => Some(
                            d.state == a.state
                                && ipv4_summary(d.ipv4.as_ref()) == ipv4_summary(a.ipv4.as_ref()),
                        ),
                        (Some(_), None) => Some(false), // desired but not present
                        _ => None,                      // unmanaged — informational
                    };
                    serde_json::json!({
                        "name": name,
                        "managed": managed,
                        "desired_state": d.map(|i| i.state.as_nmstate()),
                        "desired_ipv4": d.map(|i| ipv4_summary(i.ipv4.as_ref())),
                        "actual_state": a.map(|i| i.state.as_nmstate()),
                        "actual_ipv4": a.map(|i| ipv4_summary(i.ipv4.as_ref())),
                        "in_sync": in_sync,
                    })
                })
                .collect();
            if json {
                println!("{}", serde_json::to_string(&rows)?);
            } else if rows.is_empty() {
                println!("no interfaces observed");
            } else {
                println!(
                    "{:<12} {:<8} {:<18} {:<18} {:<8}",
                    "IFACE", "MANAGED", "DESIRED", "ACTUAL", "SYNC"
                );
                for r in &rows {
                    let sync = match r["in_sync"].as_bool() {
                        Some(true) => "ok",
                        Some(false) => "DRIFT",
                        None => "-",
                    };
                    println!(
                        "{:<12} {:<8} {:<18} {:<18} {:<8}",
                        r["name"].as_str().unwrap_or("-"),
                        r["managed"].as_bool().unwrap_or(false),
                        format!(
                            "{}/{}",
                            r["desired_state"].as_str().unwrap_or("-"),
                            r["desired_ipv4"].as_str().unwrap_or("-")
                        ),
                        format!(
                            "{}/{}",
                            r["actual_state"].as_str().unwrap_or("-"),
                            r["actual_ipv4"].as_str().unwrap_or("-")
                        ),
                        sync
                    );
                }
            }
            return Ok(());
        }
        #[cfg(feature = "async-services")]
        Cmd::Dns { cmd } => {
            // PLANES-18 — the flat <host>.mesh record set, built from
            // the live roster (the same records mesh_dns feeds resolved).
            use mackesd_core::workers::mesh_dns;
            let DnsCmd::List { json } = cmd;
            let root = mackesd_core::default_qnm_shared_root();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let svc =
                mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
            let dir = svc.build_directory(now);
            let peers: Vec<(String, String)> = dir["peers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    p["hostname"].as_str().map(|h| {
                        (
                            h.to_string(),
                            p["overlay_ip"].as_str().unwrap_or("").to_string(),
                        )
                    })
                })
                .collect();
            let records = mesh_dns::build_records(&peers);
            if json {
                let rows: Vec<serde_json::Value> = records
                    .iter()
                    .map(|r| serde_json::json!({ "fqdn": r.fqdn, "overlay_ip": r.overlay_ip }))
                    .collect();
                println!("{}", serde_json::to_string(&rows)?);
            } else if records.is_empty() {
                println!("no mesh DNS records (no roster peers with overlay IPs yet)");
            } else {
                println!("{:<28} {:<16}", "NAME", "OVERLAY IP");
                for r in &records {
                    println!("{:<28} {:<16}", r.fqdn, r.overlay_ip);
                }
            }
            return Ok(());
        }
        Cmd::Validate { cmd } => {
            // PLANES-19 — the overlay-reachability verdict (W79/W80).
            use magic_fleet::validation;
            let root = mackesd_core::default_qnm_shared_root();
            match cmd {
                ValidateCmd::Run => {
                    let vdir = root.join("validation");
                    std::fs::create_dir_all(&vdir)?;
                    std::fs::write(vdir.join("runnow"), b"mackesd")?;
                    println!("requested a fresh overlay-reachability run (the leader mints it)");
                    return Ok(());
                }
                ValidateCmd::Status { json } => {
                    let latest = validation::list_run_ids(&root).into_iter().next_back();
                    let Some(id) = latest else {
                        if json {
                            println!("{}", serde_json::json!({ "run_id": null }));
                        } else {
                            println!("no validation run yet (mded validate run to request one)");
                        }
                        return Ok(());
                    };
                    let Some(run) = validation::read_run(&root, &id) else {
                        anyhow::bail!("run {id} has no run.json");
                    };
                    let rows = validation::read_rows(&root, &id);
                    let verdict = validation::aggregate(&run, &rows);
                    let edge =
                        |e: &validation::Edge| serde_json::json!({ "from": e.from, "to": e.to });
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({
                                "run_id": run.run_id,
                                "kind": run.kind,
                                "at": run.at,
                                "passed": verdict.passed(),
                                "reachable": verdict.reachable.iter().map(edge).collect::<Vec<_>>(),
                                "failed": verdict.failed.iter().map(edge).collect::<Vec<_>>(),
                                "missing_reporters": verdict.missing_reporters,
                            })
                        );
                    } else {
                        println!(
                            "run {} ({:?}) — {}",
                            run.run_id,
                            run.kind,
                            if verdict.passed() { "PASS" } else { "FAIL" }
                        );
                        println!(
                            "  reachable edges: {}  failed: {}  missing reporters: {}",
                            verdict.reachable.len(),
                            verdict.failed.len(),
                            verdict.missing_reporters.len()
                        );
                        for e in &verdict.failed {
                            println!("  FAIL  {} → {}", e.from, e.to);
                        }
                    }
                    return Ok(());
                }
            }
        }
        Cmd::Tags { json } => {
            // PLANES-3/W82 — the fleet tag census: for each v1 tag, the
            // roster nodes that carry it (read from the cap-tags store).
            use mackes_mesh_types::cap_tags::{read_tags, CapabilityTag};
            let root = mackesd_core::default_qnm_shared_root();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let svc =
                mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
            let dir = svc.build_directory(now);
            let hosts: Vec<String> = dir["peers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["hostname"].as_str().map(str::to_string))
                .collect();
            let all_tags = [
                CapabilityTag::Hop,
                CapabilityTag::Execution,
                CapabilityTag::Headless,
            ];
            let rows: Vec<serde_json::Value> = all_tags
                .iter()
                .map(|tag| {
                    let carriers: Vec<&str> = hosts
                        .iter()
                        .filter(|h| read_tags(&root, h).has(*tag))
                        .map(String::as_str)
                        .collect();
                    serde_json::json!({ "tag": tag.as_str(), "nodes": carriers })
                })
                .collect();
            if json {
                println!("{}", serde_json::to_string(&rows)?);
            } else {
                println!("{:<12} {}", "TAG", "NODES");
                for r in &rows {
                    let nodes = r["nodes"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    println!(
                        "{:<12} {}",
                        r["tag"].as_str().unwrap_or("-"),
                        if nodes.is_empty() { "(none)" } else { &nodes }
                    );
                }
            }
            return Ok(());
        }
        Cmd::Profiles {
            json,
            set,
            rm,
            role,
            description,
            tags,
            ks_fragments,
            auto_join,
        } => {
            // PLANES-21 — the install-profile catalog (core pack + TOML).
            use mackesd_core::install_profiles;
            let root = mackesd_core::default_qnm_shared_root();
            // W56 write side — delete first (so --rm is unambiguous), else
            // --set writes/overwrites a validated profile TOML.
            if let Some(name) = rm {
                match install_profiles::delete_profile(&name, &root) {
                    Ok(true) => println!("removed profile '{name}'"),
                    Ok(false) => {
                        println!("no on-disk profile '{name}' (core profiles have no TOML)")
                    }
                    Err(e) => {
                        eprintln!("mackesd profiles: rm '{name}' failed: {e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            if let Some(name) = set {
                let Some(role) = role else {
                    eprintln!(
                        "mackesd profiles --set requires --role <lighthouse|server|workstation>"
                    );
                    std::process::exit(1);
                };
                let profile = install_profiles::InstallProfile {
                    name,
                    description,
                    role,
                    tags: tags.into_iter().collect(),
                    ks_fragments,
                    auto_join,
                };
                match install_profiles::write_profile(&profile, &root) {
                    Ok(p) => println!("wrote profile '{}' → {}", profile.name, p.display()),
                    Err(e) => {
                        eprintln!("mackesd profiles: set failed: {e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            let profiles = install_profiles::load_profiles(&root);
            if json {
                println!("{}", serde_json::to_string(&profiles)?);
            } else {
                println!(
                    "{:<14} {:<12} {:<22} {:<9}",
                    "PROFILE", "ROLE", "TAGS", "AUTO-JOIN"
                );
                for p in &profiles {
                    println!(
                        "{:<14} {:<12} {:<22} {:<9}",
                        p.name,
                        p.role,
                        p.tags.iter().cloned().collect::<Vec<_>>().join(","),
                        p.auto_join
                    );
                }
            }
            return Ok(());
        }
        Cmd::Mirrors {
            json,
            sync,
            sync_all,
            write_repo,
            repo_dir,
        } => {
            // PLANES-24 — the package-mirror catalog (core pack + TOML),
            // each with its file:// serving baseurl + last-sync state.
            use mackesd_core::mirrors;
            let root = mackesd_core::default_qnm_shared_root();
            let list = mirrors::load_mirrors(&root);
            // W62 — flip this node to self-serve: write each enabled mirror's
            // dnf .repo (local file:// first, upstream fallback).
            if write_repo {
                let dir =
                    repo_dir.unwrap_or_else(|| std::path::PathBuf::from(mirrors::DEFAULT_REPO_DIR));
                let mut failures = 0;
                for m in list.iter().filter(|m| m.enabled) {
                    match mirrors::write_dnf_repo(m, &root, &dir) {
                        Ok(p) => println!("wrote {} → {}", m.name, p.display()),
                        Err(e) => {
                            failures += 1;
                            eprintln!("mackesd mirrors: write .repo for {} failed: {e}", m.name);
                        }
                    }
                }
                if failures > 0 {
                    std::process::exit(1);
                }
                return Ok(());
            }
            // W63 — the one-puller sync path. `--sync <name>` / `--sync-all`
            // reposync the upstream into the LizardFS mirror dir, createrepo_c
            // the metadata, then stamp `.last-sync`.
            if sync.is_some() || sync_all {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let targets: Vec<&mirrors::Mirror> = if let Some(name) = &sync {
                    match list.iter().find(|m| &m.name == name) {
                        Some(m) => vec![m],
                        None => {
                            eprintln!("mackesd mirrors: no mirror named '{name}'");
                            std::process::exit(1);
                        }
                    }
                } else {
                    list.iter().filter(|m| m.enabled).collect()
                };
                if targets.is_empty() {
                    eprintln!("mackesd mirrors: nothing to sync (no enabled mirrors)");
                    return Ok(());
                }
                let mut failures = 0;
                for m in targets {
                    match mirrors::sync_mirror(&mirrors::SubprocessSync, m, &root, now_ms) {
                        Ok(r) => println!(
                            "synced {} — {} rpm(s) → {} (@{})",
                            r.name, r.rpm_count, r.served_baseurl, r.synced_at_ms
                        ),
                        Err(e) => {
                            failures += 1;
                            eprintln!("mackesd mirrors: sync {} failed: {e}", m.name);
                        }
                    }
                }
                if failures > 0 {
                    std::process::exit(1);
                }
                return Ok(());
            }
            if json {
                let rows: Vec<serde_json::Value> = list
                    .iter()
                    .map(|m| {
                        serde_json::json!({
                            "name": m.name,
                            "description": m.description,
                            "upstream": m.upstream,
                            "enabled": m.enabled,
                            "file_baseurl": m.file_baseurl(&root),
                            "last_sync_ms": m.last_sync_ms(&root),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string(&rows)?);
            } else {
                println!("{:<14} {:<8} {}", "MIRROR", "ENABLED", "UPSTREAM");
                for m in &list {
                    let synced = m
                        .last_sync_ms(&root)
                        .map_or_else(|| "never synced".to_string(), |ms| format!("synced @{ms}"));
                    println!("{:<14} {:<8} {}", m.name, m.enabled, m.upstream);
                    println!(
                        "               serves: {}  ({synced})",
                        m.file_baseurl(&root)
                    );
                }
            }
            return Ok(());
        }
        Cmd::Images {
            json,
            record,
            build,
            name,
            kind,
            version,
            size_bytes,
            profile,
        } => {
            // PLANES-22 — the four buildable kinds, each with its
            // versioned builds present on LizardFS (W53/W55).
            use mackesd_core::image_catalog::{self, ImageKind};
            let root = mackesd_core::default_qnm_shared_root();
            // W54 — build the artifact now (then record it). Runs the real
            // per-kind tool; gated to execution-tagged nodes when launched
            // via the jobs engine.
            if build {
                let (Some(name), Some(kind_s), Some(version)) =
                    (name.clone(), kind.clone(), version.clone())
                else {
                    eprintln!("mackesd images --build requires --name, --kind, and --version");
                    std::process::exit(1);
                };
                let Some(image_kind) = ImageKind::parse(&kind_s) else {
                    eprintln!(
                        "mackesd images --build: unknown kind '{kind_s}' (iso|vm|container|usb)"
                    );
                    std::process::exit(1);
                };
                use mackesd_core::image_build::{
                    build_image, now_ms, BuildInputs, SubprocessBuild,
                };
                let runner = SubprocessBuild::new(BuildInputs::default());
                match build_image(
                    &runner,
                    &root,
                    image_kind,
                    &name,
                    &version,
                    profile.clone(),
                    now_ms(),
                ) {
                    Ok(m) => println!(
                        "built {} {} v{} ({} bytes) — manifest recorded",
                        m.kind,
                        m.name,
                        m.version,
                        m.size_bytes.unwrap_or(0)
                    ),
                    Err(e) => {
                        eprintln!("mackesd images --build: {e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            // W55 — register a completed build's manifest.
            if record {
                let (Some(name), Some(kind), Some(version)) = (name, kind, version) else {
                    eprintln!("mackesd images --record requires --name, --kind, and --version");
                    std::process::exit(1);
                };
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let manifest = image_catalog::ImageManifest {
                    name,
                    kind,
                    version,
                    built_at_ms: Some(now_ms),
                    size_bytes,
                    profile,
                };
                match image_catalog::record_manifest(&manifest, &root) {
                    Ok(p) => println!(
                        "recorded {} {} v{} → {}",
                        manifest.kind,
                        manifest.name,
                        manifest.version,
                        p.display()
                    ),
                    Err(e) => {
                        eprintln!("mackesd images: record failed: {e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            let manifests = image_catalog::load_manifests(&root);
            let rows: Vec<serde_json::Value> = ImageKind::all()
                .iter()
                .map(|kind| {
                    let builds: Vec<serde_json::Value> = manifests
                        .iter()
                        .filter(|m| m.kind == kind.as_str())
                        .map(|m| {
                            serde_json::json!({
                                "name": m.name,
                                "version": m.version,
                                "built_at_ms": m.built_at_ms,
                                "size_bytes": m.size_bytes,
                                "profile": m.profile,
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "kind": kind.as_str(),
                        "label": kind.label(),
                        "description": kind.description(),
                        "builds": builds,
                    })
                })
                .collect();
            if json {
                println!("{}", serde_json::to_string(&rows)?);
            } else {
                for kind in ImageKind::all() {
                    let n = manifests.iter().filter(|m| m.kind == kind.as_str()).count();
                    println!(
                        "{:<18} {} build(s) — {}",
                        kind.label(),
                        n,
                        kind.description()
                    );
                    for m in manifests.iter().filter(|m| m.kind == kind.as_str()) {
                        println!("    {} v{}", m.name, m.version);
                    }
                }
            }
            return Ok(());
        }
        Cmd::Upgrade {
            coordinate,
            version,
        } => {
            // PLANES-7 (W28) — publish a coordinated-upgrade intent the
            // fleet's watchers process (quorum + grace barrier).
            if !coordinate {
                eprintln!("mackesd upgrade: pass --coordinate to publish an upgrade intent");
                std::process::exit(1);
            }
            let root = mackesd_core::default_qnm_shared_root();
            let label = version.unwrap_or_else(|| "latest".to_string());
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            match mackesd_core::workers::upgrade_intent_watcher::write_intent(&root, &label, now_ms)
            {
                Ok(p) => println!(
                    "coordinated upgrade '{label}' — intent published at {} \
                     (each peer upgrades behind the quorum + grace barrier)",
                    p.display()
                ),
                Err(e) => {
                    eprintln!("mackesd upgrade --coordinate: {e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        Cmd::Nodes { cmd } => {
            // CB-1.5.a — fleet node roster surface. The Iced
            // inventory panel consumes the JSON shape directly.
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            match cmd {
                NodesCmd::List { json } => {
                    let nodes = mackesd_core::store::list_nodes(&conn)
                        .context("listing nodes from store")?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&nodes_to_json(&nodes))?);
                    } else {
                        print_nodes_table(&nodes);
                    }
                }
            }
        }
        Cmd::AnsibleHistory { cmd } => {
            // CB-1.5.c follow-up — walks QNM-Shared
            // ansible-runs/<peer>/*.json and emits the union as
            // a sorted JSON array (or human-readable table).
            match cmd {
                AnsibleHistoryCmd::List { json } => {
                    let root = ansible_runs_root();
                    let rows = collect_ansible_history(&root);
                    if json {
                        println!("{}", serde_json::to_string_pretty(&rows)?);
                    } else {
                        print_ansible_history_table(&rows);
                    }
                }
            }
        }
        Cmd::Events { cmd } => {
            // CB-1.8 mesh_history follow-up — audit-log
            // viewer surface.
            let conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            match cmd {
                EventsCmd::List { json } => {
                    let rows = mackesd_core::store::load_audit_rows(&conn)
                        .context("loading events from store")?;
                    let serial: Vec<serde_json::Value> = rows
                        .into_iter()
                        .map(|r| {
                            let payload_str = String::from_utf8(r.payload).unwrap_or_default();
                            serde_json::json!({
                                "event_id":     r.event_id,
                                "timestamp_ms": r.timestamp_ms,
                                "payload":      payload_str,
                                "hash":         hex_encode(&r.hash),
                            })
                        })
                        .collect();
                    if json {
                        println!("{}", serde_json::to_string_pretty(&serial)?);
                    } else if serial.is_empty() {
                        println!("(audit chain empty — no events yet)");
                    } else {
                        for r in &serial {
                            let id = r.get("event_id").and_then(|v| v.as_u64()).unwrap_or(0);
                            let ts = r.get("timestamp_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                            let payload = r.get("payload").and_then(|v| v.as_str()).unwrap_or("");
                            println!("{id:>8}  {ts}  {payload}");
                        }
                    }
                }
            }
        }
        Cmd::Playbooks { cmd } => {
            // CB-1.5.b follow-up — curated playbook surface.
            match cmd {
                PlaybooksCmd::List { json } => {
                    let root = playbooks_root();
                    let mut entries = enumerate_playbook_roles(&root);
                    entries.sort();
                    let rows: Vec<serde_json::Value> = entries
                        .into_iter()
                        .map(|name| {
                            let description = playbook_description(&name);
                            serde_json::json!({
                                "name":        name,
                                "description": description,
                            })
                        })
                        .collect();
                    if json {
                        println!("{}", serde_json::to_string_pretty(&rows)?);
                    } else if rows.is_empty() {
                        println!("(no curated playbooks under {})", root.display());
                    } else {
                        for r in &rows {
                            let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let desc = r.get("description").and_then(|v| v.as_str()).unwrap_or("");
                            println!("{name:<28} {desc}");
                        }
                    }
                }
                PlaybooksCmd::Run { name } => {
                    // Spawn ansible-pull directly so the user sees
                    // its progress streaming. Exit with whatever
                    // ansible-pull exited with.
                    let status = std::process::Command::new("ansible-pull")
                        .args(["--tags", &name, "site.yml"])
                        .status();
                    match status {
                        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                        Err(e) => {
                            eprintln!("mded: ansible-pull spawn failed: {e}");
                            std::process::exit(2);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// `$QNM_SHARED_ROOT/.qnm-sync/playbooks/roles/` — same
/// resolution the Iced playbooks panel uses.
fn playbooks_root() -> PathBuf {
    let base = std::env::var("QNM_SHARED_ROOT").map(PathBuf::from).ok();
    let base = base.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("QNM-Shared"))
            .unwrap_or_else(|_| PathBuf::from("/var/empty"))
    });
    base.join(".qnm-sync").join("playbooks").join("roles")
}

/// Walk roles/ for subdirectories. Returns role names (bare
/// basenames); empty on any I/O error so the panel + CLI can
/// surface the empty-state message.
fn enumerate_playbook_roles(root: &std::path::Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in rd.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Curated descriptions per the Phase 1.3.0 lock. Mirrors the
/// `playbook_from_name` helper in the Iced playbooks panel so
/// the CLI and the GUI agree.
/// Lowercase hex string of a fixed byte slice. Avoids the
/// hex crate dep for one helper.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn playbook_description(name: &str) -> &'static str {
    match name {
        "system-update" => "Apply pending dnf upgrades (gated, never runs on default tag)",
        "mesh-state-snapshot" => "Snapshot QNM-Shared state for offline review",
        "selinux-permissive-toggle" => "Flip SELinux to permissive (op-tagged, never default)",
        "container-runtime-setup" => "Install + configure podman / docker runtime",
        "xfconf-baseline" => "Apply baseline xfconf keys (default-tagged)",
        "bloat-removal" => "Remove the curated bloat package list",
        "apps-install" => "Install the curated MDE app list",
        _ => "Custom role",
    }
}

/// `~/QNM-Shared/.qnm-sync/ansible-runs/` (or its
/// `$QNM_SHARED_ROOT` override). Matches the panel's
/// resolution in `mde-workbench/src/panels/run_history.rs`.
fn ansible_runs_root() -> PathBuf {
    let base = std::env::var("QNM_SHARED_ROOT").map(PathBuf::from).ok();
    let base = base.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("QNM-Shared"))
            .unwrap_or_else(|_| PathBuf::from("/var/empty"))
    });
    base.join(".qnm-sync").join("ansible-runs")
}

/// This node's short hostname (`hostname`), or `"unknown"`.
fn local_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// This node's Nebula overlay IP via `ip -4 addr show nebula1`, if up.
fn local_overlay_ip() -> Option<String> {
    let out = std::process::Command::new("ip")
        .args(["-4", "addr", "show", "nebula1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        l.trim()
            .strip_prefix("inet ")
            .and_then(|rest| rest.split('/').next())
            .map(str::to_string)
    })
}

/// Walk every peer subdir + parse each `*.json` as a record.
/// Returns the union sorted by timestamp descending. Errors
/// are swallowed silently (no peer dir / unreadable file
/// just drops that row) — matches the panel's
/// non-aborting walk.
fn collect_ansible_history(root: &std::path::Path) -> Vec<serde_json::Value> {
    let Ok(peers) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for peer_entry in peers.flatten() {
        let Ok(ft) = peer_entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let peer_name = peer_entry
            .file_name()
            .to_str()
            .map(str::to_string)
            .unwrap_or_default();
        if peer_name.is_empty() {
            continue;
        }
        let peer_dir = peer_entry.path();
        let Ok(files) = std::fs::read_dir(&peer_dir) else {
            continue;
        };
        for file_entry in files.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            // Inject the peer name + source path so the JSON
            // row is self-describing (the panel does the same
            // mapping).
            if let Some(obj) = v.as_object_mut() {
                obj.insert("peer".into(), serde_json::Value::String(peer_name.clone()));
                obj.insert(
                    "_source_path".into(),
                    serde_json::Value::String(path.to_string_lossy().into_owned()),
                );
            }
            rows.push(v);
        }
    }
    rows.sort_by(|a, b| {
        let ts_a = a.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let ts_b = b.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
        ts_b.partial_cmp(&ts_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

fn print_ansible_history_table(rows: &[serde_json::Value]) {
    if rows.is_empty() {
        println!("(no ansible-pull runs recorded)");
        return;
    }
    println!(
        "{:<16} {:<24} {:<6} {:<8} {:<8} {:<10}",
        "peer", "playbook", "exit", "changed", "ok", "trigger"
    );
    for r in rows {
        let peer = r
            .get("peer")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .chars()
            .take(16)
            .collect::<String>();
        let playbook = r
            .get("playbook")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .chars()
            .take(24)
            .collect::<String>();
        let exit = r.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(0);
        let changed = r.get("changed").and_then(|v| v.as_u64()).unwrap_or(0);
        let ok = r.get("ok").and_then(|v| v.as_u64()).unwrap_or(0);
        let trigger = r
            .get("triggered_by")
            .and_then(|v| v.as_str())
            .unwrap_or("pull");
        println!("{peer:<16} {playbook:<24} {exit:<6} {changed:<8} {ok:<8} {trigger:<10}");
    }
}

/// Serialize the `NodeRow` list into the JSON shape the Iced
/// inventory panel consumes. Kept here rather than as a
/// `#[derive(Serialize)]` on `NodeRow` because the store struct
/// already serves topology + lifecycle callers and the JSON
/// shape is a CLI-surface contract.
fn nodes_to_json(nodes: &[mackesd_core::store::NodeRow]) -> serde_json::Value {
    serde_json::Value::Array(
        nodes
            .iter()
            .map(|n| {
                serde_json::json!({
                    "node_id":    n.node_id,
                    "name":       n.name,
                    "public_key": n.public_key,
                    "role":       n.role,
                    "health":     n.health,
                    "region":     n.region,
                })
            })
            .collect(),
    )
}

fn print_nodes_table(nodes: &[mackesd_core::store::NodeRow]) {
    if nodes.is_empty() {
        println!("(no peers enrolled)");
        return;
    }
    println!(
        "{:<24} {:<24} {:<12} {:<12} {:<10}",
        "node_id", "name", "role", "health", "region"
    );
    for n in nodes {
        println!(
            "{:<24} {:<24} {:<12} {:<12} {:<10}",
            n.node_id,
            n.name,
            n.role,
            n.health,
            n.region.as_deref().unwrap_or("-"),
        );
    }
}

/// Read a revision's `spec_json` payload by id.
fn load_revision_payload(conn: &rusqlite::Connection, revision_id: &str) -> anyhow::Result<String> {
    let rev: i64 = revision_id
        .parse()
        .map_err(|_| anyhow::anyhow!("revision id must be an integer (got {revision_id})"))?;
    let payload: String = conn
        .query_row(
            "SELECT spec_json FROM desired_config WHERE revision_id = ?",
            [rev],
            |r| r.get(0),
        )
        .with_context(|| format!("loading revision {revision_id}"))?;
    Ok(payload)
}

/// List every revision (descending by id).
fn list_revisions(conn: &rusqlite::Connection) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut stmt = conn
        .prepare(
            "SELECT revision_id, author, message, state, created_at \
             FROM desired_config ORDER BY revision_id DESC",
        )
        .context("preparing revisions list")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(serde_json::json!({
                "revision_id":  r.get::<_, i64>(0)?.to_string(),
                "author":       r.get::<_, String>(1)?,
                "summary":      r.get::<_, String>(2)?,
                "state":        r.get::<_, String>(3)?,
                "created_at":   r.get::<_, String>(4)?,
            }))
        })
        .context("executing revisions list")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("materializing revisions list")?;
    Ok(rows)
}

fn print_revisions_table(rows: &[serde_json::Value]) {
    if rows.is_empty() {
        println!("(no revisions)");
        return;
    }
    for row in rows {
        let rid = row
            .get("revision_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let st = row.get("state").and_then(|v| v.as_str()).unwrap_or("?");
        let aut = row.get("author").and_then(|v| v.as_str()).unwrap_or("?");
        let cre = row
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let sm = row.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        println!("{rid:>6}  [{st}]  {aut:<16}  {cre}  {sm}");
    }
}

/// `mackesd serve` runtime. Pulls in tokio + the async supervisor
/// only when the `async-services` feature is active so the default
/// build stays sync.
///
/// v3.0.3 — wires the Phase B workers (heartbeat, mesh_router, …;
/// notification_relay retired in BUS-4.2, clipboard/mdns/fs_sync
/// retired in RETIRE-PY.1/.3/.4) into the
/// `Supervisor` alongside the legacy reconcile worker. Audit-2
/// caught all 6 as dead code: `impl Worker for X` shipped, no
/// spawn. Each worker gets a `RestartPolicy::OnFailure` so a
/// transient error (sqlite contention, mdns socket flake)
/// restarts the worker after the supervisor's 250ms back-off
/// without taking down the whole daemon.
///
/// Also wires `mackesd_core::logging::LogContext` (Tier 3 — Phase 12.1.4):
/// every log line inside `run_serve` inherits the daemon's
/// correlation_id + node_id via a top-level tracing span.
#[cfg(feature = "async-services")]
fn run_serve(
    workgroup_root: Option<PathBuf>,
    node_id: Option<String>,
    db_path: PathBuf,
) -> anyhow::Result<()> {
    use mackesd_core::workers::{
        firewall_preset::FirewallPresetWorker, fleet_reconcile, heartbeat::HeartbeatWorker,
        job_exec, lifecycle_exec, mdns_relay::MdnsRelayWorker, mesh_dns,
        mesh_router::MeshRouterWorker, netstate_apply, presence_watch, ssh_pubkey_gossip,
        sshd_overlay_bind::SshdOverlayBindWorker, validation_suite,
        voice_config::VoiceConfigWorker, RestartPolicy, Spawn, Supervisor,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::RwLock;
    let workgroup_root = workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let node_id = node_id.unwrap_or_else(default_node_id);

    // v3.0.3 — daemon-scope tracing span so every log line below
    // carries correlation_id + node_id. The JSON formatter
    // (initialized in main.rs's tracing-subscriber setup) picks up
    // span fields automatically.
    let log_ctx = mackesd_core::logging::LogContext::fresh().with_node(node_id.clone());
    let _daemon_span = tracing::info_span!(
        "daemon",
        correlation_id = log_ctx.correlation_id,
        node_id = %log_ctx.node_id.as_deref().unwrap_or("")
    )
    .entered();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    runtime.block_on(async move {
        tracing::info!("mackesd serve: starting supervisor + workers");
        let shutdown = Arc::new(AtomicBool::new(false));
        install_signal_handlers(Arc::clone(&shutdown)).context("installing signal handlers")?;

        // HYP-8.5 — load operator tag manifests on startup +
        // publish one Bus event per loaded tag. Fail-open: missing
        // dir → 0 tags loaded → no events. Per-file parse failures
        // log + skip in `load_tag_manifests`, the daemon never
        // crashes on a malformed manifest.
        if let Some(tags_dir) = mackesd_core::config::default_manifests_dir() {
            match mackesd_core::config::load_tag_manifests(&tags_dir) {
                Ok(manifests) => {
                    tracing::info!(
                        path = %tags_dir.display(),
                        count = manifests.len(),
                        "tag_manifest: loaded operator manifests",
                    );
                    for m in &manifests {
                        // Best-effort Bus publish — broker may not be
                        // up yet during the early startup phase, but
                        // the spawn-detached shell-out makes that a
                        // silent no-op rather than a daemon crash.
                        let topic = "event/config/tags/loaded".to_string();
                        let body = format!(
                            r#"{{"name":"{}","apps":{},"layout":"{}","autostart":{}}}"#,
                            m.name.replace('"', "\\\""),
                            m.apps.len(),
                            m.layout.replace('"', "\\\""),
                            m.autostart,
                        );
                        let mut cmd = std::process::Command::new("mde-bus");
                        cmd.arg("publish")
                            .arg(&topic)
                            .arg("--body-flag")
                            .arg(&body);
                        mackesd_core::proc_reap::fire_and_reap(
                            cmd,
                            mackesd_core::proc_reap::DEFAULT_REAP_TIMEOUT,
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        path = %tags_dir.display(),
                        error = %e,
                        "tag_manifest: directory load failed (expected on first boot)",
                    );
                }
            }
        }

        // v3.0.3 — async supervisor for Phase B workers. The
        // legacy reconcile worker stays on its own std::thread
        // because its sync rusqlite calls would block the tokio
        // scheduler if hosted here; both supervisors coexist.
        let mut sup = Supervisor::new();
        // EFF-24 — the live per-worker status registry: the supervisor
        // records alive/restarts/breaker transitions; the Bus healthz
        // folds them into the readiness verdict and the exporter emits
        // them as gauges.
        let worker_status = mackesd_core::workers::new_status_map();
        sup.set_status_map(Arc::clone(&worker_status));
        // EFF-21 — capture the dev-fallback backup passphrase ONCE and
        // scrub it from the process environment immediately, so none of
        // the worker subprocesses (nebula-cert, df, firewall-cmd, …)
        // inherit the secret via environ. The systemd-creds path
        // (CREDENTIALS_DIRECTORY) is unaffected — never env-borne
        // (ENT-11). The captured value feeds the backup worker; the
        // boolean feeds the exporter's backup-posture gauge.
        let backup_passphrase: Option<String> = std::env::var("MDE_BACKUP_PASSPHRASE")
            .ok()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());
        std::env::remove_var("MDE_BACKUP_PASSPHRASE");
        // v4.1 — track spawned worker names so Shell.Workers can
        // surface them via D-Bus. Strings get pushed alongside
        // each sup.spawn(); skipped workers (sqlite open failure)
        // don't get added so the report matches reality. The
        // Mutex<Vec<String>> is shared with ShellService so
        // post-registration spawns (KDC + reconcile, which come
        // after IPC registration) still appear in the roster.
        let worker_names: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        // OV-7.a/c (v2.6) — shared signal-sender slot for every
        // worker that emits dev.mackes.MDE.Nebula.Status.*
        // signals. Workers receive a clone before they spawn;
        // the IPC bootstrap fills the slot once the Nebula
        // status surface is registered. Empty slot → silent
        // emission; SQL + bundle writes still land.
        let nebula_signal_slot = mackesd_core::ipc::nebula::new_signal_sender_slot();
        // E1.2 — resolve the deployment-role rank once; every worker spawn below
        // is gated by `mackesd_core::worker_role::runs(name, role_rank)` so a
        // Lighthouse/Server starts only its tier's workers (plan §12). Unpinned
        // (dev / pre-role-pin) → Workstation rank (full set; desktop workers
        // idle gracefully without a display); malformed role.toml → Lighthouse
        // (fail closed). The resulting set is observable via `mackesd
        // role-workers` and the live worker-status listing.
        // ENT-2 (C3) — fail closed: an unpinned box refuses to start.
        let role_rank = match mackesd_core::worker_role::resolve_rank_strict() {
            Ok(rank) => rank,
            Err(msg) => {
                eprintln!("mackesd serve: {msg}");
                anyhow::bail!("worker pool refused to start: no pinned role (ENT-2)");
            }
        };
        tracing::info!(
            role_rank,
            "E1.2: spawning the role-permitted worker subset"
        );
        // E1.3 #3 — read the operator-tunable daemon config from
        // /etc/mackesd/mackesd.toml (fail-open to the locked defaults on a
        // missing/malformed file). Its cadence knobs feed the heartbeat +
        // mesh-latency worker spawns below, so an edit + `systemctl restart
        // mackesd` changes the live write cadence with no rebuild.
        let daemon_cfg = mackesd_core::config::daemon::load();
        tracing::info!(
            heartbeat_interval_secs = daemon_cfg.heartbeat_interval_secs,
            mesh_latency_sweep_secs = daemon_cfg.mesh_latency_sweep_secs,
            "E1.3: loaded /etc/mackesd/mackesd.toml daemon config",
        );
        // MESH-MDNS-RELAY — native cross-segment mDNS service relay (browses
        // the local LAN, publishes services to the mesh Bus). Rank 0: a relay
        // control-plane worker, runs on every role.
        if mackesd_core::worker_role::runs("mdns_relay", role_rank) {
            sup.spawn(Spawn::new(MdnsRelayWorker::new(), RestartPolicy::OnFailure));
            worker_names.lock().expect("worker_names mutex").push("mdns_relay".into());
        }
        // RETIRE-PY.4 (2026-06-07) — the GVFS `fs_sync` worker (supervised
        // `python3 -m mackes.mesh_gvfs.daemon`, a retired Python MDE module
        // absent in the monorepo) is removed. Mesh storage is served by
        // LizardFS (E3); per-peer share access is via the Bus file-ops, so
        // the second FUSE substrate is retired rather than rebuilt.
        if mackesd_core::worker_role::runs("heartbeat", role_rank) {
            sup.spawn(Spawn::new(
                HeartbeatWorker::new(workgroup_root.clone(), node_id.clone())
                    .with_interval(daemon_cfg.heartbeat_interval()),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("heartbeat".into());
        }
        // OV-7.a (v2.6) — health reconciler. Polls each known
        // peer's QNM-Shared heartbeat.json every 5 s, applies the
        // telemetry::health_state_from_age threshold table, writes
        // back into nodes.health, and fires PeerStateChanged on
        // transitions. Closes the gap between live heartbeats and
        // the SQLite column that NebulaStatusService::build_peer_list
        // projects. Spawn order: after HeartbeatWorker so peers
        // have at least one observable heartbeat by the first
        // reconcile tick.
        if mackesd_core::worker_role::runs("health_reconciler", role_rank) {
            sup.spawn(Spawn::new(
                mackesd_core::workers::health_reconciler::HealthReconcilerWorker::new(
                    workgroup_root.clone(),
                    db_path.clone(),
                    node_id.clone(),
                    std::sync::Arc::clone(&nebula_signal_slot),
                ),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("health_reconciler".into());
        }
        // AUD2-1 — the shared `kdc2_router_decision_us` histogram: the
        // mesh_router observes its per-tick decision time into it, and
        // the metrics_exporter snapshots it into mackesd.prom — without
        // this shared handle the SLO instrumentation was observed (or,
        // before 2026-06-12, not even attached) and never exported.
        let router_metrics: mackesd_core::workers::mesh_router::RouterMetrics = Arc::new(
            std::sync::Mutex::new(mackesd_core::metrics::kdc2_router_decision_us()),
        );
        // EFF-9 — Prometheus textfile exporter. Lighthouse-tier (the
        // observability surface lives on the control plane). Snapshots
        // mesh node-health buckets + audit-chain status + migration
        // count + the router decision histogram into
        // <textfile_collector>/mackesd.prom every 30 s; a node_exporter
        // textfile collector scrapes it.
        if mackesd_core::worker_role::runs("metrics_exporter", role_rank) {
            sup.spawn(Spawn::new(
                mackesd_core::workers::metrics_exporter::MetricsExporterWorker::new(
                    db_path.clone(),
                    mackesd_core::metrics::default_textfile_dir(),
                    Some(mackesd_core::workers::cert_authority::default_ca_cert_path()),
                )
                .with_router_metrics(Arc::clone(&router_metrics))
                // EFF-26 — worker/breaker gauges + trip alert.
                .with_worker_status(Arc::clone(&worker_status))
                // EFF-26 — disk headroom for the replicated volume + the
                // store's filesystem.
                .with_disk_paths(vec![
                    workgroup_root.clone(),
                    db_path.parent().map(PathBuf::from).unwrap_or_default(),
                ])
                // EFF-26 — backup staleness against the daily bundle.
                .with_backup_file(
                    mackesd_core::workers::nebula_ca_backup::backup_path_for(
                        &workgroup_root,
                        &node_id,
                    ),
                )
                // EFF-21 — env is scrubbed at boot; presence rides this flag.
                .with_backup_passphrase_set(backup_passphrase.is_some()),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("metrics_exporter".into());
        }
        // VV-2 (v4.1.0) — voice_config worker. Seeds the
        // /var/lib/mackesd/voice-desired.json document on first
        // tick + triggers `systemctl try-reload-or-restart` on
        // kamailio-mde + rtpengine-mde when the file changes.
        // try-reload-or-restart is a no-op while the units are
        // disabled (v4.1.0 ships them disabled per the spec
        // %post comment until VV-4 + VV-14 are green), so the
        // worker is harmless to run on a fresh peer.
        // Bug 6 (2026-06-06) — voice_config seeds the system path
        // /var/lib/mackesd/voice-desired.json (the root ExecStartPre reads it to
        // build /etc/kamailio-mde/*). A per-user daemon can't write there, so the
        // worker's 5 s tick spammed an EPERM WARN forever. Only run it when that
        // dir is actually writable (i.e. the system daemon).
        let voice_dir_writable = std::path::Path::new(
            mackesd_core::workers::voice_config::DEFAULT_DESIRED_JSON,
        )
        .parent()
        .is_some_and(|d| {
            let probe = d.join(".mackesd-write-probe");
            let ok = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&probe)
                .is_ok();
            if ok {
                let _ = std::fs::remove_file(&probe);
            }
            ok
        });
        if mackesd_core::worker_role::runs("voice_config", role_rank) {
            if voice_dir_writable {
                sup.spawn(Spawn::new(
                    VoiceConfigWorker::new(node_id.clone()),
                    RestartPolicy::OnFailure,
                ));
                worker_names.lock().expect("worker_names mutex").push("voice_config".into());
            } else {
                tracing::info!(
                    "voice_config: system voice dir not writable (per-user daemon); worker skipped"
                );
            }
        }
        // NF-21.1 — sshd overlay-bind worker. Polls
        // /var/lib/mackesd/nebula/overlay-ip every 5 s; on change,
        // writes the /etc/ssh/sshd_config.d/mackes-mesh.conf drop-in
        // + reloads sshd so the daemon binds to the new overlay
        // address. Quiet no-op on pre-enrollment peers (missing
        // publish file). Replaces mesh_nebula.py::write_sshd_overlay_bind
        // so the Python module can fully retire (DEAD-2.14 plan).
        sup.spawn(Spawn::new(
            SshdOverlayBindWorker::new(),
            RestartPolicy::OnFailure,
        ));
        worker_names.lock().expect("worker_names mutex").push("sshd_overlay_bind".into());
        // SVC-2 (Q60) — SSH pubkey gossip: publish this box's user
        // ed25519 pubkey into <root>/ssh-keys/ and merge every peer's
        // published key into ~/.ssh/authorized_keys (managed block,
        // write-on-change). LizardFS replication is the transport.
        // PD-11 — the lifecycle executor: descriptor-gated container/VM
        // start/stop requests from peers, via replicated request files.
        if mackesd_core::worker_role::runs("lifecycle_exec", role_rank) {
            sup.spawn(Spawn::new(
                lifecycle_exec::LifecycleExecWorker::new(
                    workgroup_root.clone(),
                    node_id.strip_prefix("peer:").unwrap_or(&node_id).to_string(),
                ),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("lifecycle_exec".into());
        }
        // PD-13 — presence-transition alerts: offline/online crossings
        // become desktop notifications via the alert_relay pipeline.
        if mackesd_core::worker_role::runs("presence_watch", role_rank) {
            let alerts = mackesd_core::workers::alert_relay::default_alerts_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp/mde-alerts"));
            sup.spawn(Spawn::new(
                presence_watch::PresenceWatchWorker::new(
                    workgroup_root.clone(),
                    alerts,
                    node_id.strip_prefix("peer:").unwrap_or(&node_id).to_string(),
                ),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("presence_watch".into());
        }
        // PD-9 / FPG — the reconcile driver: magic-fleet reconcile on a
        // 15-min cadence + immediately on this host's nudge file.
        if mackesd_core::worker_role::runs("fleet_reconcile", role_rank) {
            sup.spawn(Spawn::new(
                fleet_reconcile::FleetReconcileWorker::new(
                    workgroup_root.clone(),
                    node_id.strip_prefix("peer:").unwrap_or(&node_id).to_string(),
                ),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("fleet_reconcile".into());
        }
        // PLANES-18 — mesh DNS: feed <host>.mesh into resolved +
        // /etc/hosts on every node (rank 0 plumbing).
        if mackesd_core::worker_role::runs("mesh_dns", role_rank) {
            sup.spawn(Spawn::new(
                mesh_dns::MeshDnsWorker::new(Some(db_path.clone())),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("mesh_dns".into());
        }
        // PLANES-15 — netstate engine mount: converge the baseline's
        // network desired-state under a rollback checkpoint + overlay
        // self-test (W77/W78), on every node.
        if mackesd_core::worker_role::runs("netstate_apply", role_rank) {
            sup.spawn(Spawn::new(
                netstate_apply::NetstateApplyWorker::new(
                    workgroup_root.clone(),
                    Some(db_path.clone()),
                    node_id.strip_prefix("peer:").unwrap_or(&node_id).to_string(),
                ),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("netstate_apply".into());
        }
        // PLANES-19 — overlay-reachability validation suite: every node
        // participates; the leader mints nightly/run-now + writes verdicts.
        if mackesd_core::worker_role::runs("validation_suite", role_rank) {
            sup.spawn(Spawn::new(
                validation_suite::ValidationSuiteWorker::new(
                    workgroup_root.clone(),
                    Some(db_path.clone()),
                    node_id.strip_prefix("peer:").unwrap_or(&node_id).to_string(),
                    std::path::PathBuf::from(
                        mackesd_core::workers::netdata_aggregator::DEFAULT_ROLE_HOST_MARKER,
                    ),
                ),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("validation_suite".into());
        }
        // PLANES-9 — the local job executor (execution-tag gated, W84).
        if mackesd_core::worker_role::runs("job_exec", role_rank) {
            sup.spawn(Spawn::new(
                job_exec::JobExecWorker::new(
                    workgroup_root.clone(),
                    node_id.strip_prefix("peer:").unwrap_or(&node_id).to_string(),
                ),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("job_exec".into());
        }
        if mackesd_core::worker_role::runs("ssh_pubkey_gossip", role_rank) {
            sup.spawn(Spawn::new(
                ssh_pubkey_gossip::SshPubkeyGossipWorker::new(
                    workgroup_root.clone(),
                    node_id.clone(),
                ),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("ssh_pubkey_gossip".into());
        }
        // NF-21.3 — firewall_preset worker. Applies the Nebula
        // firewalld preset (UDP/4242 inbound on all peers; TCP/443
        // inbound additionally on lighthouses) on first tick + on
        // every role-flip via the /var/lib/mackesd/nebula/role.host
        // marker. Idempotent — firewall-cmd's ALREADY_ENABLED is
        // treated as success. Replaces mesh_nebula.py::apply_nebula_firewall_preset
        // so the Python helper can retire (DEAD-2.14 plan).
        sup.spawn(Spawn::new(
            FirewallPresetWorker::new(),
            RestartPolicy::OnFailure,
        ));
        worker_names.lock().expect("worker_names mutex").push("firewall_preset".into());
        // mesh_router bootstraps with the per-transport
        // registry. Phase 12.18 D.2 (2026-05-23) — the NebulaHttps443
        // transport is registered at startup so the per-peer
        // HttpsFallbackState::Active transition can actually
        // route through a real TLS tunnel. The transport
        // gracefully reports `Misconfigured(no_fallback_host)`
        // until MDE_HTTPS_FALLBACK_HOST is set, so daemons
        // running without the env var still boot clean.
        let router_state: mackesd_core::workers::mesh_router::RouterState =
            Arc::new(RwLock::new(HashMap::new()));
        let https443: Arc<dyn mackes_transport::Transport> =
            Arc::new(mackesd_core::transport::https443::NebulaHttps443Transport::new());
        let router_registry: mackesd_core::workers::mesh_router::TransportRegistry =
            Arc::new(vec![https443]);
        // KDC2-1.9 (AUD3 S-2) — load the routing policy (system +
        // user policy.toml; fail-open to baseline with a warn so a
        // typo'd file never strands the router).
        let router_policy = mackesd_core::transport::policy::load_with_paths(
            std::path::Path::new("/etc/mde/connect/policy.toml"),
            &dirs::config_dir()
                .unwrap_or_default()
                .join("mde/connect/policy.toml"),
        )
        .map(|loaded| loaded.scorer)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "mesh-router: policy.toml load failed; baseline policy");
            mackes_transport::scorer::Policy::baseline()
        });
        sup.spawn(Spawn::new(
            // AUD2-1 — attach the shared decision-time histogram so the
            // router's per-tick observe() lands in the exporter's scrape.
            // AUD3 S-2/S-5 — scorer policy + the audit sink (path flips
            // land in the hash-chained events table + alert hooks).
            MeshRouterWorker::new(Arc::clone(&router_state), router_registry)
                .with_metrics(Arc::clone(&router_metrics))
                .with_policy(router_policy)
                .with_audit_sink(db_path.clone(), node_id.clone()),
            RestartPolicy::OnFailure,
        ));
        worker_names.lock().expect("worker_names mutex").push("mesh_router".into());
        // v4.0.1 Phase 12.17 wire (2026-05-23) — STUN candidate
        // gatherer. Shares router_state with the router so
        // reflexive candidates land on every tracked peer's
        // PeerPath.candidates list. 30 s cadence; per-server
        // probe timeout 1.4 s; default server pool is Google's
        // public STUN cluster (IP-pinned so the worker doesn't
        // hit DNS on the hot path).
        sup.spawn(Spawn::new(
            mackesd_core::workers::stun_gather::StunGatherWorker::new(
                Arc::clone(&router_state),
            ),
            RestartPolicy::OnFailure,
        ));
        worker_names.lock().expect("worker_names mutex").push("stun_gather".into());
        // BUS-4.2 (2026-05-26) — `notification_relay` retired.
        // Cross-peer notification routing is now a side-effect of
        // BUS-4.4's FDO bridge: every Notify call publishes to
        // `fdo/<app>` on the Mackes Bus, and every peer subscribes
        // to `fdo/#` via the standard Bus subscription. The
        // legacy `~/QNM-Shared/<peer>/.qnm-notifications/` JSON
        // file convention is replaced by `<bus_root>/<topic>/
        // <ulid>.json` (BUS-1.4 file tree on GFS).

        // v2.5 NF-3.4 (2026-05-23) — Nebula supervisor.
        // Watches the leader-election state + the QNM-Shared
        // nebula-bundle.json mtime; on leader-promotion mints
        // the CA, writes the role.host marker, starts the
        // lighthouse + tunnel units. On bundle change, re-
        // materializes the on-disk Nebula config + reloads.
        match mackesd_core::store::open(&db_path) {
            Ok(conn) => {
                let sup_store = Arc::new(tokio::sync::Mutex::new(conn));
                // Bundle path mirrors the existing heartbeat
                // convention: QNM-Shared/<self>/mackesd/...
                let bundle_path = workgroup_root
                    .join(&node_id)
                    .join("mackesd")
                    .join(mackesd_core::ca::bundle::BUNDLE_FILENAME);
                // mesh_id defaults to the configured node-id
                // namespace when the wizard hasn't named a
                // mesh yet. NF-7.x's wizard will overwrite the
                // record once the operator types a name.
                let mesh_id = std::env::var("MDE_MESH_ID")
                    .unwrap_or_else(|_| format!("mesh-{node_id}"));
                sup.spawn(Spawn::new(
                    mackesd_core::workers::nebula_supervisor::NebulaSupervisor::new(
                        sup_store,
                        node_id.clone(),
                        mesh_id,
                        bundle_path,
                    ),
                    RestartPolicy::OnFailure,
                ));
                worker_names.lock().expect("worker_names mutex").push("nebula_supervisor".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    db_path = %db_path.display(),
                    "nebula-supervisor: sqlite open failed; worker skipped"
                );
            }
        }

        // NF-3.6.c (v2.5) — auto-signer worker. Polls QNM-Shared
        // for pending-enroll CSRs every 30 s + auto-signs each
        // one via nebula_enroll::sign_pending_csr. Runs on every
        // node — on peer-role boxes (no active CA), sign_pending_csr
        // returns SignFailed and the worker logs at debug + moves
        // on. On lighthouse-role boxes with an active CA, this
        // closes the manual `mackesd ca sign-csr` operator step
        // for the common case. Spawned outside the nebula-supervisor
        // Ok arm so the watcher runs even if the supervisor's
        // SQLite open failed (the watcher opens its own per-tick
        // connection).
        // Advisory only — sign_pending_csr authoritatively signs under the
        // peer's own token mesh (bed fix #5). Kept solely so the watcher's
        // log line names the real mesh: prefer this lighthouse's own bundle,
        // then MDE_MESH_ID, then the legacy mesh-<node> placeholder.
        let csr_watcher_mesh_id = mackesd_core::ca::bundle::read_bundle(
            &mackesd_core::ca::bundle::bundle_path(&workgroup_root, &node_id),
        )
        .ok()
        .map(|b| b.mesh_id)
        .filter(|m| !m.is_empty())
        .or_else(|| std::env::var("MDE_MESH_ID").ok())
        .unwrap_or_else(|| format!("mesh-{node_id}"));
        // Bed fix #6 (auto-signer path): the bundle the auto-signer hands
        // a joining peer must carry the lighthouse's REAL externally-dialable
        // address, not its hostname. mesh-init recorded that address in this
        // lighthouse's own bundle (`lighthouses[0].external_addr`); read it
        // back. Falls back to the hostname guess only when no bundle exists
        // (a peer-role box has no CA, so it never actually signs anyway).
        let csr_watcher_lighthouse_addr = {
            let from_bundle = mackesd_core::ca::bundle::read_bundle(
                &mackesd_core::ca::bundle::bundle_path(&workgroup_root, &node_id),
            )
            .ok()
            .and_then(|b| b.lighthouses.into_iter().next())
            .map(|lh| lh.external_addr)
            .filter(|a| !a.is_empty());
            from_bundle.unwrap_or_else(|| {
                let host = std::fs::read_to_string("/etc/hostname")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| node_id.clone());
                format!("{host}:4242")
            })
        };
        sup.spawn(Spawn::new(
            mackesd_core::workers::nebula_csr_watcher::NebulaCsrWatcher::new(
                workgroup_root.clone(),
                db_path.clone(),
                csr_watcher_mesh_id,
                node_id.clone(),
                csr_watcher_lighthouse_addr.clone(),
            )
            .with_signal_slot(std::sync::Arc::clone(&nebula_signal_slot)),
            RestartPolicy::OnFailure,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("nebula_csr_watcher".into());

        // ONBOARD-2 — the lighthouse `/enroll` rustls HTTPS listener
        // (MESH-1 fix: network bootstrap for NAT'd peers). Spawned ONLY
        // when the endpoint cert is present — `mackesd found` (ONBOARD-4)
        // generates it on a lighthouse; a peer-role box has none, so we
        // skip rather than let the bind respin under OnFailure (same
        // gate the covert :443 listener uses). The roster the signed
        // bundles advertise reuses the csr-watcher's external-addr probe
        // so peers materialize static_host_map to the PUBLIC addr
        // (MESH-2 guard). Cert/key/bind overridable for LAN testing
        // (ONBOARD-8) via MDE_ENROLL_{CERT,KEY,BIND}.
        let enroll_cert = std::env::var("MDE_ENROLL_CERT").unwrap_or_else(|_| {
            mackesd_core::workers::nebula_enroll_listener::DEFAULT_CERT_PATH.to_string()
        });
        if std::path::Path::new(&enroll_cert).exists() {
            let mut w = mackesd_core::workers::nebula_enroll_listener::NebulaEnrollListener::new(
                std::sync::Arc::new(mackesd_core::ca::SubprocessBackend),
                db_path.clone(),
                workgroup_root.clone(),
                mackesd_core::nebula_enroll::SignCsrPaths::production_defaults(),
                node_id.clone(),
                csr_watcher_lighthouse_addr.clone(),
            )
            .with_cert(PathBuf::from(&enroll_cert));
            if let Ok(p) = std::env::var("MDE_ENROLL_KEY") {
                w = w.with_key(PathBuf::from(p));
            }
            if let Ok(addr) = std::env::var("MDE_ENROLL_BIND") {
                match addr.parse() {
                    Ok(parsed) => w = w.with_bind_addr(parsed),
                    Err(_) => tracing::warn!(
                        value = %addr,
                        "nebula-enroll-listener: MDE_ENROLL_BIND parse failed; using default",
                    ),
                }
            }
            sup.spawn(Spawn::new(w, RestartPolicy::OnFailure));
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("nebula_enroll_listener".into());
        } else {
            tracing::info!(
                cert = %enroll_cert,
                "nebula-enroll-listener: no endpoint cert present; not a lighthouse \
                 (or `mackesd found` not yet run) — worker skipped",
            );
        }

        // NF-18.4 (v2.5) — automated CA backup worker.
        // Opens its own SQLite handle for the per-tick
        // assemble_from_store read. Skips silently on peer-role
        // boxes (no CA key file). Requires MDE_BACKUP_PASSPHRASE
        // env var — operators opt in via the systemd unit's
        // Environment= line.
        match mackesd_core::store::open(&db_path) {
            Ok(conn) => {
                let backup_store = Arc::new(tokio::sync::Mutex::new(conn));
                let backup_mesh = std::env::var("MDE_MESH_ID")
                    .unwrap_or_else(|_| format!("mesh-{node_id}"));
                let mut backup_worker =
                    mackesd_core::workers::nebula_ca_backup::NebulaCaBackup::new(
                        workgroup_root.clone(),
                        node_id.clone(),
                        backup_mesh,
                        backup_store,
                    );
                // EFF-21 — hand over the boot-captured passphrase (the
                // env was scrubbed right after capture, top of run_serve).
                if let Some(phrase) = backup_passphrase.clone() {
                    backup_worker = backup_worker.with_passphrase(phrase);
                }
                sup.spawn(Spawn::new(backup_worker, RestartPolicy::OnFailure));
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("nebula_ca_backup".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    db_path = %db_path.display(),
                    "nebula-ca-backup: sqlite open failed; worker skipped"
                );
            }
        }

        // MON-4 (v2.6) — alert relay worker. Polls
        // ~/.local/share/mde/alerts/*.json for events
        // written by mde-alert-emit (MON-3) via Netdata's
        // health_alarm_notify.conf custom-sender hook + fires
        // an FDO desktop notification via notify-send per
        // new event. Deduplicates by deterministic ULID.
        // RestartPolicy::Always since the tick is passive +
        // operator outage detection is the failure-tolerance
        // goal.
        //
        // v6.0 Portal-1 — attach a PortalClient so CRITICAL
        // alerts also navigate Portal-full to the Control
        // (mesh-health) layer. Graceful-degrade: if the session
        // bus or mde-portal aren't running at daemon startup
        // the relay skips the portal call and surfaces the
        // FDO notification alone.
        // DBUS-2: the portal shell IPC is the Bus now. PortalClient is
        // stateless (it appends to action/shell/<verb> per call), so the
        // relay always attaches it — a CRITICAL alert's goto(control) is
        // durable even if mde-portal is down at the time.
        // E4.20 — the portal-era "navigate to Control on CRITICAL" publish was
        // dropped: alerts already surface via `notify-send` → notifyd → the Win10
        // Action Center, so the `action/shell/goto` Bus publish (whose only
        // consumer was the retired portal) is redundant.
        let alert_relay = mackesd_core::workers::alert_relay::AlertRelayWorker::new();
        tracing::info!(
            "alert_relay: PortalClient attached \
             (CRITICAL alerts publish action/shell/goto control)"
        );
        sup.spawn(Spawn::new(alert_relay, RestartPolicy::Always));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("alert_relay".into());

        // MESHFS-2.1 (v5.0.0) — LizardFS mesh-storage fleet supervisor.
        // Silent no-op when mfsmaster/mfschunkserver binaries are absent.
        sup.spawn(Spawn::new(
            mackesd_core::workers::meshfs_worker::MeshFsWorker::new()
                .with_qnm_peer_discovery(workgroup_root.clone(), node_id.clone()),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("meshfs_worker".into());

        // INST-11 + INST-12 + INST-13 (v2.7) — fleet upgrade-barrier
        // worker. Runs on every peer; silently no-ops until a
        // `mde-update --coordinate <ver>` writes an intent file into
        // `<mesh-home>/upgrade-intent/`. Then it runs `dnf upgrade
        // mde-core` on its own schedule, marks itself ready, fires
        // `mde-install --yes` once quorum + grace are met, and — when
        // it holds the leader lease — cleans up fully-complete intent
        // files after the +24h grace. No SQLite handle needed: the
        // barrier state lives in the GFS-replicated intent files and
        // the peer roster in the PEERVER peers dir.
        sup.spawn(Spawn::new(
            mackesd_core::workers::upgrade_intent_watcher::UpgradeIntentWatcher::new(
                workgroup_root.clone(),
                node_id.clone(),
            ),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("upgrade_intent_watcher".into());

        // PRINT-2..PRINT-6 + PRINT-8 (v5.0.0) — auto CUPS print
        // sharing + sync. Spawned on headless + full; SKIPPED on
        // lighthouse (routing-only, no printers — Q8 lock). The
        // profile is read from the installed-profile marker
        // `mde-install` writes; missing marker → assume a printing
        // profile (full/headless) and spawn. The worker itself is a
        // silent no-op without cups/lpadmin, so an over-spawn on a
        // box that happens to lack cups is harmless.
        let print_profile = std::fs::read_to_string("/var/lib/mde/installed-profile")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if print_profile != "lighthouse" {
            sup.spawn(Spawn::new(
                mackesd_core::workers::cups_sync::CupsSyncWorker::new(),
                RestartPolicy::Always,
            ));
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("cups_sync".into());
        } else {
            tracing::info!("cups_sync: skipped (lighthouse profile)");
        }

        // FWMON-2..4 (v5.0.0) — firewall-denied event monitor.
        // Reads kernel journal entries logged by firewalld's
        // LogDenied=all setting (enabled by birthright's
        // apply_firewall_log_denied step), filters overlay +
        // established traffic, appends denials to
        // <mesh-storage>/firewall/<host>.jsonl, and fires a Bus
        // alert when one source crosses the threshold.
        let fw_host = std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| node_id.clone());
        sup.spawn(Spawn::new(
            mackesd_core::workers::firewall_monitor::FirewallMonitorWorker::new(fw_host.clone()),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("firewall_monitor".into());

        // VIRT-1 (v5.0.0) — unified KVM + Podman compute inventory.
        // Polls virsh + podman every 10 s and publishes the per-peer
        // inventory to `compute/inventory/<peer-nebula-addr>` per
        // docs/design/v5.0.0-compute.md §3. Silent no-op on peers
        // without virsh/podman (lighthouse, container-stripped). The
        // nebula address is auto-detected from the local nebula1
        // interface at tick time (empty hint = runtime detect).
        sup.spawn(Spawn::new(
            mackesd_core::workers::compute_registry::ComputeRegistryWorker::new(
                fw_host.clone(),
                String::new(),
            ),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("compute_registry".into());

        // VIRT-5 (v5.0.0) — VM Nebula cert signing via Bus. Every peer
        // spawns the worker; only the CA peer (presence of
        // ~/.config/mde/nebula/ca.key) actually signs + replies, the
        // others advance the cursor silently. compute_provision
        // (VIRT-6) publishes to `action/compute/cert-sign-request`
        // and awaits the reply via rpc::await_reply with the 30 s
        // rpc::DEFAULT_RPC_TIMEOUT, retrying once before marking VM
        // creation failed (per VIRT-5 acceptance bullet 4).
        sup.spawn(Spawn::new(
            mackesd_core::workers::cert_authority::CertAuthorityWorker::new(),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("cert_authority".into());

        // VIRT-7 (v5.0.0) — per-network firewalld port forwarding.
        // Each peer subscribes to its own `compute/expose/<addr>` +
        // `compute/unexpose/<addr>` topics and applies firewall-cmd
        // rich rules per selected network. WAN zone is auto-detected
        // at startup via nmcli + firewall-cmd. Publishes the active
        // rule set to `compute/exposed/<addr>` for the Workbench.
        // Silent no-op on lighthouse / container-stripped peers
        // without firewall-cmd.
        sup.spawn(Spawn::new(
            mackesd_core::workers::compute_expose::ComputeExposeWorker::new(),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("compute_expose".into());

        // VIRT-8.a (v5.0.0) — cold VM migration source-side. Each
        // peer drains `action/compute/migrate`; when own nebula IP
        // == request.source_peer, runs the shutdown→rsync→publish
        // migrate-ready→undefine flow over the Nebula overlay.
        // Target-side handler (VIRT-8.b) ships with VIRT-6
        // compute_provision and subscribes to
        // `event/compute/migrate-ready`.
        sup.spawn(Spawn::new(
            mackesd_core::workers::compute_migrate::ComputeMigrateWorker::new(),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("compute_migrate".into());

        // VIRT-21 (v5.0.0) — compute_event_toast. Subscribes to every
        // compute/event/<peer> topic and raises an FDO desktop toast on
        // VM start/stop/crash so fleet lifecycle changes surface without
        // keeping mde-virtual open.
        sup.spawn(Spawn::new(
            mackesd_core::workers::compute_event_toast::ComputeEventToastWorker::new(),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("compute_event_toast".into());

        // VIRT-6 (v5.0.0) — compute_provision. Drains this peer's
        // `compute/create/<addr>` topic: ensures the mde-vms pool,
        // allocates a per-peer /24 VM IP, runs requester-side
        // nebula-cert keygen + the cert-sign RPC, builds the NoCloud
        // seed, virt-installs the VM (with virtiofs MeshFS share when
        // requested + mounted), acks on compute/create-ack/<ulid>, and
        // fires an immediate inventory publish. workgroup_root + node_id
        // locate this peer's nebula-bundle.json for the guest
        // lighthouse roster.
        sup.spawn(Spawn::new(
            mackesd_core::workers::compute_provision::ComputeProvisionWorker::new(
                fw_host.clone(),
                workgroup_root.clone(),
                node_id.clone(),
            ),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("compute_provision".into());

        // MESH-A-1 (v5.0.0) — per-peer network assessment. Collects
        // the 9 items (docs/design/v6.0-mde-portal.md §7.1) hourly +
        // writes ~/.local/share/mde/netassess/<host>/<iso>-<hash>.json
        // with a 30-day rolling trim. Shell-outs degrade to None when
        // a tool is absent (headless / air-gapped peers).
        if let Some(data_dir) = dirs::data_dir() {
            let netassess_base = data_dir.join("mde").join("netassess");
            sup.spawn(Spawn::new(
                mackesd_core::workers::netassess::NetAssessWorker::new(fw_host.clone(), netassess_base)
                    .with_mesh_context(workgroup_root.clone(), node_id.clone(), db_path.clone()),
                RestartPolicy::Always,
            ));
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("netassess".into());

            // MESH-A-4.c.2 (v5.0.0) — surrounding-host discovery worker.
            // Sweeps the LAN (mDNS + ARP-MAC + OUI) every 10 min and
            // writes a per-peer snapshot under
            // ~/.local/share/mde/surrounding/<host>/ (mesh-synced;
            // every peer reads the union per R8-Q13).
            let surrounding_base = data_dir.join("mde").join("surrounding");
            sup.spawn(Spawn::new(
                mackesd_core::workers::surrounding_worker::SurroundingWorker::new(
                    fw_host,
                    surrounding_base,
                ),
                RestartPolicy::Always,
            ));
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("surrounding_hosts".into());

            // MESH-A-5.2 (v5.0.0) — mesh-coordinated firewall DROP:
            // reconciles firewalld source-DROP rules against the
            // mesh-synced Blocked-host consensus every minute.
            sup.spawn(Spawn::new(
                mackesd_core::workers::mesh_firewall::MeshFirewallWorker::new(
                    data_dir.join("mde").join("surrounding"),
                ),
                RestartPolicy::Always,
            ));
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("mesh_firewall".into());

            // VOIP-4.b (v5.0.0) — broadcast this peer's Vitelity-link RTT to
            // voip/link-rtt/<peer> every 60s for the dialer route override.
            sup.spawn(Spawn::new(
                mackesd_core::workers::voip_rtt_worker::VoipRttWorker::new(),
                RestartPolicy::Always,
            ));
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("voip_rtt".into());
        } else {
            tracing::warn!("netassess: no XDG data dir; skipping network assessment worker");
        }

        // EPIC-MESH-PROBE (MESH-PROBE-4) — scheduled two-tier nmap
        // probe worker. Resolves mesh-peer overlay IPs, scans them
        // (fast 60s / deep 10min), writes this peer's
        // probe-inventory.json into mesh-home, and announces
        // probe/changed on the Bus when the inventory changes. The
        // `mackesd probe scan/refresh` CLI shares the same engine.
        sup.spawn(Spawn::new(
            mackesd_core::workers::probe::ProbeWorker::new(workgroup_root.clone(), node_id.clone()),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("probe".into());

        // MON-1.b (v2.6) — Netdata aggregator-IP publisher.
        // Pairs with `apply_netdata_monitor`'s baseline
        // /etc/netdata/netdata.conf: when this peer wins
        // leader-election it publishes its overlay IP to
        // QNM-Shared so every other peer picks the same
        // aggregator; on demote it stops publishing and the
        // freshest pointer wins. Every tick re-reads the
        // freshest pointer + rewrites the local netdata.conf
        // `[stream]` block + reloads netdata when the
        // aggregator IP changes. Fail-soft per the v2.6
        // design lock: missing aggregator strips the
        // `[stream]` block so netdata stays local-only with
        // the 7-day dbengine retention. API key defaults to
        // `mesh-${MDE_MESH_ID}-netdata` so every peer in the
        // same mesh shares the value automatically without
        // an extra wizard step (operators can override via
        // MDE_NETDATA_API_KEY if they want a custom value).
        match mackesd_core::store::open(&db_path) {
            Ok(conn) => {
                let netdata_store = Arc::new(tokio::sync::Mutex::new(conn));
                let mesh_id_for_netdata = std::env::var("MDE_MESH_ID")
                    .unwrap_or_else(|_| format!("mesh-{node_id}"));
                let api_key = std::env::var("MDE_NETDATA_API_KEY")
                    .unwrap_or_else(|_| format!("{mesh_id_for_netdata}-netdata"));
                sup.spawn(Spawn::new(
                    mackesd_core::workers::netdata_aggregator::NetdataAggregator::new(
                        netdata_store,
                        node_id.clone(),
                        workgroup_root.clone(),
                        api_key,
                    ),
                    RestartPolicy::Always,
                ));
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("netdata_aggregator".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    db_path = %db_path.display(),
                    "netdata_aggregator: sqlite open failed; worker skipped"
                );
            }
        }

        // PLANES-24 W63 — scheduled one-puller mirror sync. Every node writes
        // its dnf .repo to self-serve from the local file:// mount (W62); the
        // leader additionally pulls upstream + indexes, LizardFS replicating
        // the result. No DB handle needed — it works off the replicated root.
        sup.spawn(Spawn::new(
            mackesd_core::workers::mirror_syncd::MirrorSyncd::new(workgroup_root.clone()),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("mirror_syncd".into());

        // NF-1.5 (v2.5) — TCP/443 covert listener. Binds the
        // TLS 1.3 listener on :443 (default; env-overrideable),
        // spawns the per-stream demux pump per accepted peer
        // tunnel. Cert + key paths default to
        // /etc/nebula/lighthouse.{crt,key}; overridable via
        // MDE_HTTPS_TUNNEL_{CERT,KEY} env vars so operators
        // running Let's-Encrypt-issued certs can point to the
        // existing PEM chain. On peer-role boxes (no cert
        // files), the worker fails its bind + the supervisor's
        // OnFailure backoff effectively quarantines it.
        match mackesd_core::workers::nebula_https_listener::NebulaHttpsListener::new() {
            Ok(mut w) => {
                if let Ok(p) = std::env::var("MDE_HTTPS_TUNNEL_CERT") {
                    w = w.with_cert(PathBuf::from(p));
                }
                if let Ok(p) = std::env::var("MDE_HTTPS_TUNNEL_KEY") {
                    w = w.with_key(PathBuf::from(p));
                }
                if let Ok(addr) = std::env::var("MDE_HTTPS_TUNNEL_BIND") {
                    if let Ok(parsed) = addr.parse() {
                        w = w.with_bind_addr(parsed);
                    } else {
                        tracing::warn!(
                            value = %addr,
                            "nebula-https-listener: MDE_HTTPS_TUNNEL_BIND parse failed; using default",
                        );
                    }
                }
                // Bug 6 (2026-06-06) — only run the relay :443 listener when a
                // relay cert is actually present. A box with no lighthouse /
                // Let's-Encrypt cert is not a relay; spawning anyway only fails
                // the bind (and a per-user daemon can never bind a privileged
                // port at all), which the OnFailure policy then respins ~4x/s —
                // the worker's "backoff quarantines it" claim never held. Skip.
                let https_cert = std::env::var("MDE_HTTPS_TUNNEL_CERT").unwrap_or_else(|_| {
                    mackesd_core::workers::nebula_https_listener::DEFAULT_CERT_PATH.to_string()
                });
                if std::path::Path::new(&https_cert).exists() {
                    sup.spawn(Spawn::new(w, RestartPolicy::OnFailure));
                    worker_names
                        .lock()
                        .expect("worker_names mutex")
                        .push("nebula_https_listener".into());
                } else {
                    tracing::info!(
                        cert = %https_cert,
                        "nebula-https-listener: no relay cert present; not a relay — worker skipped",
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "nebula-https-listener: construction failed; skipped",
                );
            }
        }

        // v4.0.1 AF-NET-2 (2026-05-23) — mesh-latency sniffer.
        // Pings every enrolled non-local peer every 30 s and
        // writes the result to ~/.cache/mde/mesh-latency.json.
        // The WB-2.k.a Cairo topology canvas + panel Mesh-status
        // tray badge both consume the file. Best-choice
        // deviation from the TransportRegistry-routed approach
        // — see worker doc-comment.
        match mackesd_core::store::open(&db_path) {
            Ok(conn) => {
                let lat_store = Arc::new(tokio::sync::Mutex::new(conn));
                let cache =
                    mackesd_core::workers::mesh_latency::default_cache_path();
                sup.spawn(Spawn::new(
                    mackesd_core::workers::mesh_latency::MeshLatencyWorker::new(
                        lat_store,
                        node_id.clone(),
                        cache,
                    )
                    .with_interval(daemon_cfg.mesh_latency_sweep()),
                    RestartPolicy::OnFailure,
                ));
                worker_names.lock().expect("worker_names mutex").push("mesh_latency".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    db_path = %db_path.display(),
                    "mesh-latency: sqlite open failed; worker skipped"
                );
            }
        }

        // TUNE-16.d (2026-05-30) — Q22 8-peer cap counter. Reads the
        // enrolled peer count every 30 s, writes ~/.cache/mde/peer-cap.json,
        // and publishes to mesh/peer-cap/updated. Phones count (enrolled
        // as role='peer'); federated external-mesh peers don't appear in
        // the local store and are naturally excluded.
        match mackesd_core::store::open(&db_path) {
            Ok(conn) => {
                let cap_store = Arc::new(tokio::sync::Mutex::new(conn));
                let cap_cache = dirs::cache_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                    .join("mde")
                    .join("peer-cap.json");
                sup.spawn(Spawn::new(
                    mackesd_core::workers::peer_cap::PeerCapWorker::new(cap_store, cap_cache),
                    RestartPolicy::OnFailure,
                ));
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("peer-cap".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    db_path = %db_path.display(),
                    "peer-cap: sqlite open failed; worker skipped"
                );
            }
        }

        // v4.0.1 AF-* (2026-05-23) — register the
        // dev.mackes.MDE.Fleet.Files surface on the session bus
        // so mde-files's DBusBackend can read the live mesh
        // roster + per-peer file lists. Opens a second SQLite
        // handle for the IPC service (the reconcile worker
        // holds its own). The connection is leaked so its
        // tokio background tasks outlive run_serve.
        let host = std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| node_id.clone());

        // E0.3.1 (EPIC-RETIRE-DBUS, 2026-06-03) — Nebula status
        // Bus responder. The three read-projection verbs
        // (`status` / `self-node` / `list-peers`) migrated off the
        // retired `dev.mackes.MDE.Nebula.Status` D-Bus methods onto
        // the mesh Bus at `action/nebula/<verb>`. The responder
        // runs on its own OS thread with a current-thread tokio
        // runtime — the pure builders hold an
        // `Arc<Mutex<rusqlite::Connection>>` guard across `.await`,
        // which is `!Send` and would not compile on the main
        // multi-thread executor (same constraint mde-session's
        // serve_bus solved this way). It opens its own SQLite
        // handle + the per-peer Bus Persist index, loops until the
        // shutdown flag flips. Graceful-degrade: a missing data-dir
        // or a failed SQLite/Persist open logs + skips the thread
        // (the consumers fall back to their empty/diagnostic
        // rendering exactly as they did when the daemon was down).
        match mde_bus::default_data_dir()
            .ok_or_else(|| "no XDG data dir for bus".to_string())
            .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
        {
            Ok(persist) => match mackesd_core::store::open(&db_path) {
                Ok(conn) => {
                    let resp_store = Arc::new(tokio::sync::Mutex::new(conn));
                    let resp_svc = mackesd_core::ipc::nebula::NebulaStatusService::new(
                        Arc::clone(&resp_store),
                        node_id.clone(),
                        host.clone(),
                    )
                    .with_workgroup_root(workgroup_root.clone());
                    let resp_shutdown = Arc::clone(&shutdown);
                    std::thread::Builder::new()
                        .name("nebula-bus-responder".into())
                        .spawn(move || {
                            mackesd_core::ipc::nebula::serve_bus(&persist, &resp_svc, || {
                                resp_shutdown.load(Ordering::Relaxed)
                            });
                        })
                        .map(|_handle| {
                            tracing::info!(
                                "Nebula Bus responder spawned; serving \
                                 action/nebula/{{status,self-node,list-peers}}"
                            );
                        })
                        .unwrap_or_else(|e| {
                            tracing::warn!(
                                error = %e,
                                "Nebula Bus responder thread spawn failed; \
                                 NF-10..NF-18 consumers will see no peer data"
                            );
                        });
                    worker_names
                        .lock()
                        .expect("worker_names mutex")
                        .push("nebula_bus_responder".into());
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        db_path = %db_path.display(),
                        "Nebula Bus responder: sqlite open failed; responder skipped"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Nebula Bus responder: bus persist open failed; responder skipped"
                );
            }
        }
        // E0.3.5 — Shell control surface (version/healthz/workers) on
        // the mesh Bus at action/shell/<verb>, replacing the retired
        // dev.mackes.MDE.Shell D-Bus interface. Own OS thread
        // (Persist/rusqlite isn't Send); no tokio runtime needed since
        // the Shell builders are synchronous. Graceful-degrade: a
        // missing data-dir / failed Persist open logs + skips (the
        // Overview's mackesd-alive probe then reads offline).
        match mde_bus::default_data_dir()
            .ok_or_else(|| "no XDG data dir for bus".to_string())
            .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
        {
            Ok(persist) => {
                let shell_svc = mackesd_core::ipc::shell::ShellService::new(
                    mackesd_core::ipc::shell::ShellState {
                        db_path: db_path.clone(),
                        worker_names: Arc::clone(&worker_names),
                        // EFF-24 — live worker status → healthz readiness.
                        worker_status: Some(Arc::clone(&worker_status)),
                    },
                );
                let resp_shutdown = Arc::clone(&shutdown);
                std::thread::Builder::new()
                    .name("shell-bus-responder".into())
                    .spawn(move || {
                        mackesd_core::ipc::shell::serve_bus(&persist, &shell_svc, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_handle| {
                        tracing::info!(
                            "Shell Bus responder spawned; serving \
                             action/shell/{{version,healthz,workers}}"
                        );
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            error = %e,
                            "Shell Bus responder thread spawn failed; \
                             Overview mackesd-alive probe will read offline"
                        );
                    });
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("shell_bus_responder".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Shell Bus responder: bus persist open failed; responder skipped"
                );
            }
        }
        // E0.3.3 / FPG-4 — Fleet control surface (push/list/diff/
        // rollback/nudge) on the mesh Bus at action/fleet/<verb>,
        // replacing the retired dev.mackes.MDE.Fleet D-Bus interface.
        // The verbs are REAL (FPG-4): they run against the LizardFS
        // revision log via magic-fleet; any node serves + mints
        // (leaderless, FPG-3). Own OS thread (Persist/rusqlite isn't
        // Send); no tokio runtime (the responders are sync).
        match mde_bus::default_data_dir()
            .ok_or_else(|| "no XDG data dir for bus".to_string())
            .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
        {
            Ok(persist) => {
                // FPG-4 — the verbs run against the LizardFS revision
                // log; any node serves + mints (leaderless, FPG-3).
                let fleet_svc = mackesd_core::ipc::fleet::FleetService::new(
                    &workgroup_root,
                    node_id.clone(),
                );
                let resp_shutdown = Arc::clone(&shutdown);
                std::thread::Builder::new()
                    .name("fleet-bus-responder".into())
                    .spawn(move || {
                        mackesd_core::ipc::fleet::serve_bus(&persist, &fleet_svc, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_handle| {
                        tracing::info!(
                            "Fleet Bus responder spawned; serving \
                             action/fleet/{{push-revision,list-revisions,diff-revisions,rollback}} \
                             (FPG-4, LizardFS revision log)"
                        );
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "Fleet Bus responder thread spawn failed");
                    });
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("fleet_bus_responder".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Fleet Bus responder: bus persist open failed; responder skipped"
                );
            }
        }
        // PD-1 — the peer-directory responder: action/mesh/directory
        // answers with the joined per-peer record (presence tier,
        // health, version, overlay ip/role, revision currency). Same
        // dedicated-OS-thread shape as the fleet responder.
        match mde_bus::default_data_dir()
            .ok_or_else(|| "no XDG data dir for bus".to_string())
            .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
        {
            Ok(persist) => {
                let dir_svc = mackesd_core::ipc::directory::DirectoryService::new(
                    &workgroup_root,
                    Some(db_path.clone()),
                );
                let resp_shutdown = Arc::clone(&shutdown);
                std::thread::Builder::new()
                    .name("directory-bus-responder".into())
                    .spawn(move || {
                        mackesd_core::ipc::directory::serve_bus(&persist, &dir_svc, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_h| {
                        tracing::info!("Directory Bus responder spawned (action/mesh/directory, PD-1)");
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "Directory Bus responder thread spawn failed");
                    });
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("directory_bus_responder".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Directory Bus responder: bus persist open failed; responder skipped"
                );
            }
        }
        // PLANES-9/10 — the jobs control surface (action/jobs/*):
        // list-templates / launch / runs / run-results. Same
        // dedicated-OS-thread shape; the job_exec worker does the
        // actual local runs.
        match mde_bus::default_data_dir()
            .ok_or_else(|| "no XDG data dir for bus".to_string())
            .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
        {
            Ok(persist) => {
                let jobs_svc = mackesd_core::ipc::jobs::JobsService::new(
                    &workgroup_root,
                    Some(db_path.clone()),
                );
                let resp_shutdown = Arc::clone(&shutdown);
                std::thread::Builder::new()
                    .name("jobs-bus-responder".into())
                    .spawn(move || {
                        mackesd_core::ipc::jobs::serve_bus(&persist, &jobs_svc, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_h| {
                        tracing::info!("Jobs Bus responder spawned (action/jobs/*, PLANES-9)");
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "Jobs Bus responder thread spawn failed");
                    });
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("jobs_bus_responder".into());
            }
            Err(e) => {
                tracing::warn!(error = %e, "Jobs Bus responder: bus persist open failed; skipped");
            }
        }
        // E0.3.4 — Settings store on the mesh Bus at
        // action/settings/<verb> (get/set/list-keys/snapshot/restore;
        // args in the request body), replacing the never-registered
        // dev.mackes.MDE.Settings D-Bus interface. Registering it makes
        // the store genuinely reachable for the first time. Own OS
        // thread (Persist/rusqlite isn't Send); no tokio runtime (the
        // settings free fns are synchronous).
        match mde_bus::default_data_dir()
            .ok_or_else(|| "no XDG data dir for bus".to_string())
            .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
        {
            Ok(persist) => {
                let settings_svc = mackesd_core::ipc::settings::SettingsService;
                let resp_shutdown = Arc::clone(&shutdown);
                std::thread::Builder::new()
                    .name("settings-bus-responder".into())
                    .spawn(move || {
                        mackesd_core::ipc::settings::serve_bus(&persist, &settings_svc, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_handle| {
                        tracing::info!(
                            "Settings Bus responder spawned; serving \
                             action/settings/{{get,set,list-keys,snapshot,restore}}"
                        );
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "Settings Bus responder thread spawn failed");
                    });
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("settings_bus_responder".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Settings Bus responder: bus persist open failed; responder skipped"
                );
            }
        }
        // E0.3.1.b — the Nebula signal dispatcher drains worker
        // NebulaSignal events onto the Bus event topic
        // (event/nebula/signals) + fills nebula_signal_slot so the
        // health_reconciler + nebula_csr_watcher workers pick up the
        // sender on their next tick. Relocated out of the retired
        // Fleet.Files D-Bus arm — it never depended on that connection.
        let _nebula_sender =
            mackesd_core::ipc::nebula::spawn_signal_dispatcher(&nebula_signal_slot);
        tracing::info!(
            "Nebula signal dispatcher spawned (Bus event topic {}); \
             health_reconciler + nebula_csr_watcher will emit on next \
             state transition",
            mackesd_core::ipc::nebula::NEBULA_EVENT_TOPIC,
        );

        // E0.3.2 — the five file-transfer surfaces moved off D-Bus onto
        // the mesh Bus: Fleet.Files (the live, store-backed mesh roster)
        // + the four Shell.* stubs (Inbox/Outbox/Downloads/
        // FileOperations — honest empty / transport-not-configured until
        // a future epic fills the transfer engine). One dedicated
        // responder thread serves all five over its own Persist
        // (rusqlite isn't Send); Fleet.Files locks the shared store via
        // blocking_lock on this non-async thread. Replaces
        // register_fleet_files + the session D-Bus connection (Shell +
        // Nebula already moved off it, so no D-Bus interface registers
        // anywhere now).
        match mde_bus::default_data_dir()
            .ok_or_else(|| "no XDG data dir for bus".to_string())
            .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
        {
            Ok(persist) => {
                use mackesd_core::ipc::files;
                // AUD-1/AUD-7 — the real cross-mesh transport over the
                // LizardFS-replicated QNM-Shared volume. One `FileXfer` per
                // surface (cheap: just a root path + host id) backs inbox /
                // outbox / file-ops with genuine copy/list/rollback.
                // EFF-2 — `FileXfer::new` confines send-to sources to the
                // operator's home dir (the share root), so a Bus writer
                // can't exfil /etc/shadow / keys into a peer's inbox.
                let qnm_root = mackesd_core::default_qnm_shared_root();
                let xfer_inbox = files::FileXfer::new(qnm_root.clone(), host.clone());
                let xfer_outbox = files::FileXfer::new(qnm_root.clone(), host.clone());
                let xfer_ops = files::FileXfer::new(qnm_root.clone(), host.clone());
                let mut surfaces = vec![
                    files::Surface {
                        prefix: files::INBOX_PREFIX,
                        verbs: &files::INBOX_VERBS,
                        reply: Box::new(move |verb, body| xfer_inbox.inbox_reply(verb, body)),
                    },
                    files::Surface {
                        prefix: files::OUTBOX_PREFIX,
                        verbs: &files::OUTBOX_VERBS,
                        reply: Box::new(move |verb, body| xfer_outbox.outbox_reply(verb, body)),
                    },
                    files::Surface {
                        prefix: files::DOWNLOADS_PREFIX,
                        verbs: &files::DOWNLOADS_VERBS,
                        reply: Box::new(files::downloads_reply),
                    },
                    files::Surface {
                        prefix: files::FILE_OPS_PREFIX,
                        verbs: &files::FILE_OPS_VERBS,
                        reply: Box::new(move |verb, body| xfer_ops.file_ops_reply(verb, body)),
                    },
                ];
                // Fleet.Files joins only when sqlite opens; its stub
                // siblings serve regardless.
                match mackesd_core::store::open(&db_path) {
                    Ok(conn) => {
                        let store = Arc::new(tokio::sync::Mutex::new(conn));
                        let svc =
                            files::FleetFilesService::new(store, host.clone(), node_id.clone());
                        surfaces.push(files::Surface {
                            prefix: files::FLEET_FILES_PREFIX,
                            verbs: &files::FLEET_FILES_VERBS,
                            reply: Box::new(move |verb, body| svc.reply(verb, body)),
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            db_path = %db_path.display(),
                            "Fleet.Files: sqlite open failed; mesh-roster surface \
                             omitted (the four stub surfaces still serve)"
                        );
                    }
                }
                let resp_shutdown = Arc::clone(&shutdown);
                std::thread::Builder::new()
                    .name("files-bus-responder".into())
                    .spawn(move || {
                        files::serve_all(&persist, &surfaces, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_handle| {
                        tracing::info!(
                            "Files Bus responder spawned; serving action/{{files-inbox,\
                             files-outbox,files-downloads,file-ops,fleet-files}}/*"
                        );
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "Files Bus responder thread spawn failed");
                    });
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("files_bus_responder".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Files Bus responder: bus persist open failed; responder skipped"
                );
            }
        }

        // v4.0.1 KDC2-3.3 wire-up (2026-05-23) — spawn the KDC host
        // worker. Owns the pairing store at $XDG_CONFIG_HOME/mde/
        // connect (default ~/.config/mde/connect), the shared
        // DiscoveryRegistry, the outbound packet queue, and the
        // dev.mackes.MDE.Connect D-Bus surface. Graceful-degrade
        // on D-Bus failure — the worker keeps the host alive so
        // the mesh-router can still dispatch through KDC, even if
        // the operator-facing UI methods aren't reachable.
        let kdc_config_dir = {
            let xdg = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from);
            let home_default = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|h| h.join(".config"));
            xdg.or(home_default)
                .map(|p| p.join("mde").join("connect"))
                .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/mde/connect"))
        };
        if mackesd_core::worker_role::runs("kdc_host", role_rank) {
            sup.spawn(Spawn::new(
                mackesd_core::workers::kdc_host::KdcHostWorker::new(kdc_config_dir),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("kdc_host".into());
        }

        // BUS-1.1 (v6.x Mackes Bus) — supervise the `mde-bus` daemon
        // subprocess. Gracefully degrades when the binary is absent
        // (dev box, RPM not yet installed) — the worker loops on a
        // 30s tick waiting for the binary to appear. Once the BUS-1
        // sub-epic ships, every mackesd peer carries the bus.
        sup.spawn(Spawn::new(
            mackesd_core::workers::bus_supervisor::BusSupervisor::new(),
            RestartPolicy::Always,
        ));
        worker_names.lock().expect("worker_names mutex").push("bus_supervisor".into());

        // BUS-5.1 — clipboard daemon. Spawns one `mde-clipd` process per
        // Wayland session. Idles gracefully when $WAYLAND_DISPLAY is unset
        // (e.g., early in the boot sequence or on a headless peer).
        if mackesd_core::worker_role::runs("clipd_supervisor", role_rank) {
            sup.spawn(Spawn::new(
                mackesd_core::workers::clipd_supervisor::ClipdSupervisor::new(),
                RestartPolicy::Always,
            ));
            worker_names.lock().expect("worker_names mutex").push("clipd_supervisor".into());
        }

        // TUNE-3.b (2026-05-26) — wire the v1.3.0 Fleet ansible-pull
        // worker. `crates/mackesd/src/workers/ansible_pull.rs::build`
        // has shipped since v2.0.0 Phase B.6 but stayed dead;
        // [[project_v1_3_0_fleet]] keeps the feature in scope so
        // wiring is the right cleanup. The worker invokes
        // `ansible-pull -U <MDE_ANSIBLE_PULL_URL> -i localhost,` on
        // a 15-min cadence (matches the retired
        // `mackes-ansible-pull.timer`). With MDE_ANSIBLE_PULL_URL
        // unset the ansible-pull binary fails fast + the supervisor
        // logs the error — the worker stays cheap to host.
        // Bug 6 (2026-06-06) — without MDE_ANSIBLE_PULL_URL the worker only spawns
        // `ansible-pull` to fail; a box with no fleet config-pull URL has nothing
        // to do, so skip rather than respawn-on-failure into a periodic WARN.
        let ansible_configured = std::env::var("MDE_ANSIBLE_PULL_URL")
            .map(|u| !u.is_empty())
            .unwrap_or(false);
        if mackesd_core::worker_role::runs("ansible-pull", role_rank) {
            if ansible_configured {
                sup.spawn(Spawn::new(
                    mackesd_core::workers::ansible_pull::build(),
                    RestartPolicy::OnFailure,
                ));
                worker_names.lock().expect("worker_names mutex").push("ansible-pull".into());
            } else {
                tracing::info!(
                    "ansible-pull: MDE_ANSIBLE_PULL_URL unset; fleet config-pull worker skipped"
                );
            }
        }

        // EPIC-SYNC-APP-CONFIG (Q26, 2026-05-28) — app-config sync is
        // now a native-Rust worker (`workers::app_sync`); it discovers
        // mesh media servers + writes Sublime Music / Delfin configs +
        // the `~/Mackes Media/` launcher view directly, retiring the
        // `python3 -m mackes.media_sync_daemon` subprocess (advances
        // §11 #6). `OnFailure` keeps the 60 s tick alive across a
        // transient write/probe error.
        if mackesd_core::worker_role::runs("app-sync", role_rank) {
            sup.spawn(Spawn::new(
                mackesd_core::workers::app_sync::build(),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("app-sync".into());
        }
        // remmina-sync is a native Rust tick worker (RETIRE-PY.2): every 60 s
        // it reads the mesh peer registry, TCP-probes SSH/RDP/VNC, and
        // reconciles Remmina's "Mesh Peers" group. No `python3` is spawned.
        if mackesd_core::worker_role::runs("remmina-sync", role_rank) {
            sup.spawn(Spawn::new(
                mackesd_core::workers::remmina_sync::build(),
                RestartPolicy::OnFailure,
            ));
            worker_names.lock().expect("worker_names mutex").push("remmina-sync".into());
        }

        // The reconcile worker runs on its own OS thread (kept on
        // std::thread so its sync rusqlite calls don't block the
        // tokio scheduler). Still surfaced via Shell.Workers so
        // the operator sees the legacy worker alongside the async
        // supervisor children.
        worker_names.lock().expect("worker_names mutex").push("reconcile".into());
        let reconcile = mackesd_core::worker::spawn_reconcile_worker(
            workgroup_root,
            node_id,
            db_path,
            Arc::clone(&shutdown),
        );

        // Watch loop: wake every 250 ms to check the shutdown flag.
        // When it flips, drop out so reconcile.join() can wait for
        // the worker to finish its current tick. The async
        // supervisor's workers see shutdown via the SIGTERM signal
        // handler installed above (mackesd_core::workers::ShutdownToken
        // wraps the same broadcast channel).
        while !shutdown.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if reconcile.is_finished() {
                tracing::warn!(
                    "mackesd serve: reconcile worker exited without \
                     a shutdown request"
                );
                shutdown.store(true, Ordering::Relaxed);
                break;
            }
        }
        tracing::info!("mackesd serve: shutdown requested; joining workers");
        // Tell every async worker to stop, then drain their joins.
        let outcomes = sup.shutdown_and_join().await?;
        for (name, outcome) in &outcomes {
            match outcome {
                Ok(()) => tracing::info!(worker = %name, "joined clean"),
                Err(e) => tracing::warn!(worker = %name, error = ?e, "joined with error"),
            }
        }
        if let Err(e) = reconcile.join() {
            tracing::error!("reconcile worker panicked: {e:?}");
            return Err(anyhow::anyhow!("reconcile worker panicked"));
        }
        tracing::info!("mackesd serve: all workers joined; exit");
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

/// Render a fixed-width inventory table to stdout. Columns:
/// kind / mesh? / size / mtime (ISO-8601 UTC) / path. We pad to the
/// widest cell in each column so the output stays grep-able.
fn print_inventory_table(artifacts: &[mackesd_core::legacy_inventory::LegacyArtifact]) {
    if artifacts.is_empty() {
        println!("(no legacy artifacts found)");
        return;
    }
    let mut rows: Vec<[String; 5]> = Vec::with_capacity(artifacts.len() + 1);
    rows.push([
        "KIND".to_owned(),
        "MESH".to_owned(),
        "SIZE".to_owned(),
        "MTIME (UTC)".to_owned(),
        "PATH".to_owned(),
    ]);
    for a in artifacts {
        rows.push([
            format!("{:?}", a.artifact_kind),
            if a.mesh_data {
                "yes".to_owned()
            } else {
                "no".to_owned()
            },
            format_size(a.size_bytes),
            format_mtime(a.mtime_ms),
            a.path.display().to_string(),
        ]);
    }
    let mut widths = [0usize; 5];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    for row in &rows {
        println!(
            "{:<w0$}  {:<w1$}  {:>w2$}  {:<w3$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        );
    }
}

/// Render a byte count as a short human-friendly string (binary
/// prefixes — KiB / MiB / GiB).
fn format_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", n / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", n / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GiB", n / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Render an mtime (ms since epoch) as an ISO-8601 UTC timestamp.
/// Falls back to `-` when chrono refuses the value.
fn format_mtime(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).map_or_else(
        || "-".to_owned(),
        |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

/// Build the JSON `peers why` report from a node roster (Phase
/// 12.4.4). Pure function over the store projection so callers can
/// unit-test the reason-chain shape without a real DB.
fn explain_peer(node_id: &str, nodes: &[mackesd_core::store::NodeRow]) -> serde_json::Value {
    let subject = nodes.iter().find(|n| n.node_id == node_id);
    let Some(subject) = subject else {
        return serde_json::json!({
            "node":     node_id,
            "known":    false,
            "reasons":  [],
            "note":     "node id not present in store — run `mackesd inventory-legacy` and `mackesd import-legacy` to seed.",
        });
    };
    let healthy_subject = subject.health == "healthy";
    let reasons: Vec<serde_json::Value> = nodes
        .iter()
        .filter(|other| other.node_id != node_id)
        .map(|other| {
            let same_region = match (&subject.region, &other.region) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            let both_healthy = healthy_subject && other.health == "healthy";
            let chain: Vec<&str> = {
                let mut v = Vec::new();
                if both_healthy {
                    v.push("both peers healthy");
                } else {
                    v.push("one or both peers not healthy");
                }
                if same_region {
                    v.push("same region — east-west allowed by default");
                } else {
                    v.push("different regions — gated on policy::allow_east_west");
                }
                if subject.role == "decommissioned" || other.role == "decommissioned" {
                    v.push("decommissioned — no edge expected");
                }
                v
            };
            serde_json::json!({
                "peer":       other.node_id,
                // An edge is expected when both peers are healthy and
                // neither is decommissioned. East-west (cross-region)
                // is allowed by default today, so region does NOT gate
                // `expected` (the `reasons` above still surface the
                // region context). The previous `&& (same_region ||
                // true)` term was always true — a logic bug (clippy
                // overly_complex_bool_expr); a real
                // `policy::allow_east_west` gate would re-add a
                // `(same_region || allow_east_west)` term here.
                "expected":   both_healthy
                              && subject.role != "decommissioned"
                              && other.role != "decommissioned",
                "chain":      chain,
            })
        })
        .collect();
    serde_json::json!({
        "node":    node_id,
        "known":   true,
        "region":  subject.region,
        "role":    subject.role,
        "health":  subject.health,
        "reasons": reasons,
    })
}

/// Heuristic: extract peer name candidates from a list of legacy
/// artifacts (Phase 12.13.2). Pure helper so the importer's "what
/// would I insert" question has a single source of truth that's
/// unit-testable without disk I/O.
fn derive_legacy_node_names(
    artifacts: &[&mackesd_core::legacy_inventory::LegacyArtifact],
) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut out = BTreeSet::new();
    for a in artifacts {
        // Filenames like `peer:anvil.json` or directories named after
        // peers (`~/QNM-Shared/anvil/...`) reveal candidate names.
        let path_str = a.path.display().to_string();
        for token in path_str.split(['/', '\\', '_', '.']) {
            if let Some(rest) = token.strip_prefix("peer:") {
                if !rest.is_empty() && rest.chars().all(legacy_name_char) {
                    out.insert(rest.to_owned());
                }
            }
        }
        // Also harvest the top-level directory under QNM-Shared
        // (`~/QNM-Shared/<peer>/...`).
        if path_str.contains("QNM-Shared") {
            if let Some(idx) = path_str.find("QNM-Shared/") {
                let after = &path_str[idx + "QNM-Shared/".len()..];
                if let Some(seg) = after.split('/').next() {
                    if !seg.is_empty() && seg.chars().all(legacy_name_char) {
                        out.insert(seg.to_owned());
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

fn legacy_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Resolve the stable node id from `$MACKESD_NODE_ID` then
/// `$HOSTNAME` then the `hostname` syscall, falling back to
/// `peer:unknown` so the audit-log column is never empty.
/// VV-2 helper — load `VoiceDesired` from the operator's JSON
/// override file at `desired_json`, falling back to
/// `boot_default(node_id)` when the file is absent or `force_boot`
/// is set.
///
/// `force_boot=true` is the explicit `--boot-default` CLI flag —
/// useful for testing the bootstrap path without removing the
/// override file. A missing override file is the steady-state on a
/// fresh peer (no voice policies have been approved yet), so it's
/// a silent fall-through rather than a hard error. Parse errors
/// on a present file *are* hard errors — the operator's
/// hand-edited / worker-written file is bad and we should not
/// silently fall back to defaults that hide the bug.
fn load_voice_desired(
    desired_json: &std::path::Path,
    force_boot: bool,
    node_id: &str,
) -> anyhow::Result<mde_voice_config::VoiceDesired> {
    if force_boot {
        return Ok(mde_voice_config::VoiceDesired::boot_default(node_id));
    }
    match std::fs::read_to_string(desired_json) {
        Ok(body) => serde_json::from_str(&body).map_err(|e| {
            anyhow::anyhow!("voice render-config: parse {}: {e}", desired_json.display())
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(mde_voice_config::VoiceDesired::boot_default(node_id))
        }
        Err(e) => Err(anyhow::anyhow!(
            "voice render-config: read {}: {e}",
            desired_json.display()
        )),
    }
}

/// VV-1 helper — atomic write-and-rename of the generated voice
/// configs. The directory is `mkdir -p`'d; each file is written
/// to a hidden `.tmp` sibling and renamed into place so a
/// partial render never leaves Kamailio / `RTPengine` reading a
/// half-written file.
fn write_voice_config_files(
    out_dir: &std::path::Path,
    files: &[(&str, &String)],
) -> anyhow::Result<()> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| anyhow::anyhow!("voice render-config: mkdir {}: {e}", out_dir.display()))?;
    for (name, body) in files {
        let final_path = out_dir.join(name);
        let tmp_path = out_dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp_path, body.as_bytes()).map_err(|e| {
            anyhow::anyhow!("voice render-config: write {}: {e}", tmp_path.display())
        })?;
        std::fs::rename(&tmp_path, &final_path).map_err(|e| {
            anyhow::anyhow!(
                "voice render-config: rename {} → {}: {e}",
                tmp_path.display(),
                final_path.display()
            )
        })?;
    }
    Ok(())
}

/// ONBOARD-4 — detect the primary outbound IPv4 by opening a UDP
/// socket "to" a public address and reading back the kernel-chosen
/// source IP. No packets are sent (UDP connect only sets the route);
/// works offline as long as a default route exists. Behind NAT this
/// is the LAN IP — operators pass `--external-addr <public-ip>`
/// explicitly when the lighthouse sits behind NAT.
fn detect_primary_ipv4() -> anyhow::Result<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| anyhow::anyhow!("bind probe socket: {e}"))?;
    // 198.51.100.1 is TEST-NET-2 (RFC 5737) — never routed, but the
    // connect still resolves the default-route source address.
    sock.connect("198.51.100.1:9")
        .map_err(|e| anyhow::anyhow!("no default route to detect a primary IP: {e}"))?;
    let local = sock
        .local_addr()
        .map_err(|e| anyhow::anyhow!("read local addr: {e}"))?;
    Ok(local.ip().to_string())
}

/// ONBOARD-4 — the `found` verb. One-command founding lighthouse:
/// mesh-init + `/enroll` endpoint identity + a v3 join line.
fn cmd_found(
    db_path: &std::path::Path,
    mesh_id: &str,
    external_addr: &str,
    role: &str,
    enroll_port: Option<u16>,
) -> anyhow::Result<()> {
    use mackesd_core::nebula_enroll_endpoint::{generate_endpoint_identity, DEFAULT_ENROLL_PORT};
    use mackesd_core::workers::nebula_enroll_listener::{DEFAULT_CERT_PATH, DEFAULT_KEY_PATH};

    let parsed: mde_role::Role = role.parse().map_err(|_| {
        anyhow::anyhow!("unknown role `{role}` — expected lighthouse|server|workstation")
    })?;
    // Resolve the externally-dialable IPv4 (strip any :port the operator
    // included; `auto` detects the primary outbound IP).
    let ip = if external_addr.eq_ignore_ascii_case("auto") {
        detect_primary_ipv4()?
    } else {
        external_addr
            .rsplit_once(':')
            .map_or(external_addr, |(host, _)| host)
            .to_string()
    };
    let enroll_port = enroll_port.unwrap_or(DEFAULT_ENROLL_PORT);

    let conn = mackesd_core::store::open(db_path)
        .with_context(|| format!("opening store at {}", db_path.display()))?;
    mackesd_core::store::migrate(&conn).context("migrating store")?;
    let root = mackesd_core::default_qnm_shared_root();
    let node_id = default_node_id();

    // Generate + persist the self-signed `/enroll` endpoint identity
    // BEFORE printing the token (the token pins its fingerprint). The
    // key lands at 0600.
    let identity = generate_endpoint_identity(&[ip.clone()])
        .map_err(|e| anyhow::anyhow!("generating /enroll endpoint identity: {e}"))?;
    let cert_path = std::path::Path::new(DEFAULT_CERT_PATH);
    if let Some(dir) = cert_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(cert_path, identity.cert_pem.as_bytes())
        .with_context(|| format!("writing {DEFAULT_CERT_PATH}"))?;
    std::fs::write(DEFAULT_KEY_PATH, identity.key_pem.as_bytes())
        .with_context(|| format!("writing {DEFAULT_KEY_PATH}"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(DEFAULT_KEY_PATH, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {DEFAULT_KEY_PATH}"))?;
    }

    // mesh-init: pin role, mint CA, self-sign, write the founding bundle,
    // and mint the first single-use bearer.
    let report = mackesd_core::mesh_init::mesh_init(
        &mackesd_core::ca::SubprocessBackend,
        &conn,
        &root,
        &node_id,
        mesh_id,
        &format!("{ip}:4242"),
        std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.crt"),
        std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.key"),
        std::path::Path::new("/var/lib/mackesd/nebula-ca/scratch"),
        parsed,
    )?;

    // Re-express mesh-init's freshly-minted bearer as a v3 token that
    // points at the PUBLIC ip + enroll port and pins the endpoint fp.
    let legacy = mackesd_core::nebula_enroll::parse_join_token(&report.join_token)
        .ok_or_else(|| anyhow::anyhow!("mesh-init returned an unparseable join token"))?;
    let v3 = mackesd_core::nebula_enroll::JoinToken {
        mesh_id: mesh_id.to_string(),
        lighthouse: ip.clone(),
        port: enroll_port,
        bearer: legacy.bearer,
        fp: Some(identity.fingerprint.clone()),
    };
    let join_token = v3.encode();

    // Bring the node fully live + boot-durable: enable+start the overlay, the
    // worker daemon (activates the /enroll listener), and the health watchdog.
    // `enable` makes each start at boot independently — nebula.service ships
    // disabled, and was previously only `start`ed, so a reboot left the overlay
    // down until the supervisor happened to revive it (ONBOARD-9).
    enable_now_service("nebula.service");
    enable_now_service("mackesd.service");
    enable_now_service("mesh-health.timer");

    println!(
        "mesh `{}` founded — lighthouse {} ({})",
        report.mesh_id, node_id, report.overlay_ip
    );
    if let Some(r) = &report.pinned_role {
        println!("role pinned: {r}");
    }
    println!(
        "/enroll endpoint: https://{ip}:{enroll_port}  (cert fp {})",
        identity.fingerprint
    );
    println!("bundle: {}", report.bundle_path.display());
    println!("services: nebula + mackesd + mesh-health enabled (boot-durable) and running");
    println!("\nAdd a peer — run this on the joining box:\n  mackesd join '{join_token}'");
    Ok(())
}

/// ONBOARD-4 — the `join` verb. One-command peer join: pin role +
/// fingerprint-pinned network-enroll + materialize /etc/nebula.
fn cmd_join(
    token: Option<String>,
    role: &str,
    name: Option<String>,
    workgroup_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    // No token → hand off to the enrollment TUI (ONBOARD-5, `mde-enroll`).
    let Some(raw_token) = token else {
        let launched = std::process::Command::new("mde-enroll").status();
        return match launched {
            Ok(s) if s.success() => Ok(()),
            _ => Err(anyhow::anyhow!(
                "no token given and the `mde-enroll` TUI isn't on PATH. \
                 Pass the token from `mackesd found`:\n  mackesd join '<token>'"
            )),
        };
    };

    let parsed: mde_role::Role = role.parse().map_err(|_| {
        anyhow::anyhow!("unknown role `{role}` — expected lighthouse|server|workstation")
    })?;
    let token = mackesd_core::nebula_enroll::parse_join_token(&raw_token).ok_or_else(|| {
        anyhow::anyhow!("invalid join token (expected mesh:<id>@<ip>:<port>#<bearer>?fp=<sha256>)")
    })?;
    let root = workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let node_id = default_node_id();
    let display_name = name.unwrap_or_else(|| {
        node_id
            .strip_prefix("peer:")
            .unwrap_or(&node_id)
            .to_string()
    });

    // Pin the role when unpinned (an already-pinned box keeps its role).
    match mde_role::load() {
        Ok(existing) => println!("role already pinned: {existing}"),
        Err(mde_role::LoadError::NotPinned) => {
            mde_role::pin(parsed).map_err(|e| anyhow::anyhow!("pinning role: {e}"))?;
            println!("role pinned: {}", parsed.as_str());
        }
        Err(e) => anyhow::bail!("reading role: {e}"),
    }

    if token.fp.is_none() {
        // No fingerprint → legacy co-located QNM-Shared flow (the network
        // path requires the pinned fp). Honest fallback, not an error.
        println!("token has no fingerprint — using the co-located QNM-Shared enroll flow");
        let outcome = mackesd_core::nebula_enroll::enroll_with_token(
            &root,
            &node_id,
            &display_name,
            &raw_token,
        )
        .map_err(|e| anyhow::anyhow!("enroll: {e}"))?;
        println!(
            "enrolled into `{}` as {} (waited {:?})",
            outcome.mesh_id, outcome.overlay_ip, outcome.waited
        );
        return Ok(());
    }

    // Network enroll (the MESH-1 fix) — runs on a small async runtime.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building async runtime for network enroll")?;
    let config_dir = std::path::PathBuf::from("/etc/nebula");
    let bundle = runtime.block_on(mackesd_core::nebula_enroll_client::network_enroll(
        &root,
        &config_dir,
        &node_id,
        &display_name,
        token,
    ))?;

    // Bring the peer fully live + boot-durable (ONBOARD-9): the overlay, the
    // worker daemon, and the health watchdog — not just nebula. A `join` now
    // leaves a node that survives reboot and self-recovers, instead of one the
    // operator must `systemctl restart mackesd` by hand.
    enable_now_service("nebula.service");
    enable_now_service("mackesd.service");
    enable_now_service("mesh-health.timer");

    println!(
        "joined `{}` as {} (overlay {})",
        bundle.mesh_id, node_id, bundle.overlay_ip
    );
    println!("services: nebula + mackesd + mesh-health enabled (boot-durable) and running");
    Ok(())
}

/// ONBOARD-4/9 — enable + start a systemd unit so it is live now AND comes up
/// automatically on every boot. Best-effort: a container/dev env without
/// systemd just no-ops (the daemon also self-heals via the supervisor).
fn enable_now_service(name: &str) {
    let _ = std::process::Command::new("systemctl")
        .args(["enable", "--now", name])
        .status();
}

fn default_node_id() -> String {
    if let Ok(v) = std::env::var("MACKESD_NODE_ID") {
        return v;
    }
    let host = std::env::var("HOSTNAME").ok().or_else(|| {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_owned())
    });
    match host {
        Some(h) if !h.is_empty() => format!("peer:{h}"),
        _ => "peer:unknown".to_owned(),
    }
}

/// Register a SIGTERM + SIGINT handler that flips `shutdown` to
/// true. Uses `signal-hook`'s safe `Signals` iterator API — a
/// background thread reads from the kernel-managed signal queue
/// and stores into the shared atomic. No `unsafe` required (the
/// workspace forbids `unsafe_code`).
///
/// The reader thread is daemon-style: it lives as long as the
/// process and exits naturally when the process exits. Since
/// `mackesd reconcile` returns from main only after the reconcile
/// worker joins, we don't need to track the reader's handle.
fn install_signal_handlers(
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<()> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;
    let mut signals =
        Signals::new([SIGTERM, SIGINT]).context("installing SIGTERM/SIGINT iterator")?;
    std::thread::Builder::new()
        .name("mackesd-signal".into())
        .spawn(move || {
            for sig in &mut signals {
                tracing::info!(signal = sig, "received shutdown signal");
                shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
                // Keep reading so a second signal doesn't terminate
                // the process before the worker drains.
            }
        })
        .context("spawning signal-reader thread")?;
    Ok(())
}
