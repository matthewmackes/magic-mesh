//! The 4-step VM creation wizard for the Compute group (E6.10 slice 4).
//!
//! A self-contained sub-component: `WizardState` holds the form, `update`
//! folds a `WizardMsg` and returns a [`WizardAction`] the parent acts on
//! (close, or build the `CreateRequest` → real `virt-install`), and
//! `view` renders the current step. Ported from the legacy
//! `mde-virtual::wizard` (VIRT-15.a) onto the workbench's iced 0.14 +
//! `mde-theme` palette/type/spacing tokens, dropping the `ulid`/`dirs`
//! deps (a SystemTime suffix + XDG path stand in).

use std::path::{Path, PathBuf};

use iced::widget::{checkbox, column, container, row, text, text_input, Space};
use iced::{Background, Border, Element, Length};
use mde_theme::{spacing, FontSize, Palette, TypeRole};
use serde::{Deserialize, Serialize};

use crate::controls::{variant_button, ButtonVariant};

/// Directory scanned for installer / cloud-image ISOs.
const ISO_DIR: &str = "/var/lib/mde-vms/isos";

/// Carbon 8px-grid spacing token (`mde_theme::spacing::BASE`) as f32.
fn sp(i: usize) -> f32 {
    f32::from(spacing::BASE[i])
}

fn body_size() -> f32 {
    TypeRole::Body.size_in(FontSize::defaults())
}
fn caption_size() -> f32 {
    TypeRole::Caption.size_in(FontSize::defaults())
}

/// Create-request payload — the resolved VM spec the parent turns into a
/// real `virt-install` invocation (or, with a mesh peer, a Bus publish).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreateRequest {
    pub request_id: String,
    pub name: String,
    pub vcpus: u32,
    pub ram_mb: u64,
    pub disk_gb: u64,
    pub iso_path: Option<String>,
    pub share_meshfs: bool,
}

impl CreateRequest {
    /// The real `virt-install` argv this request maps to. The VM is
    /// created against the system libvirtd; `--noautoconsole` keeps the
    /// CLI from blocking (the console is the separate virt-viewer action,
    /// slice 5). With no ISO it imports a blank disk (`--pxe`-less).
    #[must_use]
    pub fn virt_install_command(&self) -> (&'static str, Vec<String>) {
        let mut args = vec![
            "--connect".to_string(),
            "qemu:///system".to_string(),
            "--name".to_string(),
            self.name.clone(),
            "--vcpus".to_string(),
            self.vcpus.to_string(),
            "--memory".to_string(),
            self.ram_mb.to_string(),
            "--disk".to_string(),
            format!("size={}", self.disk_gb),
            "--noautoconsole".to_string(),
        ];
        match &self.iso_path {
            Some(iso) => {
                args.push("--cdrom".to_string());
                args.push(iso.clone());
            }
            None => {
                // No installer media — define the domain without booting it.
                args.push("--import".to_string());
            }
        }
        ("virt-install", args)
    }
}

/// Messages the wizard emits (wrapped by the parent).
#[derive(Debug, Clone)]
pub enum WizardMsg {
    NameInput(String),
    ApplyTemplate(usize),
    DeleteTemplate(usize),
    /// Add the current form to the template store (review step).
    SaveTemplate,
    VcpusDelta(i64),
    RamDelta(i64),
    DiskDelta(i64),
    SelectIso(Option<String>),
    CustomIsoInput(String),
    ToggleMeshfs,
    Next,
    Back,
    Cancel,
    Create,
}

/// What the parent should do after folding a [`WizardMsg`].
#[derive(Debug, Clone, PartialEq)]
pub enum WizardAction {
    None,
    Cancel,
    Create(CreateRequest),
}

/// The wizard's form state.
#[derive(Debug, Clone)]
pub struct WizardState {
    step: u8,
    name: String,
    vcpus: u32,
    ram_mb: u64,
    disk_gb: u64,
    iso: Option<String>,
    custom_iso: String,
    share_meshfs: bool,
    isos: Vec<String>,
    templates: Vec<(PathBuf, Template)>,
}

impl Default for WizardState {
    fn default() -> Self {
        Self::new()
    }
}

impl WizardState {
    /// Open a fresh wizard with the spec defaults (2 vCPU / 2048 MB /
    /// 20 GB / MeshFS on).
    #[must_use]
    pub fn new() -> Self {
        Self {
            step: 1,
            name: String::new(),
            vcpus: 2,
            ram_mb: 2048,
            disk_gb: 20,
            iso: None,
            custom_iso: String::new(),
            share_meshfs: true,
            isos: list_isos(),
            templates: list_templates(),
        }
    }

    pub fn update(&mut self, msg: WizardMsg) -> WizardAction {
        match msg {
            WizardMsg::NameInput(s) => {
                self.name = sanitize_name(&s);
                WizardAction::None
            }
            WizardMsg::ApplyTemplate(i) => {
                if let Some(t) = self.templates.get(i).map(|(_, t)| t.clone()) {
                    self.name = sanitize_name(&t.name);
                    self.vcpus = t.vcpus.clamp(1, 16);
                    self.ram_mb = t.ram_mb.clamp(512, 65536);
                    self.disk_gb = t.disk_gb.clamp(10, 500);
                    self.share_meshfs = t.share_meshfs;
                }
                WizardAction::None
            }
            WizardMsg::DeleteTemplate(i) => {
                if let Some((path, _)) = self.templates.get(i) {
                    let _ = std::fs::remove_file(path);
                }
                self.templates = list_templates();
                WizardAction::None
            }
            WizardMsg::SaveTemplate => {
                let _ = save_template(&self.current_template());
                self.templates = list_templates();
                WizardAction::None
            }
            WizardMsg::VcpusDelta(d) => {
                self.vcpus = clamp_i64(i64::from(self.vcpus) + d, 1, 16) as u32;
                WizardAction::None
            }
            WizardMsg::RamDelta(d) => {
                self.ram_mb = clamp_i64(self.ram_mb as i64 + d, 512, 65536) as u64;
                WizardAction::None
            }
            WizardMsg::DiskDelta(d) => {
                self.disk_gb = clamp_i64(self.disk_gb as i64 + d, 10, 500) as u64;
                WizardAction::None
            }
            WizardMsg::SelectIso(o) => {
                self.iso = o;
                WizardAction::None
            }
            WizardMsg::CustomIsoInput(s) => {
                self.custom_iso = s;
                WizardAction::None
            }
            WizardMsg::ToggleMeshfs => {
                self.share_meshfs = !self.share_meshfs;
                WizardAction::None
            }
            WizardMsg::Next => {
                if self.step < 4 && self.can_advance() {
                    self.step += 1;
                }
                WizardAction::None
            }
            WizardMsg::Back => {
                if self.step > 1 {
                    self.step -= 1;
                }
                WizardAction::None
            }
            WizardMsg::Cancel => WizardAction::Cancel,
            WizardMsg::Create => {
                if self.step == 4 && name_valid(&self.name) {
                    WizardAction::Create(self.build_request())
                } else {
                    WizardAction::None
                }
            }
        }
    }

    fn can_advance(&self) -> bool {
        if self.step == 1 {
            name_valid(&self.name)
        } else {
            true
        }
    }

    /// The current form as a reusable [`Template`] (the base name, no
    /// ULID suffix — that's added at create time).
    fn current_template(&self) -> Template {
        Template {
            name: self.name.clone(),
            vcpus: self.vcpus,
            ram_mb: self.ram_mb,
            disk_gb: self.disk_gb,
            share_meshfs: self.share_meshfs,
        }
    }

    fn effective_iso(&self) -> Option<String> {
        let trimmed = self.custom_iso.trim();
        if trimmed.is_empty() {
            self.iso.clone()
        } else {
            Some(trimmed.to_string())
        }
    }

    /// Build the create request, appending a short unique suffix to the
    /// libvirt name for cross-mesh uniqueness.
    fn build_request(&self) -> CreateRequest {
        let id = unique_id();
        let suffix: String = id.chars().take(8).collect();
        CreateRequest {
            request_id: id.clone(),
            name: format!("{}-{}", self.name, suffix),
            vcpus: self.vcpus,
            ram_mb: self.ram_mb,
            disk_gb: self.disk_gb,
            iso_path: self.effective_iso(),
            share_meshfs: self.share_meshfs,
        }
    }

    pub fn view(&self, palette: Palette) -> Element<'_, WizardMsg> {
        let title = text(format!("Add VM — step {} of 4", self.step))
            .size(TypeRole::Subheading.size_in(FontSize::defaults()))
            .color(palette.text.into_iced_color());

        let body: Element<'_, WizardMsg> = match self.step {
            1 => self.step_name(palette),
            2 => self.step_resources(palette),
            3 => self.step_disk_iso(palette),
            _ => self.step_review(palette),
        };

        let mut nav = row![ghost(palette, "Cancel", Some(WizardMsg::Cancel))].spacing(sp(1));
        if self.step > 1 {
            nav = nav.push(ghost(palette, "Back", Some(WizardMsg::Back)));
        }
        nav = nav.push(Space::new().width(Length::Fill));
        if self.step < 4 {
            let next = self.can_advance().then_some(WizardMsg::Next);
            nav = nav.push(primary(palette, "Next", next));
        } else {
            let create = name_valid(&self.name).then_some(WizardMsg::Create);
            nav = nav.push(primary(palette, "Add VM", create));
        }

        let col = column![title, body, nav].spacing(sp(4)).width(Length::Fill);
        let surface = palette.surface.into_iced_color();
        let border = palette.border.into_iced_color();
        container(col)
            .width(Length::Fill)
            .padding([sp(4), sp(6)])
            .style(move |_t| container::Style {
                background: Some(Background::Color(surface)),
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    fn step_name(&self, palette: Palette) -> Element<'_, WizardMsg> {
        let mut col = column![
            label(palette, "VM name"),
            text_input("my-vm", &self.name)
                .on_input(WizardMsg::NameInput)
                .padding(sp(0))
                .size(body_size()),
        ]
        .spacing(sp(0))
        .width(Length::Fill);
        if !name_valid(&self.name) {
            col = col.push(muted(
                palette,
                "Name must be non-empty (letters, digits, hyphens).",
            ));
        }
        if !self.templates.is_empty() {
            col = col.push(label(palette, "Start from a template"));
            for (i, (_, t)) in self.templates.iter().enumerate() {
                let lbl = format!(
                    "{} — {} vCPU / {} MB / {} GB",
                    t.name, t.vcpus, t.ram_mb, t.disk_gb
                );
                col = col.push(
                    row![
                        secondary(palette, &lbl, Some(WizardMsg::ApplyTemplate(i))),
                        Space::new().width(Length::Fill),
                        ghost(palette, "Remove", Some(WizardMsg::DeleteTemplate(i))),
                    ]
                    .spacing(sp(1))
                    .align_y(iced::alignment::Vertical::Center),
                );
            }
        }
        col.into()
    }

    fn step_resources(&self, palette: Palette) -> Element<'_, WizardMsg> {
        column![
            stepper(
                palette,
                "vCPUs",
                &self.vcpus.to_string(),
                WizardMsg::VcpusDelta(-1),
                WizardMsg::VcpusDelta(1),
            ),
            stepper(
                palette,
                "RAM (MB)",
                &self.ram_mb.to_string(),
                WizardMsg::RamDelta(-512),
                WizardMsg::RamDelta(512),
            ),
        ]
        .spacing(sp(1))
        .width(Length::Fill)
        .into()
    }

    fn step_disk_iso(&self, palette: Palette) -> Element<'_, WizardMsg> {
        let mut col = column![
            stepper(
                palette,
                "Disk (GB)",
                &self.disk_gb.to_string(),
                WizardMsg::DiskDelta(-1),
                WizardMsg::DiskDelta(1),
            ),
            label(palette, "Installer ISO"),
        ]
        .spacing(sp(1))
        .width(Length::Fill);

        col = col.push(iso_choice(
            palette,
            "None",
            self.iso.is_none(),
            WizardMsg::SelectIso(None),
        ));
        for iso in &self.isos {
            let selected = self.iso.as_deref() == Some(iso.as_str());
            let label_txt = Path::new(iso)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(iso.as_str())
                .to_string();
            col = col.push(iso_choice(
                palette,
                &label_txt,
                selected,
                WizardMsg::SelectIso(Some(iso.clone())),
            ));
        }
        col = col.push(
            text_input("…or a custom ISO path", &self.custom_iso)
                .on_input(WizardMsg::CustomIsoInput)
                .padding(sp(0))
                .size(caption_size()),
        );
        col = col.push(
            checkbox(self.share_meshfs)
                .label("Share MeshFS")
                .on_toggle(|_| WizardMsg::ToggleMeshfs),
        );
        col.into()
    }

    fn step_review(&self, palette: Palette) -> Element<'_, WizardMsg> {
        let iso = self.effective_iso().unwrap_or_else(|| "none".to_string());
        // "Add template" is enabled once the name is valid (the template
        // store keys on a usable name).
        let save = name_valid(&self.name).then_some(WizardMsg::SaveTemplate);
        column![
            kv(palette, "Name", &self.name),
            kv(palette, "vCPUs", &self.vcpus.to_string()),
            kv(palette, "RAM", &format!("{} MB", self.ram_mb)),
            kv(palette, "Disk", &format!("{} GB", self.disk_gb)),
            kv(palette, "ISO", &iso),
            kv(
                palette,
                "MeshFS",
                if self.share_meshfs { "shared" } else { "off" },
            ),
            secondary(palette, "Add template", save),
        ]
        .spacing(sp(0))
        .width(Length::Fill)
        .into()
    }
}

/// Sanitize a name as typed: ASCII alphanumeric + hyphen only.
fn sanitize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// Validate a VM name: non-empty, ASCII alphanumeric + hyphens only.
#[must_use]
pub fn name_valid(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A unique-ish id (UNIX-nanos hex) standing in for the legacy ULID — no
/// extra dep, monotone enough for a per-create suffix.
fn unique_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:032x}")
}

/// List `*.iso` files under [`ISO_DIR`] (sorted; empty when absent).
fn list_isos() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(ISO_DIR) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("iso") {
                if let Some(s) = p.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// A saved VM config template (the step-1 picker store).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub vcpus: u32,
    pub ram_mb: u64,
    pub disk_gb: u64,
    #[serde(default)]
    pub share_meshfs: bool,
}

/// The template store dir (`$XDG_DATA_HOME/mde/vm-templates`, falling back
/// to `$HOME/.local/share/mde/vm-templates`).
#[must_use]
pub fn templates_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .map(|d| d.join("mde").join("vm-templates"))
}

/// Load all saved templates as `(path, template)` (sorted by name).
fn list_templates() -> Vec<(PathBuf, Template)> {
    let Some(dir) = templates_dir() else {
        return vec![];
    };
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("json") {
                if let Ok(body) = std::fs::read_to_string(&p) {
                    if let Ok(t) = serde_json::from_str::<Template>(&body) {
                        out.push((p, t));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    out
}

/// Persist a template to `templates_dir()/<unique>.json`. Returns the
/// written path, or `None` if the store dir can't be resolved/created.
fn save_template(t: &Template) -> Option<PathBuf> {
    save_template_to(&templates_dir()?, t)
}

/// Write a template into `dir` as `<unique>.json` (creating `dir`). Split
/// out so it's testable without mutating the process environment.
fn save_template_to(dir: &Path, t: &Template) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{}.json", unique_id()));
    let body = serde_json::to_string_pretty(t).ok()?;
    std::fs::write(&path, body).ok()?;
    Some(path)
}

fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64 {
    v.max(lo).min(hi)
}

// ── small view helpers (mde-theme palette/type/spacing tokens) ───────────

fn label<'a>(palette: Palette, t: &str) -> Element<'a, WizardMsg> {
    text(t.to_string())
        .size(body_size())
        .color(palette.text.into_iced_color())
        .into()
}

fn muted<'a>(palette: Palette, t: &str) -> Element<'a, WizardMsg> {
    text(t.to_string())
        .size(caption_size())
        .color(palette.text_muted.into_iced_color())
        .into()
}

fn kv<'a>(palette: Palette, k: &str, v: &str) -> Element<'a, WizardMsg> {
    row![
        text(k.to_string())
            .size(caption_size())
            .color(palette.text_muted.into_iced_color())
            .width(Length::FillPortion(2)),
        text(v.to_string())
            .size(body_size())
            .color(palette.text.into_iced_color())
            .width(Length::FillPortion(3)),
    ]
    .spacing(sp(1))
    .into()
}

fn ghost<'a>(palette: Palette, lbl: &str, msg: Option<WizardMsg>) -> Element<'a, WizardMsg> {
    variant_button(lbl.to_string(), ButtonVariant::Ghost, msg, palette)
}

fn secondary<'a>(palette: Palette, lbl: &str, msg: Option<WizardMsg>) -> Element<'a, WizardMsg> {
    variant_button(lbl.to_string(), ButtonVariant::Secondary, msg, palette)
}

fn primary<'a>(palette: Palette, lbl: &str, msg: Option<WizardMsg>) -> Element<'a, WizardMsg> {
    variant_button(lbl.to_string(), ButtonVariant::Primary, msg, palette)
}

/// `label  [-] value [+]` numeric stepper.
fn stepper<'a>(
    palette: Palette,
    lbl: &str,
    value: &str,
    dec: WizardMsg,
    inc: WizardMsg,
) -> Element<'a, WizardMsg> {
    row![
        text(lbl.to_string())
            .size(body_size())
            .color(palette.text.into_iced_color())
            .width(Length::Fill),
        secondary(palette, "-", Some(dec)),
        text(value.to_string())
            .size(body_size())
            .color(palette.text.into_iced_color()),
        secondary(palette, "+", Some(inc)),
    ]
    .spacing(sp(1))
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

/// A selectable ISO row (Secondary/accent when selected, Ghost otherwise).
fn iso_choice<'a>(
    palette: Palette,
    lbl: &str,
    selected: bool,
    msg: WizardMsg,
) -> Element<'a, WizardMsg> {
    let variant = if selected {
        ButtonVariant::Secondary
    } else {
        ButtonVariant::Ghost
    };
    variant_button(lbl.to_string(), variant, Some(msg), palette)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(name_valid("web-01"));
        assert!(name_valid("db"));
        assert!(!name_valid(""));
        assert!(!name_valid("bad name"));
        assert!(!name_valid("under_score"));
    }

    #[test]
    fn name_input_sanitizes() {
        let mut w = WizardState::new();
        w.update(WizardMsg::NameInput("my vm!_01".to_string()));
        assert_eq!(w.name, "myvm01");
    }

    #[test]
    fn next_blocked_until_name_valid() {
        let mut w = WizardState::new();
        assert_eq!(w.step, 1);
        w.update(WizardMsg::Next);
        assert_eq!(w.step, 1);
        w.update(WizardMsg::NameInput("web".to_string()));
        w.update(WizardMsg::Next);
        assert_eq!(w.step, 2);
    }

    #[test]
    fn steppers_clamp_to_range() {
        let mut w = WizardState::new();
        for _ in 0..30 {
            w.update(WizardMsg::VcpusDelta(1));
        }
        assert_eq!(w.vcpus, 16);
        for _ in 0..30 {
            w.update(WizardMsg::VcpusDelta(-1));
        }
        assert_eq!(w.vcpus, 1);
        w.update(WizardMsg::RamDelta(-512));
        assert_eq!(w.ram_mb, 1536);
        for _ in 0..10 {
            w.update(WizardMsg::DiskDelta(-100));
        }
        assert_eq!(w.disk_gb, 10);
    }

    #[test]
    fn cancel_returns_cancel() {
        let mut w = WizardState::new();
        assert_eq!(w.update(WizardMsg::Cancel), WizardAction::Cancel);
    }

    #[test]
    fn create_only_on_step_4_with_valid_name() {
        let mut w = WizardState::new();
        assert_eq!(w.update(WizardMsg::Create), WizardAction::None);
        w.update(WizardMsg::NameInput("web".into()));
        w.update(WizardMsg::Next);
        w.update(WizardMsg::Next);
        w.update(WizardMsg::Next);
        assert_eq!(w.step, 4);
        let action = w.update(WizardMsg::Create);
        assert!(
            matches!(action, WizardAction::Create(_)),
            "expected Create, got {action:?}"
        );
        if let WizardAction::Create(req) = action {
            assert!(req.name.starts_with("web-"));
            assert_eq!(req.vcpus, 2);
            assert_eq!(req.ram_mb, 2048);
            assert_eq!(req.disk_gb, 20);
            assert!(req.share_meshfs);
            assert!(!req.request_id.is_empty());
        }
    }

    #[test]
    fn custom_iso_overrides_selection() {
        let mut w = WizardState::new();
        w.iso = Some("/var/lib/mde-vms/isos/a.iso".into());
        w.custom_iso = "  /tmp/custom.iso  ".into();
        assert_eq!(w.effective_iso(), Some("/tmp/custom.iso".to_string()));
        w.custom_iso = "   ".into();
        assert_eq!(
            w.effective_iso(),
            Some("/var/lib/mde-vms/isos/a.iso".to_string())
        );
    }

    #[test]
    fn apply_template_prefills_and_clamps() {
        let mut w = WizardState::new();
        w.templates = vec![(
            PathBuf::from("/tmp/x.json"),
            Template {
                name: "big_box!".into(),
                vcpus: 99,
                ram_mb: 8192,
                disk_gb: 5,
                share_meshfs: false,
            },
        )];
        w.update(WizardMsg::ApplyTemplate(0));
        assert_eq!(w.name, "bigbox");
        assert_eq!(w.vcpus, 16);
        assert_eq!(w.ram_mb, 8192);
        assert_eq!(w.disk_gb, 10);
        assert!(!w.share_meshfs);
    }

    #[test]
    fn virt_install_command_maps_spec_to_argv() {
        let req = CreateRequest {
            request_id: "id".into(),
            name: "web-abcd1234".into(),
            vcpus: 4,
            ram_mb: 4096,
            disk_gb: 40,
            iso_path: Some("/iso/fedora.iso".into()),
            share_meshfs: true,
        };
        let (prog, args) = req.virt_install_command();
        assert_eq!(prog, "virt-install");
        assert!(args.windows(2).any(|w| w == ["--name", "web-abcd1234"]));
        assert!(args.windows(2).any(|w| w == ["--vcpus", "4"]));
        assert!(args.windows(2).any(|w| w == ["--memory", "4096"]));
        assert!(args.windows(2).any(|w| w == ["--disk", "size=40"]));
        assert!(args.windows(2).any(|w| w == ["--cdrom", "/iso/fedora.iso"]));
        // No ISO → --import instead of --cdrom.
        let (_, no_iso) = CreateRequest {
            iso_path: None,
            ..req
        }
        .virt_install_command();
        assert!(no_iso.iter().any(|a| a == "--import"));
        assert!(!no_iso.iter().any(|a| a == "--cdrom"));
    }

    #[test]
    fn current_template_captures_form_spec() {
        let mut w = WizardState::new();
        w.update(WizardMsg::NameInput("web".into()));
        w.update(WizardMsg::VcpusDelta(1)); // 2 -> 3
        let t = w.current_template();
        assert_eq!(t.name, "web");
        assert_eq!(t.vcpus, 3);
        assert_eq!(t.ram_mb, 2048);
        assert_eq!(t.disk_gb, 20);
        assert!(t.share_meshfs);
    }

    #[test]
    fn save_template_writes_readable_json() {
        let dir = std::env::temp_dir().join(format!("mde-wiz-tmpl-{}", unique_id()));
        let t = Template {
            name: "web".into(),
            vcpus: 4,
            ram_mb: 4096,
            disk_gb: 40,
            share_meshfs: true,
        };
        let path = save_template_to(&dir, &t).expect("template written");
        let body = std::fs::read_to_string(&path).expect("read back");
        let back: Template = serde_json::from_str(&body).expect("parse");
        assert_eq!(back, t);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn templates_dir_lands_under_share() {
        // With HOME set and XDG unset, the dir is under .local/share/mde.
        if let Some(dir) = templates_dir() {
            assert!(dir.ends_with("mde/vm-templates"));
        }
    }
}
