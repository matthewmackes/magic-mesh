//! Workloads U7 — the `container-deploy` verb: a Podman **Quadlet** service
//! container, rootless by default.
//!
//! The GUI form (name / image / ports / env / volumes) is rendered into a Podman
//! Quadlet `.container` unit — the systemd-native way to run a container as a unit
//! (`podman-systemd.unit(5)`). The rendered unit is staged into the mesh's
//! Syncthing-replicated tree (so the placement node picks it up with no egress) and
//! handed to the Ansible container path, which installs it as a (user, for rootless)
//! systemd unit and starts it.
//!
//! Rootless is the default (Q — least privilege): the unit carries no `User=`/root
//! directives and installs under the invoking user's `~/.config/containers/systemd/`;
//! an explicit `rootful` request installs it system-wide instead. Either way the
//! scope is passed to Ansible as an extra-var — the role owns the install location.
//!
//! Honest by construction (§7): a bad form, a missing armed token, or an absent
//! Ansible path is a truthful reject/gate — never a fabricated "deployed". A staged
//! (un-armed) request still renders the unit (returned in the raw log) but installs
//! nothing.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use mackes_mesh_types::cloud::{AnsibleSummary, CloudReply};

use super::super::{path_key, CloudWorker};

const QUADLET_SUFFIX: &str = ".container";
/// A container pull needs room for layers and transient extraction files. Keep a
/// small absolute reserve so a nearly-full seat cannot be driven into ENOSPC by a
/// deploy; the image's actual size is not knowable from an arbitrary OCI reference.
const MIN_CONTAINER_FREE_BYTES: u64 = 512 * 1024 * 1024;

/// The parsed `container-deploy` request body.
#[derive(Debug, Clone, Default, Deserialize)]
struct ContainerDeployBody {
    /// The placement node (the armed-token binding + the drain's placement key).
    #[serde(default)]
    node: String,
    /// The container / unit name (path-safe; becomes `<name>.container`).
    #[serde(default)]
    name: Option<String>,
    /// The container image reference (`registry/repo:tag`).
    #[serde(default)]
    image: Option<String>,
    /// Published ports (`host:container` or `container`), one `PublishPort=` each.
    #[serde(default)]
    ports: Vec<String>,
    /// Environment entries (`KEY=VALUE`), one `Environment=` each.
    #[serde(default)]
    env: Vec<String>,
    /// Volume mounts (`source:/dest[:opts]`), one `Volume=` each.
    #[serde(default)]
    volumes: Vec<String>,
    /// Run system-wide (rootful) instead of the rootless default.
    #[serde(default)]
    rootful: bool,
    /// The armed-token capability authorizing a live install.
    #[serde(default)]
    armed_token: Option<String>,
}

/// Handle a `container-deploy` request end to end.
pub(crate) fn handle(w: &CloudWorker, verb_name: &str, raw: &str) -> CloudReply {
    let body: ContainerDeployBody = serde_json::from_str(raw.trim()).unwrap_or_default();

    // Validate the form before doing anything.
    let Some(name) = clean(body.name.as_deref()) else {
        return reject(verb_name, "container-deploy requires a `name`");
    };
    if let Err(e) = path_key::file_stem("container name", &name, QUADLET_SUFFIX) {
        return reject(verb_name, &format!("invalid container name `{name}`: {e}"));
    }
    let stage_node = if body.node.trim().is_empty() {
        "local"
    } else {
        match path_key::segment("placement node", &body.node) {
            Ok(node) => node,
            Err(e) => return reject(verb_name, &e),
        }
    };
    let Some(image) = clean(body.image.as_deref()) else {
        return reject(verb_name, "container-deploy requires an `image` reference");
    };

    // Render the Quadlet unit (pure — always, even for a staged request).
    let scope = if body.rootful { "rootful" } else { "rootless" };
    let unit = render_quadlet(&name, &image, scope, &body);

    // The armed-token gate — a request without a valid capability installs nothing,
    // but honestly returns the rendered unit so the operator can review it.
    let verdict = w.consume_armed_token(
        body.armed_token.as_deref(),
        verb_name,
        body.node.trim(),
        &name,
        raw,
    );
    if !verdict.is_valid() {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!(
                "live container deploy is gated ({}) — quadlet unit `{name}.container` rendered but nothing installed",
                verdict.reason()
            )),
            raw_log: Some(unit),
            ..Default::default()
        };
    }

    // Check the filesystem that carries the staged state before creating a
    // replicated desired artifact or asking the host to pull image layers.
    if let Err(reason) = disk_preflight(&w.state_root) {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!(
                "container deploy blocked by disk preflight: {reason}"
            )),
            raw_log: Some(unit),
            ..Default::default()
        };
    }

    // Stage the unit into the Syncthing-replicated tree so the placement node's
    // container-host role picks it up.
    let staged = match stage_unit(&w.state_root, stage_node, &name, &unit) {
        Ok(p) => p,
        Err(e) => return reject(verb_name, &format!("stage quadlet unit: {e}")),
    };
    let staged_disp = staged.display().to_string();

    // Drive the Ansible container path (installs the quadlet as a systemd unit). The
    // role/tool may be absent → honest gate, never a fabricated install.
    let (playbook, inventory) = ansible_paths();
    let extra = format!(
        "mde_quadlet_unit={staged_disp} mde_quadlet_scope={scope} mde_container_name={name}"
    );
    let args = [
        "-i",
        inventory.as_str(),
        playbook.as_str(),
        // site.yml and the container_host role do not tag their tasks. A
        // `--tags container` filter would therefore let Ansible exit 0 after
        // running no install tasks, which this handler would misreport as a
        // deployed unit.
        "--extra-vars",
        extra.as_str(),
    ];
    match w.runner.run_tool("ansible-playbook", &args) {
        Err(spawn) => {
            let rollback = rollback_staged(&staged);
            CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some(format!(
                    "ansible unavailable: {spawn} — quadlet `{name}.container` was not installed{rollback}"
                )),
                raw_log: Some(unit),
                ..Default::default()
            }
        }
        Ok(run) => {
            let summary = parse_recap(&run.stdout);
            let clean_run = run.ok && summary.failed == 0 && summary.unreachable == 0;
            if clean_run {
                CloudReply {
                    ok: true,
                    verb: verb_name.to_string(),
                    ansible: Some(summary),
                    raw_log: Some(format!(
                        "quadlet unit `{name}.container` ({scope}) installed via ansible; staged at {staged_disp}"
                    )),
                    ..Default::default()
                }
            } else {
                let rollback = rollback_staged(&staged);
                CloudReply {
                    ok: false,
                    verb: verb_name.to_string(),
                    ansible: Some(summary),
                    error: Some(format!(
                        "ansible container install failed for `{name}.container`{rollback}"
                    )),
                    raw_log: Some(format!("{}{rollback}", pick_log(&run.stdout, &run.stderr))),
                    ..Default::default()
                }
            }
        }
    }
}

/// Render a rootless-by-default Podman Quadlet `.container` unit from the form.
fn render_quadlet(name: &str, image: &str, scope: &str, body: &ContainerDeployBody) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# Rendered by MCNF Workloads (container-deploy, U7) — {scope} by default."
    );
    let _ = writeln!(
        s,
        "# Installed as a Podman Quadlet systemd unit by the container-host role."
    );
    let _ = writeln!(s, "[Unit]");
    let _ = writeln!(s, "Description=MCNF service container: {name}");
    let _ = writeln!(s);
    let _ = writeln!(s, "[Container]");
    let _ = writeln!(s, "Image={image}");
    let _ = writeln!(s, "ContainerName={name}");
    for p in body
        .ports
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
    {
        let _ = writeln!(s, "PublishPort={p}");
    }
    for e in body.env.iter().map(|e| e.trim()).filter(|e| !e.is_empty()) {
        let _ = writeln!(s, "Environment={e}");
    }
    for v in body
        .volumes
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        let _ = writeln!(s, "Volume={v}");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "[Service]");
    let _ = writeln!(s, "Restart=always");
    let _ = writeln!(s);
    let _ = writeln!(s, "[Install]");
    // Rootless quadlets are wanted by the user's default target; rootful by multi-user.
    let wanted = if scope == "rootful" {
        "multi-user.target"
    } else {
        "default.target"
    };
    let _ = writeln!(s, "WantedBy={wanted}");
    s
}

/// The staged file plus the bytes it replaced, used to roll back a failed live
/// install without leaving a failed image pull queued in the replicated tree.
#[derive(Debug)]
struct StagedUnit {
    path: std::path::PathBuf,
    previous: Option<Vec<u8>>,
}

impl StagedUnit {
    fn display(&self) -> String {
        self.path.display().to_string()
    }
}

/// Stage the rendered unit under `<workgroup>/quadlets/<node>/<name>.container`
/// (Syncthing-replicated so the placement node sees it), retaining the prior
/// leaf so a failed live install can roll back.
fn stage_unit(root: &Path, node: &str, name: &str, unit: &str) -> Result<StagedUnit, String> {
    let node = path_key::segment("placement node", node)?;
    let name = path_key::file_stem("container name", name, QUADLET_SUFFIX)?;
    let dir = root.join("quadlets").join(node);
    reject_symlinked_stage_directories(&dir)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create quadlet stage dir {}: {e}", dir.display()))?;
    // Re-check after mkdir so an already-present replicated component cannot be
    // traversed if it was replaced while the directory was being prepared.
    reject_symlinked_stage_directories(&dir)?;
    let path = dir.join(format!("{name}{QUADLET_SUFFIX}"));
    let previous =
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refusing symlinked quadlet stage {}",
                    path.display()
                ));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!(
                    "quadlet stage is not a regular file {}",
                    path.display()
                ));
            }
            Ok(_) => Some(std::fs::read(&path).map_err(|error| {
                format!("read existing quadlet stage {}: {error}", path.display())
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "read existing quadlet stage {}: {error}",
                    path.display()
                ));
            }
        };
    write_stage_atomic(&path, unit.as_bytes())?;
    Ok(StagedUnit { path, previous })
}

/// Refuse to traverse a replicated directory symlink while preparing a
/// container stage. A peer-controlled state tree must not redirect a write
/// outside the worker's state root.
fn reject_symlinked_stage_directories(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refusing symlinked quadlet stage directory {}",
                    current.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "quadlet stage path component is not a directory: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "inspect quadlet stage directory {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

/// Write a complete stage through a create-new temporary and atomic rename.
/// The temporary cannot be a planted symlink, and rename replaces (rather than
/// follows) a hostile final leaf if one appears after validation.
fn write_stage_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;

    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid quadlet stage filename {}", path.display()))?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp = path.with_file_name(format!(".{leaf}.tmp-{}-{nonce}", std::process::id()));

    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| {
                format!("create temporary quadlet stage {}: {error}", temp.display())
            })?;
        file.write_all(bytes).map_err(|error| {
            format!("write temporary quadlet stage {}: {error}", temp.display())
        })?;
        file.sync_all()
            .map_err(|error| format!("sync temporary quadlet stage {}: {error}", temp.display()))?;
        drop(file);
        std::fs::rename(&temp, path)
            .map_err(|error| format!("replace quadlet stage {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Restore the prior stage, or remove a newly-created stage, after Ansible fails.
/// A rollback failure is surfaced in the reply because the replicated tree may
/// otherwise retry a deployment that did not complete.
fn rollback_staged(staged: &StagedUnit) -> String {
    let result = match &staged.previous {
        Some(previous) => write_stage_atomic(&staged.path, previous)
            .map_err(|error| format!("restore failed stage {}: {error}", staged.display())),
        None => match std::fs::remove_file(&staged.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("remove failed stage {}: {error}", staged.display())),
        },
    };
    match result {
        Ok(()) => "; staged Quadlet rolled back".to_string(),
        Err(error) => format!("; staged Quadlet rollback failed: {error}"),
    }
}

/// Refuse a live deploy when the backing filesystem is below the reserve.
fn disk_preflight(root: &std::path::Path) -> Result<(), String> {
    let output = std::process::Command::new("df")
        .arg("-B1")
        .arg("--output=avail")
        .arg(root)
        .output()
        .map_err(|error| format!("disk availability probe unavailable: {error}"))?;
    if !output.status.success() {
        return Err("disk availability probe failed".to_string());
    }
    let available = String::from_utf8_lossy(&output.stdout)
        .lines()
        .nth(1)
        .and_then(|line| line.trim().parse::<u64>().ok())
        .ok_or_else(|| "disk availability probe returned no numeric result".to_string())?;
    disk_capacity_gate(Some(available))
}

fn disk_capacity_gate(available: Option<u64>) -> Result<(), String> {
    let Some(available) = available else {
        return Err("disk availability is unknown".to_string());
    };
    if available < MIN_CONTAINER_FREE_BYTES {
        return Err(format!(
            "only {available} bytes are free; at least {MIN_CONTAINER_FREE_BYTES} bytes are required"
        ));
    }
    Ok(())
}

/// The Ansible playbook + mesh dynamic inventory paths (the same tree the configure
/// leg drives), rooted at the deployed IaC tree.
fn ansible_paths() -> (String, String) {
    let ansible = super::super::runner::default_iac_root()
        .join("automation")
        .join("ansible");
    (
        ansible
            .join("playbooks")
            .join("site.yml")
            .to_string_lossy()
            .into_owned(),
        ansible
            .join("inventory")
            .join("mesh.py")
            .to_string_lossy()
            .into_owned(),
    )
}

/// Parse an Ansible `PLAY RECAP` line (`ok=N changed=N unreachable=N failed=N`) into
/// the neutral summary. An absent recap folds to zeros (honest — the raw log carries
/// the detail).
fn parse_recap(stdout: &str) -> AnsibleSummary {
    let mut summary = AnsibleSummary::default();
    for line in stdout.lines() {
        if line.contains("ok=") && line.contains("changed=") {
            summary.ok = field(line, "ok=");
            summary.changed = field(line, "changed=");
            summary.unreachable = field(line, "unreachable=");
            summary.failed = field(line, "failed=");
        }
    }
    summary
}

/// The unsigned integer immediately after `key` in `line` (`ok=3` → 3), else 0.
fn field(line: &str, key: &str) -> u32 {
    line.split(key)
        .nth(1)
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|d| d.parse().ok())
        .unwrap_or(0)
}

/// Trim + drop-empty a wire string field.
fn clean(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn pick_log(stdout: &str, stderr: &str) -> String {
    if stderr.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        stderr.trim().to_string()
    }
}

fn reject(verb_name: &str, why: &str) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(why.to_string()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::super::gate::{ArmedToken, HmacTokenSigner};
    use super::super::super::runner::fake::FakeRunner;
    use super::super::super::{now_ms, CloudWorker};
    use super::*;

    const KEY: &[u8] = b"test-mesh-arming-key";

    fn signer() -> HmacTokenSigner {
        HmacTokenSigner::new(KEY.to_vec())
    }

    fn armed_worker(root: &std::path::Path, runner: Arc<FakeRunner>) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), root.to_path_buf())
            .with_runner(runner)
            .with_signer(Arc::new(signer()))
            .with_bus_root(None)
    }

    fn staged_worker(root: &std::path::Path, runner: Arc<FakeRunner>) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), root.to_path_buf())
            .with_runner(runner)
            .with_bus_root(None)
    }

    fn token(body: &str, target: &str) -> String {
        ArmedToken::mint(
            &signer(),
            "nonce-12345678",
            now_ms().saturating_add(super::super::super::MAX_AUTH_TTL_MS),
            "container-deploy",
            "me",
            target,
            &mackes_mesh_types::cloud::cloud_request_digest(body).unwrap(),
        )
        .encode()
    }

    fn armed_request(mut body: serde_json::Value) -> String {
        let raw = body.to_string();
        let target = body["name"].as_str().unwrap();
        body["armed_token"] = serde_json::Value::String(token(&raw, target));
        body.to_string()
    }

    #[test]
    fn render_is_rootless_by_default_and_carries_the_form() {
        let body = ContainerDeployBody {
            ports: vec!["8080:80".into()],
            env: vec!["LOG=info".into()],
            volumes: vec!["data:/var/lib/app".into()],
            ..Default::default()
        };
        let unit = render_quadlet("web", "docker.io/library/nginx:1", "rootless", &body);
        assert!(unit.contains("[Container]"));
        assert!(unit.contains("Image=docker.io/library/nginx:1"));
        assert!(unit.contains("ContainerName=web"));
        assert!(unit.contains("PublishPort=8080:80"));
        assert!(unit.contains("Environment=LOG=info"));
        assert!(unit.contains("Volume=data:/var/lib/app"));
        assert!(unit.contains("WantedBy=default.target"));
        // Rootless: no root/User directive.
        assert!(!unit.contains("User="));
        assert!(unit.contains("rootless by default"));
    }

    #[test]
    fn render_rootful_targets_multi_user() {
        let unit = render_quadlet("svc", "img:1", "rootful", &ContainerDeployBody::default());
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn deploy_without_a_token_stages_the_unit_but_installs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(tmp.path(), runner.clone());
        let reply = w.handle(
            "container-deploy",
            r#"{"schema_version":1,"node":"me","name":"web","image":"nginx:1"}"#,
        );
        assert!(!reply.ok);
        let gated = reply.gated.unwrap();
        assert!(gated.contains("gated"));
        assert!(gated.contains("nothing installed"));
        // The rendered unit is returned for review.
        assert!(reply.raw_log.unwrap().contains("ContainerName=web"));
        // No ansible run for a staged request.
        assert!(runner.tool_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_armed_deploy_renders_stages_and_installs_via_ansible() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(tmp.path(), runner.clone());
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me",
            "name": "web",
            "image": "nginx:1",
            "ports": ["8080:80"],
        }));
        let reply = w.handle("container-deploy", &raw);
        assert!(reply.ok, "gated:{:?} err:{:?}", reply.gated, reply.error);
        let ansible = reply.ansible.expect("ansible summary");
        assert_eq!(ansible.ok, 3);
        assert_eq!(ansible.changed, 1);
        // The unit was staged to the Syncthing tree for the placement node.
        let staged = tmp.path().join("quadlets").join("me").join("web.container");
        assert!(staged.is_file(), "quadlet staged at {}", staged.display());
        // The ansible container path was driven.
        let calls = runner.tool_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "ansible-playbook");
        assert!(
            !calls[0]
                .1
                .windows(2)
                .any(|pair| pair[0] == "--tags" && pair[1] == "container"),
            "an untagged role must not be filtered into a successful no-op"
        );
        let extra = calls[0]
            .1
            .windows(2)
            .find(|pair| pair[0] == "--extra-vars")
            .map(|pair| pair[1].as_str())
            .expect("quadlet install variables");
        assert!(extra.contains("mde_quadlet_unit="));
        assert!(extra.contains("mde_container_name=web"));
    }

    #[test]
    fn ansible_absent_is_honestly_gated_and_new_stage_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner {
            tool_absent: true,
            ..Default::default()
        });
        let w = armed_worker(tmp.path(), runner);
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me",
            "name": "web",
            "image": "nginx:1",
        }));
        let reply = w.handle("container-deploy", &raw);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("ansible unavailable"));
        assert!(!tmp
            .path()
            .join("quadlets")
            .join("me")
            .join("web.container")
            .is_file());
    }

    #[test]
    fn an_ansible_failure_is_an_honest_error() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner {
            tool_fail: true,
            ..Default::default()
        });
        let w = armed_worker(tmp.path(), runner);
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me",
            "name": "web",
            "image": "nginx:1",
        }));
        let reply = w.handle("container-deploy", &raw);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("failed"));
        assert!(!tmp
            .path()
            .join("quadlets")
            .join("me")
            .join("web.container")
            .exists());
    }

    #[test]
    fn failed_deploy_restores_the_previous_staged_unit() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("quadlets").join("me").join("web.container");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, "old-unit\n").unwrap();
        let runner = Arc::new(FakeRunner {
            tool_fail: true,
            ..Default::default()
        });
        let w = armed_worker(tmp.path(), runner);
        let raw = armed_request(serde_json::json!({
            "schema_version": 1,
            "node": "me",
            "name": "web",
            "image": "nginx:1",
        }));

        let reply = w.handle("container-deploy", &raw);
        assert!(!reply.ok);
        assert_eq!(std::fs::read_to_string(staged).unwrap(), "old-unit\n");
    }

    #[test]
    fn disk_capacity_gate_rejects_the_live_seat_low_space_case() {
        let low = 324 * 1024 * 1024;
        assert!(disk_capacity_gate(Some(low)).is_err());
        assert!(disk_capacity_gate(Some(MIN_CONTAINER_FREE_BYTES)).is_ok());
        assert!(disk_capacity_gate(None).is_err());
    }

    #[test]
    fn a_token_cannot_be_reused_after_image_or_rootful_substitution() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(tmp.path(), runner.clone());
        let authorized = serde_json::json!({
            "schema_version": 1,
            "node": "me",
            "name": "web",
            "image": "nginx:1",
            "rootful": false,
        });
        let token = token(&authorized.to_string(), "web");
        let mut altered = authorized;
        altered["image"] = serde_json::Value::String("attacker/image:latest".to_string());
        altered["rootful"] = serde_json::Value::Bool(true);
        altered["armed_token"] = serde_json::Value::String(token);

        let reply = w.handle("container-deploy", &altered.to_string());
        assert!(!reply.ok);
        assert!(reply
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("request body")));
        assert!(runner.tool_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_missing_name_or_image_is_an_honest_rejection() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        let no_name = w.handle(
            "container-deploy",
            r#"{"schema_version":1,"node":"me","image":"nginx:1"}"#,
        );
        assert!(!no_name.ok && no_name.error.unwrap().contains("name"));
        let no_image = w.handle(
            "container-deploy",
            r#"{"schema_version":1,"node":"me","name":"web"}"#,
        );
        assert!(!no_image.ok && no_image.error.unwrap().contains("image"));
        // A path-escaping name is refused.
        let bad = w.handle(
            "container-deploy",
            r#"{"schema_version":1,"node":"me","name":"../evil","image":"nginx:1"}"#,
        );
        assert!(!bad.ok && bad.error.unwrap().contains("invalid container name"));
    }

    #[test]
    fn a_path_like_placement_node_is_rejected_before_staging() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        for node in ["..", "../escape", "/tmp/escape", "node/child"] {
            let raw =
                format!(r#"{{"schema_version":1,"node":"{node}","name":"web","image":"nginx:1"}}"#);
            let reply = w.handle("container-deploy", &raw);
            assert!(!reply.ok);
            assert!(reply.error.unwrap().contains("path-safe"));
        }
        assert!(!tmp.path().join("quadlets").exists());
    }

    #[cfg(unix)]
    #[test]
    fn stage_refuses_symlinked_directories_and_leaf_without_touching_outside() {
        use std::os::unix::fs::symlink;

        let parent_case = tempfile::tempdir().unwrap();
        let parent_outside = tempfile::tempdir().unwrap();
        symlink(parent_outside.path(), parent_case.path().join("quadlets")).unwrap();
        let parent_error = stage_unit(parent_case.path(), "me", "web", "unit").unwrap_err();
        assert!(
            parent_error.contains("symlink"),
            "unexpected error: {parent_error}"
        );
        assert!(!parent_outside.path().join("me").exists());

        let leaf_case = tempfile::tempdir().unwrap();
        let leaf_outside = tempfile::tempdir().unwrap();
        let leaf_dir = leaf_case.path().join("quadlets").join("me");
        std::fs::create_dir_all(&leaf_dir).unwrap();
        let victim = leaf_outside.path().join("victim");
        std::fs::write(&victim, "must remain unchanged\n").unwrap();
        symlink(&victim, leaf_dir.join("web.container")).unwrap();
        let leaf_error = stage_unit(leaf_case.path(), "me", "web", "unit").unwrap_err();
        assert!(
            leaf_error.contains("symlink"),
            "unexpected error: {leaf_error}"
        );
        assert_eq!(
            std::fs::read_to_string(victim).unwrap(),
            "must remain unchanged\n"
        );
    }

    #[test]
    fn an_overlong_quadlet_stem_is_rejected_before_staging_io() {
        let tmp = tempfile::tempdir().unwrap();
        let long_name = "x".repeat(246);

        let err = stage_unit(tmp.path(), "me", &long_name, "unit").unwrap_err();
        assert!(err.contains("too long"), "unexpected error: {err}");
        assert!(
            !tmp.path().join("quadlets").exists(),
            "invalid filename must fail before I/O"
        );
    }

    #[test]
    fn parse_recap_reads_the_play_recap_counts() {
        let s = parse_recap("meshnode : ok=5 changed=2 unreachable=0 failed=1 skipped=3");
        assert_eq!(s.ok, 5);
        assert_eq!(s.changed, 2);
        assert_eq!(s.unreachable, 0);
        assert_eq!(s.failed, 1);
    }
}
