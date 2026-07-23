//! `mackesd` — CLI entry point for the Mesh control plane.
//!
//! Subcommands land alongside their backing Phase 12 substeps. Today
//! only `mackesd migrate` ships (Phase 12.2 store + migrations); the
//! rest follow as substeps complete. We deliberately do NOT register
//! stub commands here — every `mackesd X` either does X or is absent.

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[path = "../cli/mod.rs"]
mod cli;

#[path = "mackesd/spawn.rs"]
mod spawn;
use spawn::*;

#[derive(Parser)]
#[command(
    name = "mackesd",
    // QBRAND-1 — `--version` prints the single baked build-identity line
    // (version · git hash · date · channel) from the brand crate, not a bare
    // `CARGO_PKG_VERSION`. Shared verbatim with `mde-shell-egui --version`.
    // `full_static` is the memoized `&'static str` clap's `Str` requires.
    version = mde_theme::brand::build::full_static(),
    about = "MCNF control plane — secure no-fixed-center workgroup mesh on Fedora-Cosmic"
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

    /// MESHFS-1 — print the Mesh-Sync (`/mnt/mesh-storage`) storage status as a
    /// JSON line. The verb was deleted with the LizardFS plane (SUBSTRATE-6) but
    /// the Workbench Mesh Storage panel + `mde-files` still shell it, so it is
    /// restored Syncthing-native: this node's `df` (used/avail) as one peer plus
    /// the goal/quota fields both GUIs read. Per-peer aggregation (MESHFS-2) and
    /// Syncthing completion (MESHFS-3) build on this.
    MeshFsStatus,

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

    /// ROUTE-TRACE-1 — assemble + print the logical path (a PathGraph) between
    /// two endpoints, the CLI parity for `action/route/trace`. Built from the
    /// CONNECT exposure policy + the peer directory on the shared substrate.
    RouteTrace {
        /// Destination: a published service id (ingress) or an external host/IP
        /// (egress).
        #[arg(long = "to", value_name = "DEST")]
        to: String,
        /// Source mesh node (egress direction).
        #[arg(long = "from", value_name = "NODE", default_value = "")]
        from: String,
        /// `ingress` (default) or `egress`.
        #[arg(long = "direction", value_name = "DIR", default_value = "ingress")]
        direction: String,
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
        /// MEDIA-1 — pin the `Lighthouse_Media` subclass: the §9 media
        /// capability tag on top of the role. Only valid on `lighthouse`
        /// (a media tag on server/workstation is dropped — it is a lighthouse
        /// subclass). The Navidrome music worker gates to this class.
        #[arg(long)]
        media: bool,
    },

    /// OW-2 — node onboarding engine: the headless verbs both onboarding
    /// front-ends drive (the egui first-run wizard + the TUI enroll flow). Today
    /// two verbs land here — `self-test` (node self-diagnostic) and
    /// `role-provision` (apply a role's systemd unit set); the complex verbs
    /// (mesh-create/invite/enroll/mesh-dns/network) arrive with OW-3..OW-6.
    Onboard {
        #[command(subcommand)]
        verb: OnboardCmd,
    },

    /// MV-7 — day-2 adopt an existing XCP-ng host into this mesh: enroll its dom0
    /// as a static Nebula member (not a role) and drive its XAPI toolstack via
    /// `xe`/tofu (as the live farm does), so it serves VMs to the mesh. Needs a
    /// founded mesh (a CA to sign the member) + a resolvable host credential;
    /// otherwise it stays blocked (retry available). The live enroll + xe/tofu
    /// apply is integration-gated behind the Adopter seam; `--dry-run` prints the
    /// plan + ordered steps without touching the host.
    AdoptXcp {
        /// The XCP-ng pool master / dom0 address (`ip` or host, e.g. `172.20.0.9`).
        #[arg(long, value_name = "ADDR")]
        pool_address: String,
        /// The overlay IP the dom0 takes as a static Nebula member (e.g. `10.42.0.9`).
        #[arg(long, value_name = "IP")]
        overlay_ip: String,
        /// A handle to the host root credential (e.g. `secret:xcp-host`), never the
        /// secret itself — resolved at apply time.
        #[arg(long, value_name = "REF", default_value = "secret:xcp-adopt")]
        credential_ref: String,
        /// Print the plan + ordered steps without enrolling / driving the toolstack.
        #[arg(long)]
        dry_run: bool,
    },

    /// OW-13 — recover a reinstalled/replaced box: plan its FRESH re-enroll into the
    /// mesh and report the OLD identity's passive-revocation status. Short-TTL certs
    /// mean the old cert self-lapses (no CRL, no key-backup) and the current cert
    /// auto-renews before its lead-time cliff. Needs a fresh join token/invite to
    /// re-enroll (else blocked, retry available). The live re-enroll is
    /// integration-gated behind the RecoveryApply seam; `--dry-run` prints the plan +
    /// ordered steps + status without enrolling; `--evict` additionally records the
    /// old identity into the ENT-3 blocklist for immediate (vs passive) removal.
    Recovery {
        /// The node id being recovered (defaults to this box's id).
        #[arg(long, value_name = "ID")]
        node_id: Option<String>,
        /// A fresh join token/invite to re-enroll with. Absent ⇒ the plan is blocked
        /// (mint one on a lighthouse with `mackesd onboard invite-issue`, then retry).
        #[arg(long, value_name = "TOKEN")]
        token: Option<String>,
        /// Print the plan + ordered steps + passive-revocation status without enrolling.
        #[arg(long)]
        dry_run: bool,
        /// Immediately evict the old identity into the ENT-3 blocklist (reuse
        /// `ca::blocklist`) instead of waiting for its short TTL to lapse.
        #[arg(long)]
        evict: bool,
    },

    /// LIGHTHOUSE-10 — set this lighthouse's PUBLIC underlay address
    /// (`ip` or `ip:port`, port defaults to 4242). Persisted so the
    /// heartbeat publishes it to the directory and every node's enroll
    /// roster includes this lighthouse (full redundancy). `found`/`join`
    /// auto-detect it; use this to correct a misdetected/NAT'd address.
    SetExternalAddr {
        /// The public `ip` or `ip:port` peers dial (e.g. `203.0.113.7` or `203.0.113.7:4242`).
        addr: String,
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
    /// refuses to start: desktop units require rank 1 (Workstation).
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

    /// DDNS-EGRESS-3 — manage the `[ddns]` dynamic-DNS config (CLI parity for the
    /// `action/ddns/*` RPCs: get/set config, add/remove a record, query a record's
    /// live reconcile status). Calls the SAME `ipc::ddns::build_reply` the bus
    /// responder serves, so the CLI and the GUI act on one config.
    Ddns {
        #[command(subcommand)]
        action: DdnsCmd,
    },

    /// EFF-28 / MESHFS-14.1 — restore the Nebula CA from an armored
    /// `state-backup.enc` bundle. CA rows go straight into the local
    /// SQLite store via `ca::backup::restore_to_store`.
    StateRestore {
        /// Path to the armored `state-backup.enc` bundle.
        bundle: std::path::PathBuf,
        /// EFF-28 — dry-run: decode + unseal + report the bundle's
        /// contents WITHOUT touching the store. Exit 0 = the bundle is
        /// restorable with this passphrase. Use in DR drills + before a
        /// real restore.
        #[clap(long)]
        verify: bool,
        /// Passphrase env-var. Defaults to
        /// `MDE_BACKUP_PASSPHRASE` (same as the daily backup
        /// worker's env).
        #[clap(long, default_value = "MDE_BACKUP_PASSPHRASE")]
        passphrase_env: String,
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

    /// QC-15 — audit a CONSTRUCT-CLOUD cutover node for retired VM-stack leftovers
    /// and Q58 fresh-VM rebuild evidence. Exits nonzero when any check fails.
    CutoverAudit {
        /// Repository root to inspect for old-stack artifacts.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
        /// Operator ledger proving every pre-cutover VM was rebuilt fresh, or
        /// that no pre-cutover VMs existed on this node.
        #[arg(
            long,
            default_value = mackesd_core::cutover_audit::DEFAULT_VM_REBUILD_LEDGER
        )]
        vm_rebuild_ledger: PathBuf,
        /// Emit JSON instead of a short table.
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
        /// Role to pin when unpinned (lighthouse|workstation).
        #[arg(long, default_value = "lighthouse")]
        role: String,
    },

    /// ENT-1 — mint a single-use 256-bit enrollment bearer on this
    /// lighthouse and print the join token a new box runs
    /// `mackesd join <…>` with. The ledger records only the
    /// bearer's hash; the raw value is shown once, here.
    EnrollToken {
        /// Mesh id to embed in the token (e.g. `home-mesh`).
        #[arg(long)]
        mesh_id: String,
        /// Lighthouse address the joining box dials. Defaults to this
        /// lighthouse's published PUBLIC address (`set-external-addr`),
        /// or the detected primary IPv4 — NEVER the overlay IP (a
        /// not-yet-enrolled node can't reach it; DAR-19). Always paired
        /// with the dedicated `/enroll` HTTPS port, not the one given here.
        #[arg(long)]
        lighthouse: Option<String>,
        /// Operator note recorded beside the issued hash.
        #[arg(long, default_value = "")]
        note: String,
    },

    /// SETUP-4/5 — mint a single-use **v3** join token for a new peer (or a
    /// second/third lighthouse) on THIS lighthouse, pinning the `/enroll`
    /// endpoint cert fingerprint so the joining box can use the network path.
    /// mesh-id is read from the local founding bundle. Prints the
    /// ready-to-paste token + a `magic-setup`/`mackesd join` line.
    AddPeer {
        /// Role the new peer will pin (lighthouse|workstation).
        #[arg(long, default_value = "workstation")]
        role: String,
        /// Operator note recorded beside the issued bearer hash.
        #[arg(long, default_value = "")]
        note: String,
        /// Public address the joining box dials (`ip` or `ip:port`).
        /// Defaults to this lighthouse's detected primary IPv4.
        #[arg(long)]
        lighthouse: Option<String>,
        /// Override the `/enroll` HTTPS port the token advertises.
        #[arg(long)]
        enroll_port: Option<u16>,
    },

    /// SETUP-5 — remove a peer: decommission its directory row, revoke its
    /// cert, and ban its node-id from re-enrolling. The inverse of `add-peer`.
    RemovePeer {
        /// The peer's node-id (`peer:<host>`), as shown by `mackesd peers`.
        node_id: String,
        /// Proceed even when the peer is unreachable.
        #[arg(long)]
        force: bool,
    },

    /// DATACENTER-3 — seal/read a leader-managed mesh secret. `put` reads the
    /// plaintext on stdin and age-encrypts it into the store; `get` decrypts to
    /// stdout. `--local` forces the Syncthing-replicated LocalAead store (so a
    /// repo node can seal a secret — e.g. `media-spaces` — that the lighthouses
    /// then read via their own LocalAead store, keyed by the shared mesh age
    /// identity). Used to provision MEDIA / DR / VPN credentials without hand-
    /// editing per-node files.
    Secret {
        #[command(subcommand)]
        cmd: SecretCmd,
    },

    /// DAR-2 — seal arbitrary bytes from stdin under the canonical Argon2id +
    /// XChaCha20-Poly1305 envelope (`ca::backup::seal_bytes`), emitting the
    /// ASCII-armored bundle on stdout. This is the ONE passphrase-sealed-blob
    /// path the DR CA/identity bundle uses (DAR-42); it is NOT the control-VM
    /// secret-zero path (that is on-VM age keygen + re-seal, no passphrase).
    /// Reuses the audited KDF/AEAD — no new crypto. The passphrase is read from
    /// a `--passphrase-file` (a 0600 file, never argv/env so it can't leak via
    /// `/proc/<pid>/{cmdline,environ}` to a child or `ps`).
    SecretSeal {
        /// Path to a file whose FIRST LINE is the passphrase (trailing newline
        /// stripped). 0600-recommended; never argv/env. Required — an empty
        /// passphrase is rejected by the envelope.
        #[arg(long, value_name = "PATH")]
        passphrase_file: PathBuf,
    },

    /// DAR-2 — inverse of `secret-seal`: read the ASCII-armored bundle from
    /// stdin, de-armor + unseal with the passphrase from `--passphrase-file`,
    /// and write the exact original plaintext bytes to stdout. A wrong or empty
    /// passphrase (or a tampered bundle) is rejected with the existing AEAD
    /// error; no partial/garbage output is emitted on failure.
    SecretUnseal {
        /// Path to the passphrase file (see `secret-seal`).
        #[arg(long, value_name = "PATH")]
        passphrase_file: PathBuf,
    },

    /// SETUP-7 — re-apply the steady-state convergence playbook
    /// (`/etc/mackesd/site.yml`) locally via `ansible-playbook`. The playbook
    /// is generated at `found`/`join`; this restores role/services/mount
    /// idempotently. No-op (with a hint) when ansible or the playbook is absent.
    Converge {
        /// Override the playbook path (default `/etc/mackesd/site.yml`).
        #[arg(long)]
        site: Option<PathBuf>,
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
        /// Role to pin when unpinned (lighthouse|workstation).
        #[arg(long, default_value = "lighthouse")]
        role: String,
        /// Override the `/enroll` HTTPS port the token advertises.
        #[arg(long)]
        enroll_port: Option<u16>,
        /// DAR-18 (DEVOPS-AUTOMATION-REBUILD, Lock 3) — opt in to the DevOps
        /// backoffice at genesis. Bare `--with-backoffice` = `minimal`;
        /// `--with-backoffice=full` adds CI + reconciler + build-farm + DR.
        /// ABSENT (default) = OFF: found is byte-for-byte unchanged.
        ///
        /// This flag is INTENT-RECORDING + non-destructive: when set, found runs
        /// the full existing flow, then records `/mcnf/backoffice/intent
        /// {tier,host,ts}` to etcd and PRINTS the gated next command
        /// (`backoffice-up.sh --tier <t>`). It does NOT itself provision the
        /// control VM, run `tofu apply`, or spend money — those stay
        /// operator-gated on the control VM.
        #[arg(
            long,
            value_name = "TIER",
            num_args = 0..=1,
            default_missing_value = "minimal"
        )]
        with_backoffice: Option<String>,
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
        /// Role to pin when unpinned (lighthouse|workstation).
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

    /// #13 — **turn-key lighthouse lifecycle.** `add` provisions a DigitalOcean
    /// droplet that JOINS this mesh as a full lighthouse (CA signer + etcd voter,
    /// no manual `etcdctl`/`scp`); `retire` drains it (holding the HA floor),
    /// removes it from the etcd quorum + revokes its cert, then deletes the
    /// droplet. Run on an existing lighthouse.
    Lighthouse {
        #[command(subcommand)]
        cmd: LighthouseCmd,
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
        /// Role for --set (lighthouse|workstation).
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
    /// upstream into the mirror dir on the share, `createrepo_c` the metadata,
    /// then stamp `.last-sync` (Syncthing replicates it to every node).
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
    /// the versioned builds present on the share (W55). The Provisioning
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
    /// every row from the `nodes` table as a JSON array; GUI
    /// inventory surfaces consume the same shape. Without `--json`
    /// the command prints a human-readable table.
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

    /// FILEMGR-6 — provision / re-key the shared mesh SSH key (sshfs auth over
    /// the overlay). The keypair is sealed in the mesh secret store under
    /// `mesh-ssh-key` (the ref the FILEMGR-5 mesh-mount worker reads); the public
    /// half installs for the mesh user behind an overlay-only sshd Match block.
    MeshSshKey {
        #[command(subcommand)]
        cmd: MeshSshKeyCmd,
    },

    /// TRANSFERS-1 — drive the daemon's `transfers` queue (§9 CLI parity for the
    /// `transfer.submit/cancel/pause/resume/list` verbs). Submit/cancel/pause/resume
    /// hand a typed verb to the running daemon through the node-local inbox; list
    /// reads the persistent ledger directly. The same store the daemon uses, so the
    /// CLI and the GUI share one queue.
    Transfer {
        #[command(subcommand)]
        cmd: TransferCmd,
    },
}

/// TRANSFERS-1 — `mackesd transfer <sub>`: the CLI half of the typed verb set.
#[derive(Subcommand)]
enum TransferCmd {
    /// Enqueue a new transfer (`transfer.submit`). Mints a Queued job + hands it to
    /// the daemon; prints the new job id.
    Submit {
        /// Where the bytes come from (a path, a URL, a `host:path`, or a peer —
        /// the lane parses it per method).
        #[arg(long, value_name = "SRC")]
        source: String,
        /// Where the bytes land.
        #[arg(long, value_name = "DEST")]
        dest: String,
        /// The protocol lane: sftp | rsync | http | browser-download | node | music.
        #[arg(long, value_name = "METHOD")]
        method: String,
        /// Optional per-job bandwidth cap (Q12 — passed to the tool: `rsync
        /// --bwlimit` / `wget --limit-rate`, e.g. `2m`).
        #[arg(long, value_name = "RATE")]
        bwlimit: Option<String>,
        /// Verify integrity on completion (Q15 — size + checksum; a mismatch is a
        /// failure, not a silent pass).
        #[arg(long)]
        verify: bool,
    },
    /// List every job in the ledger (`transfer.list`). `--json` for the raw records.
    List {
        /// Emit the ledger as a JSON array instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List auto-discovered transfer destinations from the mesh node-state plane.
    Destinations {
        /// Emit destination rows as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Cancel a job (`transfer.cancel`) — removes it + frees any slot it held.
    Cancel {
        /// The job id (from `submit` / `list`).
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Pause a Queued/Running job (`transfer.pause`).
    Pause {
        /// The job id.
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Resume a Paused job (`transfer.resume`).
    Resume {
        /// The job id.
        #[arg(value_name = "ID")]
        id: String,
    },
}

/// FILEMGR-6 — `mackesd mesh-ssh-key <sub>`: the shared-key lifecycle
/// (provision / install / rotate / status). The re-key path is `rotate`.
#[derive(Subcommand)]
enum MeshSshKeyCmd {
    /// Provision the shared key: generate + seal it if absent (idempotent), then
    /// install the public half + the overlay-only sshd drop-in on THIS node.
    Provision(MeshSshKeyArgs),
    /// Install the already-sealed key's public half + the sshd drop-in on THIS
    /// node (no generation). Fails honestly if nothing is sealed yet.
    Install(MeshSshKeyArgs),
    /// Re-key (revoke + rotate): generate a fresh keypair, reseal (revoking the
    /// old private half), and re-install so the old public key drops.
    Rotate(MeshSshKeyArgs),
    /// Print whether the shared key is sealed + its installed public line.
    Status(MeshSshKeyArgs),
}

/// Shared flags for the `mesh-ssh-key` verbs.
#[derive(clap::Args)]
struct MeshSshKeyArgs {
    /// Repo root holding `automation/secrets/mcnf-secret.sh` (defaults to
    /// `MCNF_REPO` / `/root/magic-mesh`) — selects the etcd-backed Mesh store
    /// when present, else the local-AEAD fallback under the workgroup root.
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Workgroup root for the local-AEAD fallback store (defaults to
    /// `MDE_WORKGROUP_ROOT` / the canonical mesh mount).
    #[arg(long)]
    workgroup_root: Option<PathBuf>,
    /// Override the mesh SSH login user the key authorizes (default `root`).
    #[arg(long)]
    mesh_user: Option<String>,
    /// Skip the live `systemctl reload sshd` — write the config only (the
    /// activation stays deploy-gated). Also the default off-node behaviour.
    #[arg(long)]
    no_reload: bool,
}

/// #13 — `mackesd lighthouse <sub>` subcommands: the turn-key add/retire lifecycle.
#[derive(Subcommand)]
enum SecretCmd {
    /// Seal the plaintext read from stdin under `<name>` in the mesh secret store.
    Put {
        /// The secret name/ref (e.g. `media-spaces`), as the readers reference it.
        name: String,
        /// Force the Syncthing-replicated LocalAead store under the workgroup root
        /// (vs. auto-resolving to the etcd-backed Mesh store when the repo helper
        /// is present). Use on a repo node to seal a secret the lighthouses read.
        #[arg(long)]
        local: bool,
    },
    /// Decrypt the secret stored under `<name>` to stdout (exit 3 if absent).
    Get {
        /// The secret name/ref to read.
        name: String,
        /// Force the LocalAead store (see `put --local`).
        #[arg(long)]
        local: bool,
    },
}

#[derive(clap::Subcommand)]
enum LighthouseCmd {
    /// Provision a DigitalOcean droplet that JOINS this mesh as a full lighthouse:
    /// mint a role-scoped lighthouse token here, then shell the join provisioner
    /// (`do-lighthouse-join.sh`) whose cloud-init runs `mackesd join --role
    /// lighthouse`. The daemon auto-joins the etcd quorum (#11), the CA key is
    /// delivered (#12), and the supervisor reconcile flips it to am_lighthouse.
    Add {
        /// DigitalOcean region slug (e.g. `nyc3`, `sfo3`, `fra1`).
        #[arg(long)]
        region: String,
        /// DO droplet size slug (default: the provisioner's `s-1vcpu-512mb-10gb`).
        #[arg(long)]
        size: Option<String>,
        /// DO image slug (default: the provisioner's `fedora-43-x64`).
        #[arg(long)]
        image: Option<String>,
    },
    /// Retire a lighthouse: drain-gate (hold the HA floor unless `--force`), remove
    /// it from the etcd quorum + revoke/ban its cert, then delete the droplet.
    Retire {
        /// The lighthouse's node-id (`peer:<host>`), as shown by `mackesd peers`.
        node_id: String,
        /// DigitalOcean droplet id to delete after draining (via `doctl`).
        #[arg(long)]
        droplet_id: Option<String>,
        /// Proceed even if retiring breaches the HA floor / the node is unreachable.
        #[arg(long)]
        force: bool,
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

/// OW-2 — `mackesd onboard <verb>` subcommands. The engine lives in
/// `mackesd_core::onboard` so the egui + TUI onboarding front-ends call the same
/// code these CLI verbs do.
#[derive(Subcommand)]
enum OnboardCmd {
    /// OW-2 — node self-diagnostic: KVM stack readiness (the KVM_SERVICES
    /// catalog), the mesh peer directory, and identity + CA presence. Prints a
    /// human report (or `--json`) and exits non-zero when a *critical* check
    /// fails (missing identity / unreadable directory).
    SelfTest {
        /// Emit the report as a single JSON line instead of text.
        #[arg(long)]
        json: bool,
    },
    /// OW-2 — apply a deployment role's systemd unit set: enable the units the
    /// role runs, mask the ones it does not. Idempotent. `--dry-run` prints the
    /// plan without touching systemd.
    RoleProvision {
        /// `lighthouse` | `workstation`.
        #[arg(long)]
        role: String,
        /// Print the planned enable/mask actions without applying them.
        #[arg(long)]
        dry_run: bool,
    },
    /// OW-3 — found this Workstation's mesh: mint the Nebula CA + bring up a
    /// LAN-only overlay (no lighthouse, no internet) so a lone box is a working
    /// mesh-of-one. Idempotent — re-running on an already-founded node is a safe
    /// no-op. `--label <friendly>` cosmetically names the mesh.
    MeshCreate {
        /// Optional friendly label folded into the mesh-id (cosmetic).
        #[arg(long)]
        label: Option<String>,
    },
    /// OW-4 — mint a short-TTL, mesh-scoped join invite on THIS enrolled node: an
    /// authenticated bearer (recorded in the bearer ledger so it can be verified +
    /// revoked), emitted as a typeable code + a QR string a new box presents to
    /// join. `--ttl` overrides the default lifetime (minutes).
    InviteIssue {
        /// Invite lifetime in minutes (default: 15). Kept short — a join code is
        /// meant to be presented promptly.
        #[arg(long)]
        ttl: Option<u64>,
    },
    /// OW-5 — bring up the primary LAN interface *before* the overlay: detect
    /// DHCP-vs-static (reusing `router_discovery` for the default gateway) and write
    /// the NetworkManager keyfile, so a fresh box reaches its LAN even on a
    /// static-only, no-DHCP network. Idempotent. `--dry-run` prints the plan + the
    /// keyfile it would write without touching NetworkManager.
    Network {
        /// Print the plan + rendered keyfile without writing it / reloading NM.
        #[arg(long)]
        dry_run: bool,
    },
    /// OW-6 — publish this mesh's DNS: fold the replicated peer roster into a
    /// `<host>.<mesh-id>` → overlay-IP zone and write the managed `/etc/hosts`
    /// block, so nodes are reachable by name over the overlay instead of by
    /// Nebula IP. Idempotent. `--dry-run` prints the zone + the rendered block
    /// without touching `/etc/hosts`.
    MeshDns {
        /// Print the built zone + the rendered hosts block without writing
        /// `/etc/hosts`.
        #[arg(long)]
        dry_run: bool,
    },
    /// OW-7 / QC-15 — spawn this mesh's first lighthouse and migrate the CA to it:
    /// provision a cloud droplet, push-enroll it as a lighthouse, then migrate the
    /// CA over #12's lighthouse-scoped-bearer key delivery. With no cloud token the
    /// mesh stays LAN-only (retry available). The live provision/SSH/CA-move is
    /// integration-gated behind the Provisioner seam; `--dry-run` prints the plan +
    /// the provision spec without provisioning.
    SpawnLighthouse {
        /// Provision a PAIR (two lighthouses) for an HA / two-voter quorum.
        #[arg(long)]
        pair: bool,
        /// Print the plan + provision spec without provisioning / migrating.
        #[arg(long)]
        dry_run: bool,
    },
    /// OW-8 / QC-15 — bring up this Workstation's FIRST cloud-backed VM desktop:
    /// select a VM image from the mesh image catalog, place it through the VDI
    /// broker's Nova desktop path, and open a broker session the shell's Desktop
    /// surface renders. A desktop VM already present ⇒ reconnect (offer it, not a
    /// duplicate); no VM image ⇒ a real no-image outcome (see Services ▸ Images).
    /// The live placement/session path is integration-gated behind the
    /// FirstDesktopApply seam; `--dry-run` prints the plan + ordered steps without
    /// creating anything.
    FirstDesktop {
        /// Print the plan + ordered steps without placing / opening.
        #[arg(long)]
        dry_run: bool,
    },
    /// OW-11 — add a curated back-office service (Music / Files / Voice) as a
    /// separate day-2 Services flow that never blocks the working network (#20).
    /// Music provisions Navidrome on a media-lighthouse (DO Spaces); Files is P2P
    /// Send-To (no VM); Voice registers to an external SIP provider. The live
    /// Navidrome provision / SIP register is integration-gated behind the
    /// ServiceApply seam; `--dry-run` prints the plan + ordered steps.
    ServiceAdd {
        /// Which service to add: `music`, `files`, or `voice`.
        kind: String,
        /// Voice only: the external SIP registrar host (e.g. `sip.provider.net`).
        #[arg(long)]
        sip_registrar: Option<String>,
        /// Voice only: the SIP domain (defaults to the registrar when omitted).
        #[arg(long)]
        sip_domain: Option<String>,
        /// Voice only: the SIP account username.
        #[arg(long)]
        sip_username: Option<String>,
        /// Print the plan + ordered steps without provisioning / registering.
        #[arg(long)]
        dry_run: bool,
    },
}

/// DDNS-EGRESS-3 — `mackesd ddns <sub>` subcommands. Each calls the same
/// `mackesd_core::ipc::ddns::build_reply` verb the `action/ddns/*` bus responder
/// serves (CLI/GUI parity over one config), rooted at the shared workgroup root.
#[derive(Subcommand)]
enum DdnsCmd {
    /// Print the current `[ddns]` config as JSON (`get-config`).
    GetConfig {
        /// Mesh-home root (defaults to `$MDE_WORKGROUP_ROOT` / the system mount).
        #[clap(long, env = "MDE_WORKGROUP_ROOT")]
        workgroup_root: Option<PathBuf>,
    },
    /// Replace the whole `[ddns]` config from a JSON body (`set-config`).
    SetConfig {
        /// The `DdnsConfig` JSON (e.g. `{"enabled":true,"token_ref":"secret:do-token",…}`).
        #[clap(value_name = "JSON")]
        config: String,
        #[clap(long, env = "MDE_WORKGROUP_ROOT")]
        workgroup_root: Option<PathBuf>,
    },
    /// Add or update one managed record (`add-record`). Upserts by name template.
    AddRecord {
        /// Name template, e.g. `{node}-{provider}`.
        #[clap(value_name = "NAME")]
        name: String,
        /// IP source: a `tunnel:<id>` or `wan`.
        #[clap(value_name = "SOURCE")]
        source: String,
        /// Kill-switch policy when the source is down: `remove` | `sentinel` | `keep`.
        #[clap(long, default_value = "remove")]
        on_down: String,
        #[clap(long, env = "MDE_WORKGROUP_ROOT")]
        workgroup_root: Option<PathBuf>,
    },
    /// Remove a managed record by its name template (`remove-record`).
    RemoveRecord {
        /// The record name template to remove.
        #[clap(value_name = "NAME")]
        name: String,
        #[clap(long, env = "MDE_WORKGROUP_ROOT")]
        workgroup_root: Option<PathBuf>,
    },
    /// Query a record's live reconcile decision + reachability (`record-status`):
    /// given the live source state, print the planned DdnsAction + the
    /// reachability flag (DDNS-EGRESS-4).
    Status {
        /// The record name template.
        #[clap(value_name = "NAME")]
        name: String,
        /// Live source state: `up` (with `--ip`) or `down`.
        #[clap(long, default_value = "up")]
        state: String,
        /// The verified exit/WAN IP for an `up` state.
        #[clap(long, default_value = "")]
        ip: String,
        /// Whether the up source has an inbound port-forward (reachability flag).
        #[clap(long)]
        port_forward: bool,
        /// Whether the down source is kill-switched (leak-coupling).
        #[clap(long)]
        kill_switch: bool,
        /// The last-published value (omit ⇒ never published).
        #[clap(long, default_value = "")]
        last: String,
        #[clap(long, env = "MDE_WORKGROUP_ROOT")]
        workgroup_root: Option<PathBuf>,
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
    // PERF-8: default deps to WARN but keep mackesd's own lifecycle (start/converge/
    // exit) at INFO — so a fleet of idle daemons isn't a journald firehose while
    // real daemon events stay visible. RUST_LOG still overrides (checked first).
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,mackesd=info"));
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
        Cmd::Migrate => cli::migrate::run(db_path)?,
        Cmd::Status => cli::status::run(db_path)?,
        Cmd::Healthz => cli::healthz::run(db_path)?,
        Cmd::MeshFsStatus => cli::mesh_fs_status::run()?,
        Cmd::Connect { ip, port } => cli::connect::run(ip, port)?,
        Cmd::ClassifyHost {
            mdns,
            port,
            vendor,
            hostname,
            mac,
        } => cli::classify_host::run(mdns, port, vendor, hostname, mac)?,
        Cmd::DiscoverMdns => cli::discover_mdns::run()?,
        Cmd::SurroundingList => cli::surrounding_list::run()?,
        Cmd::SurroundingTrust { key, state } => cli::surrounding_trust::run(key, state)?,
        Cmd::MeshFirewallPlan => cli::mesh_firewall_plan::run()?,
        Cmd::ArpSpoofCheck => cli::arp_spoof_check::run()?,
        Cmd::RogueDhcpCheck => cli::rogue_dhcp_check::run()?,
        Cmd::CaptivePortalCheck => cli::captive_portal_check::run()?,
        Cmd::VoipRtt => cli::voip_rtt::run()?,
        Cmd::Tag { host, set } => cli::tag::run(host, set)?,
        Cmd::HopAdvertise { subnets, exit } => cli::hop_advertise::run(subnets, exit)?,
        Cmd::VpnImport { name, kind, file } => cli::vpn_import::run(name, kind, file)?,
        Cmd::RolePin { role, media } => cli::role_pin::run(role, media)?,
        Cmd::SetExternalAddr { addr } => cli::set_external_addr::run(addr)?,
        Cmd::RoleWorkers { role } => cli::role_workers::run(role)?,
        Cmd::RoleGate { min_rank } => cli::role_gate::run(min_rank)?,
        Cmd::Onboard { verb } => cli::onboard::run(verb, db_path)?,
        Cmd::AdoptXcp {
            pool_address,
            overlay_ip,
            credential_ref,
            dry_run,
        } => cli::adopt_xcp::run(pool_address, overlay_ip, credential_ref, dry_run)?,
        Cmd::Recovery {
            node_id,
            token,
            dry_run,
            evict,
        } => cli::recovery::run(node_id, token, dry_run, evict, db_path)?,
        Cmd::DnsLeakCheck { expected } => cli::dns_leak_check::run(expected)?,
        Cmd::EvilTwinCheck => cli::evil_twin_check::run()?,
        Cmd::RecordAttack { source } => cli::record_attack::run(source)?,
        Cmd::AuditLog { event, detail } => cli::audit_log::run(event, detail)?,
        Cmd::DiscoverMdePeers => cli::discover_mde_peers::run()?,
        Cmd::Probe { action } => cli::probe::run(action)?,
        Cmd::Ddns { action } => cli::ddns::run(action)?,
        Cmd::PresetLaunch { tag } => cli::preset_launch::run(tag)?,
        Cmd::StateRestore {
            bundle,
            verify,
            passphrase_env,
        } => cli::state_restore::run(bundle, verify, passphrase_env, db_path)?,
        Cmd::GeneratePasscode { store, cred_path } => {
            cli::generate_passcode::run(store, cred_path)?
        }
        Cmd::LogEmit {
            level,
            target,
            message,
        } => cli::log_emit::run(level, target, message)?,
        Cmd::RouteTrace {
            to,
            from,
            direction,
        } => cli::route_trace::run(to, from, direction)?,
        Cmd::FleetStatus { json } => cli::fleet_status::run(json, db_path)?,
        Cmd::Identity { json } => cli::identity::run(json)?,
        Cmd::AuditVerify { json } => cli::audit_verify::run(json, db_path)?,
        Cmd::CutoverAudit {
            repo_root,
            vm_rebuild_ledger,
            json,
        } => cli::cutover_audit::run(repo_root, vm_rebuild_ledger, json)?,
        Cmd::RotatePasscode { store, cred_path } => cli::rotate_passcode::run(store, cred_path)?,
        Cmd::ShowPasscode { cred_path } => cli::show_passcode::run(cred_path)?,
        Cmd::PeersWhy { node_id } => cli::peers_why::run(node_id, db_path)?,
        Cmd::Apply { dry_run } => cli::apply::run(dry_run)?,
        Cmd::Enroll {
            passcode,
            passcode_stdin,
            token,
            token_stdin,
            name,
            workgroup_root,
        } => cli::enroll::run(
            passcode,
            passcode_stdin,
            token,
            token_stdin,
            name,
            workgroup_root,
        )?,
        Cmd::Decommission { node_id, force } => cli::decommission::run(node_id, force, db_path)?,
        Cmd::Reenroll { node_id } => cli::reenroll::run(node_id, db_path)?,
        Cmd::TakeLeadership { as_node } => cli::take_leadership::run(as_node)?,
        Cmd::ImportLegacy { dry_run } => cli::import_legacy::run(dry_run, db_path)?,
        Cmd::Reconcile {
            once,
            workgroup_root,
            node_id,
        } => cli::reconcile::run(once, workgroup_root, node_id, db_path)?,
        Cmd::InventoryLegacy { mesh_only, json } => cli::inventory_legacy::run(mesh_only, json)?,
        Cmd::Serve {
            workgroup_root,
            node_id,
        } => {
            // v2.0.0 Phase B.12 — unified meta-daemon entry point.
            // Boots the tokio runtime, registers the worker pool +
            // the existing reconcile worker, blocks on SIGTERM.
            run_serve(workgroup_root, node_id, db_path)?;
        }
        Cmd::Ca { sub } => cli::ca::run(sub, db_path)?,
        Cmd::Nebula { sub } => cli::nebula::run(sub, db_path)?,
        Cmd::WakePeer {
            mac,
            broadcast,
            via_lighthouse,
            port,
        } => cli::wake_peer::run(mac, broadcast, via_lighthouse, port)?,
        Cmd::FleetPushSetting {
            key,
            value,
            peers,
            author,
            dry_run,
        } => cli::fleet_push_setting::run(key, value, peers, author, dry_run, db_path)?,
        Cmd::Revisions { cmd } => cli::revisions::run(cmd, db_path)?,
        Cmd::Leave { yes } => cli::leave::run(yes)?,
        Cmd::MeshInit {
            mesh_id,
            external_addr,
            role,
        } => cli::mesh_init::run(mesh_id, external_addr, role, db_path)?,
        Cmd::EnrollToken {
            mesh_id,
            lighthouse,
            note,
        } => cli::enroll_token::run(mesh_id, lighthouse, note)?,
        Cmd::AddPeer {
            role,
            note,
            lighthouse,
            enroll_port,
        } => cli::node_admin::add_peer(&role, &note, lighthouse, enroll_port)?,
        Cmd::RemovePeer { node_id, force } => {
            cli::node_admin::remove_peer(&db_path, &node_id, force)?
        }
        Cmd::MeshSshKey { cmd } => cli::mesh_ssh_key::run(cmd)?,
        Cmd::Transfer { cmd } => cli::transfer::run(cmd)?,
        Cmd::Secret { cmd } => cli::secret::run(cmd)?,
        Cmd::SecretSeal { passphrase_file } => cli::secret::seal(&passphrase_file)?,
        Cmd::SecretUnseal { passphrase_file } => cli::secret::unseal(&passphrase_file)?,
        Cmd::Lighthouse { cmd } => match cmd {
            LighthouseCmd::Add {
                region,
                size,
                image,
            } => cli::node_admin::lighthouse_add(&region, size, image)?,
            LighthouseCmd::Retire {
                node_id,
                droplet_id,
                force,
            } => cli::node_admin::lighthouse_retire(&db_path, &node_id, droplet_id, force)?,
        },
        Cmd::Converge { site } => cli::converge::run(site)?,
        Cmd::Found {
            mesh_id,
            external_addr,
            role,
            enroll_port,
            with_backoffice,
        } => cli::found::run(
            &db_path,
            &mesh_id,
            &external_addr,
            &role,
            enroll_port,
            with_backoffice.as_deref(),
        )?,
        Cmd::Join {
            token,
            role,
            name,
            workgroup_root,
        } => cli::join::run(token, &role, name, workgroup_root)?,
        Cmd::Peers { json } => cli::peers::run(json, db_path)?,
        Cmd::Remediate { cmd } => cli::remediate::run(cmd, db_path)?,
        Cmd::Policy { cmd } => cli::policy::run(cmd, db_path)?,
        Cmd::Netstate { cmd } => cli::netstate::run(cmd)?,
        Cmd::Dns { cmd } => cli::dns::run(cmd, db_path)?,
        Cmd::Validate { cmd } => cli::validate::run(cmd)?,
        Cmd::Tags { json } => cli::tags::run(json, db_path)?,
        Cmd::Profiles {
            json,
            set,
            rm,
            role,
            description,
            tags,
            ks_fragments,
            auto_join,
        } => cli::profiles::run(
            json,
            set,
            rm,
            role,
            description,
            tags,
            ks_fragments,
            auto_join,
        )?,
        Cmd::Mirrors {
            json,
            sync,
            sync_all,
            write_repo,
            repo_dir,
        } => cli::mirrors::run(json, sync, sync_all, write_repo, repo_dir)?,
        Cmd::Images {
            json,
            record,
            build,
            name,
            kind,
            version,
            size_bytes,
            profile,
        } => cli::images::run(
            json, record, build, name, kind, version, size_bytes, profile,
        )?,
        Cmd::Upgrade {
            coordinate,
            version,
        } => cli::upgrade::run(coordinate, version)?,
        Cmd::Nodes { cmd } => cli::nodes::run(cmd, db_path)?,
        Cmd::AnsibleHistory { cmd } => cli::ansible_history::run(cmd)?,
        Cmd::Events { cmd } => cli::events::run(cmd, db_path)?,
        Cmd::Playbooks { cmd } => cli::playbooks::run(cmd)?,
    }
    Ok(())
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

/// Serialize the `NodeRow` list into the JSON shape the Iced
/// inventory panel consumes. Kept here rather than as a
/// `#[derive(Serialize)]` on `NodeRow` because the store struct
/// already serves topology + lifecycle callers and the JSON
/// shape is a CLI-surface contract.
/// Project the replicated directory (`action/mesh/directory` /
/// `mackesd peers`) onto the flat roster shape the `nodes list`
/// consumers (Workbench Fleet Inventory / node_roster / inventory /
/// home / node_roles) expect.
///
/// The roster source is the directory, NOT the local sqlite `nodes`
/// table: enrollment records land in QNM-Shared `peers/`, so
/// `store::list_nodes` is empty mesh-wide (confirmed on the lighthouse,
/// the dev host, and .13 — every node's sqlite roster is `[]` while the
/// directory holds the full fleet). Reading the directory here is what
/// makes "No peers enrolled" turn into the real 4-node roster.
fn directory_to_node_rows(dir: &serde_json::Value) -> Vec<mackesd_core::store::NodeRow> {
    dir["peers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let hostname = p["hostname"].as_str().unwrap_or("").to_string();
            if hostname.is_empty() {
                return None;
            }
            // The directory role is often null; fall back to the
            // capability tags (a "lighthouse" tag → lighthouse) then
            // to a plain peer so the panel still renders a role.
            let tags = p["tags"].as_array();
            let role = p["role"].as_str().map(str::to_string).unwrap_or_else(|| {
                let is_lh = tags.into_iter().flatten().any(|t| {
                    t.as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case("lighthouse"))
                });
                if is_lh { "lighthouse" } else { "peer" }.to_string()
            });
            Some(mackesd_core::store::NodeRow {
                node_id: hostname.clone(),
                name: hostname,
                public_key: String::new(),
                role,
                health: p["health"].as_str().unwrap_or("unknown").to_string(),
                region: p["overlay_ip"].as_str().map(str::to_string),
            })
        })
        .collect()
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
/// BULLETPROOF-1 — total bytes of the filesystem hosting `path`, via `df`.
/// `None` if the probe fails (caller falls back to the tmpfs-safe defaults).
#[cfg(feature = "async-services")]
fn filesystem_total_bytes(path: &std::path::Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-B1")
        .arg("--output=size")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // Line 1 is the "1B-blocks" header; line 2 is the size in bytes.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)?
        .trim()
        .parse::<u64>()
        .ok()
}

/// BUS-RETENTION-2 — free (available) bytes on the filesystem backing `path`.
/// Used to detect a near-full `/run` (the bus tmpfs) so mackesd can warn before
/// it fills + breaks runtime locks. `df --output=avail` mirrors
/// [`filesystem_total_bytes`]; `None` if `df` fails (degrade silently).
#[cfg(feature = "async-services")]
fn filesystem_avail_bytes(path: &std::path::Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-B1")
        .arg("--output=avail")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)?
        .trim()
        .parse::<u64>()
        .ok()
}

/// BULLETPROOF-1 / BUS-RUN-INODE-1 — a filesystem-relative bus retention policy.
/// The bus spool lives on `/run` (tmpfs), whose size ranges from ~190 MB
/// (lighthouse) to multiple GB (workstation). Cap hard at ~50% of the hosting
/// filesystem BYTES and soft at ~33%, with floors, so the spool is bounded well
/// below ENOSPC on any node.
///
/// BUS-RUN-INODE-1 — a tmpfs ALSO has a fixed inode budget unrelated to its byte
/// size (a ~3.9 GB DO-lighthouse `/run` has only ~819,200 inodes), and the bus
/// writes one small file per message — so the byte caps above cannot prevent
/// inode exhaustion. `/run/mde-bus` reached ~754,000 files / ~390 MB on both
/// lighthouses (12.1.0), wedging mackesd's namespace setup
/// (`status=226/NAMESPACE`) while every byte cap read "under budget". So also cap
/// the total spool FILE count at ~25% of the filesystem's inode budget (leaving
/// 75% for journald, systemd runtime state, dnf locks, etc.), floored so a small
/// `/run` still gets a usable window. Falls back to the (already tmpfs-safe)
/// library defaults if `df` can't read the byte size / inode count.
#[cfg(feature = "async-services")]
fn bus_retention_policy(bus_root: &std::path::Path) -> mde_bus::retention::RetentionPolicy {
    let mut policy = mde_bus::retention::RetentionPolicy::default();
    if let Some(total) = filesystem_total_bytes(bus_root) {
        policy.quota_hard_bytes = (total / 2).max(32 * 1024 * 1024);
        policy.quota_soft_bytes = (total / 3).max(16 * 1024 * 1024);
        if policy.quota_soft_bytes >= policy.quota_hard_bytes {
            policy.quota_soft_bytes = policy.quota_hard_bytes.saturating_sub(8 * 1024 * 1024);
        }
    }
    if let Some((itotal, _iavail)) = mde_bus::retention::filesystem_total_avail_inodes(bus_root) {
        // Keep the bus to at most a quarter of the tmpfs inode budget, floored so
        // a tiny `/run` still gets a usable window. This is the aggregate bound
        // the per-topic entry caps cannot provide (they bound files WITHIN a
        // topic, not the topic count) — the direct fix for the /run-inode wedge.
        policy.max_spool_files = (itotal / 4).max(10_000);
    }
    policy
}

#[cfg(feature = "async-services")]
fn run_serve(
    workgroup_root: Option<PathBuf>,
    node_id: Option<String>,
    db_path: PathBuf,
) -> anyhow::Result<()> {
    use mackesd_core::workers::{
        mesh_router::MeshRouterWorker, reconcile::ReconcileWorker, RestartPolicy, Spawn, Supervisor,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::RwLock;
    let workgroup_root = workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let node_id = node_id.unwrap_or_else(default_node_id);

    // SUBSTRATE-V2 — fail-loud shared-state assertion. On a deployed node the
    // workgroup root is the plain Syncthing directory at /mnt/mesh-storage; if
    // it's that canonical path but the directory is absent, the §1 file plane is
    // DOWN (the peer directory, fleet rollups, and the CA backup all silently
    // no-op against a missing dir). Surface it LOUDLY at startup rather than
    // degrading in silence. Not fatal: the daemon still runs the overlay and
    // coordinates over etcd; Syncthing provisions the dir on first sync.
    if workgroup_root == std::path::Path::new("/mnt/mesh-storage")
        && !mackesd_core::shared_root_writable(&workgroup_root)
    {
        tracing::error!(
            target: "mackesd",
            path = %workgroup_root.display(),
            "shared-state directory /mnt/mesh-storage is missing — the file plane is DOWN; \
             the peer directory + CA backup will not federate mesh-wide. Provision it: \
             `mackesd found`/`join` auto-runs setup-syncthing, or run \
             `/usr/libexec/mackesd/setup-syncthing` by hand."
        );
    }

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

    // WATCHDOG-2 — floor the runtime at 4 worker threads even on a 1-vCPU
    // lighthouse. The default (`worker_threads = num_cpus`) gives a SINGLE
    // worker on a 1-vCPU droplet, and that one worker also owns the tokio time
    // driver. When it reaches a blocking bridge (`substrate::peers::block_on` →
    // `block_in_place` for an etcd round-trip) the time driver FREEZES, so every
    // `tokio::time::sleep` — including the watchdog heartbeat below — stops
    // firing and systemd SIGABRT's a daemon that is actually healthy. That was
    // the lh1+lh2 crash-loop (1355/1348 aborts over ~40 h), which a missing
    // broker reliably triggered by adding block_in_place churn. A small fixed
    // pool keeps a second worker driving timers while the first blocks; on a
    // 1-vCPU box these just time-share and stay cheap (the daemon is I/O-bound).
    let worker_threads = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .max(4);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
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
                    // Best-effort in-process Bus publish (perf-10 / arch-6) — one
                    // reused Persist handle for the whole startup batch instead
                    // of a fork+exec of the `mde-bus` CLI (a process + fresh
                    // SQLite open + a reaper thread) per manifest. Targets
                    // `bus_publish::default_bus_root` (honours `MDE_BUS_ROOT` —
                    // the SAME root the CLI resolved). A bus that isn't openable
                    // yet at early startup is swallowed, never a daemon crash.
                    let mut bus = mackesd_core::bus_publish::open_bus(
                        mackesd_core::bus_publish::default_bus_root(),
                    );
                    for m in &manifests {
                        let body = format!(
                            r#"{{"name":"{}","apps":{},"layout":"{}","autostart":{}}}"#,
                            m.name.replace('"', "\\\""),
                            m.apps.len(),
                            m.layout.replace('"', "\\\""),
                            m.autostart,
                        );
                        if let Some(persist) = bus.as_mut() {
                            mackesd_core::bus_publish::publish_body(
                                persist,
                                "event/config/tags/loaded",
                                &body,
                            );
                        }
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

        // v3.0.3 — async supervisor for Phase B workers. The legacy
        // reconcile worker (mackesd-06) is now registered here too via
        // ReconcileWorker: its blocking sync rusqlite tick still runs on
        // a dedicated OS thread inside the worker (so it can't stall the
        // tokio scheduler), but the supervisor owns its restart/breaker.
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
        // MEDIA-1 — resolve the full deployment CLASS (rank + capability tags),
        // not just the rank: capability-gated workers (the Lighthouse_Media
        // Navidrome worker) gate on `runs_in(name, deploy_class)`, which checks
        // the media tag on top of the rank. `role_rank` is kept for the existing
        // rank-only `runs(name, role_rank)` gates (a plain worker has no tag).
        let deploy_class = match mackesd_core::worker_role::resolve_class_strict() {
            Ok(class) => class,
            Err(msg) => {
                eprintln!("mackesd serve: {msg}");
                anyhow::bail!("worker pool refused to start: no pinned role (ENT-2)");
            }
        };
        let role_rank = deploy_class.rank;
        tracing::info!(
            role_rank,
            media = deploy_class.media,
            "E1.2/MEDIA-1: spawning the class-permitted worker subset"
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
        // SUBSTRATE-2 — when the etcd coordination plane is provisioned on this
        // node (setup-etcd.sh wrote the endpoints file; absent on pre-cutover
        // nodes ⇒ no-op), probe it at startup so the substrate's reachability is
        // observable. The full leader/directory/health move onto these endpoints
        // is the rest of SUBSTRATE-V2; this proves the client + endpoints contract.
        {
            let eps = mackesd_core::substrate::etcd::default_endpoints();
            if !eps.is_empty() {
                tokio::spawn(async move {
                    if mackesd_core::substrate::etcd::probe(&eps).await {
                        tracing::info!(endpoints = ?eps, "SUBSTRATE-2: etcd coordination plane reachable");
                    } else {
                        tracing::warn!(endpoints = ?eps, "SUBSTRATE-2: etcd endpoints configured but unreachable");
                    }
                });
            }
        }
        spawn_compute_lifecycle_workers(&mut sup, &worker_names, role_rank, &node_id, &workgroup_root, &db_path, &daemon_cfg, &nebula_signal_slot);
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
        spawn_tiered(&mut sup, &worker_names, role_rank, "metrics_exporter", || {
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
            .with_backup_file(mackesd_core::workers::nebula_ca_backup::backup_path_for(
                &workgroup_root,
                &node_id,
            ))
            // EFF-21 — env is scrubbed at boot; presence rides this flag.
            .with_backup_passphrase_set(backup_passphrase.is_some())
        });
        spawn_mesh_plumbing_workers(&mut sup, &worker_names, role_rank, &node_id, &workgroup_root, &db_path);
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
        let https_transport =
            mackesd_core::transport::https443::NebulaHttps443Transport::from_bundle(
                &mackesd_core::ca::bundle::bundle_path(&workgroup_root, &node_id),
            );
        let https_udp_bridge = https_transport.relay_peer_id().and_then(|peer_id| {
            let raw = std::env::var(
                mackesd_core::workers::mesh_router::HTTPS_UDP_BRIDGE_BIND_ENV,
            )
            .unwrap_or_else(|_| {
                mackesd_core::workers::mesh_router::DEFAULT_HTTPS_UDP_BRIDGE_BIND.to_string()
            });
            match raw.parse() {
                Ok(bind_addr) => Some(
                    mackesd_core::workers::mesh_router::HttpsUdpBridgeConfig {
                        bind_addr,
                        nebula_source: mackesd_core::workers::mesh_router::DEFAULT_NEBULA_UDP_SOURCE
                            .parse()
                            .expect("default Nebula UDP source is a socket address"),
                        peer_id: peer_id.to_string(),
                    },
                ),
                Err(error) => {
                    tracing::warn!(value = %raw, error = %error, "mesh-router: invalid HTTPS UDP bridge bind");
                    None
                }
            }
        });
        let https443: Arc<dyn mackes_transport::Transport> = Arc::new(https_transport);
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
        spawn_tiered(&mut sup, &worker_names, role_rank, "mesh_router", || {
            // AUD2-1 — attach the shared decision-time histogram so the
            // router's per-tick observe() lands in the exporter's scrape.
            // AUD3 S-2/S-5 — scorer policy + the audit sink (path flips
            // land in the hash-chained events table + alert hooks).
            let worker = MeshRouterWorker::new(Arc::clone(&router_state), router_registry)
                .with_metrics(Arc::clone(&router_metrics))
                .with_policy(router_policy)
                .with_audit_sink(db_path.clone(), node_id.clone());
            match https_udp_bridge.clone() {
                Some(config) => worker.with_https_udp_bridge(config),
                None => worker,
            }
        });
        // v4.0.1 Phase 12.17 wire (2026-05-23) — STUN candidate
        // gatherer. Shares router_state with the router so
        // reflexive candidates land on every tracked peer's
        // PeerPath.candidates list. 30 s cadence; per-server
        // probe timeout 1.4 s; default server pool is Google's
        // public STUN cluster (IP-pinned so the worker doesn't
        // hit DNS on the hot path).
        spawn_tiered(&mut sup, &worker_names, role_rank, "stun_gather", || {
            mackesd_core::workers::stun_gather::StunGatherWorker::new(Arc::clone(&router_state))
        });
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
                spawn_tiered(&mut sup, &worker_names, role_rank, "nebula_supervisor", || {
                    mackesd_core::workers::nebula_supervisor::NebulaSupervisor::new(
                        sup_store,
                        node_id.clone(),
                        mesh_id,
                        bundle_path,
                    )
                });
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
        let enroll_key = std::env::var("MDE_ENROLL_KEY").unwrap_or_else(|_| {
            mackesd_core::workers::nebula_enroll_listener::DEFAULT_KEY_PATH.to_string()
        });
        // LIGHTHOUSE-ENROLL-SELF-CERT — a JOINED lighthouse (one that received the
        // sealed CA key over the enroll bundle, #12) holds the CA and CAN sign, but
        // `mackesd found` never ran on it, so it lacked the self-signed /enroll
        // endpoint cert and :4243 stayed DOWN — it could sign yet could not SERVE
        // enrollment. That made a joined lighthouse only a half lighthouse: the mesh
        // could not enroll new nodes once the founding lighthouse was retired (found
        // live during the 2026-06-27 lighthouse migration: nyc3/sfo3/fra1 came up
        // full — am_lighthouse + CA key + etcd voter — but :4243 never bound). If
        // this node holds the CA key and the endpoint cert is absent, self-generate
        // it now (the same self-signed rcgen identity `found` writes; SAN = the
        // node's primary public IPv4). Tokens later minted ON this lighthouse pin
        // THIS cert's fingerprint. Best-effort + idempotent: a failure just leaves
        // :4243 unbound (logged), never blocks startup.
        const CA_KEY_PATH: &str = "/var/lib/mackesd/nebula-ca/ca.key";
        if !std::path::Path::new(&enroll_cert).exists()
            && std::path::Path::new(CA_KEY_PATH).exists()
        {
            match detect_primary_ipv4() {
                Ok(ip) => match mackesd_core::nebula_enroll_endpoint::ensure_self_signed_cert(
                    std::path::Path::new(&enroll_cert),
                    std::path::Path::new(&enroll_key),
                    std::slice::from_ref(&ip),
                ) {
                    Ok(_) => tracing::info!(
                        ip = %ip,
                        "enroll-endpoint: self-signed cert generated for joined lighthouse \
                         (LIGHTHOUSE-ENROLL-SELF-CERT) — :4243 will now bind"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        "enroll-endpoint: self-cert generation failed; :4243 stays down"
                    ),
                },
                Err(e) => tracing::warn!(
                    error = %e,
                    "enroll-endpoint: primary IPv4 detection failed; :4243 stays down"
                ),
            }
        }
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

        spawn_datacenter_scheduler_workers(&mut sup, &worker_names, &node_id, &workgroup_root);

        spawn_broker_terminal_workers(&mut sup, &worker_names, role_rank, &workgroup_root);

        spawn_browser_workers(&mut sup, &worker_names, role_rank, &node_id, &workgroup_root);

        spawn_desktop_discovery_workers(&mut sup, &worker_names, role_rank, &node_id, &workgroup_root);

        spawn_fleet_compute_workers(&mut sup, &worker_names, role_rank, deploy_class, &node_id, &workgroup_root, &db_path);

        spawn_probe_observability_workers(&mut sup, &worker_names, role_rank, &node_id, &workgroup_root, &db_path, &daemon_cfg);

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

        // run_serve round-2 — the IPC/Bus-responder region (the ~22 std::thread
        // responders that share shutdown/worker_status/worker_names) was factored
        // out of run_serve into the cohesive `start_*` helpers below. Each helper
        // holds the exact thread-spawn + `worker_names.push(...)` registration
        // VERBATIM and runs its blocks in the original order, so the WORKER_REGISTRY
        // census + the ARCH-5 drift guard stay byte-identical (round-1 factored the
        // sup.spawn worker blocks; this is the responder half).
        start_control_surface_bus_responders(
            &worker_names,
            &worker_status,
            &shutdown,
            &node_id,
            &host,
            &workgroup_root,
            &db_path,
        );
        start_bus_retention_gc(&worker_names, &shutdown);
        start_connectivity_bus_responders(&worker_names, &shutdown, &node_id, &workgroup_root);
        start_datacenter_bus_responders(&worker_names, &shutdown, &workgroup_root);
        start_egress_bus_responders(&worker_names, &shutdown, &node_id, &workgroup_root);
        start_directory_jobs_bus_responders(&worker_names, &shutdown, &workgroup_root, &db_path);
        start_platform_bus_responders(&worker_names, &shutdown, &workgroup_root, &db_path);
        start_nebula_signal_dispatcher(&nebula_signal_slot);
        start_files_bus_responder(&worker_names, &shutdown, &host, &db_path);

        spawn_messaging_sync_workers(&mut sup, &worker_names, role_rank, &node_id, &workgroup_root, &worker_status);

        // mackesd-06 — the reconcile worker now runs UNDER the supervisor (it was
        // a raw std::thread::spawn with NO restart-on-panic and NO supervision, so
        // a panic silently killed reconcile for the daemon's lifetime). Its
        // blocking tick still runs on a dedicated OS thread INSIDE the worker
        // (ReconcileWorker bridges shutdown + surfaces an inner-thread panic as an
        // Err), so its sync rusqlite calls still never block the tokio scheduler —
        // it just gets the same restart + back-off + breaker treatment as every
        // sibling. Still surfaced via Shell.Workers (the worker_names roster) AND
        // now the supervisor's status map.
        spawn_tiered(&mut sup, &worker_names, role_rank, "reconcile", || {
            ReconcileWorker::new(workgroup_root, node_id, db_path)
        });

        // BULLETPROOF-2 — the daemon is up (supervisor + all responders +
        // workers spawned). Tell systemd we're READY (Type=notify gate) and
        // arm the watchdog ping below. Best-effort: a non-systemd launch
        // (NOTIFY_SOCKET unset) is a clean no-op.
        match mackesd_core::sd_notify::notify_ready() {
            Ok(true) => tracing::info!("sd_notify: READY=1 (Type=notify)"),
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, "sd_notify READY=1 failed"),
        }
        // WATCHDOG-2 — the systemd watchdog heartbeat runs on a DEDICATED OS
        // thread gated on an async *liveness beacon*, NOT inline on the runtime.
        //
        // Why this replaces the old in-loop ping (BROKER-RESILIENCE-1, which was
        // wrong): the ping used to ride `tokio::time::sleep` on the runtime. On a
        // 1-vCPU lighthouse the single worker owns the time driver, so when a
        // worker reached a blocking bridge (`substrate::peers::block_on` →
        // `block_in_place`) the time driver froze, the sleep never fired, and
        // systemd SIGABRT'd a healthy daemon — the lh1/lh2 crash-loop. The
        // `worker_threads` floor above keeps timers alive during a block, and
        // THIS thread makes the ping itself unstarvable: it is scheduled by the
        // kernel, and pings only while the serve loop keeps stamping `beat`
        // (every 250 ms). A genuine runtime wedge stops the beat → the thread
        // withholds the ping (see `watchdog_should_ping`) → systemd restarts us,
        // preserving the watchdog's real purpose. `notify_watchdog` is a
        // stateless datagram send, safe to call off-runtime.
        let watchdog_interval = mackesd_core::sd_notify::watchdog_interval();
        let wd_base = std::time::Instant::now();
        let wd_beat = Arc::new(std::sync::atomic::AtomicU64::new(0));
        if let Some(iv) = watchdog_interval {
            tracing::info!(
                secs = iv.as_secs(),
                "systemd watchdog armed (dedicated thread + liveness beacon)"
            );
            let beat = Arc::clone(&wd_beat);
            let wd_shutdown = Arc::clone(&shutdown);
            // A beat older than `fresh_ms` means the serve loop (which beats every
            // 250 ms) is genuinely wedged. WIFI-WATCHDOG: a node on a slow/high-
            // latency link (e.g. a laptop on WiFi) can legitimately starve the beat
            // for tens of seconds during a blocking coordination op — at 5 s that
            // false-restarted mackesd into a crash-loop and blocked in-place
            // upgrades on WiFi. Default 60 s, raisable per-node via
            // MACKESD_WATCHDOG_FRESH_MS; the systemd WatchdogSec window (180 s) is
            // sized to match, so a genuine wedge is still caught within minutes.
            let fresh_ms = std::env::var("MACKESD_WATCHDOG_FRESH_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60_000u64);
            let _ = std::thread::Builder::new()
                .name("mackesd-watchdog".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(iv);
                        if wd_shutdown.load(Ordering::Relaxed) {
                            break;
                        }
                        let now_ms = wd_base.elapsed().as_millis() as u64;
                        let beat_ms = beat.load(Ordering::Relaxed);
                        if mackesd_core::sd_notify::watchdog_should_ping(
                            now_ms, beat_ms, fresh_ms,
                        ) {
                            let _ = mackesd_core::sd_notify::notify_watchdog();
                        }
                    }
                });
        }

        // Watch loop: wake every 250 ms to check the shutdown flag and stamp the
        // watchdog liveness beacon (above). When shutdown flips, drop out and
        // drain the supervisor. The async supervisor's workers — including the
        // reconcile worker (mackesd-06) — see shutdown via the SIGTERM signal
        // handler installed above (mackesd_core::workers::ShutdownToken wraps the
        // same broadcast channel); a reconcile panic is now handled by the
        // supervisor's per-worker restart, not by tearing down the whole daemon.
        while !shutdown.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            wd_beat.store(wd_base.elapsed().as_millis() as u64, Ordering::Relaxed);
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
        // mackesd-06 — reconcile is drained by sup.shutdown_and_join() above
        // (it is a supervised worker now, not a standalone JoinHandle to join).
        tracing::info!("mackesd serve: all workers joined; exit");
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

/// DAR-19 / XPA-5 — resolve the host an `enroll-token` v3 token should embed
/// for the joining box to dial. NEVER the overlay IP: a not-yet-enrolled node
/// can't reach it (chicken-and-egg — it isn't on the mesh yet), which is
/// exactly the DAR-19 live failure (`mesh:<id>@10.42.0.1:4242`, unreachable by
/// a fresh non-overlay `mcnf-control`). `explicit` is the operator's
/// `--lighthouse` override; `external_addr` is this lighthouse's own
/// persisted PUBLIC address ([`mackesd_core::lighthouse_addr::read_external_addr`],
/// itself `host:4242` — Nebula's UDP data-plane port). Either input may carry
/// a trailing `:port`, always stripped here: the enroll port is fixed
/// independently by the caller
/// ([`mackesd_core::nebula_enroll_endpoint::DEFAULT_ENROLL_PORT`], the
/// dedicated `/enroll` HTTPS listener — port `:4242` is not an HTTP(S)
/// service). Returns `None` when neither input is present (the caller falls
/// back to auto-detection).
#[must_use]
fn resolve_enroll_endpoint_host(
    explicit: Option<&str>,
    external_addr: Option<&str>,
) -> Option<String> {
    let raw = explicit.or(external_addr)?;
    let host = raw.rsplit_once(':').map_or(raw, |(host, _)| host);
    (!host.is_empty()).then(|| host.to_string())
}

#[cfg(test)]
mod enroll_endpoint_host_tests {
    //! DAR-19 / XPA-5 regression coverage: `mackesd enroll-token` must never
    //! default to the overlay IP (`10.42.0.1`) at Nebula's UDP data-plane
    //! port (`:4242`) — a fresh, not-yet-enrolled node cannot reach either.
    //! It must default to the lighthouse's PUBLIC address at the dedicated
    //! `/enroll` HTTPS port (`DEFAULT_ENROLL_PORT`, `:4243`).
    use super::resolve_enroll_endpoint_host;
    use mackesd_core::nebula_enroll::JoinToken;
    use mackesd_core::nebula_enroll_endpoint::DEFAULT_ENROLL_PORT;

    #[test]
    fn prefers_the_explicit_override_over_the_external_addr() {
        let host = resolve_enroll_endpoint_host(Some("203.0.113.9"), Some("198.51.100.1:4242"));
        assert_eq!(host.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn strips_a_trailing_port_off_the_explicit_override() {
        // An operator-supplied `--lighthouse host:port` must not leak an
        // arbitrary port into the token — the enroll port is fixed
        // independently at DEFAULT_ENROLL_PORT (or an explicit override).
        let host = resolve_enroll_endpoint_host(Some("203.0.113.9:9999"), None);
        assert_eq!(host.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn falls_back_to_the_persisted_external_addr_and_strips_its_nebula_port() {
        // lighthouse_addr::read_external_addr() persists `host:4242` (the
        // Nebula UDP data-plane port, for static_host_map) — only the host
        // survives into the enroll token; the port is re-derived separately
        // as the enroll HTTPS port, never left at 4242.
        let host = resolve_enroll_endpoint_host(None, Some("165.227.188.238:4242"));
        assert_eq!(host.as_deref(), Some("165.227.188.238"));
    }

    #[test]
    fn none_when_neither_input_is_present() {
        // The caller (cmd EnrollToken) falls back to detect_primary_ipv4().
        assert_eq!(resolve_enroll_endpoint_host(None, None), None);
    }

    #[test]
    fn never_resolves_to_the_overlay_ip_drk19_live_regression() {
        // The exact live DAR-19 failure: `mcnf-control` (a68ab38b) got a
        // token embedding `10.42.0.1:4242` (LH1's OWN overlay IP + Nebula's
        // UDP port) instead of the LH's public `:4243` `/enroll` endpoint —
        // a fresh box off the overlay cannot dial either coordinate. Feeding
        // the founding lighthouse's real public external-addr through the
        // resolver must never reproduce the overlay-IP/4242 shape, and the
        // full v3 token built from it must carry the enroll port, not 4242.
        let external_addr = Some("165.227.188.238:4242");
        let host = resolve_enroll_endpoint_host(None, external_addr).expect("host resolves");
        assert_ne!(host, "10.42.0.1", "must not fall back to the overlay IP");
        assert_eq!(host, "165.227.188.238", "must be the LH's public host");

        let token = JoinToken {
            mesh_id: "magic-mesh".to_string(),
            lighthouse: host,
            port: DEFAULT_ENROLL_PORT,
            bearer: "bearer".to_string(),
            fp: Some("a".repeat(64)),
        };
        assert_eq!(token.port, 4243, "must be the public enroll port, not 4242");
        let wire = token.encode();
        assert!(
            !wire.contains("10.42.0.1"),
            "encoded token must not carry the overlay IP: {wire}"
        );
        assert!(
            wire.starts_with("mesh:magic-mesh@165.227.188.238:4243#bearer?fp="),
            "encoded token must target the public host at the enroll port: {wire}"
        );
    }
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

/// OW-4 — re-express an issued `MDEINV1` invite as the endpoint-bearing `mesh:`
/// token consumed by `mackesd join`.
///
/// Unlike `mint_join_token`, this does NOT mint a second bearer. The bearer is
/// the invite's canonical payload, already recorded by `invite::issue`, so either
/// presentation spends the same single-use ledger entry.
fn invite_issue_join_token(
    issued: &mackesd_core::onboard::invite::IssuedInvite,
    lighthouse: Option<String>,
    enroll_port: Option<u16>,
) -> anyhow::Result<String> {
    let cert_path = mackesd_core::workers::nebula_enroll_listener::DEFAULT_CERT_PATH;
    let cert_pem = std::fs::read(cert_path)
        .map_err(|e| anyhow::anyhow!("reading the /enroll endpoint cert {cert_path}: {e}"))?;
    invite_issue_join_token_from_cert(issued, &cert_pem, lighthouse, enroll_port)
}

fn invite_issue_join_token_from_cert(
    issued: &mackesd_core::onboard::invite::IssuedInvite,
    cert_pem: &[u8],
    lighthouse: Option<String>,
    enroll_port: Option<u16>,
) -> anyhow::Result<String> {
    use mackesd_core::onboard::invite::{to_join_token, EnrollEndpoint};

    let fp = mackesd_core::nebula_enroll_endpoint::endpoint_fingerprint_from_pem(cert_pem)
        .ok_or_else(|| anyhow::anyhow!("no certificate in /enroll endpoint PEM"))?;
    let addr = lighthouse
        .or_else(mackesd_core::lighthouse_addr::read_external_addr)
        .map_or_else(detect_primary_ipv4, Ok)?;
    let host = addr
        .rsplit_once(':')
        .map_or(addr.as_str(), |(h, _)| h)
        .to_string();
    let endpoint = EnrollEndpoint {
        lighthouse: host,
        port: enroll_port.unwrap_or(mackesd_core::nebula_enroll_endpoint::DEFAULT_ENROLL_PORT),
        fp: Some(fp),
    };
    Ok(to_join_token(&issued.invite, &endpoint).encode())
}

/// CONNECT-4 — on a Lighthouse (the public ingress role) install + wire Caddy so
/// the `connect_firewall` worker's rendered ingress fragment takes effect. No-op
/// for non-lighthouse roles. Best-effort + idempotent: a miss logs but never
/// fails the enroll (the lighthouse still joins; only public web ingress is
/// deferred until Caddy is present).
fn provision_caddy_if_lighthouse(role: mde_role::Role) {
    if role != mde_role::Role::Lighthouse {
        return;
    }
    println!("CONNECT-4: provisioning Caddy public ingress (lighthouse role)");
    match std::process::Command::new("timeout")
        .args(["360", "/usr/libexec/mackesd/setup-caddy"])
        .status()
    {
        Ok(s) if s.success() => println!("CONNECT-4: Caddy ingress ready"),
        Ok(s) => eprintln!(
            "provision: setup-caddy exited {:?} — public web ingress deferred; \
             check `systemctl status caddy`",
            s.code()
        ),
        Err(e) => eprintln!(
            "provision: setup-caddy not run ({e}) — is the RPM installed? \
             public web ingress is unavailable until Caddy is set up"
        ),
    }
}

/// SETUP-7 — write `/etc/mackesd/site.yml` from the bootstrap result so a later
/// `mackesd converge` (or the ansible-pull worker) can idempotently restore
/// steady state. Best-effort: a write failure logs but never fails the enroll.
fn emit_site_yml_best_effort(role: &str, mesh_id: &str, lighthouses: Vec<String>) {
    let facts =
        mackesd_core::site_yml::SiteFacts::new(role, mesh_id, lighthouses, "/mnt/mesh-storage");
    let path = std::path::Path::new(mackesd_core::site_yml::DEFAULT_SITE_YML);
    match mackesd_core::site_yml::write_site_yml(path, &facts) {
        Ok(()) => println!("convergence playbook written: {}", path.display()),
        Err(e) => eprintln!(
            "site.yml: could not write {} ({e}) — `mackesd converge` skipped",
            path.display()
        ),
    }
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
