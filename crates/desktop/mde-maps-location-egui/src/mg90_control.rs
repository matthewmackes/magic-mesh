//! Live MG90 control session for the Maps Admin console.
//!
//! Reads (`inspect`, `list-config`, `get-config`) publish on `action/vehicle/*`
//! from this crate. Privileged mutations (`set-mcu`, `set-gps`, `reboot`) are
//! queued for the Construct shell to mint, because production arming stays in
//! the root shell.

use mackes_mesh_types::vehicle::{vehicle_action_topic, VehicleReply};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde::Deserialize;

/// Allowlisted MCU keys the console can change. Matches the live MG90 `mcu.yaml`.
pub const MCU_KEYS: &[&str] = &[
    "IGNTHRESH",
    "LOWVOLT",
    "HIGHVOLT",
    "OFFDELAY",
    "ONDELAY",
    "IGNOFFDELAY",
    "INACTIVETIME",
    "HARDOFF",
    "AUTOPWR",
    "HIGHTEMP",
    "LOWTEMP",
];

/// One privileged mutation waiting for the shell to mint and publish.
#[derive(Debug, Clone)]
pub struct PendingVehicleMutation {
    /// Bus verb (`reboot`, `set-mcu`, `set-gps`).
    pub verb: String,
    /// Capability verb bound by the HMAC (`vehicle-reboot`, …).
    pub auth_verb: String,
    /// Unsigned JSON body, including `schema_version` and `typed_name`.
    pub body: String,
    /// Operator-facing in-flight label.
    pub label: String,
}

/// Parsed `inspect` payload from the gateway. Missing fields stay empty.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Mg90Inspect {
    /// `hostname` on the gateway.
    #[serde(default)]
    pub hostname: String,
    /// `/proc/uptime` seconds.
    #[serde(default)]
    pub uptime_s: u64,
    /// `globals.yaml` country.
    #[serde(default)]
    pub country: String,
    /// `gps.yaml` enable flag (`yes`/`no`).
    #[serde(default)]
    pub gps_enable: String,
    /// MCU ignition threshold volts.
    #[serde(default)]
    pub ign_thresh: String,
    /// MCU low-voltage cutoff.
    #[serde(default)]
    pub low_volt: String,
    /// MCU high-voltage cutoff.
    #[serde(default)]
    pub high_volt: String,
    /// MCU ignition-off delay seconds.
    #[serde(default)]
    pub off_delay: String,
    /// `wlan1` SSID when the AP is up.
    #[serde(default)]
    pub wlan1_ssid: String,
    /// `wlan1` type (`AP` / `managed`).
    #[serde(default)]
    pub wlan1_type: String,
}

/// Operator session for the MG90 console.
#[derive(Debug, Clone, Default)]
pub struct Mg90ControlState {
    /// In-flight read ULID, if any.
    pending_ulid: Option<String>,
    /// Last operator-facing note (ok / gated / error).
    pub note: Option<String>,
    /// Last YAML or inspect JSON payload.
    pub payload: Option<String>,
    /// Parsed inspect snapshot, when the last payload was inspect JSON.
    pub inspect: Option<Mg90Inspect>,
    /// Files returned by `list-config`.
    pub config_files: Vec<String>,
    /// Selected committed YAML name.
    pub config_file: String,
    /// Typed ESN confirmation for destructive actions.
    pub reboot_arm: String,
    /// Selected MCU key.
    pub mcu_key: String,
    /// MCU value draft.
    pub mcu_value: String,
    /// GPS enable draft (`yes` / `no`).
    pub gps_enable_draft: String,
    /// Whether the console already requested the first inspect/list.
    pub primed: bool,
    queued_reads: Vec<(String, String, String)>,
    pending_mutations: Vec<PendingVehicleMutation>,
}

impl Mg90ControlState {
    /// Production empty session. `mcu.yaml` is the first control file because it
    /// is the live ignition/voltage plane.
    #[must_use]
    pub fn live() -> Self {
        Self {
            config_file: "mcu.yaml".to_string(),
            mcu_key: "IGNTHRESH".to_string(),
            gps_enable_draft: String::new(),
            ..Self::default()
        }
    }

    /// Queue a non-privileged vehicle read. Published on the next bus refresh.
    pub fn queue_read(&mut self, verb: &str, body: &str, label: &str) {
        self.note = Some(format!("Requested {label}…"));
        self.queued_reads
            .push((verb.to_string(), body.to_string(), label.to_string()));
    }

    /// Queue a privileged mutation for the Construct shell to mint.
    pub fn queue_mutation(
        &mut self,
        verb: &str,
        auth_verb: &str,
        body: serde_json::Value,
        label: &str,
    ) {
        self.note = Some(format!("Requested {label}…"));
        self.pending_mutations.push(PendingVehicleMutation {
            verb: verb.to_string(),
            auth_verb: auth_verb.to_string(),
            body: body.to_string(),
            label: label.to_string(),
        });
    }

    /// Drain privileged mutations for the shell mint path.
    pub fn take_mutations(&mut self) -> Vec<PendingVehicleMutation> {
        std::mem::take(&mut self.pending_mutations)
    }

    /// True when a read or reply is waiting — skip the Maps poll gate.
    #[must_use]
    pub fn needs_bus(&self) -> bool {
        self.pending_ulid.is_some() || !self.queued_reads.is_empty()
    }

    /// One privileged mutation, only when no other vehicle request is in flight.
    pub fn take_mutation_if_idle(&mut self) -> Option<PendingVehicleMutation> {
        if self.pending_ulid.is_some() {
            return None;
        }
        if self.pending_mutations.is_empty() {
            return None;
        }
        Some(self.pending_mutations.remove(0))
    }

    /// Remember a published request so the next poll harvests `reply/<ulid>`.
    pub fn track_inflight(&mut self, ulid: String, label: &str) {
        self.pending_ulid = Some(ulid);
        self.note = Some(format!("Waiting for {label}…"));
    }

    /// Operator-facing failure from the shell mint path.
    pub fn fail_note(&mut self, error: String) {
        self.note = Some(error);
    }

    /// Publish queued reads and harvest the in-flight reply.
    pub fn sync_bus(&mut self, persist: &Persist) {
        self.publish_reads(persist);
        self.poll_reply(persist);
    }

    fn publish_reads(&mut self, persist: &Persist) {
        if self.pending_ulid.is_some() {
            return;
        }
        let Some((verb, body, label)) = self.queued_reads.first().cloned() else {
            return;
        };
        match persist.write(
            &vehicle_action_topic(&verb),
            Priority::Default,
            None,
            Some(&body),
        ) {
            Ok(msg) => {
                self.pending_ulid = Some(msg.ulid);
                self.note = Some(format!("Waiting for {label}…"));
                self.queued_reads.remove(0);
            }
            Err(error) => {
                self.note = Some(format!("Could not request {label}: {error}"));
                self.queued_reads.remove(0);
            }
        }
    }

    fn poll_reply(&mut self, persist: &Persist) {
        let Some(ulid) = self.pending_ulid.clone() else {
            return;
        };
        let Ok(mut msgs) = persist.list_since(&reply_topic(&ulid), None) else {
            return;
        };
        let Some(msg) = msgs.pop() else {
            return;
        };
        self.pending_ulid = None;
        let Some(body) = msg.body.as_deref() else {
            self.note = Some("MG90 reply had no body".to_string());
            return;
        };
        let Ok(reply) = serde_json::from_str::<VehicleReply>(body) else {
            self.note = Some("MG90 reply was not a typed VehicleReply".to_string());
            return;
        };
        if let Some(gated) = reply.gated {
            self.note = Some(gated);
            return;
        }
        if let Some(error) = reply.error {
            self.note = Some(error);
            return;
        }
        if let Some(applied) = reply.applied {
            match reply.verb.as_str() {
                "list-config" => {
                    self.config_files = applied
                        .lines()
                        .map(str::trim)
                        .filter(|line| is_config_file_name(line))
                        .map(ToOwned::to_owned)
                        .collect();
                    self.note = Some(format!(
                        "Loaded {} committed files",
                        self.config_files.len()
                    ));
                }
                "inspect" => {
                    self.inspect = serde_json::from_str(&applied).ok();
                    self.payload = Some(applied);
                    if let Some(inspect) = &self.inspect {
                        if self.gps_enable_draft.is_empty() {
                            self.gps_enable_draft = inspect.gps_enable.clone();
                        }
                        if self.mcu_value.is_empty() {
                            self.mcu_value = inspect.ign_thresh.clone();
                        }
                    }
                    self.note = Some("Inspected live MG90".to_string());
                }
                _ => {
                    self.payload = Some(applied);
                    self.note = Some(format!("{} ok", reply.verb));
                }
            }
        } else {
            self.note = Some(format!("{} ok", reply.verb));
        }
    }
}

/// Bare `*.yaml` names the console will fetch. Matches the worker guard.
#[must_use]
pub fn is_config_file_name(name: &str) -> bool {
    name.len() > 5
        && name.ends_with(".yaml")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// MCU values are numbers or `yes`/`no` only — never free text.
#[must_use]
pub fn is_mcu_value(value: &str) -> bool {
    let value = value.trim();
    if matches!(value, "yes" | "no") {
        return true;
    }
    let mut parts = value.split('.');
    let head = parts.next().unwrap_or("");
    let tail = parts.next();
    if parts.next().is_some() {
        return false;
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match tail {
        None => digits(head.trim_start_matches('-')),
        Some(frac) => digits(head.trim_start_matches('-')) && digits(frac),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_guard_rejects_paths() {
        assert!(is_config_file_name("mcu.yaml"));
        assert!(is_config_file_name("wifi-networks.yaml"));
        assert!(!is_config_file_name("../mcu.yaml"));
        assert!(!is_config_file_name(
            "/opt/inmotiontechnology/config/mcu.yaml"
        ));
        assert!(!is_config_file_name("mcu.txt"));
    }

    #[test]
    fn mcu_value_guard_is_numeric_or_yes_no() {
        assert!(is_mcu_value("1.5"));
        assert!(is_mcu_value("11"));
        assert!(is_mcu_value("yes"));
        assert!(!is_mcu_value("1.5; reboot"));
        assert!(!is_mcu_value(""));
    }

    #[test]
    fn mutation_queue_waits_while_a_read_is_in_flight() {
        let mut state = Mg90ControlState::live();
        state.queue_mutation(
            "reboot",
            "vehicle-reboot",
            serde_json::json!({"schema_version": 1, "typed_name": "ESN"}),
            "reboot",
        );
        state.track_inflight("01TESTULID".to_string(), "inspect");
        assert!(state.take_mutation_if_idle().is_none());
        state.pending_ulid = None;
        let taken = state.take_mutation_if_idle().expect("idle mutation");
        assert_eq!(taken.verb, "reboot");
        assert_eq!(taken.auth_verb, "vehicle-reboot");
    }
}
