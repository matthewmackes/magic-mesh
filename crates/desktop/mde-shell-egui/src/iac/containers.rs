//! U19 — the **Containers** lens: the `container-deploy` form. The GUI fields
//! (name / image / published ports / named volumes / env) render a Podman
//! **Quadlet** `.container` unit, installed as a systemd service by the
//! container-host role — **rootless by default** (least privilege). Any OCI
//! registry is allowed (no allowlist).
//!
//! The lens emits [`VERB_CONTAINER_DEPLOY`] through the cockpit's preserved emit
//! path (the same `issue` seam the provision lens uses). A live install requires
//! exact typed confirmation and a target/body-bound token minted by the root DRM
//! shell. The unit is previewed live below the form (a faithful mirror of the
//! backend's render) so the operator reviews exactly what would be installed before
//! requesting it.

use mde_egui::egui::{self, RichText};
use mde_egui::{carbon_icon, Style};

use mackes_mesh_types::cloud::VERB_CONTAINER_DEPLOY;

use super::WorkloadsState;

/// The Containers lens's own state (U19 owns the draft `.container` form fields).
#[derive(Debug, Default)]
pub(super) struct State {
    /// The container / unit name (`<name>.container`; a `[A-Za-z0-9._-]` token).
    name: String,
    /// The OCI image reference (`registry/repo:tag`; any registry, no allowlist).
    image: String,
    /// Published ports, one `host:container` (or `container`) per line.
    ports: String,
    /// Named volumes, one `source:/dest[:opts]` per line.
    volumes: String,
    /// Environment entries, one `KEY=VALUE` per line.
    env: String,
    /// Install system-wide (rootful) instead of the rootless default.
    rootful: bool,
}

/// Split a free-text multiline field into trimmed, non-empty entries (one per
/// line) — the shape the `container-deploy` body's `ports` / `volumes` / `env`
/// arrays take.
fn split_lines(s: &str) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether a container / unit name is a path-safe `[A-Za-z0-9._-]+` token (mirrors
/// the backend's `is_unit_safe`, so the form only enables a deploy the worker will
/// accept). Empty / `.` / `..` are refused.
fn is_unit_safe(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Render the rootless-by-default Podman Quadlet `.container` unit the form would
/// deploy — a faithful local preview of what the backend emits, so the operator
/// reviews the exact unit before requesting an install.
fn render_quadlet(
    name: &str,
    image: &str,
    rootful: bool,
    ports: &[String],
    env: &[String],
    volumes: &[String],
) -> String {
    use std::fmt::Write as _;
    let scope = if rootful { "rootful" } else { "rootless" };
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# Rendered by MCNF Workloads (container-deploy) \u{2014} {scope} by default."
    );
    let _ = writeln!(s, "[Unit]");
    let _ = writeln!(s, "Description=MCNF service container: {name}");
    let _ = writeln!(s);
    let _ = writeln!(s, "[Container]");
    let _ = writeln!(s, "Image={image}");
    let _ = writeln!(s, "ContainerName={name}");
    for p in ports {
        let _ = writeln!(s, "PublishPort={p}");
    }
    for e in env {
        let _ = writeln!(s, "Environment={e}");
    }
    for v in volumes {
        let _ = writeln!(s, "Volume={v}");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "[Service]");
    let _ = writeln!(s, "Restart=always");
    let _ = writeln!(s);
    let _ = writeln!(s, "[Install]");
    let wanted = if rootful {
        "multi-user.target"
    } else {
        "default.target"
    };
    let _ = writeln!(s, "WantedBy={wanted}");
    s
}

/// Build the JSON `container-deploy` request body from the frozen form. The root
/// shell inserts a target-bound token only after typed confirmation.
fn deploy_request_body(
    node: &str,
    name: &str,
    image: &str,
    rootful: bool,
    ports: &[String],
    env: &[String],
    volumes: &[String],
) -> String {
    serde_json::json!({
        "schema_version": mackes_mesh_types::cloud::CLOUD_ACTION_SCHEMA_VERSION,
        "node": node,
        "name": name,
        "image": image,
        "ports": ports,
        "env": env,
        "volumes": volumes,
        "rootful": rootful,
    })
    .to_string()
}

/// Render the Containers lens.
pub(super) fn containers_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    header(ui);
    ui.add_space(Style::SP_S);
    form(ui, state);
    ui.add_space(Style::SP_S);

    // Snapshot the form into owned values so the immutable field borrows end before
    // the mutable emit seam runs.
    let name = state.containers.name.trim().to_string();
    let image = state.containers.image.trim().to_string();
    let rootful = state.containers.rootful;
    let ports = split_lines(&state.containers.ports);
    let env = split_lines(&state.containers.env);
    let volumes = split_lines(&state.containers.volumes);
    let node = state
        .selected_node()
        .map(str::trim)
        .filter(|node| !node.is_empty())
        .map(str::to_string)
        .unwrap_or_default();

    preview(ui, &name, &image, rootful, &ports, &env, &volumes);
    ui.add_space(Style::SP_S);

    let ready = is_unit_safe(&name) && !image.is_empty() && !node.is_empty();
    let deploy = ui
        .add_enabled(
            ready,
            egui::Button::new(
                RichText::new("Deploy container\u{2026}")
                    .size(Style::SMALL)
                    .color(Style::ACCENT),
            ),
        )
        .clicked();
    mde_egui::muted_note(
        ui,
        "Deploy opens exact typed confirmation. The root DRM shell then mints a single-use \
         capability bound to this placement node and the complete frozen unit request.",
    );

    if deploy {
        let body = deploy_request_body(&node, &name, &image, rootful, &ports, &env, &volumes);
        state.arm_prepared(
            VERB_CONTAINER_DEPLOY,
            node,
            name.clone(),
            body,
            format!("container deploy ({name})"),
            name.clone(),
            "Deploy",
            format!("container {name}"),
        );
    }
}

/// The lens header card — the Workloads-accent glyph, the title, and the
/// rootless / any-registry blurb.
fn header(ui: &mut egui::Ui) {
    mde_egui::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.scope(|ui| {
                ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
                carbon_icon(ui, "overlay", Style::ICON_S);
            });
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("Deploy a service container")
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
        });
        mde_egui::muted_note(
            ui,
            "A rootless Podman Quadlet .container unit, installed as a systemd service. Any OCI \
             registry \u{2014} no allowlist.",
        );
    });
}

/// The `.container` form card — name / image / ports / volumes / env / scope.
fn form(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    mde_egui::card().show(ui, |ui| {
        // Name (with an inline validity hint mirroring the backend's rule).
        ui.label(
            RichText::new("Name")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add(
            egui::TextEdit::singleline(&mut state.containers.name)
                .hint_text("web \u{00B7} a [A-Za-z0-9._-] token")
                .desired_width(f32::INFINITY),
        );
        let name = state.containers.name.trim();
        if !name.is_empty() && !is_unit_safe(name) {
            ui.colored_label(
                Style::DANGER,
                RichText::new("Name must be a [A-Za-z0-9._-] token.").size(Style::SMALL),
            );
        }
        ui.add_space(Style::SP_XS);

        // Image.
        ui.label(
            RichText::new("Image")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add(
            egui::TextEdit::singleline(&mut state.containers.image)
                .hint_text("registry/repo:tag \u{00B7} e.g. ghcr.io/org/app:1.2")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(Style::SP_XS);

        // Ports / volumes / env (one entry per line).
        multiline_field(
            ui,
            "Published ports",
            "host:container, one per line (e.g. 8080:80)",
            &mut state.containers.ports,
        );
        multiline_field(
            ui,
            "Volumes",
            "source:/dest[:opts], one per line",
            &mut state.containers.volumes,
        );
        multiline_field(
            ui,
            "Environment",
            "KEY=VALUE, one per line",
            &mut state.containers.env,
        );

        ui.checkbox(&mut state.containers.rootful, "Run system-wide (rootful)");
        mde_egui::muted_note(
            ui,
            "Rootless by default (least privilege) \u{2014} installs under your user; rootful \
             installs the unit system-wide.",
        );
    });
}

/// One labelled multiline field on the spacing grid.
fn multiline_field(ui: &mut egui::Ui, label: &str, hint: &str, buf: &mut String) {
    ui.label(
        RichText::new(label)
            .size(Style::SMALL)
            .color(Style::TEXT_DIM),
    );
    ui.add(
        egui::TextEdit::multiline(buf)
            .hint_text(hint)
            .desired_rows(2)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(Style::SP_XS);
}

/// The live Quadlet-unit preview card — a recessed well showing the exact
/// `.container` unit the form would deploy. An incomplete form reads an honest
/// prompt rather than a partial unit.
fn preview(
    ui: &mut egui::Ui,
    name: &str,
    image: &str,
    rootful: bool,
    ports: &[String],
    env: &[String],
    volumes: &[String],
) {
    mde_egui::card().show(ui, |ui| {
        ui.label(
            RichText::new("Quadlet unit preview")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_XS);
        if is_unit_safe(name) && !image.is_empty() {
            let unit = render_quadlet(name, image, rootful, ports, env, volumes);
            mde_egui::inset().show(ui, |ui| {
                ui.add(egui::Label::new(
                    RichText::new(unit)
                        .monospace()
                        .size(Style::SMALL)
                        .color(Style::TEXT),
                ));
            });
        } else {
            mde_egui::muted_note(
                ui,
                "Enter a valid name and an image to preview the .container unit.",
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadlet_is_rootless_by_default_and_carries_the_form() {
        let unit = render_quadlet(
            "web",
            "ghcr.io/org/app:1.2",
            false,
            &["8080:80".to_string()],
            &["LOG=info".to_string()],
            &["data:/var/lib/app".to_string()],
        );
        assert!(unit.contains("Image=ghcr.io/org/app:1.2"), "{unit}");
        assert!(unit.contains("ContainerName=web"));
        assert!(unit.contains("PublishPort=8080:80"));
        assert!(unit.contains("Environment=LOG=info"));
        assert!(unit.contains("Volume=data:/var/lib/app"));
        assert!(unit.contains("WantedBy=default.target"));
        // Rootless: no User= / root directive.
        assert!(!unit.contains("User="));
    }

    #[test]
    fn rootful_targets_multi_user() {
        let unit = render_quadlet("svc", "img:1", true, &[], &[], &[]);
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn names_are_validated_like_the_backend() {
        assert!(is_unit_safe("web-1.app_v2"));
        assert!(!is_unit_safe(""));
        assert!(!is_unit_safe("."));
        assert!(!is_unit_safe(".."));
        assert!(!is_unit_safe("../evil"));
        assert!(!is_unit_safe("has space"));
    }

    #[test]
    fn split_lines_trims_and_drops_blanks() {
        let v = split_lines(" a \n\n b:c \n");
        assert_eq!(v, vec!["a".to_string(), "b:c".to_string()]);
    }

    #[test]
    fn deploy_body_is_rootless_by_default_and_carries_the_lists() {
        let body = deploy_request_body(
            "eagle",
            "web",
            "img:1",
            false,
            &["8080:80".to_string()],
            &[],
            &[],
        );
        assert!(body.contains(r#""rootful":false"#), "{body}");
        assert!(body.contains(r#""schema_version":1"#), "{body}");
        assert!(body.contains(r#""ports":["8080:80"]"#), "{body}");
        assert!(body.contains(r#""node":"eagle""#), "{body}");
        assert!(body.contains(r#""name":"web""#), "{body}");
    }
}
