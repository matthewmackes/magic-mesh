//! KDC-MESH-6 mousepad plugin — `kdeconnect.mousepad.request` body.
//!
//! KDE Connect's Android remote-input surface multiplexes relative mouse
//! motion, clicks, scrolls, and keyboard tokens through one request packet.
//! This module only parses and normalizes those packets; host-side injection is
//! intentionally a separate seat/uinput concern.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

const MAX_RELATIVE_DELTA: f64 = 4096.0;
const MAX_KEY_CHARS: usize = 16;

/// `kdeconnect.mousepad.request` body.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MousepadBody {
    /// Relative x movement.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub dx: f64,
    /// Relative y movement.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub dy: f64,
    /// Relative scroll delta.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub scroll: f64,
    /// Primary-button click.
    #[serde(default, alias = "singleclick")]
    pub single_click: bool,
    /// Primary-button double click.
    #[serde(default, alias = "doubleclick")]
    pub double_click: bool,
    /// Secondary-button click.
    #[serde(default, alias = "rightclick")]
    pub right_click: bool,
    /// Middle-button click.
    #[serde(default, alias = "middleclick")]
    pub middle_click: bool,
    /// Text key token.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// Special key code token.
    #[serde(default, alias = "specialkey", skip_serializing_if = "Option::is_none")]
    pub special_key: Option<i64>,
    /// Shift modifier.
    #[serde(default)]
    pub shift: bool,
    /// Control modifier.
    #[serde(default)]
    pub ctrl: bool,
    /// Alt modifier.
    #[serde(default)]
    pub alt: bool,
    /// Super/meta modifier.
    #[serde(default, rename = "super", alias = "superKey")]
    pub super_key: bool,
}

fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}

/// Normalized mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Primary/left button.
    Primary,
    /// Secondary/right button.
    Secondary,
    /// Middle button.
    Middle,
}

impl MouseButton {
    /// Stable wire name used by daemon handoff payloads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Middle => "middle",
        }
    }
}

/// Keyboard modifiers attached to a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MouseModifiers {
    /// Shift modifier.
    pub shift: bool,
    /// Control modifier.
    pub ctrl: bool,
    /// Alt modifier.
    pub alt: bool,
    /// Super/meta modifier.
    #[serde(rename = "super")]
    pub super_key: bool,
}

/// Normalized remote-input event.
#[derive(Debug, Clone, PartialEq)]
pub enum MousepadEvent {
    /// Relative pointer motion.
    Move {
        /// Bounded x movement.
        dx: f64,
        /// Bounded y movement.
        dy: f64,
    },
    /// Relative scroll movement.
    Scroll {
        /// Bounded scroll delta.
        delta: f64,
    },
    /// Mouse-button click.
    Button {
        /// Button clicked.
        button: MouseButton,
        /// Number of clicks.
        clicks: u8,
    },
    /// Text key token.
    Text {
        /// Text to inject.
        text: String,
        /// Active modifiers.
        modifiers: MouseModifiers,
    },
    /// Special-key code token.
    SpecialKey {
        /// Bounded special key code.
        code: i64,
        /// Active modifiers.
        modifiers: MouseModifiers,
    },
}

impl MousepadBody {
    /// Normalize a KDE Connect mousepad request into bounded events.
    #[must_use]
    pub fn events(&self) -> Vec<MousepadEvent> {
        let mut events = Vec::new();
        if let Some((dx, dy)) = bounded_motion(self.dx, self.dy) {
            events.push(MousepadEvent::Move { dx, dy });
        }
        if let Some(delta) = bounded_nonzero_scalar(self.scroll) {
            events.push(MousepadEvent::Scroll { delta });
        }
        if self.single_click {
            events.push(MousepadEvent::Button {
                button: MouseButton::Primary,
                clicks: 1,
            });
        }
        if self.double_click {
            events.push(MousepadEvent::Button {
                button: MouseButton::Primary,
                clicks: 2,
            });
        }
        if self.right_click {
            events.push(MousepadEvent::Button {
                button: MouseButton::Secondary,
                clicks: 1,
            });
        }
        if self.middle_click {
            events.push(MousepadEvent::Button {
                button: MouseButton::Middle,
                clicks: 1,
            });
        }

        let modifiers = MouseModifiers {
            shift: self.shift,
            ctrl: self.ctrl,
            alt: self.alt,
            super_key: self.super_key,
        };
        if let Some(text) = bounded_key(&self.key) {
            events.push(MousepadEvent::Text { text, modifiers });
        }
        if let Some(code) = self.special_key.filter(|code| (0..=255).contains(code)) {
            events.push(MousepadEvent::SpecialKey { code, modifiers });
        }
        events
    }
}

fn bounded_motion(dx: f64, dy: f64) -> Option<(f64, f64)> {
    let dx = bounded_scalar(dx)?;
    let dy = bounded_scalar(dy)?;
    if dx == 0.0 && dy == 0.0 {
        None
    } else {
        Some((dx, dy))
    }
}

fn bounded_scalar(value: f64) -> Option<f64> {
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(-MAX_RELATIVE_DELTA, MAX_RELATIVE_DELTA))
}

fn bounded_nonzero_scalar(value: f64) -> Option<f64> {
    let value = bounded_scalar(value)?;
    if value == 0.0 {
        None
    } else {
        Some(value)
    }
}

fn bounded_key(value: &str) -> Option<String> {
    let text = value.trim();
    if text.is_empty() || text.chars().count() > MAX_KEY_CHARS {
        None
    } else {
        Some(text.to_string())
    }
}

/// Build a remote-input packet.
#[must_use]
pub fn mousepad_packet(id_ms: i64, body: MousepadBody) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.mousepad.request".to_string(),
        body: serde_json::to_value(body).expect("MousepadBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mousepad_body_classifies_motion_click_key_events() {
        let body: MousepadBody = serde_json::from_value(serde_json::json!({
            "dx": 12.0,
            "dy": -3.5,
            "scroll": 1.0,
            "singleclick": true,
            "rightclick": true,
            "key": "a",
            "shift": true,
            "specialKey": 32
        }))
        .unwrap();

        assert_eq!(
            body.events(),
            vec![
                MousepadEvent::Move { dx: 12.0, dy: -3.5 },
                MousepadEvent::Scroll { delta: 1.0 },
                MousepadEvent::Button {
                    button: MouseButton::Primary,
                    clicks: 1,
                },
                MousepadEvent::Button {
                    button: MouseButton::Secondary,
                    clicks: 1,
                },
                MousepadEvent::Text {
                    text: "a".into(),
                    modifiers: MouseModifiers {
                        shift: true,
                        ..Default::default()
                    },
                },
                MousepadEvent::SpecialKey {
                    code: 32,
                    modifiers: MouseModifiers {
                        shift: true,
                        ..Default::default()
                    },
                },
            ]
        );
    }

    #[test]
    fn mousepad_packet_kind_matches_plugin_token() {
        let p = mousepad_packet(1, MousepadBody::default());
        assert_eq!(p.kind, crate::plugins::PluginKind::Mousepad.packet_kind());
        assert!(MousepadBody::default().events().is_empty());
    }

    #[test]
    fn mousepad_body_drops_or_bounds_untrusted_values() {
        let body = MousepadBody {
            dx: f64::INFINITY,
            dy: 1.0,
            scroll: 9000.0,
            key: "this-key-token-is-too-long".into(),
            special_key: Some(400),
            ..Default::default()
        };

        assert_eq!(
            body.events(),
            vec![MousepadEvent::Scroll {
                delta: MAX_RELATIVE_DELTA,
            }]
        );
    }
}
