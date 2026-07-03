//! `bt_pairing` — the shell side of the `BlueZ` pairing agent (E12-17).
//!
//! `mde-seat`'s [`mde_seat::PairingAgent`] serves `org.bluez.Agent1` and hands each
//! PIN / passkey / confirmation prompt to a [`mde_seat::PairingResponder`]. That
//! callback is **synchronous and runs off the egui thread** (the agent invokes it
//! on a blocking task so a slow operator never stalls the bus), yet the answer must
//! come from an egui dialog on the render thread. This module bridges the two:
//!
//! * [`PairingBridge`] is the shared seam. Its [`PairingResponder`] impl posts the
//!   incoming prompt into a one-slot mailbox and **blocks** (off-thread) on a
//!   channel until the render thread posts the operator's [`PairingReply`].
//! * [`pairing_dialog`] runs each frame the System surface is shown, drains the
//!   mailbox, renders an [`egui::Modal`], and answers the awaiting prompt — never
//!   blocking the render thread (it only reads/writes the mailbox).
//!
//! This mirrors the channel-plus-shared-state bridge `toast_bridge` uses for the
//! Bus alert lane: one small `Arc`-shared state the egui frame drains, no reactor
//! call on the render thread.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use mde_egui::egui::{self, Id, RichText};
use mde_egui::Style;
use mde_seat::{AgentPrompt, PairingReply, PairingResponder};

/// The modal's content width — ten spacing units (§4: derived from the shared
/// scale, never a bare pixel constant).
const DIALOG_WIDTH: f32 = Style::SP_XL * 10.0;

/// One pairing interaction awaiting the render thread.
struct Pending {
    /// The prompt to render.
    prompt: AgentPrompt,
    /// The channel the render thread answers on. `Some` for a prompt that needs a
    /// typed answer; `None` for an informational display (`Display*`), whose reply
    /// `BlueZ` ignores — the dialog is dismiss-only.
    reply: Option<Sender<PairingReply>>,
}

/// The shared pairing seam: the agent's responder posts prompts here and blocks;
/// the egui frame drains and answers. Cloneable — one handle lives in the
/// registered agent (as the `PairingResponder`), the twin in the System surface.
#[derive(Clone, Default)]
pub(crate) struct PairingBridge {
    /// The one-slot mailbox. `BlueZ`'s agent runs on a single current-thread
    /// reactor, so at most one answer-prompt is ever in flight; a later prompt
    /// (or a `Cancel`) supersedes it.
    slot: Arc<Mutex<Option<Pending>>>,
}

impl PairingBridge {
    /// Lock the mailbox, recovering a poisoned guard rather than panicking (a
    /// panicked holder must not wedge pairing — and `unwrap_used` is denied).
    fn guard(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The prompt the render thread should show, cloned so the lock is released
    /// before the frame draws. `None` when no pairing is in flight.
    pub(crate) fn current(&self) -> Option<AgentPrompt> {
        self.guard().as_ref().map(|p| p.prompt.clone())
    }

    /// Post the operator's answer to the awaiting prompt and clear the dialog. A
    /// no-op if nothing is pending; harmless for an informational prompt (no
    /// channel — it just clears).
    pub(crate) fn answer(&self, reply: PairingReply) {
        // Take the slot and drop the lock before sending (never hold it across the
        // channel send).
        let taken = self.guard().take();
        if let Some(mut pending) = taken {
            if let Some(tx) = pending.reply.take() {
                // The receiver is only gone if the agent already tore down — then
                // the reply is moot, so a failed send is fine to drop.
                let _ = tx.send(reply);
            }
        }
    }
}

impl PairingResponder for PairingBridge {
    fn prompt(&self, prompt: AgentPrompt) -> PairingReply {
        match prompt {
            // BlueZ aborted: unblock any awaiting prompt with a Cancel and drop the
            // dialog. The Cancel call's own reply is ignored.
            AgentPrompt::Cancel => {
                let taken = self.guard().take();
                if let Some(mut pending) = taken {
                    if let Some(tx) = pending.reply.take() {
                        let _ = tx.send(PairingReply::Cancel);
                    }
                }
                PairingReply::Dismiss
            }
            // Informational: show it for the operator, but return AT ONCE — the
            // agent method is void and BlueZ expects a prompt return (blocking here
            // would stall the bus). The dialog is dismiss-only; a later prompt (or
            // Cancel, or the operator's Dismiss) clears it.
            AgentPrompt::DisplayPin { .. } | AgentPrompt::DisplayPasskey { .. } => {
                *self.guard() = Some(Pending {
                    prompt,
                    reply: None,
                });
                PairingReply::Dismiss
            }
            // Answerable: post it with a reply channel, then block (off the egui
            // thread) until the frame answers. A dropped channel (agent torn down,
            // or the prompt superseded) folds to Reject — never a hang, never a
            // fabricated code.
            _ => {
                let (tx, rx) = mpsc::channel();
                *self.guard() = Some(Pending {
                    prompt,
                    reply: Some(tx),
                });
                rx.recv().unwrap_or(PairingReply::Reject)
            }
        }
    }
}

/// Render the pairing modal for the current prompt, if any, and answer it on a
/// button click. Called once per frame while the System surface is shown. Never
/// blocks — it only reads/writes the shared mailbox. `input` is the persistent
/// PIN/passkey entry buffer (owned by the surface so it survives across frames).
pub(crate) fn pairing_dialog(ctx: &egui::Context, bridge: &PairingBridge, input: &mut String) {
    let Some(prompt) = bridge.current() else {
        // No dialog in flight — reset the entry buffer so a fresh prompt starts
        // blank.
        input.clear();
        return;
    };
    egui::Modal::new(Id::new("mcnf-bt-pairing")).show(ctx, |ui| {
        ui.set_width(DIALOG_WIDTH);
        ui.label(
            RichText::new("Bluetooth pairing")
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        render_prompt(ui, bridge, &prompt, input);
    });
}

/// The per-variant dialog body — one total dispatch over the prompt taxonomy, so
/// every `AgentPrompt` shape has an explicit, legible render in one place.
#[allow(clippy::too_many_lines)] // the whole prompt taxonomy reads best together
fn render_prompt(
    ui: &mut egui::Ui,
    bridge: &PairingBridge,
    prompt: &AgentPrompt,
    input: &mut String,
) {
    match prompt {
        AgentPrompt::RequestPin { device } => {
            prompt_line(ui, &format!("Enter the PIN for {}", device_label(device)));
            ui.add_space(Style::SP_XS);
            ui.add(egui::TextEdit::singleline(input).hint_text("PIN"));
            ui.add_space(Style::SP_S);
            answer_buttons(ui, |ui| {
                if primary(ui, "Pair") {
                    bridge.answer(PairingReply::Pin(std::mem::take(input)));
                }
                if secondary(ui, "Cancel") {
                    bridge.answer(PairingReply::Cancel);
                    input.clear();
                }
            });
        }
        AgentPrompt::RequestPasskey { device } => {
            prompt_line(
                ui,
                &format!("Enter the passkey for {}", device_label(device)),
            );
            ui.add_space(Style::SP_XS);
            ui.add(egui::TextEdit::singleline(input).hint_text("Passkey (0–999999)"));
            // A non-numeric / out-of-range entry is refused inline rather than sent
            // as a wrong code (the agent would reject it anyway).
            let parsed = input.trim().parse::<u32>().ok().filter(|k| *k <= 999_999);
            if !input.trim().is_empty() && parsed.is_none() {
                ui.colored_label(
                    Style::WARN,
                    RichText::new("A passkey is a number 0–999999.").size(Style::SMALL),
                );
            }
            ui.add_space(Style::SP_S);
            answer_buttons(ui, |ui| {
                let pair =
                    egui::Button::new(RichText::new("Pair").size(Style::SMALL).color(Style::TEXT))
                        .fill(Style::ACCENT);
                if ui.add_enabled(parsed.is_some(), pair).clicked() {
                    if let Some(k) = parsed {
                        bridge.answer(PairingReply::Passkey(k));
                        input.clear();
                    }
                }
                if secondary(ui, "Cancel") {
                    bridge.answer(PairingReply::Cancel);
                    input.clear();
                }
            });
        }
        AgentPrompt::ConfirmPasskey { device, passkey } => {
            prompt_line(
                ui,
                &format!("Does {} show this passkey?", device_label(device)),
            );
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(format!("{passkey:06}"))
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
            ui.add_space(Style::SP_S);
            confirm_buttons(ui, bridge, "It matches", "It doesn't");
        }
        AgentPrompt::Authorize { device } => {
            prompt_line(ui, &format!("Allow {} to pair?", device_label(device)));
            ui.add_space(Style::SP_S);
            confirm_buttons(ui, bridge, "Allow", "Deny");
        }
        AgentPrompt::AuthorizeService { device, uuid } => {
            prompt_line(
                ui,
                &format!("Allow {} to use service {uuid}?", device_label(device)),
            );
            ui.add_space(Style::SP_S);
            confirm_buttons(ui, bridge, "Allow", "Deny");
        }
        AgentPrompt::DisplayPin { device, pin } => {
            prompt_line(ui, &format!("Enter this PIN on {}:", device_label(device)));
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(pin)
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
            ui.add_space(Style::SP_S);
            dismiss_button(ui, bridge);
        }
        AgentPrompt::DisplayPasskey {
            device,
            passkey,
            entered,
        } => {
            prompt_line(
                ui,
                &format!("Enter this passkey on {}:", device_label(device)),
            );
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(format!("{passkey:06}"))
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
            if *entered > 0 {
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(format!("{entered} digit(s) entered")).size(Style::SMALL),
                );
            }
            ui.add_space(Style::SP_S);
            dismiss_button(ui, bridge);
        }
        // Cancel is handled in the responder (it clears the slot), so it never
        // reaches the render path — but stay total rather than panic.
        AgentPrompt::Cancel => {
            dismiss_button(ui, bridge);
        }
    }
}

/// A dim prompt caption line.
fn prompt_line(ui: &mut egui::Ui, text: &str) {
    ui.colored_label(Style::TEXT_DIM, RichText::new(text).size(Style::SMALL));
}

/// A horizontal button row (the shared button-strip layout).
fn answer_buttons(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(body);
}

/// The accept/reject button pair for a yes/no prompt.
fn confirm_buttons(ui: &mut egui::Ui, bridge: &PairingBridge, yes: &str, no: &str) {
    answer_buttons(ui, |ui| {
        if primary(ui, yes) {
            bridge.answer(PairingReply::Accept);
        }
        if secondary(ui, no) {
            bridge.answer(PairingReply::Reject);
        }
    });
}

/// The dismiss-only button for an informational (`Display*`) prompt.
fn dismiss_button(ui: &mut egui::Ui, bridge: &PairingBridge) {
    if secondary(ui, "Dismiss") {
        bridge.answer(PairingReply::Dismiss);
    }
}

/// The primary (accent) action button.
fn primary(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).size(Style::SMALL).color(Style::TEXT))
            .fill(Style::ACCENT),
    )
    .clicked()
}

/// A secondary (default-fill) button.
fn secondary(ui: &mut egui::Ui, label: &str) -> bool {
    ui.button(RichText::new(label).size(Style::SMALL)).clicked()
}

/// A friendly device tag from a `BlueZ` device object path
/// (`/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF` → `AA:BB:CC:DD:EE:FF`). Falls back to
/// the raw tail for a non-`dev_` path — honest, never invented.
fn device_label(path: &str) -> String {
    let tail = path.rsplit('/').next().unwrap_or(path);
    tail.strip_prefix("dev_")
        .map_or_else(|| tail.to_owned(), |mac| mac.replace('_', ":"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Spin until the bridge has a pending prompt (the responder posts it just
    /// before it blocks), bounded so a wiring regression fails instead of hanging.
    fn wait_for_prompt(bridge: &PairingBridge) -> AgentPrompt {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(p) = bridge.current() {
                return p;
            }
            assert!(Instant::now() < deadline, "no prompt ever posted");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn device_label_prettifies_a_bluez_device_path() {
        assert_eq!(
            device_label("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"),
            "AA:BB:CC:DD:EE:FF"
        );
        // A non-dev path falls back to its tail, never a fabricated address.
        assert_eq!(device_label("/org/bluez/hci0"), "hci0");
    }

    #[test]
    fn an_answerable_prompt_blocks_until_the_frame_answers() {
        let bridge = PairingBridge::default();
        let worker = bridge.clone();
        let handle = std::thread::spawn(move || {
            worker.prompt(AgentPrompt::RequestPin {
                device: "/org/bluez/hci0/dev_AA".into(),
            })
        });

        // The render thread sees the prompt, then answers it.
        let prompt = wait_for_prompt(&bridge);
        assert!(matches!(prompt, AgentPrompt::RequestPin { .. }));
        bridge.answer(PairingReply::Pin("1234".into()));

        let reply = handle.join().expect("responder thread joins");
        assert_eq!(reply, PairingReply::Pin("1234".into()));
        // The slot is cleared once answered.
        assert!(bridge.current().is_none());
    }

    #[test]
    fn a_confirmation_prompt_round_trips_accept() {
        let bridge = PairingBridge::default();
        let worker = bridge.clone();
        let handle = std::thread::spawn(move || {
            worker.prompt(AgentPrompt::ConfirmPasskey {
                device: "/org/bluez/hci0/dev_BB".into(),
                passkey: 424_242,
            })
        });
        let prompt = wait_for_prompt(&bridge);
        assert!(
            matches!(prompt, AgentPrompt::ConfirmPasskey { passkey, .. } if passkey == 424_242)
        );
        bridge.answer(PairingReply::Accept);
        assert_eq!(handle.join().expect("joins"), PairingReply::Accept);
    }

    #[test]
    fn a_cancel_unblocks_an_awaiting_prompt() {
        let bridge = PairingBridge::default();
        let worker = bridge.clone();
        let handle = std::thread::spawn(move || {
            worker.prompt(AgentPrompt::RequestPasskey {
                device: "/org/bluez/hci0/dev_CC".into(),
            })
        });
        let _ = wait_for_prompt(&bridge);

        // BlueZ raises Cancel on the (single-reactor) agent — it unblocks the
        // awaiting prompt with a Cancel reply and clears the dialog.
        let cancel_reply = bridge.prompt(AgentPrompt::Cancel);
        assert_eq!(cancel_reply, PairingReply::Dismiss);
        assert_eq!(handle.join().expect("joins"), PairingReply::Cancel);
        assert!(bridge.current().is_none());
    }

    #[test]
    fn an_informational_prompt_returns_at_once_and_shows() {
        let bridge = PairingBridge::default();
        // DisplayPin must NOT block (BlueZ expects a prompt return): the call
        // returns immediately with Dismiss, and the dialog is left showing.
        let reply = bridge.prompt(AgentPrompt::DisplayPin {
            device: "/org/bluez/hci0/dev_DD".into(),
            pin: "654321".into(),
        });
        assert_eq!(reply, PairingReply::Dismiss);
        assert!(matches!(
            bridge.current(),
            Some(AgentPrompt::DisplayPin { .. })
        ));
        // The operator dismisses it.
        bridge.answer(PairingReply::Dismiss);
        assert!(bridge.current().is_none());
    }

    #[test]
    fn answer_with_nothing_pending_is_a_no_op() {
        let bridge = PairingBridge::default();
        bridge.answer(PairingReply::Accept);
        assert!(bridge.current().is_none());
    }
}
