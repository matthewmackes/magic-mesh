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

use mackes_mesh_types::cloud::{AnsibleSummary, CloudReply};

use super::super::{gate, CloudWorker};

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
    if !is_unit_safe(&name) {
        return reject(
            verb_name,
            &format!("invalid container name `{name}` (need a [A-Za-z0-9._-] token)"),
        );
    }
    let Some(image) = clean(body.image.as_deref()) else {
        return reject(verb_name, "container-deploy requires an `image` reference");
    };

    // Render the Quadlet unit (pure — always, even for a staged request).
    let scope = if body.rootful { "rootful" } else { "rootless" };
    let unit = render_quadlet(&name, &image, scope, &body);

    // The armed-token gate — a request without a valid capability installs nothing,
    // but honestly returns the rendered unit so the operator can review it.
    let verdict = gate::verify_token(
        body.armed_token.as_deref(),
        verb_name,
        body.node.trim(),
        super::super::now_ms(),
        w.signer.as_ref(),
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

    // Stage the unit into the Syncthing-replicated tree so the placement node's
    // container-host role picks it up.
    let staged = match stage_unit(&w.state_root, &body.node, &name, &unit) {
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
        "--tags",
        "container",
        "--extra-vars",
        extra.as_str(),
    ];
    match w.runner.run_tool("ansible-playbook", &args) {
        Err(spawn) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!(
                "ansible unavailable: {spawn} — quadlet `{name}.container` staged at {staged_disp} but not installed"
            )),
            raw_log: Some(unit),
            ..Default::default()
        },
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
                CloudReply {
                    ok: false,
                    verb: verb_name.to_string(),
                    ansible: Some(summary),
                    error: Some(format!(
                        "ansible container install failed for `{name}.container`"
                    )),
                    raw_log: Some(pick_log(&run.stdout, &run.stderr)),
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

/// Stage the rendered unit under `<workgroup>/quadlets/<node>/<name>.container`
/// (Syncthing-replicated so the placement node sees it).
fn stage_unit(
    root: &std::path::Path,
    node: &str,
    name: &str,
    unit: &str,
) -> std::io::Result<std::path::PathBuf> {
    let node_dir = sanitize_node(node);
    let dir = root.join("quadlets").join(node_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.container"));
    std::fs::write(&path, unit)?;
    Ok(path)
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

/// A container/unit name is a path-safe `[A-Za-z0-9._-]+` token (a unit filename +
/// a ContainerName).
fn is_unit_safe(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// A path-safe node dir segment (empty / unsafe → sanitized), so the staged tree is
/// never escaped.
fn sanitize_node(node: &str) -> String {
    let node = node.trim();
    if node.is_empty() {
        return "local".to_string();
    }
    node.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
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

    fn token() -> String {
        ArmedToken::mint(
            &signer(),
            "nonce-12345678",
            now_ms() + 3_600_000,
            "container-deploy",
            "me",
        )
        .encode()
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
            r#"{"node":"me","name":"web","image":"nginx:1"}"#,
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
        let raw = format!(
            r#"{{"node":"me","name":"web","image":"nginx:1","ports":["8080:80"],"armed_token":"{}"}}"#,
            token()
        );
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
        assert!(calls[0].1.iter().any(|a| a == "container"));
    }

    #[test]
    fn ansible_absent_is_honestly_gated_and_the_unit_is_still_staged() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner {
            tool_absent: true,
            ..Default::default()
        });
        let w = armed_worker(tmp.path(), runner);
        let raw = format!(
            r#"{{"node":"me","name":"web","image":"nginx:1","armed_token":"{}"}}"#,
            token()
        );
        let reply = w.handle("container-deploy", &raw);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("ansible unavailable"));
        assert!(tmp
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
        let raw = format!(
            r#"{{"node":"me","name":"web","image":"nginx:1","armed_token":"{}"}}"#,
            token()
        );
        let reply = w.handle("container-deploy", &raw);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("failed"));
    }

    #[test]
    fn a_missing_name_or_image_is_an_honest_rejection() {
        let tmp = tempfile::tempdir().unwrap();
        let w = armed_worker(tmp.path(), Arc::new(FakeRunner::default()));
        let no_name = w.handle("container-deploy", r#"{"node":"me","image":"nginx:1"}"#);
        assert!(!no_name.ok && no_name.error.unwrap().contains("name"));
        let no_image = w.handle("container-deploy", r#"{"node":"me","name":"web"}"#);
        assert!(!no_image.ok && no_image.error.unwrap().contains("image"));
        // A path-escaping name is refused.
        let bad = w.handle(
            "container-deploy",
            r#"{"node":"me","name":"../evil","image":"nginx:1"}"#,
        );
        assert!(!bad.ok && bad.error.unwrap().contains("invalid container name"));
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
