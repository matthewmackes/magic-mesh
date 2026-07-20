//! Workloads U2 — the backend-execution seam of the `cloud` worker: shell
//! OpenTofu / Ansible / virsh through the injectable [`CloudRunner`] trait.
//!
//! Split out of the pre-U2 monolith (behavior-preserving relocation): the runner
//! trait, its production [`ShellCloudRunner`], the [`CloudRunOutcome`] result, the
//! per-tool health probe, the roster fold, and the injectable test fake all live
//! here so the drain / gate / reply paths (in the sibling modules) are exercised
//! WITHOUT a live hypervisor. Nothing here changed in U2 — only its home did.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use mackes_mesh_types::cloud::{
    CloudInstance, EndpointInterface, HealthState, LifecycleAction, ResourceRow, ResourceTable,
    ServiceHealth,
};

// ── IaC tree + libvirt env overrides ──

/// The env override for the IaC tree root (holds `infra/tofu/cloud` + the Ansible
/// tree). Defaults to [`DEFAULT_IAC_ROOT`] when unset.
pub(crate) const IAC_ROOT_ENV: &str = "MDE_IAC_ROOT";

/// The env override for the libvirt connection URI the runner drives.
pub(crate) const LIBVIRT_URI_ENV: &str = "MDE_LIBVIRT_URI";

/// Default IaC tree root when [`IAC_ROOT_ENV`] is unset (a deployed node ships the
/// tree here; a dev checkout sets `MDE_IAC_ROOT` to the repo root).
pub(crate) const DEFAULT_IAC_ROOT: &str = "/usr/share/mde/iac";

/// Default libvirt connection URI (local system KVM — E12 local-first).
pub(crate) const DEFAULT_LIBVIRT_URI: &str = "qemu:///system";

/// The OpenTofu root (provision), relative to the IaC root.
pub(crate) const TOFU_SUBDIR: &str = "infra/tofu/cloud";

/// The Ansible tree (configure), relative to the IaC root.
pub(crate) const ANSIBLE_SUBDIR: &str = "automation/ansible";

// ── backend tool identifiers (the `service_type` on each health row) ──
/// OpenTofu (provision leg).
pub(crate) const TOOL_TOFU: &str = "opentofu";
/// Ansible (configure leg).
pub(crate) const TOOL_ANSIBLE: &str = "ansible";
/// libvirt/KVM (the local VM backend).
pub(crate) const TOOL_LIBVIRT: &str = "libvirt";

/// Every backend tool the state mirror reports health for, in render order.
pub(crate) const BACKEND_TOOLS: [&str; 3] = [TOOL_TOFU, TOOL_ANSIBLE, TOOL_LIBVIRT];

/// The honest outcome of one runner op — its success, a short human summary, and
/// whether a live mutation was actually attempted (drives the reply's `audited`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRunOutcome {
    /// Whether the op succeeded.
    pub ok: bool,
    /// A short human summary (a `tofu`/`ansible` result line, or the failure).
    pub summary: String,
    /// Whether a live mutation was attempted (`apply == true`). A staged dry-run is
    /// `false` (nothing was applied).
    pub applied: bool,
}

impl CloudRunOutcome {
    /// A successful outcome.
    #[must_use]
    pub fn ok(summary: impl Into<String>, applied: bool) -> Self {
        Self {
            ok: true,
            summary: summary.into(),
            applied,
        }
    }

    /// A failed outcome (never marked applied — a failure durably changed nothing
    /// we can claim).
    #[must_use]
    pub fn failed(summary: impl Into<String>) -> Self {
        Self {
            ok: false,
            summary: summary.into(),
            applied: false,
        }
    }
}

/// The captured result of one generic backend-tool invocation through the
/// [`CloudRunner::run_tool`] seam (`bootc-image-builder` / `osbuild` /
/// `ansible-playbook` / `podman` …). `ok` is the process exit success; the streams
/// are captured so the image-build (U6) + container-deploy (U7) verbs parse them
/// (an artifact path, a `PLAY RECAP`) and surface an honest failure detail. A spawn
/// failure (the binary is absent) is an `Err` from `run_tool`, never a fabricated
/// `ToolRun` (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRun {
    /// Whether the process exited successfully.
    pub ok: bool,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

/// The backend-execution seam the worker drives: shell OpenTofu / Ansible / virsh.
///
/// Production is [`ShellCloudRunner`]; tests inject a fake so the drain, gate,
/// reply, audit, and state-publish paths are exercised WITHOUT a live hypervisor
/// (the `router_action` applier-injection idiom).
pub trait CloudRunner: Send + Sync {
    /// Honestly probe whether backend tool `tool` ([`BACKEND_TOOLS`]) is present +
    /// reachable, as a [`ServiceHealth`] row (`Up` / `Down` / `Absent`, never a
    /// fabricated up).
    fn probe_tool(&self, tool: &str) -> ServiceHealth;

    /// List the local cloud instances (a READ — `virsh list`). `Err` when the
    /// backend can't be reached (an honest gate, never a fabricated empty roster).
    fn list_instances(&self) -> Result<Vec<CloudInstance>, String>;

    /// Provision via OpenTofu. `apply` ⇒ `tofu apply`; else `tofu plan` (staged).
    fn provision(&self, apply: bool) -> CloudRunOutcome;

    /// Configure via Ansible. `apply` ⇒ `ansible-playbook`; else `--check` (staged).
    fn configure(&self, apply: bool) -> CloudRunOutcome;

    /// Destroy via OpenTofu. `apply` ⇒ `tofu destroy`; else `tofu plan -destroy`.
    fn destroy(&self, apply: bool) -> CloudRunOutcome;

    /// A per-instance lifecycle op via `virsh`. `apply` ⇒ the real op; else staged.
    fn lifecycle(&self, action: LifecycleAction, instance: &str, apply: bool) -> CloudRunOutcome;

    /// Workloads U4/U5 — write the caller-rendered `terraform.tfvars.json` into the
    /// OpenTofu root and run `tofu plan -json`, returning the raw newline-delimited
    /// JSON stream (the caller parses the `change_summary` into `PlanCounts`).
    ///
    /// A staged READ (never applies): it renders + plans a node's desired slice.
    ///
    /// # Errors
    /// An honest `Err` when the tfvars can't be written or `tofu` can't run (tool
    /// absent / plan failed) — never a fabricated empty plan.
    fn plan_json(&self, tfvars_json: &str) -> Result<String, String>;
    /// Run an arbitrary backend tool `bin` with `args`, capturing its output — the
    /// generic seam the image-build (U6) + container-deploy (U7) verbs drive their
    /// per-tool pipelines through (`bootc-image-builder`/`osbuild` for a golden disk;
    /// `ansible-playbook` for the Quadlet install), so those verbs stay fully
    /// fake-testable without the real tools installed. `Err` ONLY on a spawn failure
    /// (the binary is absent / unexecutable), so the caller reports an honest "tool
    /// unavailable" gate rather than a fabricated failure (§7). The default shells the
    /// process; the injected test fake scripts it.
    fn run_tool(&self, bin: &str, args: &[&str]) -> Result<ToolRun, String> {
        let out = Command::new(bin)
            .args(args)
            .output()
            .map_err(|e| format!("{bin}: {e}"))?;
        Ok(ToolRun {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
    /// Resolve the mesh Ansible inventory (`ansible-inventory --list`) as raw JSON —
    /// a READ (U10). `Err` when the tool is absent or the resolve fails (an honest
    /// gate, never a fabricated inventory). The default is "unsupported by this
    /// runner" so a partial runner degrades honestly.
    ///
    /// # Errors
    /// The tool's failure/absence, as an honest message.
    fn resolve_inventory(&self) -> Result<String, String> {
        Err("inventory resolution not supported by this runner".to_string())
    }

    /// The tofu outputs (`tofu output -json`) for this node's workloads as raw JSON —
    /// a READ (U10). `Err` when `tofu` is absent or the read fails. Default:
    /// unsupported.
    ///
    /// # Errors
    /// The tool's failure/absence, as an honest message.
    fn tofu_outputs(&self) -> Result<String, String> {
        Err("tofu outputs not supported by this runner".to_string())
    }
}

/// Normalize a libvirt domain state to the OpenStack-style status token the
/// neutral [`CloudInstance`] carries (the roster the KDC client filters on
/// `ACTIVE`/`SHUTOFF`). Unknown states pass through upper-cased (honest).
#[must_use]
pub(crate) fn normalize_domain_state(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "running" => "ACTIVE".to_string(),
        "shut off" | "shutoff" => "SHUTOFF".to_string(),
        "paused" => "PAUSED".to_string(),
        "" => "UNKNOWN".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// Build the `compute`/`instances` [`ResourceTable`] for a roster (name · status).
#[must_use]
pub(crate) fn instances_table(instances: &[CloudInstance]) -> ResourceTable {
    ResourceTable {
        service_type: "compute".to_string(),
        collection: "instances".to_string(),
        columns: vec!["name".to_string(), "status".to_string()],
        rows: instances
            .iter()
            .map(|i| ResourceRow {
                id: i.id.clone(),
                cells: vec![i.name.clone(), i.status.clone()],
            })
            .collect(),
    }
}

/// The production runner: shells `tofu` / `ansible-playbook` / `virsh` rooted at
/// the deployed IaC tree + the local libvirt URI.
pub(crate) struct ShellCloudRunner {
    /// The OpenTofu root (`<iac_root>/infra/tofu/cloud`).
    tofu_dir: PathBuf,
    /// The Ansible tree (`<iac_root>/automation/ansible`).
    ansible_dir: PathBuf,
    /// The libvirt connection URI (`qemu:///system` by default).
    libvirt_uri: String,
}

impl ShellCloudRunner {
    /// Construct from an IaC tree root + a libvirt URI.
    #[must_use]
    pub fn new(iac_root: &Path, libvirt_uri: String) -> Self {
        Self {
            tofu_dir: iac_root.join(TOFU_SUBDIR),
            ansible_dir: iac_root.join(ANSIBLE_SUBDIR),
            libvirt_uri,
        }
    }

    /// Run `bin args…`, returning `(success, stdout, stderr)`. `Err` only on a
    /// spawn failure (binary absent) so the caller can report an honest "tool
    /// unavailable" rather than a fake failure.
    fn run(bin: &str, args: &[&str]) -> Result<(bool, String, String), String> {
        let out = Command::new(bin)
            .args(args)
            .output()
            .map_err(|e| format!("{bin}: {e}"))?;
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    /// Map a shell result into a [`CloudRunOutcome`] for `action` (`applied` is the
    /// requested apply flag; a failed apply is reported but not marked applied).
    fn outcome(
        res: Result<(bool, String, String), String>,
        applied: bool,
        action: &str,
    ) -> CloudRunOutcome {
        match res {
            Ok((true, out, _)) => {
                CloudRunOutcome::ok(format!("{action}: {}", summary_line(&out)), applied)
            }
            Ok((false, _, err)) => {
                CloudRunOutcome::failed(format!("{action} failed: {}", summary_line(&err)))
            }
            Err(e) => CloudRunOutcome::failed(format!("{action} unavailable: {e}")),
        }
    }

    fn tofu_chdir(&self) -> String {
        format!("-chdir={}", self.tofu_dir.display())
    }
}

impl CloudRunner for ShellCloudRunner {
    fn probe_tool(&self, tool: &str) -> ServiceHealth {
        let start = Instant::now();
        let uri = self.libvirt_uri.clone();
        let (bin, args, url): (&str, Vec<&str>, &str) = match tool {
            TOOL_TOFU => ("tofu", vec!["version"], "(local)"),
            TOOL_ANSIBLE => ("ansible-playbook", vec!["--version"], "(local)"),
            TOOL_LIBVIRT => ("virsh", vec!["-c", uri.as_str(), "version"], uri.as_str()),
            _ => {
                return ServiceHealth {
                    service_type: tool.to_string(),
                    interface: EndpointInterface::Internal,
                    url: String::new(),
                    state: HealthState::Absent,
                    latency_ms: None,
                    microversion: None,
                    version_id: None,
                    detail: Some("unknown backend tool".to_string()),
                }
            }
        };
        match Self::run(bin, &args) {
            Ok((true, out, _)) => ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: url.to_string(),
                state: HealthState::Up,
                latency_ms: Some(elapsed_ms(start)),
                microversion: None,
                version_id: None,
                detail: Some(summary_line(&out)),
            },
            // Present but errored (e.g. libvirtd unreachable) ⇒ Down, not faked up.
            Ok((false, _, err)) => ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: url.to_string(),
                state: HealthState::Down,
                latency_ms: Some(elapsed_ms(start)),
                microversion: None,
                version_id: None,
                detail: Some(summary_line(&err)),
            },
            // Binary absent ⇒ honestly Absent (nothing to reach).
            Err(e) => ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: String::new(),
                state: HealthState::Absent,
                latency_ms: None,
                microversion: None,
                version_id: None,
                detail: Some(e),
            },
        }
    }

    fn list_instances(&self) -> Result<Vec<CloudInstance>, String> {
        let (ok, out, err) = Self::run(
            "virsh",
            &["-c", &self.libvirt_uri, "list", "--all", "--name"],
        )
        .map_err(|e| format!("libvirt unavailable: {e}"))?;
        if !ok {
            return Err(format!("virsh list failed: {}", summary_line(&err)));
        }
        let mut instances = Vec::new();
        for name in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let status = match Self::run("virsh", &["-c", &self.libvirt_uri, "domstate", name]) {
                Ok((true, sout, _)) => normalize_domain_state(&sout),
                _ => "UNKNOWN".to_string(),
            };
            instances.push(CloudInstance {
                id: name.to_string(),
                name: name.to_string(),
                status,
                flavor: None,
                image: None,
                networks: None,
            });
        }
        Ok(instances)
    }

    fn provision(&self, apply: bool) -> CloudRunOutcome {
        let chdir = self.tofu_chdir();
        let args: Vec<&str> = if apply {
            vec![
                &chdir,
                "apply",
                "-auto-approve",
                "-input=false",
                "-no-color",
            ]
        } else {
            vec![&chdir, "plan", "-input=false", "-no-color"]
        };
        Self::outcome(
            Self::run("tofu", &args),
            apply,
            if apply {
                "tofu apply"
            } else {
                "tofu plan (staged)"
            },
        )
    }

    fn destroy(&self, apply: bool) -> CloudRunOutcome {
        let chdir = self.tofu_chdir();
        let args: Vec<&str> = if apply {
            vec![
                &chdir,
                "destroy",
                "-auto-approve",
                "-input=false",
                "-no-color",
            ]
        } else {
            vec![&chdir, "plan", "-destroy", "-input=false", "-no-color"]
        };
        Self::outcome(
            Self::run("tofu", &args),
            apply,
            if apply {
                "tofu destroy"
            } else {
                "tofu plan -destroy (staged)"
            },
        )
    }

    fn configure(&self, apply: bool) -> CloudRunOutcome {
        let playbook = self.ansible_dir.join("playbooks").join("site.yml");
        let playbook_str = playbook.display().to_string();
        let inventory = self.ansible_dir.join("inventory").join("mesh.py");
        let inventory_str = inventory.display().to_string();
        let mut args: Vec<&str> = vec!["-i", &inventory_str, &playbook_str];
        if !apply {
            args.push("--check");
        }
        Self::outcome(
            Self::run("ansible-playbook", &args),
            apply,
            if apply {
                "ansible-playbook"
            } else {
                "ansible-playbook --check (staged)"
            },
        )
    }

    fn lifecycle(&self, action: LifecycleAction, instance: &str, apply: bool) -> CloudRunOutcome {
        if !apply {
            return CloudRunOutcome::ok(
                format!("virsh {} {instance} (staged)", action.cli_verb()),
                false,
            );
        }
        // Map the neutral lifecycle action onto the virsh subcommand.
        let subcmd = match action {
            LifecycleAction::Start => "start",
            LifecycleAction::Stop => "shutdown",
            LifecycleAction::Reboot => "reboot",
            LifecycleAction::Delete => "undefine",
        };
        // A Delete first force-stops the domain (best-effort) so the undefine
        // succeeds on a running persistent domain.
        if matches!(action, LifecycleAction::Delete) {
            let _ = Self::run("virsh", &["-c", &self.libvirt_uri, "destroy", instance]);
        }
        Self::outcome(
            Self::run("virsh", &["-c", &self.libvirt_uri, subcmd, instance]),
            true,
            &format!("virsh {subcmd} {instance}"),
        )
    }

    fn plan_json(&self, tfvars_json: &str) -> Result<String, String> {
        // Persist the rendered slice as the tofu root's auto-loaded var file, so
        // `tofu plan` converges to exactly this node's declared workloads.
        let tfvars_path = self.tofu_dir.join("terraform.tfvars.json");
        std::fs::write(&tfvars_path, tfvars_json)
            .map_err(|e| format!("write {}: {e}", tfvars_path.display()))?;
        let chdir = self.tofu_chdir();
        let (ok, out, err) = Self::run(
            "tofu",
            &[&chdir, "plan", "-json", "-input=false", "-no-color"],
        )
        .map_err(|e| format!("tofu unavailable: {e}"))?;
        // `plan -json` writes its ndjson stream to stdout even on a non-zero exit
        // (a plan error rides the stream as a `diagnostic`), so hand back stdout —
        // the parser surfaces the error honestly. A hard failure with no stream is
        // an honest Err.
        if out.trim().is_empty() {
            let reason = summary_line(&err);
            let reason = if reason.is_empty() {
                "tofu plan produced no output".to_string()
            } else {
                reason
            };
            return Err(if ok {
                reason
            } else {
                format!("tofu plan failed: {reason}")
            });
        }
        Ok(out)
    }
    fn resolve_inventory(&self) -> Result<String, String> {
        let inventory = self.ansible_dir.join("inventory").join("mesh.py");
        let inventory_str = inventory.display().to_string();
        match Self::run("ansible-inventory", &["-i", &inventory_str, "--list"]) {
            Ok((true, out, _)) => Ok(out),
            Ok((false, _, err)) => Err(format!("ansible-inventory failed: {}", summary_line(&err))),
            Err(e) => Err(format!("ansible-inventory unavailable: {e}")),
        }
    }

    fn tofu_outputs(&self) -> Result<String, String> {
        let chdir = self.tofu_chdir();
        match Self::run("tofu", &[&chdir, "output", "-json", "-no-color"]) {
            Ok((true, out, _)) => Ok(out),
            Ok((false, _, err)) => Err(format!("tofu output failed: {}", summary_line(&err))),
            Err(e) => Err(format!("tofu output unavailable: {e}")),
        }
    }
}

// ─────────────────────────── small helpers ───────────────────────────

pub(crate) fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The first non-empty line of a command's output, trimmed + length-capped — the
/// human "why" carried in a health/reply detail (never a multi-KB dump).
pub(crate) fn summary_line(s: &str) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    if line.len() > 200 {
        format!("{}…", &line[..200])
    } else {
        line
    }
}

/// The default IaC tree root: [`IAC_ROOT_ENV`] or [`DEFAULT_IAC_ROOT`].
pub(crate) fn default_iac_root() -> PathBuf {
    std::env::var_os(IAC_ROOT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_IAC_ROOT))
}

/// The default libvirt URI: [`LIBVIRT_URI_ENV`] or [`DEFAULT_LIBVIRT_URI`].
pub(crate) fn default_libvirt_uri() -> String {
    std::env::var(LIBVIRT_URI_ENV).unwrap_or_else(|_| DEFAULT_LIBVIRT_URI.to_string())
}

// ─────────────────────────── the injectable test fake ───────────────────────────

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A scripted fake runner: records the `apply` flag each mutation was called
    /// with, returns canned outcomes, and serves a fixed roster. Shared by the
    /// sibling modules' tests (the drain / gate / verb paths).
    #[derive(Default)]
    pub(crate) struct FakeRunner {
        pub roster: Vec<CloudInstance>,
        pub roster_err: Option<String>,
        pub tofu_up: bool,
        /// (verb, apply) calls the worker made — proves the gate.
        pub calls: Mutex<Vec<(String, bool)>>,
        /// Canned `tofu plan -json` ndjson the fake returns from `plan_json`
        /// (`None` ⇒ a default all-zero no-op summary).
        pub plan_ndjson: Option<String>,
        /// When set, `plan_json` returns this as an honest `Err` (tool absent /
        /// plan failed) instead of a stream.
        pub plan_err: Option<String>,
        /// The tfvars documents `plan_json` was handed — proves the renderer ran.
        pub tfvars_written: Mutex<Vec<String>>,
        /// `run_tool` invocations recorded as `(bin, args)` — proves the image-build
        /// / container-deploy pipelines drove the right tools.
        pub tool_calls: Mutex<Vec<(String, Vec<String>)>>,
        /// Make `run_tool` report a spawn failure (binary absent) — the honest
        /// "tool unavailable" gate path.
        pub tool_absent: bool,
        /// Make `run_tool` report a non-zero exit (the tool ran + failed).
        pub tool_fail: bool,
        /// Suppress the simulated artifact write even on success (exercises the honest
        /// "build reported success but produced no artifact" branch).
        pub tool_no_artifact: bool,
        /// Scripted `ansible-inventory --list` result (U10). `None` ⇒ no fake wired.
        pub inventory_json: Option<Result<String, String>>,
        /// Scripted `tofu output -json` result (U10). `None` ⇒ no fake wired.
        pub outputs_json: Option<Result<String, String>>,
    }

    impl FakeRunner {
        fn record(&self, verb: &str, apply: bool) {
            self.calls.lock().unwrap().push((verb.to_string(), apply));
        }
    }

    /// The argument immediately following `flag` in `args`, if present.
    fn arg_after<'a>(args: &'a [&str], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| *a == flag)
            .and_then(|i| args.get(i + 1))
            .copied()
    }

    impl CloudRunner for FakeRunner {
        fn probe_tool(&self, tool: &str) -> ServiceHealth {
            let state = if tool == TOOL_TOFU && self.tofu_up {
                HealthState::Up
            } else {
                HealthState::Absent
            };
            ServiceHealth {
                service_type: tool.to_string(),
                interface: EndpointInterface::Internal,
                url: "(local)".to_string(),
                state,
                latency_ms: Some(1),
                microversion: None,
                version_id: None,
                detail: Some("fake".to_string()),
            }
        }
        fn list_instances(&self) -> Result<Vec<CloudInstance>, String> {
            match &self.roster_err {
                Some(e) => Err(e.clone()),
                None => Ok(self.roster.clone()),
            }
        }
        fn provision(&self, apply: bool) -> CloudRunOutcome {
            self.record("provision", apply);
            CloudRunOutcome::ok("2 to add, 0 to change", apply)
        }
        fn configure(&self, apply: bool) -> CloudRunOutcome {
            self.record("configure", apply);
            CloudRunOutcome::ok("ok=3 changed=1", apply)
        }
        fn destroy(&self, apply: bool) -> CloudRunOutcome {
            self.record("destroy", apply);
            CloudRunOutcome::ok("1 to destroy", apply)
        }
        fn lifecycle(
            &self,
            action: LifecycleAction,
            instance: &str,
            apply: bool,
        ) -> CloudRunOutcome {
            self.record(&format!("lifecycle-{}", action.cli_verb()), apply);
            CloudRunOutcome::ok(format!("virsh {} {instance}", action.cli_verb()), apply)
        }

        fn plan_json(&self, tfvars_json: &str) -> Result<String, String> {
            self.tfvars_written
                .lock()
                .unwrap()
                .push(tfvars_json.to_string());
            if let Some(e) = &self.plan_err {
                return Err(e.clone());
            }
            Ok(self.plan_ndjson.clone().unwrap_or_else(|| {
                r#"{"type":"change_summary","changes":{"add":0,"change":0,"remove":0}}"#.to_string()
            }))
        }

        fn run_tool(&self, bin: &str, args: &[&str]) -> Result<ToolRun, String> {
            self.tool_calls.lock().unwrap().push((
                bin.to_string(),
                args.iter().map(|a| (*a).to_string()).collect(),
            ));
            if self.tool_absent {
                return Err(format!("{bin}: No such file or directory (os error 2)"));
            }
            // Simulate a disk builder writing its artifact under the `--output` dir,
            // so the caller's find-artifact + sha256 + record path runs end to end.
            if !self.tool_fail && !self.tool_no_artifact {
                if let Some(out) = arg_after(args, "--output") {
                    let dir = std::path::Path::new(out);
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(dir.join("disk.qcow2"), b"fake-golden-image-bytes");
                }
            }
            Ok(ToolRun {
                ok: !self.tool_fail,
                // An `ansible-playbook` run reports a PLAY RECAP the caller parses.
                stdout: if bin.contains("ansible") {
                    "meshnode : ok=3 changed=1 unreachable=0 failed=0 skipped=0".to_string()
                } else {
                    String::new()
                },
                stderr: if self.tool_fail {
                    format!("{bin}: simulated tool failure")
                } else {
                    String::new()
                },
            })
        }
        fn resolve_inventory(&self) -> Result<String, String> {
            self.inventory_json
                .clone()
                .unwrap_or_else(|| Err("no fake inventory scripted".to_string()))
        }
        fn tofu_outputs(&self) -> Result<String, String> {
            self.outputs_json
                .clone()
                .unwrap_or_else(|| Err("no fake outputs scripted".to_string()))
        }
    }

    /// A one-line [`CloudInstance`] fixture (name == id).
    pub(crate) fn instance(name: &str, status: &str) -> CloudInstance {
        CloudInstance {
            id: name.to_string(),
            name: name.to_string(),
            status: status.to_string(),
            flavor: None,
            image: None,
            networks: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_domain_state_maps_libvirt_states_to_neutral_status() {
        assert_eq!(normalize_domain_state("running"), "ACTIVE");
        assert_eq!(normalize_domain_state("shut off"), "SHUTOFF");
        assert_eq!(normalize_domain_state("paused"), "PAUSED");
        assert_eq!(normalize_domain_state("pmsuspended"), "PMSUSPENDED");
        assert_eq!(normalize_domain_state(""), "UNKNOWN");
    }

    #[test]
    fn instances_table_folds_name_and_status_cells() {
        let rows = vec![
            fake::instance("web", "ACTIVE"),
            fake::instance("db", "SHUTOFF"),
        ];
        let t = instances_table(&rows);
        assert_eq!(t.service_type, "compute");
        assert_eq!(t.collection, "instances");
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.row_label(&t.rows[0]), "web");
    }

    #[test]
    fn a_run_outcome_failure_is_never_marked_applied() {
        let f = CloudRunOutcome::failed("boom");
        assert!(!f.ok && !f.applied);
        let ok = CloudRunOutcome::ok("done", true);
        assert!(ok.ok && ok.applied);
    }
}
