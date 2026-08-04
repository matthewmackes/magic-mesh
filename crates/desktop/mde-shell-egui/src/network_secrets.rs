//! The shell-side bridge for the typed NetworkManager SecretAgent.
//!
//! The agent callback runs off the egui thread and waits on this one-slot
//! mailbox. The render thread owns the ephemeral input buffer and answers the
//! prompt; after the answer is sent, the buffer is cleared and no secret is
//! persisted or mirrored into This Node state.

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use mde_egui::egui::{self, Id, RichText};
use mde_egui::Style;
use mde_seat::{NetworkSecretResponder, SecretReply, SecretRequest, SecretSettings};
use zbus::zvariant::{OwnedValue, Value};

struct Pending {
    request: SecretRequest,
    reply: Sender<SecretReply>,
}

/// Cloneable shell mailbox shared by the NetworkManager agent and the egui
/// surface.
#[derive(Clone, Default)]
pub(crate) struct NetworkSecretBridge {
    slot: Arc<Mutex<Option<Pending>>>,
}

impl NetworkSecretBridge {
    fn guard(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn current(&self) -> Option<SecretRequest> {
        self.guard().as_ref().map(|pending| pending.request.clone())
    }

    fn answer(&self, reply: SecretReply) {
        let pending = self.guard().take();
        if let Some(pending) = pending {
            let _ = pending.reply.send(reply);
        }
    }

    pub(crate) fn refuse(&self) {
        self.answer(SecretReply::Refused);
    }

    fn submit(&self, request: &SecretRequest, value: String) {
        let Some(key) = request.hints.first().filter(|key| safe_secret_key(key)) else {
            self.refuse();
            return;
        };
        let mut values = HashMap::new();
        let Ok(secret) = OwnedValue::try_from(Value::from(value)) else {
            self.refuse();
            return;
        };
        values.insert(key.clone(), secret);
        let mut settings = SecretSettings::new();
        settings.insert(request.setting_name.clone(), values);
        self.answer(SecretReply::Secrets(settings));
    }
}

impl NetworkSecretResponder for NetworkSecretBridge {
    fn request(&self, request: SecretRequest) -> SecretReply {
        let (tx, rx) = mpsc::channel();
        let old = self.guard().replace(Pending { request, reply: tx });
        drop(old);
        rx.recv().unwrap_or(SecretReply::Refused)
    }
}

fn safe_secret_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 96
        && !key.chars().any(char::is_control)
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Render the one-shot SecretAgent prompt. It is deliberately a modal: profile
/// activation cannot continue in the background while the operator is deciding.
pub(crate) fn network_secret_dialog(
    ctx: &egui::Context,
    bridge: &NetworkSecretBridge,
    input: &mut String,
) {
    let Some(request) = bridge.current() else {
        input.clear();
        return;
    };
    egui::Modal::new(Id::new("mcnf-network-secret")).show(ctx, |ui| {
        ui.set_width(Style::SP_XL * 10.0);
        ui.label(
            RichText::new("Network profile credentials")
                .strong()
                .size(Style::BODY),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(format!("NetworkManager requests {}", request.setting_name))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        if let Some(key) = request.hints.first() {
            ui.label(
                RichText::new(format!("Secret field: {key}"))
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        } else {
            ui.colored_label(Style::WARN, "The provider supplied no bounded secret field; activation is refused.");
        }
        ui.add_space(Style::SP_XS);
        ui.add(egui::TextEdit::singleline(input).password(true).hint_text("Enter secret"));
        ui.add_space(Style::SP_S);
        ui.horizontal_wrapped(|ui| {
            let can_submit = !input.is_empty() && request.hints.first().is_some_and(|key| safe_secret_key(key));
            if ui.add_enabled(can_submit, egui::Button::new("Activate")).clicked() {
                bridge.submit(&request, std::mem::take(input));
            }
            if ui.button("Cancel").clicked() {
                bridge.refuse();
                input.clear();
            }
        });
        ui.label(
            RichText::new("Credentials are used only for this NetworkManager request and are never saved or published.")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_keys_are_bounded_and_not_shell_fragments() {
        assert!(safe_secret_key("psk"));
        assert!(!safe_secret_key("password=value"));
        assert!(!safe_secret_key("secret\nfield"));
    }
}
