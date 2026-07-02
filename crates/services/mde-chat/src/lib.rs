//! mde-chat — the pure model + logic for the ICQ-style **Mesh Chat** unified
//! messaging + notification interface (NOTIFY-CHAT-1; design:
//! `docs/design/mesh-chat-icq.md`, 25 locks).
//!
//! Mesh Chat makes **every host a contact and every one of its alerts a message
//! from that contact** — the hostname *is* the username (lock 2/21). Human chat
//! and machine notifications share one timeline per contact. This crate is the
//! shared, headless model that both the mackesd `chat` worker (NOTIFY-CHAT-2)
//! and the `Surface::Chat` UI (NOTIFY-CHAT-3) import:
//!
//!   * [`Message`] + [`MessageKind`] — the six message kinds (Text, Clipboard,
//!     Alert, File, Call, Remote), each serde-round-trippable, carrying the
//!     sender **host**, an injected-time [`MessageId`], and an optional Ed25519
//!     [`Signature`] (message.rs).
//!   * [`Conversation`] + [`Room`] — an append-only, bounded **ring buffer**
//!     (evicts oldest, lock 8) with a stable total order (sender timestamp,
//!     signature tiebreak — lock 22); ad-hoc rooms + the auto system rooms
//!     ([`system_room_descriptors`]: All Fleet + per-severity), open self-join,
//!     creator-only dissolve (lock 7/25) (conversation.rs).
//!   * [`NotifyPrefs`] — the pure **notification policy** (NOTIFY-CHAT-5):
//!     per-contact / per-room mute + a global per-severity threshold, deciding
//!     whether an event *rings* (KIRON chyron + sound) or stays silent — a
//!     muted / below-threshold event is silent but STILL logged (notify.rs).
//!   * [`Roster`] / [`Contact`] / [`Presence`] — a contact is a mesh member
//!     (hostname identity + cosmetic nickname + free-text status, lock 21);
//!     presence is auto (Online/Away/Offline) ∪ manual (Away/DND/Invisible/
//!     Free-for-Chat, lock 5) (roster.rs).
//!   * [`sign`] / [`Message::verify`] — Ed25519 over the canonical message
//!     bytes; a tampered sender or body fails verify (lock 10) (message.rs).
//!   * [`fold_alert`] + [`Severity`] — the pure alert-fold: a real Bus alert
//!     (`event/security/alert`, …) → an [`MessageKind::Alert`] from the
//!     originating host (lock 11) (alert.rs).
//!
//! **Zero I/O**: no Bus, no Syncthing, no zbus, no wall-clock — the live
//! plumbing is the NOTIFY-CHAT-2 worker's. Services tier: no desktop-shell dep
//! (the layered-tiers gate).

#![forbid(unsafe_code)]

mod alert;
mod conversation;
mod message;
mod notify;
mod roster;

pub use alert::{alert_flag, fold_alert, Severity};
pub use conversation::{
    severity_room_id, system_room_descriptors, Conversation, Room, RoomDescriptor, RoomKind,
    SYS_ALL_FLEET_ID,
};
pub use message::{sign, Message, MessageId, MessageKind, Signature};
pub use notify::NotifyPrefs;
pub use roster::{Contact, NodeRole, Presence, Roster};
