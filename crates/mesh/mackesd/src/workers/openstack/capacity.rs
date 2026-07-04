//! QC-10 — the node-capacity read + the two open-cloud guardrails it derives:
//! **capacity-derived Nova flavors** (design Q39) and **hard per-user quotas**
//! (design Q89, the ENT-12 blast-radius boundary).
//!
//! The design's two guardrails on the open cloud are both *derived from real
//! capacity*, never fixed `OpenStack` defaults (Q29/39/89):
//!
//! - **Flavors** ([`derive_flavors`]) — a tiny/small/medium/large ladder sized
//!   as fractions of this node's actual shape, so a bigger node offers bigger
//!   instances and a small node still yields a usable-but-modest set. The
//!   [`super::config_render::render_cloud_bootstrap`] seed the leader applies
//!   turns these into `openstack flavor create` calls.
//! - **Hard per-user quotas** ([`derive_quotas`]) — a per-member ceiling that
//!   is a *fraction* of the node (so several members coexist and one member can
//!   never claim the whole fleet). These are the mesh's first hard authorization
//!   boundary (Q89, a documented §9 no-RBAC departure); the same seed registers
//!   them as Keystone unified limits and `nova.conf` enforces them via the
//!   `UnifiedLimitsDriver`.
//!
//! The read itself ([`NodeCapacity::probe`]) is honest (§7): logical CPUs off
//! [`std::thread::available_parallelism`], total RAM off `/proc/meminfo`, and
//! the writable partition's total size off `df` (the same dependency-free idiom
//! the metrics exporter uses — no libc/statvfs dep). A read that fails returns a
//! typed [`CapacityError`] the worker surfaces on the alert lane rather than a
//! fabricated capacity. The pure derivations ([`derive_flavors`]/
//! [`derive_quotas`]) take a [`NodeCapacity`] value, so the guardrail logic is
//! headless-testable against fixtures with no host probe.

use std::path::Path;
use std::process::Command;

use thiserror::Error;

/// `/proc/meminfo` — the total-RAM source (`MemTotal:` line, in kB).
const PROC_MEMINFO: &str = "/proc/meminfo";

/// The writable partition whose total size sizes flavors/quotas (design Q59 —
/// the partition carrying Nova ephemeral + the Cinder VG + the Glance/Swift
/// dirs). Its *total* capacity is what the derivations scale against.
const CAPACITY_DISK_PATH: &str = "/var/lib";

/// RAM/disk are quantized to these units so a rendered flavor/quota is a clean
/// figure (Nova RAM is MiB; a 512-MiB grain keeps the ladder tidy).
const RAM_GRAIN_MIB: u64 = 512;

/// This node's real compute/memory/disk capacity — the input both open-cloud
/// guardrails derive from (design Q29/39/89).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeCapacity {
    /// Logical CPUs (`available_parallelism`) — the vCPU headroom flavors and
    /// the per-user core quota scale against.
    pub vcpus: u32,
    /// Total RAM in MiB (`/proc/meminfo` `MemTotal`).
    pub ram_mib: u64,
    /// Total size of the writable partition in GiB (`df` on
    /// [`CAPACITY_DISK_PATH`]).
    pub disk_gib: u64,
}

/// A typed capacity-read failure — carried to the worker's alert lane, never a
/// fabricated capacity (§7).
#[derive(Debug, Error)]
pub enum CapacityError {
    /// The logical CPU count couldn't be read.
    #[error("reading the logical CPU count failed — {0}")]
    Cpus(String),
    /// A capacity source file couldn't be read.
    #[error("reading {path} failed — {source}")]
    Read {
        /// The path that failed.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A capacity source was read but didn't carry the expected field.
    #[error("parsing {what} failed — {detail}")]
    Parse {
        /// What was being parsed.
        what: String,
        /// The parse-failure detail.
        detail: String,
    },
    /// `df` couldn't report the writable partition's size.
    #[error("`df` for {path} failed — {detail}")]
    Df {
        /// The path queried.
        path: String,
        /// The failure detail.
        detail: String,
    },
}

impl NodeCapacity {
    /// A capacity value (tests + the doctrine fold).
    #[must_use]
    pub const fn new(vcpus: u32, ram_mib: u64, disk_gib: u64) -> Self {
        Self {
            vcpus,
            ram_mib,
            disk_gib,
        }
    }

    /// Probe this node's real capacity off the host (§7 — honest, typed on
    /// failure).
    ///
    /// vCPUs from [`std::thread::available_parallelism`], total RAM from
    /// `/proc/meminfo`, and the writable partition's total size from `df`
    /// (dependency-free, the metrics-exporter idiom).
    ///
    /// # Errors
    /// A [`CapacityError`] when any source is unreadable or unparseable.
    pub fn probe() -> Result<Self, CapacityError> {
        let logical = std::thread::available_parallelism()
            .map_err(|e| CapacityError::Cpus(e.to_string()))?
            .get();
        let vcpus = u32::try_from(logical).unwrap_or(u32::MAX);

        let meminfo =
            std::fs::read_to_string(PROC_MEMINFO).map_err(|source| CapacityError::Read {
                path: PROC_MEMINFO.to_string(),
                source,
            })?;
        let mem_total_kib = parse_memtotal_kib(&meminfo).ok_or_else(|| CapacityError::Parse {
            what: format!("{PROC_MEMINFO} MemTotal"),
            detail: "no `MemTotal:` line".to_string(),
        })?;
        let ram_mib = mem_total_kib / 1024;

        let disk_gib = probe_disk_gib(Path::new(CAPACITY_DISK_PATH))?;
        Ok(Self {
            vcpus,
            ram_mib,
            disk_gib,
        })
    }
}

/// One derived Nova flavor (design Q39) — a named vCPU/RAM/disk shape the
/// bootstrap seed creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flavor {
    /// The Nova flavor name (`m1.tiny` … `m1.large`).
    pub name: &'static str,
    /// vCPUs.
    pub vcpus: u32,
    /// RAM in MiB.
    pub ram_mib: u64,
    /// Root/ephemeral disk in GiB.
    pub disk_gib: u64,
}

/// The tiny→large ladder: `(name, vcpu-divisor, ram-divisor, disk-divisor,
/// floor-vcpu, floor-ram-mib, floor-disk-gib)`.
///
/// Each flavor is `capacity / divisor`, floored — so on a real (large) node the
/// fraction dominates and the set scales with capacity (Q39, "not fixed
/// defaults"), while the floors keep even a toy node's ladder usable and
/// strictly increasing. Disk divisors are larger than the vCPU/RAM ones so a
/// root disk stays modest relative to the whole partition.
const FLAVOR_LADDER: [(&str, u32, u64, u64, u32, u64, u64); 4] = [
    ("m1.tiny", 16, 16, 32, 1, 512, 5),
    ("m1.small", 8, 8, 16, 1, 1024, 10),
    ("m1.medium", 4, 4, 8, 2, 2048, 20),
    ("m1.large", 2, 2, 4, 4, 4096, 40),
];

/// Derive the flavor ladder from real node capacity (design Q39).
///
/// Returns tiny/small/medium/large, each dimension a fraction of `cap` (floored
/// so the set is always usable and strictly increasing). A bigger node yields
/// bigger flavors — the set regenerates as the fleet's shape changes.
#[must_use]
pub fn derive_flavors(cap: &NodeCapacity) -> Vec<Flavor> {
    FLAVOR_LADDER
        .iter()
        .map(
            |&(name, vcpu_div, ram_div, disk_div, floor_vcpu, floor_ram, floor_disk)| Flavor {
                name,
                vcpus: (cap.vcpus / vcpu_div).max(floor_vcpu),
                ram_mib: round_up((cap.ram_mib / ram_div).max(floor_ram), RAM_GRAIN_MIB),
                disk_gib: (cap.disk_gib / disk_div).max(floor_disk),
            },
        )
        .collect()
}

/// A hard per-user quota (design Q89) — the ceiling a single member may hold.
///
/// Every field is a *fraction* of node capacity, so the fleet can carry several
/// members and no member can exhaust it (the ENT-12 blast-radius guardrail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserQuota {
    /// Max concurrent instances.
    pub instances: u32,
    /// Max total vCPUs (Nova cores).
    pub vcpus: u32,
    /// Max total RAM in MiB.
    pub ram_mib: u64,
    /// Max Cinder volumes.
    pub volumes: u32,
    /// Max total volume storage in GiB (Cinder gigabytes).
    pub gigabytes: u64,
    /// Max floating IPs.
    pub floating_ips: u32,
}

/// The per-user capacity share: a member gets at most `1/DIVISOR` of the node,
/// so at least `DIVISOR` members coexist before the node is claimed (Q89).
const QUOTA_DIVISOR: u32 = 4;

/// Derive the hard per-user quota from real node capacity (design Q89).
///
/// The per-user vCPU/RAM/disk ceilings are each `capacity` divided by
/// [`QUOTA_DIVISOR`] (floored to a sane minimum), and the instance/volume/FIP
/// counts
/// follow — a hard, capacity-scaled boundary, never the `OpenStack` defaults.
#[must_use]
pub fn derive_quotas(cap: &NodeCapacity) -> UserQuota {
    let vcpus = (cap.vcpus / QUOTA_DIVISOR).max(1);
    let instances = vcpus; // ~1 vCPU per instance ceiling
    UserQuota {
        instances,
        vcpus,
        ram_mib: round_up(
            (cap.ram_mib / u64::from(QUOTA_DIVISOR)).max(512),
            RAM_GRAIN_MIB,
        ),
        volumes: (instances * 2).max(2),
        gigabytes: (cap.disk_gib / u64::from(QUOTA_DIVISOR)).max(20),
        floating_ips: instances.max(1),
    }
}

/// Round `value` up to the next multiple of `grain` (never zero).
const fn round_up(value: u64, grain: u64) -> u64 {
    value.div_ceil(grain) * grain
}

/// Parse `MemTotal:` (kB) out of a `/proc/meminfo` body.
fn parse_memtotal_kib(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

/// Parse the first numeric value out of a `df --output=size` body (the second
/// line's first token — leading whitespace tolerated).
fn parse_df_size_bytes(df_output: &str) -> Option<u64> {
    df_output
        .lines()
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// The writable partition's total size in GiB via `df -B1 --output=size`
/// (bounded by the shared subprocess timeout; no libc/statvfs dep — the
/// metrics-exporter idiom).
fn probe_disk_gib(path: &Path) -> Result<u64, CapacityError> {
    let mut cmd = Command::new("df");
    cmd.arg("-B1").arg("--output=size").arg(path);
    let out =
        crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .map_err(|e| CapacityError::Df {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(CapacityError::Df {
            path: path.display().to_string(),
            detail: format!("df exited {}", out.status),
        });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let bytes = parse_df_size_bytes(&text).ok_or_else(|| CapacityError::Parse {
        what: "df --output=size".to_string(),
        detail: format!("unparseable df output: {text:?}"),
    })?;
    Ok(bytes / (1024 * 1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A big node and a small node — the two fixtures the scaling assertions
    /// compare (design §7 — flavors/quotas derived from a fixture capacity).
    const BIG: NodeCapacity = NodeCapacity::new(32, 131_072, 2_000);
    const SMALL: NodeCapacity = NodeCapacity::new(4, 8_192, 100);

    // ── capacity-derived flavors (Q39) ──

    #[test]
    fn flavor_ladder_is_tiny_small_medium_large() {
        let f = derive_flavors(&BIG);
        assert_eq!(
            f.iter().map(|x| x.name).collect::<Vec<_>>(),
            vec!["m1.tiny", "m1.small", "m1.medium", "m1.large"]
        );
    }

    #[test]
    fn flavors_are_derived_from_capacity_not_fixed_defaults() {
        // A 32-vCPU / 128-GiB node: the largest flavor is HALF the node
        // (fraction dominates the floor), proving the ladder is sized from the
        // real shape rather than fixed OpenStack defaults.
        let f = derive_flavors(&BIG);
        let large = f.last().unwrap();
        assert_eq!(large.vcpus, 16, "large = vcpus/2");
        assert_eq!(large.ram_mib, 65_536, "large = ram/2");
        assert_eq!(large.disk_gib, 500, "large = disk/4");
    }

    #[test]
    fn the_flavor_set_scales_with_capacity() {
        // §7 — a bigger node yields strictly bigger flavors at every rung.
        let big = derive_flavors(&BIG);
        let small = derive_flavors(&SMALL);
        for (b, s) in big.iter().zip(small.iter()) {
            assert_eq!(b.name, s.name);
            assert!(
                b.vcpus >= s.vcpus && b.ram_mib >= s.ram_mib && b.disk_gib >= s.disk_gib,
                "{}: big {b:?} must dominate small {s:?}",
                b.name
            );
        }
        // And at the top rung a big node is strictly larger (real scaling, not
        // just the shared floors).
        assert!(big.last().unwrap().vcpus > small.last().unwrap().vcpus);
        assert!(big.last().unwrap().ram_mib > small.last().unwrap().ram_mib);
        assert!(big.last().unwrap().disk_gib > small.last().unwrap().disk_gib);
    }

    #[test]
    fn flavors_are_strictly_increasing_and_floored() {
        // Even a toy node yields a usable, strictly-increasing ladder (the
        // floors clamp; no zero-sized flavor, §7).
        let tiny_node = NodeCapacity::new(1, 1_024, 8);
        let f = derive_flavors(&tiny_node);
        for w in f.windows(2) {
            assert!(w[0].vcpus <= w[1].vcpus, "{w:?}");
            assert!(w[0].ram_mib < w[1].ram_mib, "{w:?}");
            assert!(w[0].disk_gib < w[1].disk_gib, "{w:?}");
        }
        assert!(f[0].vcpus >= 1 && f[0].ram_mib >= 512 && f[0].disk_gib >= 5);
        // RAM is always a clean 512-MiB multiple.
        assert!(f.iter().all(|x| x.ram_mib % RAM_GRAIN_MIB == 0));
    }

    // ── hard per-user quotas (Q89) ──

    #[test]
    fn per_user_quota_is_a_hard_fraction_of_the_node() {
        // The blast-radius guardrail: a single member's ceiling is a fraction of
        // the node (here a quarter), so several members coexist and none can
        // claim the fleet.
        let q = derive_quotas(&BIG);
        assert_eq!(q.vcpus, 8, "cores = vcpus/4");
        assert_eq!(q.instances, 8);
        assert_eq!(q.ram_mib, 32_768, "ram = ram/4");
        assert_eq!(q.gigabytes, 500, "gigabytes = disk/4");
        // Strictly a fraction — never the whole node (the hard boundary).
        assert!(q.vcpus < BIG.vcpus);
        assert!(q.ram_mib < BIG.ram_mib);
        assert!(u64::from(q.instances) * u64::from(QUOTA_DIVISOR) <= u64::from(BIG.vcpus));
        // Every count is a real, non-zero cap.
        assert!(q.volumes >= 2 && q.floating_ips >= 1);
    }

    #[test]
    fn quotas_scale_with_capacity() {
        // §7 — a bigger node grants a bigger (still-fractional) per-user ceiling.
        let big = derive_quotas(&BIG);
        let small = derive_quotas(&SMALL);
        assert!(big.vcpus > small.vcpus);
        assert!(big.ram_mib > small.ram_mib);
        assert!(big.gigabytes > small.gigabytes);
        assert!(big.instances > small.instances);
    }

    #[test]
    fn quotas_floor_on_a_tiny_node() {
        // A 1-vCPU node still gets a valid (minimum) hard cap, never zero.
        let q = derive_quotas(&NodeCapacity::new(1, 512, 10));
        assert_eq!(q.vcpus, 1);
        assert_eq!(q.instances, 1);
        assert!(q.ram_mib >= 512);
        assert!(q.gigabytes >= 20);
    }

    // ── the honest probe parsers ──

    #[test]
    fn parses_memtotal_from_proc_meminfo() {
        let body = "MemTotal:       16307892 kB\nMemFree:         1234 kB\n";
        assert_eq!(parse_memtotal_kib(body), Some(16_307_892));
        assert_eq!(parse_memtotal_kib("SwapTotal: 0 kB\n"), None);
    }

    #[test]
    fn parses_df_size_output() {
        // `df -B1 --output=size` → a header line then the byte total.
        let body = "  1K-blocks\n 500107862016\n";
        assert_eq!(parse_df_size_bytes(body), Some(500_107_862_016));
        assert_eq!(parse_df_size_bytes("only-a-header\n"), None);
    }

    #[test]
    fn round_up_snaps_to_the_grain() {
        assert_eq!(round_up(1, 512), 512);
        assert_eq!(round_up(512, 512), 512);
        assert_eq!(round_up(513, 512), 1024);
    }
}
