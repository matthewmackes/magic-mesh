//! SUBSTRATE-V2 — the split coordination/file substrate (design:
//! `docs/design/substrate-v2.md`).
//!
//! Coordination (leader election + peer directory + health) lives in **etcd**
//! (strongly consistent, off the filesystem); bulk files ride **Syncthing**.
//! This module holds the mackesd-side clients. [`etcd`] is the coordination
//! client foundation (endpoints contract + key schema + connect/probe) the
//! leader/directory/health migrations (SUBSTRATE-2/3/4) build on.

pub mod etcd;
pub mod leader;
pub mod peers;
