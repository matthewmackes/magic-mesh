//! Datacenter actions for host-independent planning, storage, Tofu, genesis, and
//! DigitalOcean operations. VM lifecycle is owned exclusively by typed Workloads.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// The only supported DigitalOcean lighthouse shape. Lighthouses are thin
/// relay/control-plane appliances; callers cannot opt them into a larger or
/// media/fileshare-capable droplet.
pub const THIN_LIGHTHOUSE_SIZE: &str = "s-1vcpu-512mb-10gb";

/// Authorization node scope for the Datacenter responder. Mutation
/// targets are the existing resource lock keys, so a capability cannot be
/// replayed against a different storage object or generated IaC resource.
pub const DC_ACTION_NODE_SCOPE: &str = "fleet-control";

/// The Datacenter planning and storage responder.
///
/// DATACENTER-6 (op-lock half): the service also carries an in-flight op-lock —
/// a shared set of the resource keys currently being mutated. [`build_reply`]
/// try-inserts the key before
/// dispatching a mutating verb and rejects a second concurrent mutation on the
/// same resource with a clear `busy` reason; a [`OpLockGuard`] removes the key
/// when the op completes (RAII). `Clone` shares the same lock (the spawn in
/// `bin/mackesd.rs` clones the service into the responder thread), so two
/// in-flight requests — even across `Clone`d handles — see one set.
#[derive(Debug, Clone)]
pub struct DatacenterService {
    // The repo root used by allow-listed Tofu, genesis, and backoffice planning.
    workgroup_root: PathBuf,
    /// In-flight resource keys currently being mutated. `Arc<Mutex<…>>` so a
    /// `Clone` of the service (the responder-thread handle) shares ONE set, and
    /// so concurrent `build_reply` calls serialize on insert/remove.
    in_flight: Arc<Mutex<BTreeSet<String>>>,
    /// Root-only capability verifier for the production Bus responder.
    authorizer: Arc<ActionAuthorizer>,
}

impl DatacenterService {
    /// Build the service rooted at the shared workgroup root, with an empty
    /// in-flight op-lock set.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            in_flight: Arc::new(Mutex::new(BTreeSet::new())),
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    /// Inject an isolated verifier and replay ledger for hostile responder
    /// tests. Production construction always uses the root-only systemd
    /// credential through [`ActionAuthorizer::production`].
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Try to claim `key` in the in-flight set. Returns a [`OpLockGuard`] (which
    /// releases the key on drop) when the key was free, or `None` when a mutation
    /// on the same resource is already in flight — the caller turns that into the
    /// `busy` reject. A poisoned lock is recovered (the set is plain data; a panic
    /// mid-mutation cannot leave it inconsistent), so the op-lock never wedges the
    /// responder.
    #[must_use]
    fn try_lock(&self, key: String) -> Option<OpLockGuard<'_>> {
        let mut set = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if set.insert(key.clone()) {
            Some(OpLockGuard {
                in_flight: &self.in_flight,
                key,
            })
        } else {
            None
        }
    }
}

/// RAII release for one claimed in-flight resource key: dropping it removes the
/// key from the service's in-flight set, so a panic or early return in
/// [`build_reply`] still frees the lock (the resource never gets stuck `busy`).
struct OpLockGuard<'a> {
    in_flight: &'a Arc<Mutex<BTreeSet<String>>>,
    key: String,
}

impl Drop for OpLockGuard<'_> {
    fn drop(&mut self) {
        let mut set = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set.remove(&self.key);
    }
}

/// Non-VM action verbs served on `action/dc/<verb>`.
///
/// DATACENTER-12: the trailing five are the storage verbs ([`crate::ipc::storage_ops`]),
/// served on THIS responder so the panel's Storage tab acts through the same Bus
/// round trip as the other Datacenter operations. `build_reply` routes them into
/// `storage_ops`.
pub const ACTION_VERBS: [&str; 10] = [
    "do-regions",
    // DATACENTER-19 — the guided new-lighthouse flow's Tofu-write half.
    "lighthouse-create",
    // DATACENTER-18 — New-Mesh genesis: plan (read-only) + write (the founding
    // lighthouse droplet + its DNS A-record). Reuses DC-19's lighthouse Tofu-write;
    // the live apply + `mackesd found` stay operator-gated.
    "genesis-plan",
    "genesis-write",
    // DAR-45 (DEVOPS-AUTOMATION-REBUILD) — the genesis-wizard backoffice step's
    // read-only planner probe: shells out to `backoffice-plan.sh --tier <t>` and
    // returns the RENDERED ordered unit list + a `secrets_ready` boolean. NOT a
    // canned plan — the acceptance asserts it matches the script's output. The
    // live bring-up (`backoffice-up.sh`) stays operator-gated on the control VM.
    "backoffice-plan",
    "sr-create",
    "vdi-create",
    "vdi-attach",
    "vdi-detach",
    "sr-snapshot",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);
/// Maximum number of retained action requests admitted by one responder tick.
/// The SQL limit is applied before decoding so a stalled Datacenter consumer
/// cannot materialize its entire retained action history.
pub const MAX_MESSAGES_PER_POLL: usize = 64;

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// The resource key a mutating `verb` op-locks, or `None` for a read-only verb
/// that needs no lock. PURE (used by [`build_reply`]'s op-lock and unit-testable
/// on its own).
#[must_use]
pub fn lock_key(verb: &str, req_body: Option<&str>) -> Option<String> {
    // DATACENTER-19 — `lighthouse-create` has no droplet id yet; it locks on the
    // new lighthouse's name so two creates of the same name can't race the same
    // generated-`.tf` write.
    if verb == "lighthouse-create" {
        let name = serde_json::from_str::<serde_json::Value>(req_body?)
            .ok()?
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)?;
        if name.is_empty() {
            return None;
        }
        return Some(format!("lighthouse-new:{name}"));
    }
    // DATACENTER-18 — `genesis-write` mutates the shared `dc-lighthouses.tf`; it
    // locks on the new mesh id so two genesis writes of the same mesh can't race
    // the same `.tf` write. (`genesis-plan` is read-only → no lock, below.)
    if verb == "genesis-write" {
        let mesh_id = serde_json::from_str::<serde_json::Value>(req_body?)
            .ok()?
            .get("mesh_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)?;
        if mesh_id.is_empty() {
            return None;
        }
        return Some(format!("mesh-new:{mesh_id}"));
    }
    // DATACENTER-12 — storage verbs lock on the resource they target so two
    // mutations on the same SR/VDI/VBD don't race. Each reads a different body
    // field for its key; an absent/empty field returns `None` (no lock — the
    // per-verb builder produces the real validation error).
    let keyed = match verb {
        "sr-create" => Some(("name", "sr-new")),
        "vdi-create" => Some(("sr", "sr")),
        "vdi-attach" | "sr-snapshot" => Some(("vdi", "vdi")),
        "vdi-detach" => Some(("vbd", "vbd")),
        _ => None,
    };
    if let Some((field, ns)) = keyed {
        let val = serde_json::from_str::<serde_json::Value>(req_body?)
            .ok()?
            .get(field)
            .and_then(|v| v.as_str())
            .map(str::to_string)?;
        if val.is_empty() {
            return None;
        }
        return Some(format!("{ns}:{val}"));
    }
    None
}

/// Return the capability target for a privileged datacenter mutation. Read-only
/// inventory/planning verbs remain open; every mutation must carry a resource
/// lock key so the signed body is bound to the exact target that the dispatcher
/// is about to touch.
fn mutation_target(verb: &str, req_body: Option<&str>) -> Result<Option<String>, String> {
    if !ACTION_VERBS.contains(&verb)
        || matches!(verb, "do-regions" | "genesis-plan" | "backoffice-plan")
    {
        return Ok(None);
    }
    let target = lock_key(verb, req_body)
        .ok_or_else(|| format!("{verb}: missing or invalid mutation target"))?;
    Ok(Some(target))
}

/// Verify a datacenter mutation before the op-lock or any filesystem/backend
/// call. The production responder's verifier is
/// root-credential-backed; tests inject an isolated verifier through
/// [`DatacenterService::with_authorizer`].
fn authorize_mutation(
    svc: &DatacenterService,
    verb: &str,
    req_body: Option<&str>,
) -> Result<(), String> {
    let target = mutation_target(verb, req_body)?;
    let Some(target) = target else {
        return Ok(());
    };
    svc.authorizer.authorize(
        req_body.expect("a mutation target requires a body"),
        MutationContext {
            verb,
            node: DC_ACTION_NODE_SCOPE,
            target: &target,
        },
    )
}

/// Production Bus dispatch wrapper. Ordinary reads remain available without a
/// capability; mutations fail closed before resource locking or backend work.
fn build_authorized_reply(svc: &DatacenterService, verb: &str, req_body: Option<&str>) -> String {
    if let Err(error) = authorize_mutation(svc, verb, req_body) {
        tracing::warn!(
            target: "mackesd::action_auth",
            verb,
            %error,
            "refused unauthorized Datacenter mutation"
        );
        return json!({ "error": format!("{verb}: authorization refused: {error}") }).to_string();
    }
    build_reply(svc, verb, req_body)
}

/// Build the reply for one `action/dc/<verb>` request, dispatching on `verb`.
///
/// DATACENTER-6 (op-lock half): before dispatching a *mutating* verb, the resource
/// key ([`lock_key`]) is claimed in the service's in-flight set. If a mutation on
/// the same resource is already in flight, this returns the clear `busy` reject
/// WITHOUT running the op; otherwise a [`OpLockGuard`] holds the key for the
/// duration of the (synchronous) dispatch and releases it on return (RAII).
/// Read-only verbs ([`lock_key`] → `None`) take no lock and never reject.
#[must_use]
pub fn build_reply(svc: &DatacenterService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    // Op-lock: claim the resource for the duration of a mutating dispatch. The
    // guard is dropped at the end of this function (after the reply is built),
    // releasing the key. Read-only verbs (lock_key → None) are unguarded.
    let _guard = match lock_key(verb, req_body) {
        Some(key) => match svc.try_lock(key.clone()) {
            Some(g) => Some(g),
            None => {
                return err(format!(
                    "resource {key} busy: a {verb} is already in flight"
                ));
            }
        },
        None => None,
    };
    match verb {
        "do-regions" => do_regions_reply(),
        "lighthouse-create" => lighthouse_create_reply(svc, req_body),
        "genesis-plan" => genesis_plan_reply(req_body),
        "genesis-write" => genesis_write_reply(svc, req_body),
        // DAR-45 — read-only backoffice planner probe (no lock; mutates nothing).
        "backoffice-plan" => backoffice_plan_reply(svc, req_body),
        // DATACENTER-12 — storage verbs are served on this responder but built by
        // the sibling storage_ops module (the op-lock above already guards them).
        v if crate::ipc::storage_ops::is_storage_verb(v) => {
            crate::ipc::storage_ops::build_reply(v, req_body)
        }
        _ => err("unknown dc verb".into()),
    }
}

/// Parse a `doctl compute region list -o json` array into `(slug, name, available)`
/// triples. PURE.
///
/// Each array element is expected to be an object with string `slug`/`name` and a
/// boolean `available`. Missing string fields default to empty, a missing/non-bool
/// `available` defaults to `false`. Non-array or unparsable input yields an empty
/// vector (best-effort — the caller turns that into the doctl-failed error).
#[must_use]
pub fn parse_regions(json: &str) -> Vec<(String, String, bool)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|r| {
            let slug = r
                .get("slug")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = r
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let available = r
                .get("available")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            (slug, name, available)
        })
        .collect()
}

/// Handle a `do-regions` request: run `doctl compute region list` (read-only) and
/// reply with the parsed regions. The doctl context is `MCNF_DOCTL_CONTEXT`
/// (default `mackes`). Best-effort: doctl missing/failed → the doctl-failed error.
fn do_regions_reply() -> String {
    let err = |m: &str| json!({ "error": m }).to_string();
    let context = std::env::var("MCNF_DOCTL_CONTEXT").unwrap_or_else(|_| "mackes".to_string());
    let output = std::process::Command::new("doctl")
        .args([
            "compute",
            "region",
            "list",
            "--context",
            &context,
            "-o",
            "json",
        ])
        .output();
    let Ok(out) = output else {
        return err("doctl region list failed");
    };
    if !out.status.success() {
        return err("doctl region list failed");
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let regions: Vec<serde_json::Value> = parse_regions(&stdout)
        .into_iter()
        .map(
            |(slug, name, available)| json!({ "slug": slug, "name": name, "available": available }),
        )
        .collect();
    json!({ "ok": true, "regions": regions }).to_string()
}

/// DATACENTER-19 — `true` iff `s` is a non-empty `DigitalOcean` region slug:
/// lowercase ASCII alphanumerics + dash (e.g. `nyc3`, `sfo3`, `fra1`). PURE — used
/// to validate the `region`/`size`/`image` slugs before they reach the HCL.
#[must_use]
fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// DATACENTER-19 — recommend a region for a NEW lighthouse that ADDS geographic
/// spread. PURE.
///
/// `available` is the available-region universe (the `do-regions` reply's slugs);
/// `used` is the regions the EXISTING lighthouses already sit in (read off the
/// panel's `droplet` rows). The recommendation:
///   * never recommends a region that already hosts a lighthouse (no spread gain);
///   * prefers a region in a DIFFERENT geo group (the slug's leading letters, e.g.
///     `nyc`/`sfo`/`fra`/`sgp`) from every used region — that's the honest
///     geo-spread nudge (doctl's region list exposes no latency/price, so the nudge
///     is geo-based, not latency/price-based);
///   * failing a new geo, falls back to any available region not already used;
///   * returns `None` when every available region is already used (or the universe
///     is empty) — the caller then surfaces "no spread-adding region".
///
/// `available` is taken in slug order, so the pick is deterministic for a given
/// input (first new-geo slug, else first unused slug).
#[must_use]
pub fn recommend_spread_region(available: &[String], used: &[String]) -> Option<String> {
    // The geo prefix of a slug: its leading ASCII letters (`nyc3` → `nyc`). An
    // all-digit / empty slug folds to "" — its own (degenerate) group.
    let geo =
        |slug: &str| -> String { slug.chars().take_while(char::is_ascii_alphabetic).collect() };
    let used_set: BTreeSet<&str> = used.iter().map(String::as_str).collect();
    let used_geos: BTreeSet<String> = used.iter().map(|s| geo(s)).collect();
    // First pass: an unused region whose geo group no existing lighthouse occupies.
    if let Some(r) = available
        .iter()
        .find(|s| !used_set.contains(s.as_str()) && !used_geos.contains(&geo(s)))
    {
        return Some(r.clone());
    }
    // Fallback: any unused region (same geo as an existing one is still a distinct
    // failure domain — better than recommending a region that's already hosting a
    // lighthouse).
    available
        .iter()
        .find(|s| !used_set.contains(s.as_str()))
        .cloned()
}

/// DATACENTER-19 — build a `digitalocean_droplet` Tofu resource for a new
/// lighthouse. It validates before interpolation and returns
/// `(resource_address, hcl_block)`. PURE — no I/O.
///
/// Every interpolated field is validated first: `name` → `[A-Za-z0-9._-]` (also the
/// droplet's `name`); `region`/`size`/`image` → DO region/size/image slug chars
/// (`[a-z0-9-]`).
///
/// The resource address is `digitalocean_droplet.lighthouse_<sanitized-name>`
/// (dots/dashes → underscores — an HCL block label must be a bare identifier). The
/// block tags `magic-lighthouse` (so the orchestrator's droplet inventory picks it
/// up) and registers the mesh SSH key, matching the `zone1-do` grow-path comment.
///
/// # Errors
/// Returns `Err` for any field that fails its validation.
pub fn lighthouse_create_resource(
    name: &str,
    region: &str,
    size: &str,
    image: &str,
) -> Result<(String, String), String> {
    if name.is_empty() {
        return Err("empty name".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("name contains invalid characters".into());
    }
    if !is_slug(region) {
        return Err("region is not a valid slug".into());
    }
    if size != THIN_LIGHTHOUSE_SIZE {
        return Err(format!(
            "lighthouse size must be the thin profile ({THIN_LIGHTHOUSE_SIZE})"
        ));
    }
    if !is_slug(size) {
        return Err("size is not a valid slug".into());
    }
    if !is_slug(image) {
        return Err("image is not a valid slug".into());
    }
    // An HCL block label must be a bare identifier — fold the name's `.`/`-` to `_`.
    let ident: String = name
        .chars()
        .map(|c| if matches!(c, '.' | '-') { '_' } else { c })
        .collect();
    let addr = format!("digitalocean_droplet.lighthouse_{ident}");
    let hcl = format!(
        "resource \"digitalocean_droplet\" \"lighthouse_{ident}\" {{\n  \
         name     = \"{name}\"\n  \
         region   = \"{region}\"\n  \
         size     = \"{size}\"\n  \
         image    = \"{image}\"\n  \
         tags     = [\"magic-lighthouse\"]\n  \
         ssh_keys = [digitalocean_ssh_key.mackes_mesh_claude.id]\n  \
         lifecycle {{\n    \
         ignore_changes = [image, user_data, ssh_keys, tags]\n  \
         }}\n}}\n"
    );
    Ok((addr, hcl))
}

/// DATACENTER-19 — the HCL block label inside a
/// `digitalocean_droplet.lighthouse_<ident>` address (the part after the
/// `digitalocean_droplet.` type prefix). PURE — for the duplicate check.
fn droplet_addr_label(addr: &str) -> &str {
    addr.strip_prefix("digitalocean_droplet.").unwrap_or(addr)
}

/// DATACENTER-19 — handle a `lighthouse-create` request: parse + validate, then
/// WRITE a `digitalocean_droplet` resource into the `zone1-do` workspace's
/// generated `dc-lighthouses.tf`. A duplicate `name` is rejected so a create
/// never silently overwrites an existing resource.
///
/// The actual `tofu apply` (live droplet provision) + the bootstrap (`mackesd found
/// --role lighthouse`) + the DNS record are the CARRIED live-DO step — this only
/// records the structural change in Tofu, so the provision goes through Tofu (no
/// drift). Replies `{"ok":true,"resource":..,"path":..}`.
fn lighthouse_create_reply(svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("lighthouse-create: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("lighthouse-create: bad json: {e}")),
    };
    let name = req
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let region = req
        .get("region")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // `size`/`image` default to the standard lighthouse slugs (the `zone1-do`
    // workspace's `lighthouse_size`/`lighthouse_image` variable defaults), so the
    // guided flow only requires name + region. The size is deliberately fail
    // closed in `lighthouse_create_resource`: larger and media/fileshare
    // lighthouse variants are retired.
    let size = req
        .get("size")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(THIN_LIGHTHOUSE_SIZE);
    let image = req
        .get("image")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("fedora-43-x64");
    let (addr, hcl) = match lighthouse_create_resource(name, region, size, image) {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // The generated file lives in the allow-listed `zone1-do` workspace under the
    // repo root the daemon runs in — the same tree `action/dc/tofu-apply` plans.
    let tf_dir = svc.workgroup_root.join("infra/tofu/zone1-do");
    let tf_path = tf_dir.join("dc-lighthouses.tf");
    let rel = "infra/tofu/zone1-do/dc-lighthouses.tf";
    // Refuse to overwrite an existing block for the same name (idempotent create).
    let existing = std::fs::read_to_string(&tf_path).unwrap_or_default();
    let marker = format!(
        "resource \"digitalocean_droplet\" \"{}\"",
        droplet_addr_label(&addr)
    );
    if existing.contains(&marker) {
        return err(format!(
            "a lighthouse resource named {name} already exists in {rel}"
        ));
    }
    if let Err(e) = std::fs::create_dir_all(&tf_dir) {
        return err(format!("lighthouse-create: cannot create {rel} dir: {e}"));
    }
    // Append the new block (a header comment is written once, on the first create).
    let mut out = existing;
    if out.is_empty() {
        out.push_str(
            "# DATACENTER-19 — Network-tab-created lighthouses (the guided\n\
             # new-lighthouse flow). Each block is written by the\n\
             # `action/dc/lighthouse-create` flow and PROVISIONED by a `tofu apply`\n\
             # of this workspace, so every create goes through Tofu (no drift). After\n\
             # apply: bootstrap mackesd + `mackesd found --role lighthouse`, then add\n\
             # the lighthouse-NN A record. Edit/remove via Tofu, not by hand.\n",
        );
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&hcl);
    if let Err(e) = std::fs::write(&tf_path, out) {
        return err(format!("lighthouse-create: cannot write {rel}: {e}"));
    }
    json!({ "ok": true, "resource": addr, "path": rel }).to_string()
}

// ── DATACENTER-18 — New-Mesh genesis ("give birth to a new Nebula") ──────────
//
// The genesis wizard's backend half — GLUE over DC-19, not a rewrite. It does NOT
// found a live mesh itself (founding is irreversible + costs real DO money + would
// create a rogue mesh), so — mirroring DC-19's `lighthouse-create` honesty — the
// verbs here only PLAN the genesis (`genesis-plan`, read-only) and WRITE the
// founding lighthouse's Tofu (`genesis-write`). The droplet half REUSES
// [`lighthouse_create_resource`] (the same `digitalocean_droplet` DC-19 emits);
// genesis adds the founding DNS A-record. The real `tofu apply` (droplet spend)
// goes through the gated `action/dc/tofu-apply` on `zone1-do`; the real `mackesd
// found` runs on the booted droplet (the founding cloud-init,
// `install-helpers/do-lighthouse-cloudinit.sh`). No credential is ever written to
// the repo/HCL/log here.

/// The ordered genesis step labels.
///
/// Shown in the wizard's review step and echoed in the `genesis-plan` reply so the
/// GUI and the backend agree on the sequence. PURE constant — the plan is
/// descriptive; each step is executed by a distinct, already-shipped primitive
/// (the DC-19 lighthouse Tofu-write + gated apply, the cloud-init `mackesd found`,
/// the DNS record, the printed join token).
pub const GENESIS_STEPS: [&str; 6] = [
    "generate the mesh CA (minted by `mackesd found` on the new lighthouse)",
    "provision the first lighthouse droplet (DigitalOcean, via the gated zone1-do tofu apply)",
    "found the mesh on it (`mackesd found` — self-signs, brings up the overlay)",
    "seed the founding bundle + bring up QNM-Shared / Caddy",
    "register the lighthouse DNS A-record",
    "emit the first single-use join token",
];

/// Validate a new-mesh id as typed. PURE.
///
/// A mesh id is a DNS-ish label: non-empty, ASCII lowercase alphanumeric + hyphen,
/// not starting/ending with a hyphen, at most 63 chars (it becomes the founding
/// droplet's name + DNS record + HCL block label). Rejects anything that could
/// widen the resource namespace or inject HCL.
///
/// # Errors
/// Returns `Err` describing the first rule the id breaks.
pub fn genesis_mesh_id_valid(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("empty mesh id".into());
    }
    if id.len() > 63 {
        return Err("mesh id too long (max 63 chars)".into());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("mesh id must be lowercase letters, digits, or hyphens".into());
    }
    if id.starts_with('-') || id.ends_with('-') {
        return Err("mesh id must not start or end with a hyphen".into());
    }
    Ok(())
}

/// Build the founding lighthouse's `digitalocean_droplet` + `digitalocean_record`
/// HCL for a brand-new `mesh_id` in `region`. PURE.
///
/// GLUE over DC-19: the droplet half is exactly [`lighthouse_create_resource`]
/// (named `lh-<mesh_id>-01`, standard lighthouse size/image), so the genesis
/// droplet is byte-identical to a DC-19 lighthouse the orchestrator already manages.
/// Genesis ADDS the founding DNS A-record (`lighthouse-<mesh_id>`) pointing at the
/// droplet's `ipv4_address` — the DC-19 flow leaves DNS to a manual step, but a
/// genesis IS the DNS-registration step, so it's emitted here. NO credential is
/// ever interpolated.
///
/// Returns `(droplet_resource_address, hcl_block)` (the droplet+record blocks).
///
/// # Errors
/// Returns `Err` if `mesh_id` / `region` fail their validation.
pub fn genesis_lighthouse_resource(
    mesh_id: &str,
    region: &str,
) -> Result<(String, String), String> {
    genesis_mesh_id_valid(mesh_id)?;
    if !is_slug(region) {
        return Err("region is not a valid slug".into());
    }
    let droplet_name = format!("lh-{mesh_id}-01");
    // REUSE DC-19's droplet HCL (standard lighthouse size/image defaults).
    let (addr, droplet_hcl) =
        lighthouse_create_resource(&droplet_name, region, THIN_LIGHTHOUSE_SIZE, "fedora-43-x64")?;
    // The block label DC-19 minted (`lighthouse_<ident>`) — reuse it for the record.
    let ident = droplet_addr_label(&addr)
        .strip_prefix("lighthouse_")
        .unwrap_or_else(|| droplet_addr_label(&addr));
    let record_name = format!("lighthouse-{mesh_id}");
    let record_hcl = format!(
        "\nresource \"digitalocean_record\" \"genesis_{ident}\" {{\n  \
         domain = digitalocean_domain.primary.id\n  \
         type   = \"A\"\n  \
         name   = \"{record_name}\"\n  \
         value  = digitalocean_droplet.lighthouse_{ident}.ipv4_address\n  \
         ttl    = 3600\n}}\n"
    );
    Ok((addr, format!("{droplet_hcl}{record_hcl}")))
}

/// Probe the mesh credential store for the presence of the `do-token` (the DO API
/// credential a live genesis apply needs). Returns only a boolean — the token is
/// NEVER read into a reply/log. A store/tooling failure is treated as "absent" (the
/// wizard then warns to provision it), never as a fake-present.
fn do_token_present() -> bool {
    let store = crate::ipc::secret_store::SecretStore::resolve(
        &crate::ipc::secret_store::repo_root(),
        &crate::default_qnm_shared_root(),
    );
    matches!(store.get("do-token"), Ok(Some(_)))
}

/// Build the `genesis-plan` reply value for `(mesh_id, region)`. PURE except for
/// the credential-store presence probe the caller injects via `do_token_present`.
///
/// Validates both inputs, then reports the ordered [`GENESIS_STEPS`], the Tofu
/// resource address the write would land, the rel path it writes, the gated
/// workspace (`zone1-do`), and `secrets_ready` (whether `do-token` is already in
/// the store — the boolean only, never the token). Read-only: it plans, never founds.
///
/// # Errors
/// Returns `Err(message)` for an invalid `mesh_id` / `region`.
pub fn genesis_plan(
    mesh_id: &str,
    region: &str,
    do_token_present: bool,
) -> Result<serde_json::Value, String> {
    let (addr, _hcl) = genesis_lighthouse_resource(mesh_id, region)?;
    Ok(json!({
        "ok": true,
        "mesh_id": mesh_id,
        "region": region,
        "steps": GENESIS_STEPS,
        "resource": addr,
        "path": "infra/tofu/zone1-do/dc-lighthouses.tf",
        "workspace": "zone1-do",
        // Only the PRESENCE boolean — never the credential itself.
        "secrets_ready": do_token_present,
    }))
}

/// Handle a `genesis-plan` request body `{ "mesh_id", "region" }` (read-only):
/// validate, probe the credential store for the `do-token` presence (the boolean
/// only), and reply with the ordered genesis steps + the Tofu resource preview +
/// the gated `zone1-do` workspace. It PLANS the genesis; it never founds a mesh.
fn genesis_plan_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("genesis-plan: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("genesis-plan: bad json: {e}")),
    };
    let mesh_id = req
        .get("mesh_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let region = req
        .get("region")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match genesis_plan(mesh_id, region, do_token_present()) {
        Ok(v) => v.to_string(),
        Err(e) => err(e),
    }
}

/// Handle a `genesis-write` request body `{ "mesh_id", "region", "confirm": true }`.
///
/// A STRUCTURAL change → it WRITES the founding lighthouse's `digitalocean_droplet`
/// resource (REUSING DC-19's [`lighthouse_create_resource`]) plus its founding DNS
/// A-record into the allow-listed `zone1-do` workspace's generated
/// `dc-lighthouses.tf` (idempotent; a repeated `mesh_id` is rejected so a write
/// never silently overwrites). It does NOT apply — the caller then runs the gated
/// `action/dc/tofu-apply` on `zone1-do` (the real droplet spend), and the live
/// `mackesd found` runs on the booted droplet (the founding cloud-init). The
/// destructive write requires `confirm == true` (the wizard's arm→confirm gate).
/// Replies `{"ok":true,"resource":..,"path":..,"workspace":"zone1-do"}`. NO
/// credential is read or written here.
fn genesis_write_reply(svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("genesis-write: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("genesis-write: bad json: {e}")),
    };
    // DESTRUCTIVE-ish (writes Tofu that founds a real mesh on apply): refuse unless
    // the caller explicitly confirms.
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return err("genesis-write requires confirm:true".into());
    }
    let mesh_id = req
        .get("mesh_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let region = req
        .get("region")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let (addr, hcl) = match genesis_lighthouse_resource(mesh_id, region) {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // The generated file lives in the allow-listed `zone1-do` workspace under the
    // repo root the daemon runs in — the same tree DC-19 + `tofu-apply` use.
    let tf_dir = svc.workgroup_root.join("infra/tofu/zone1-do");
    let tf_path = tf_dir.join("dc-lighthouses.tf");
    let rel = "infra/tofu/zone1-do/dc-lighthouses.tf";
    // Refuse to overwrite an existing block for the same mesh's founding droplet
    // (idempotent — the operator removes via Tofu, not by silently clobbering).
    let existing = std::fs::read_to_string(&tf_path).unwrap_or_default();
    let marker = format!(
        "resource \"digitalocean_droplet\" \"{}\"",
        droplet_addr_label(&addr)
    );
    if existing.contains(&marker) {
        return err(format!(
            "a genesis lighthouse for mesh {mesh_id} already exists in {rel}"
        ));
    }
    if let Err(e) = std::fs::create_dir_all(&tf_dir) {
        return err(format!("genesis-write: cannot create {rel} dir: {e}"));
    }
    // Append the new blocks (a header comment is written once, on the first write).
    let mut out = existing;
    if out.is_empty() {
        out.push_str(
            "# DATACENTER-18/19 — DO lighthouses written by the Datacenter flows\n\
             # (DC-19 add-lighthouse + DC-18 New-Mesh genesis) and PROVISIONED by a\n\
             # gated `tofu apply` of this workspace, so every create goes through Tofu\n\
             # (no drift). A genesis block also founds the mesh on the booted droplet\n\
             # via the founding cloud-init (`mackesd found`). All credentials come\n\
             # from the mesh credential store, never from this file. Edit/remove via\n\
             # Tofu, not by hand.\n",
        );
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&hcl);
    if let Err(e) = std::fs::write(&tf_path, out) {
        return err(format!("genesis-write: cannot write {rel}: {e}"));
    }
    // DAR-45 — echo the chosen backoffice tier so the wizard knows the genesis
    // recorded a backoffice opt-in. `backoffice_tier` in the body (minimal|full)
    // → `backoffice_intent {tier}` in the reply; ABSENT/off → null (behavior
    // unchanged — genesis-write does NOT itself record intent or run the
    // orchestrator; that stays `mackesd found --with-backoffice` / the operator).
    let backoffice_intent = match req
        .get("backoffice_tier")
        .and_then(serde_json::Value::as_str)
    {
        Some(t) if backoffice_tier_valid(t).is_ok() => json!({ "tier": t }),
        _ => serde_json::Value::Null,
    };
    json!({
        "ok": true,
        "resource": addr,
        "path": rel,
        "workspace": "zone1-do",
        "backoffice_intent": backoffice_intent,
    })
    .to_string()
}

/// DAR-45 — validate a backoffice tier. Accepts only `minimal` / `full`. PURE.
///
/// # Errors
/// Returns `Err` for any other tier.
fn backoffice_tier_valid(tier: &str) -> Result<&str, String> {
    match tier {
        "minimal" | "full" => Ok(tier),
        _ => Err(format!(
            "invalid backoffice tier '{tier}' (expected minimal|full)"
        )),
    }
}

/// Handle a `backoffice-plan` request body `{ "tier": "minimal"|"full" }` (READ-ONLY).
///
/// DAR-45 — the genesis-wizard's backoffice step fires this to render the ordered
/// unit list the orchestrator (`backoffice-up.sh`) would enable. It is a REAL
/// PROBE, not a canned list: it shells out to `automation/backoffice/backoffice-plan.sh
/// --tier <t>` (the single source of truth, which reads the tier manifest) and
/// passes its JSON `units` through verbatim — so the acceptance "RPC units MATCH
/// `backoffice-plan.sh --tier <t>`" holds by construction. The `secrets_ready`
/// boolean is RE-STAMPED from the SAME [`do_token_present`] probe the genesis-plan
/// step uses, so both wizard steps report the credential state identically (and the
/// wizard never has to trust the script's own probe for that one field). The script
/// mutates nothing; this verb takes no op-lock (read-only).
///
/// On a missing script / non-zero exit (e.g. a broken manifest with a dangling
/// `via_script`), replies `{"error":..}` carrying the script's stderr — surfaced
/// honestly rather than faked-present.
fn backoffice_plan_reply(_svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("backoffice-plan: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("backoffice-plan: bad json: {e}")),
    };
    let tier = req
        .get("tier")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if let Err(e) = backoffice_tier_valid(tier) {
        return err(e);
    }
    // Resolve the planner under the deployed repo root (the `MCNF_REPO` convention,
    // same as the secret store) so the relative `automation/...` path resolves
    // regardless of the daemon's cwd (`/` under systemd).
    let repo = crate::ipc::secret_store::repo_root();
    let script = repo.join("automation/backoffice/backoffice-plan.sh");
    if !script.is_file() {
        return err(format!(
            "backoffice-plan: planner not found at {} (is the repo deployed?)",
            script.display()
        ));
    }
    let output = std::process::Command::new("bash")
        .arg(&script)
        .arg("--tier")
        .arg(tier)
        .current_dir(&repo)
        .output();
    let out = match output {
        Ok(o) => o,
        Err(e) => return err(format!("backoffice-plan: spawn failed: {e}")),
    };
    if !out.status.success() {
        // The script prints its JSON on stdout and the broken-unit diagnostic on
        // stderr; surface stderr so a dangling via_script is named, not hidden.
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = msg.trim();
        return err(format!(
            "backoffice-plan: planner failed{}",
            if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            }
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut plan: serde_json::Value = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(e) => {
            return err(format!(
                "backoffice-plan: planner output not decodable: {e}"
            ))
        }
    };
    // Re-stamp secrets_ready from the SAME Rust probe genesis-plan uses, so the two
    // wizard steps agree (and the boolean is the daemon's view, not the shell's).
    if let Some(obj) = plan.as_object_mut() {
        obj.insert("secrets_ready".into(), json!(do_token_present()));
    }
    plan.to_string()
}

/// Run the datacenter Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DatacenterService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(
    persist: &Persist,
    svc: &DatacenterService,
    cursors: &mut HashMap<String, String>,
) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since_limit(&topic, since, MAX_MESSAGES_PER_POLL) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_authorized_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "dc responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Process-wide environment mutations are serialized across planner tests.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const AUTH_KEY: &[u8] = b"datacenter-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn authorized_service(root: &std::path::Path) -> DatacenterService {
        DatacenterService::new(root.to_path_buf()).with_authorizer(Arc::new(
            ActionAuthorizer::for_test(AUTH_KEY, root.join("auth"), AUTH_NOW),
        ))
    }

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn production_dispatch_refuses_unsigned_structural_mutations_before_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = authorized_service(tmp.path());
        let cases = [
            (
                "lighthouse-create",
                json!({ "name": "unsigned-lighthouse", "region": "sfo3" }).to_string(),
                "infra/tofu/zone1-do/dc-lighthouses.tf",
            ),
            (
                "genesis-write",
                json!({ "mesh_id": "unsigned-mesh", "region": "sfo3", "confirm": true })
                    .to_string(),
                "infra/tofu/zone1-do/dc-lighthouses.tf",
            ),
        ];

        for (verb, body, output) in cases {
            let reply = build_authorized_reply(&svc, verb, Some(&body));
            assert!(reply.contains("authorization refused"), "{verb}: {reply}");
            assert!(
                !tmp.path().join(output).exists(),
                "{verb} wrote {output} before authorization: {reply}"
            );
        }
    }

    #[test]
    fn retired_vm_actions_are_absent_and_refused() {
        let svc = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        for verb in [
            "vm-power",
            "vm-snapshot",
            "vm-clone",
            "vm-delete",
            "vm-console",
            "vm-suspend",
            "vm-migrate",
            "vm-resize",
            "vm-create",
            "vm-snapshots",
            "vm-snapshot-revert",
            "vm-snapshot-delete",
        ] {
            assert!(
                !ACTION_VERBS.contains(&verb),
                "retired verb registered: {verb}"
            );
            assert_eq!(lock_key(verb, Some(r#"{"uuid":"abcd-1234"}"#)), None);
            let reply = build_authorized_reply(&svc, verb, None);
            assert!(reply.contains("unknown dc verb"), "{verb}: {reply}");
        }

        assert_eq!(action_topic("do-regions"), "action/dc/do-regions");
        assert_eq!(
            action_topic("lighthouse-create"),
            "action/dc/lighthouse-create"
        );
        assert!(ACTION_VERBS.contains(&"do-regions"));
        assert!(ACTION_VERBS.contains(&"lighthouse-create"));
    }

    // ── DAR-45 — genesis-wizard backoffice step + backoffice-plan verb ──

    #[test]
    fn backoffice_plan_verb_and_topic_registered() {
        assert!(ACTION_VERBS.contains(&"backoffice-plan"));
        assert_eq!(action_topic("backoffice-plan"), "action/dc/backoffice-plan");
        // read-only → no op-lock key (so two plan probes never collide / reject).
        assert!(lock_key("backoffice-plan", Some(r#"{"tier":"full"}"#)).is_none());
    }

    #[test]
    fn backoffice_tier_validation() {
        assert!(backoffice_tier_valid("minimal").is_ok());
        assert!(backoffice_tier_valid("full").is_ok());
        assert!(backoffice_tier_valid("bogus").is_err());
        assert!(backoffice_tier_valid("").is_err());
    }

    #[test]
    fn backoffice_plan_reply_rejects_bad_input() {
        let svc = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // Missing body.
        let r = backoffice_plan_reply(&svc, None);
        assert!(r.contains("error"), "{r}");
        // Bad json.
        let r = backoffice_plan_reply(&svc, Some("not json"));
        assert!(r.contains("error") && r.contains("bad json"), "{r}");
        // Invalid tier.
        let r = backoffice_plan_reply(&svc, Some(r#"{"tier":"bogus"}"#));
        assert!(
            r.contains("error") && r.contains("invalid backoffice tier"),
            "{r}"
        );
    }

    #[test]
    fn backoffice_plan_reply_matches_the_script_output() {
        // The acceptance: the RPC's rendered unit list MUST match
        // `backoffice-plan.sh --tier <t>` output (a REAL probe, not canned). We
        // point MCNF_REPO at the worktree (CARGO_MANIFEST_DIR is crates/mesh/mackesd,
        // so ../../.. is the repo root) and run BOTH the verb and the script, then
        // assert their `units` arrays are identical. Skips gracefully if the script
        // isn't present in this checkout (so the suite stays green off-repo).
        let _g = lock_env();
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .expect("repo root");
        let script = repo.join("automation/backoffice/backoffice-plan.sh");
        if !script.is_file() {
            eprintln!(
                "skipping: {} not present in this checkout",
                script.display()
            );
            return;
        }
        // Test-only env set under the serializing ENV_LOCK.
        let prev_repo = std::env::var_os("MCNF_REPO");
        std::env::set_var("MCNF_REPO", &repo);
        let svc = DatacenterService::new(repo.clone());

        for tier in ["minimal", "full"] {
            // The verb's reply.
            let body = format!(r#"{{"tier":"{tier}"}}"#);
            let reply = backoffice_plan_reply(&svc, Some(&body));
            let rpc: serde_json::Value = serde_json::from_str(&reply)
                .unwrap_or_else(|e| panic!("rpc json {tier}: {e}\n{reply}"));
            assert_eq!(rpc["ok"], true, "rpc not ok for {tier}: {reply}");
            assert_eq!(rpc["tier"], tier, "rpc tier mismatch: {reply}");

            // The script's own output (the source of truth).
            let out = std::process::Command::new("bash")
                .arg(&script)
                .arg("--tier")
                .arg(tier)
                .current_dir(&repo)
                .output()
                .expect("run backoffice-plan.sh");
            assert!(
                out.status.success(),
                "script failed for {tier}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let script_json: serde_json::Value =
                serde_json::from_slice(&out.stdout).expect("script json");

            // The REAL probe: the rendered unit list matches byte-for-byte (same
            // ids, phases, ordering, live_gated, via_script). This is the canned-vs-
            // real assertion the critique demanded.
            assert_eq!(
                rpc["units"], script_json["units"],
                "RPC units must MATCH backoffice-plan.sh --tier {tier} (not a canned list)"
            );
            // secrets_ready is a bool either way (re-stamped from the Rust probe).
            assert!(rpc["secrets_ready"].is_boolean(), "{reply}");
        }
        match prev_repo {
            Some(v) => std::env::set_var("MCNF_REPO", v),
            None => std::env::remove_var("MCNF_REPO"),
        }
    }

    #[test]
    fn parse_regions_parses_doctl_json() {
        let json = r#"[
            {"slug":"nyc3","name":"New York 3","available":true,"sizes":["s-1vcpu-512mb-10gb","s-1vcpu-1gb"]},
            {"slug":"ams2","name":"Amsterdam 2","available":false}
        ]"#;
        let regions = parse_regions(json);
        assert_eq!(
            regions,
            vec![
                ("nyc3".to_string(), "New York 3".to_string(), true),
                ("ams2".to_string(), "Amsterdam 2".to_string(), false),
            ]
        );
    }

    #[test]
    fn parse_regions_garbage_is_empty() {
        assert!(parse_regions("not json at all").is_empty());
        // valid JSON but not an array
        assert!(parse_regions(r#"{"slug":"nyc3"}"#).is_empty());
        // empty array
        assert!(parse_regions("[]").is_empty());
    }

    // ── DATACENTER-18 — New-Mesh genesis ──

    #[test]
    fn genesis_verbs_and_topics_are_registered() {
        assert!(ACTION_VERBS.contains(&"genesis-plan"));
        assert!(ACTION_VERBS.contains(&"genesis-write"));
        assert_eq!(action_topic("genesis-plan"), "action/dc/genesis-plan");
        assert_eq!(action_topic("genesis-write"), "action/dc/genesis-write");
    }

    #[test]
    fn genesis_mesh_id_validation() {
        assert!(genesis_mesh_id_valid("home-mesh").is_ok());
        assert!(genesis_mesh_id_valid("m1").is_ok());
        assert!(genesis_mesh_id_valid("").is_err());
        // uppercase / underscore / space / dot are rejected (DNS-ish label only)
        assert!(genesis_mesh_id_valid("HomeMesh").is_err());
        assert!(genesis_mesh_id_valid("home_mesh").is_err());
        assert!(genesis_mesh_id_valid("home mesh").is_err());
        assert!(genesis_mesh_id_valid("home.mesh").is_err());
        // injection-ish characters rejected
        assert!(genesis_mesh_id_valid("a;rm -rf /").is_err());
        assert!(genesis_mesh_id_valid("a\"b").is_err());
        // leading/trailing hyphen rejected
        assert!(genesis_mesh_id_valid("-mesh").is_err());
        assert!(genesis_mesh_id_valid("mesh-").is_err());
        // too long
        assert!(genesis_mesh_id_valid(&"a".repeat(64)).is_err());
    }

    #[test]
    fn genesis_lighthouse_resource_reuses_dc19_droplet_adds_dns_no_secret() {
        let (addr, hcl) = genesis_lighthouse_resource("home-mesh", "nyc3").unwrap();
        // The droplet half is exactly DC-19's lighthouse resource for lh-<id>-01.
        let (dc19_addr, _) = lighthouse_create_resource(
            "lh-home-mesh-01",
            "nyc3",
            THIN_LIGHTHOUSE_SIZE,
            "fedora-43-x64",
        )
        .unwrap();
        assert_eq!(addr, dc19_addr);
        assert!(hcl.contains("resource \"digitalocean_droplet\" \"lighthouse_lh_home_mesh_01\""));
        assert!(hcl.contains("name     = \"lh-home-mesh-01\""));
        assert!(hcl.contains("region   = \"nyc3\""));
        // Genesis ADDS the founding DNS A-record.
        assert!(hcl.contains("resource \"digitalocean_record\" \"genesis_lh_home_mesh_01\""));
        assert!(hcl.contains("name   = \"lighthouse-home-mesh\""));
        assert!(hcl.contains(".ipv4_address"));
        // SECRET-HANDLING: no credential material is ever emitted into the HCL.
        let lc = hcl.to_lowercase();
        assert!(
            !lc.contains("token"),
            "HCL must not carry a DO token: {hcl}"
        );
        assert!(
            !lc.contains("passphrase"),
            "HCL must not carry a passphrase"
        );
        assert!(
            !lc.contains("private"),
            "HCL must not carry private key material"
        );
    }

    #[test]
    fn genesis_lighthouse_resource_rejects_invalid_inputs() {
        assert!(genesis_lighthouse_resource("", "nyc3").is_err());
        assert!(genesis_lighthouse_resource("home_mesh", "nyc3").is_err());
        // an invalid region slug is rejected by the reused DC-19 is_slug guard
        assert!(genesis_lighthouse_resource("home-mesh", "NYC3").is_err());
    }

    #[test]
    fn genesis_plan_reports_steps_resource_and_secret_presence() {
        // do_token absent → secrets_ready:false (the wizard warns before a live apply).
        let plan = genesis_plan("home-mesh", "nyc3", false).unwrap();
        assert_eq!(plan["ok"], true);
        assert_eq!(plan["mesh_id"], "home-mesh");
        assert_eq!(plan["region"], "nyc3");
        assert_eq!(plan["workspace"], "zone1-do");
        assert_eq!(plan["path"], "infra/tofu/zone1-do/dc-lighthouses.tf");
        assert_eq!(
            plan["resource"],
            "digitalocean_droplet.lighthouse_lh_home_mesh_01"
        );
        assert_eq!(plan["secrets_ready"], false);
        let steps = plan["steps"].as_array().unwrap();
        assert_eq!(steps.len(), GENESIS_STEPS.len());
        // do_token present → secrets_ready:true.
        let ready = genesis_plan("home-mesh", "nyc3", true).unwrap();
        assert_eq!(ready["secrets_ready"], true);
    }

    #[test]
    fn genesis_plan_reply_validates_inputs() {
        assert!(genesis_plan_reply(None).contains("missing request body"));
        assert!(genesis_plan_reply(Some("not json")).contains("bad json"));
        let bad = json!({ "mesh_id": "Bad_Id", "region": "nyc3" }).to_string();
        assert!(genesis_plan_reply(Some(&bad)).contains("error"));
    }

    #[test]
    fn genesis_write_requires_confirm_true() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp/dc18-noconfirm"));
        // confirm missing
        let body = json!({ "mesh_id": "home-mesh", "region": "nyc3" }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("genesis-write requires confirm:true"), "{r}");
        // confirm false
        let body =
            json!({ "mesh_id": "home-mesh", "region": "nyc3", "confirm": false }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("genesis-write requires confirm:true"), "{r}");
        // confirm as a non-bool string does not satisfy the gate
        let body =
            json!({ "mesh_id": "home-mesh", "region": "nyc3", "confirm": "true" }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("genesis-write requires confirm:true"), "{r}");
    }

    #[test]
    fn genesis_write_lands_tofu_and_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!("dc18-genesis-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let s = DatacenterService::new(tmp.clone());
        let body = json!({ "mesh_id": "home-mesh", "region": "nyc3", "confirm": true }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("\"ok\":true"), "{r}");
        assert!(
            r.contains("digitalocean_droplet.lighthouse_lh_home_mesh_01"),
            "{r}"
        );
        let tf = tmp.join("infra/tofu/zone1-do/dc-lighthouses.tf");
        let written = std::fs::read_to_string(&tf).unwrap();
        assert!(
            written.contains("resource \"digitalocean_droplet\" \"lighthouse_lh_home_mesh_01\"")
        );
        assert!(written.contains("resource \"digitalocean_record\" \"genesis_lh_home_mesh_01\""));
        assert!(written.contains("DATACENTER-18/19"));
        // No credential material reached the file.
        assert!(!written.to_lowercase().contains("token"));
        // A second write for the SAME mesh id is rejected.
        let r2 = build_reply(&s, "genesis-write", Some(&body));
        assert!(r2.contains("already exists"), "{r2}");
        // A DIFFERENT mesh id appends a second pair of blocks.
        let body2 = json!({ "mesh_id": "lab-mesh", "region": "fra1", "confirm": true }).to_string();
        let r3 = build_reply(&s, "genesis-write", Some(&body2));
        assert!(r3.contains("\"ok\":true"), "{r3}");
        let written2 = std::fs::read_to_string(&tf).unwrap();
        assert!(written2.contains("lighthouse_lh_home_mesh_01"));
        assert!(written2.contains("lighthouse_lh_lab_mesh_01"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn genesis_write_rejects_invalid_mesh_id_before_writing() {
        let tmp = std::env::temp_dir().join(format!("dc18-genesis-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let s = DatacenterService::new(tmp.clone());
        let body = json!({ "mesh_id": "Bad_Id", "region": "nyc3", "confirm": true }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("error"), "{r}");
        assert!(!tmp.join("infra/tofu/zone1-do/dc-lighthouses.tf").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn genesis_plan_takes_no_lock_write_locks_on_mesh_id() {
        assert_eq!(
            lock_key("genesis-plan", Some(r#"{"mesh_id":"home-mesh"}"#)),
            None
        );
        assert_eq!(
            lock_key("genesis-write", Some(r#"{"mesh_id":"home-mesh"}"#)),
            Some("mesh-new:home-mesh".to_string())
        );
        assert_eq!(lock_key("genesis-write", Some(r#"{"mesh_id":""}"#)), None);
        assert_eq!(lock_key("genesis-write", Some("{}")), None);
    }

    #[test]
    fn recommend_spread_prefers_a_new_geo() {
        let available = vec![
            "nyc1".to_string(),
            "nyc3".to_string(),
            "sfo3".to_string(),
            "fra1".to_string(),
        ];
        // Lighthouses already sit in nyc3 (geo `nyc`). The pick must skip every
        // `nyc*` region and land on the first region of a NEW geo (sfo).
        let used = vec!["nyc3".to_string()];
        assert_eq!(
            recommend_spread_region(&available, &used),
            Some("sfo3".to_string())
        );
    }

    #[test]
    fn recommend_spread_never_recommends_a_used_region() {
        let available = vec!["nyc3".to_string(), "sfo3".to_string()];
        let used = vec!["nyc3".to_string(), "sfo3".to_string()];
        // Every available region already hosts a lighthouse → no spread to add.
        assert_eq!(recommend_spread_region(&available, &used), None);
    }

    #[test]
    fn recommend_spread_falls_back_to_an_unused_region_when_all_geos_taken() {
        // Both geos (nyc, sfo) are occupied, but a SECOND nyc region (nyc1) is free.
        // No new geo is available, so the fallback picks the unused nyc1 (a distinct
        // failure domain still beats recommending a region already in use).
        let available = vec!["nyc1".to_string(), "nyc3".to_string(), "sfo3".to_string()];
        let used = vec!["nyc3".to_string(), "sfo3".to_string()];
        assert_eq!(
            recommend_spread_region(&available, &used),
            Some("nyc1".to_string())
        );
    }

    #[test]
    fn recommend_spread_with_no_used_picks_the_first_available() {
        let available = vec!["ams3".to_string(), "nyc3".to_string()];
        // No existing lighthouses → any region adds spread; deterministic first pick.
        assert_eq!(
            recommend_spread_region(&available, &[]),
            Some("ams3".to_string())
        );
        // Empty universe → nothing to recommend.
        assert_eq!(recommend_spread_region(&[], &["nyc3".to_string()]), None);
    }

    #[test]
    fn lighthouse_create_resource_emits_valid_hcl() {
        let (addr, hcl) = lighthouse_create_resource(
            "lighthouse-04",
            "sfo3",
            THIN_LIGHTHOUSE_SIZE,
            "fedora-43-x64",
        )
        .unwrap();
        assert_eq!(addr, "digitalocean_droplet.lighthouse_lighthouse_04");
        assert!(hcl.contains("resource \"digitalocean_droplet\" \"lighthouse_lighthouse_04\""));
        assert!(hcl.contains("name     = \"lighthouse-04\""));
        assert!(hcl.contains("region   = \"sfo3\""));
        assert!(hcl.contains("size     = \"s-1vcpu-512mb-10gb\""));
        assert!(hcl.contains("image    = \"fedora-43-x64\""));
        assert!(hcl.contains("tags     = [\"magic-lighthouse\"]"));
        assert!(hcl.contains("digitalocean_ssh_key.mackes_mesh_claude.id"));
    }

    #[test]
    fn lighthouse_create_resource_rejects_unsafe_fields() {
        assert!(lighthouse_create_resource("", "sfo3", "s", "f").is_err());
        assert!(lighthouse_create_resource("a b", "sfo3", "s", "f").is_err());
        assert!(lighthouse_create_resource("a;rm", "sfo3", "s", "f").is_err());
        // A region/size/image must be a lowercase slug — uppercase / shell metachars
        // / spaces are rejected before they reach the HCL.
        assert!(lighthouse_create_resource("ok", "SFO3", "s", "f").is_err());
        assert!(lighthouse_create_resource("ok", "sfo3;rm", "s", "f").is_err());
        assert!(lighthouse_create_resource("ok", "", "s", "f").is_err());
        assert!(lighthouse_create_resource("ok", "sfo3", "s 1", "f").is_err());
        assert!(lighthouse_create_resource("ok", "sfo3", "s", "F").is_err());
    }

    #[test]
    fn lighthouse_create_resource_rejects_a_larger_profile() {
        let error =
            lighthouse_create_resource("lighthouse-04", "sfo3", "s-2vcpu-2gb", "fedora-43-x64")
                .unwrap_err();
        assert!(error.contains(THIN_LIGHTHOUSE_SIZE), "{error}");
    }

    #[test]
    fn lighthouse_create_reply_writes_a_tofu_resource_and_rejects_a_dup() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DatacenterService::new(tmp.path().to_path_buf());
        // Only name + region are required; size/image default to the lighthouse slugs.
        let body = json!({ "name": "lighthouse-04", "region": "sfo3" }).to_string();
        let r = build_reply(&svc, "lighthouse-create", Some(&body));
        assert!(r.contains("\"ok\":true"), "expected ok, got: {r}");
        assert!(
            r.contains("digitalocean_droplet.lighthouse_lighthouse_04"),
            "{r}"
        );

        // The generated file exists and carries the block + the one-time header, and
        // the defaulted size/image slugs.
        let tf = std::fs::read_to_string(tmp.path().join("infra/tofu/zone1-do/dc-lighthouses.tf"))
            .unwrap();
        assert!(tf.contains("DATACENTER-19"));
        assert!(tf.contains("resource \"digitalocean_droplet\" \"lighthouse_lighthouse_04\""));
        assert!(tf.contains("region   = \"sfo3\""));
        assert!(tf.contains("size     = \"s-1vcpu-512mb-10gb\""));
        assert!(tf.contains("image    = \"fedora-43-x64\""));

        // A second create of the SAME name is rejected (no silent overwrite).
        let r2 = build_reply(&svc, "lighthouse-create", Some(&body));
        assert!(r2.contains("already exists"), "expected dup reject: {r2}");
    }

    #[test]
    fn lighthouse_create_reply_rejects_a_bad_region() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DatacenterService::new(tmp.path().to_path_buf());
        let body = json!({ "name": "lighthouse-04", "region": "SFO3;rm" }).to_string();
        let r = build_reply(&svc, "lighthouse-create", Some(&body));
        assert!(r.contains("region is not a valid slug"), "{r}");
        // No file is written on a validation failure.
        assert!(!tmp
            .path()
            .join("infra/tofu/zone1-do/dc-lighthouses.tf")
            .exists());
    }

    #[test]
    fn lighthouse_create_lock_key_is_name_scoped() {
        let body = json!({ "name": "lighthouse-04", "region": "sfo3" }).to_string();
        assert_eq!(
            lock_key("lighthouse-create", Some(&body)),
            Some("lighthouse-new:lighthouse-04".to_string())
        );
        // An empty/missing name → no lock (the per-verb handler produces the error).
        assert_eq!(lock_key("lighthouse-create", Some(r#"{"name":""}"#)), None);
        assert_eq!(lock_key("lighthouse-create", Some("{}")), None);
    }

    // DATACENTER-11 — snapshot list / revert / delete command builders + replies --

    #[test]
    fn poller_does_not_consume_retired_vm_action_topics() {
        use mde_bus::rpc::publish_request;

        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let svc = DatacenterService::new(tmp.path().to_path_buf());
        let topic = "action/dc/vm-power";
        let request = publish_request(
            &persist,
            topic,
            Priority::Default,
            None,
            Some(r#"{"uuid":"abcd-1234","op":"start"}"#),
        )
        .unwrap();

        let mut cursors = HashMap::new();
        poll_once(&persist, &svc, &mut cursors);

        assert!(!cursors.contains_key(topic));
        assert!(persist
            .list_since(&reply_topic(&request), None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn datacenter_action_recovery_reads_a_bounded_page_and_advances_cursor() {
        use mde_bus::rpc::publish_request;

        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let svc = DatacenterService::new(tmp.path().to_path_buf());
        let topic = action_topic("genesis-plan");
        let mut requests = Vec::new();
        for index in 0..(MAX_MESSAGES_PER_POLL + 1) {
            let body = json!({
                "mesh_id": format!("mesh-{index}"),
                "region": "nyc3"
            })
            .to_string();
            requests.push(
                publish_request(&persist, &topic, Priority::Default, None, Some(&body)).unwrap(),
            );
        }

        let first_page = persist
            .list_since_limit(&topic, None, MAX_MESSAGES_PER_POLL)
            .unwrap();
        assert_eq!(first_page.len(), MAX_MESSAGES_PER_POLL);
        assert_eq!(first_page[0].ulid, requests[0]);
        assert_eq!(first_page[63].ulid, requests[63]);

        let mut cursors = HashMap::new();
        poll_once(&persist, &svc, &mut cursors);
        assert_eq!(
            cursors.get(&topic),
            Some(&requests[MAX_MESSAGES_PER_POLL - 1])
        );
        for request in &requests[..MAX_MESSAGES_PER_POLL] {
            assert_eq!(
                persist
                    .list_since(&reply_topic(request), None)
                    .unwrap()
                    .len(),
                1,
                "first page request did not receive a reply"
            );
        }
        assert!(persist
            .list_since(&reply_topic(&requests[MAX_MESSAGES_PER_POLL]), None)
            .unwrap()
            .is_empty());

        poll_once(&persist, &svc, &mut cursors);
        assert_eq!(cursors.get(&topic), Some(&requests[MAX_MESSAGES_PER_POLL]));
        assert_eq!(
            persist
                .list_since(&reply_topic(&requests[MAX_MESSAGES_PER_POLL]), None)
                .unwrap()
                .len(),
            1
        );
    }
}
