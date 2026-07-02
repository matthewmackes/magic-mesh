//! The `BlueZ` pairing **agent** — the object `bluetoothd` calls back to answer a
//! PIN / passkey / confirmation prompt while a device pairs (E12-17, lock 5).
//!
//! On a bare DRM seat there is no desktop agent, so pairing anything that needs a
//! code would silently fail. This module registers an `org.bluez.Agent1` object
//! and makes it the default agent; when `BlueZ` raises a prompt during
//! [`crate::BluezClient::pair`], the agent turns it into a typed [`AgentPrompt`]
//! and hands it to the shell's injected [`PairingResponder`] — the shell renders
//! the dialog and returns a [`PairingReply`]. **No PIN is ever hard-coded**: every
//! code comes back from the operator through the responder.
//!
//! The design splits cleanly for headless testing (the build host has no BT
//! adapter): the reply→outcome mapping is the pure [`resolve_pin`] /
//! [`resolve_passkey`] / [`resolve_confirm`] functions, unit-tested with hand-fed
//! replies; the zbus object + `AgentManager1` registration is the thin I/O shell
//! (a live pair-a-device demo is hardware-gated).

// The zbus `#[interface]` contract forces `async fn` + `&self` even on methods
// whose body is trivial, and deserializes params we may ignore — the same
// pattern mde-musicd's MPRIS surface documents.
#![allow(
    clippy::unused_async,
    clippy::unused_self,
    clippy::used_underscore_binding
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use zbus::zvariant::ObjectPath;

use crate::error::{classify_call, Backend, SeatError};

/// The object path our agent is served at (an MDE-owned path under `/org/mde`).
const AGENT_PATH: &str = "/org/mde/seat/bt_agent";
/// The `BlueZ` well-known name + `AgentManager1` object.
const BLUEZ: &str = "org.bluez";
/// The `AgentManager1` object path.
const AGENT_MANAGER_PATH: &str = "/org/bluez";
/// The `AgentManager1` interface.
const AGENT_MANAGER_IFACE: &str = "org.bluez.AgentManager1";
/// Our agent's capability. `KeyboardDisplay` is the fullest profile — it can
/// answer PIN entry, passkey entry, and yes/no confirmation, so the shell dialog
/// covers every prompt shape.
const AGENT_CAPABILITY: &str = "KeyboardDisplay";

/// A prompt `BlueZ` raised during pairing that needs an operator answer. The
/// shell renders each as a dialog; its answer becomes a [`PairingReply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentPrompt {
    /// `RequestPinCode` — legacy PIN entry. Reply with [`PairingReply::Pin`].
    RequestPin {
        /// The device being paired (object path).
        device: String,
    },
    /// `RequestPasskey` — numeric passkey entry (0–999999). Reply with
    /// [`PairingReply::Passkey`].
    RequestPasskey {
        /// The device being paired.
        device: String,
    },
    /// `RequestConfirmation` — "does this code match on both ends?". Reply
    /// [`PairingReply::Accept`] or [`PairingReply::Reject`].
    ConfirmPasskey {
        /// The device being paired.
        device: String,
        /// The passkey both ends should show.
        passkey: u32,
    },
    /// `RequestAuthorization` — authorize an incoming just-worked pairing (no
    /// code). Reply [`PairingReply::Accept`] / [`PairingReply::Reject`].
    Authorize {
        /// The device being paired.
        device: String,
    },
    /// `AuthorizeService` — authorize a service (profile) on a paired device.
    /// Reply [`PairingReply::Accept`] / [`PairingReply::Reject`].
    AuthorizeService {
        /// The device.
        device: String,
        /// The service UUID being requested.
        uuid: String,
    },
    /// `DisplayPinCode` — show this PIN so the operator types it on the device.
    /// Informational: the reply is ignored (the dialog is dismiss-only).
    DisplayPin {
        /// The device.
        device: String,
        /// The PIN to display.
        pin: String,
    },
    /// `DisplayPasskey` — show this passkey (called repeatedly as the remote end
    /// enters digits). Informational: the reply is ignored.
    DisplayPasskey {
        /// The device.
        device: String,
        /// The passkey to display.
        passkey: u32,
        /// How many digits the remote end has entered so far.
        entered: u16,
    },
    /// `Cancel` — `BlueZ` aborted the pairing; the shell should dismiss any open
    /// dialog. Informational: the reply is ignored.
    Cancel,
}

/// The operator's answer to an [`AgentPrompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingReply {
    /// A PIN string (for [`AgentPrompt::RequestPin`]).
    Pin(String),
    /// A numeric passkey 0–999999 (for [`AgentPrompt::RequestPasskey`]).
    Passkey(u32),
    /// Accept / confirm (for the confirmation + authorization prompts).
    Accept,
    /// Reject — folds to `org.bluez.Error.Rejected`.
    Reject,
    /// Cancel — folds to `org.bluez.Error.Canceled`.
    Cancel,
    /// The dialog was dismissed with no meaningful answer — treated as a
    /// [`PairingReply::Reject`] for prompts that need one; ignored for the
    /// informational prompts.
    Dismiss,
}

/// Why the agent refused a prompt — mapped to the matching `org.bluez` error the
/// pairing state machine understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `org.bluez.Error.Rejected` — the operator said no (or answered wrongly).
    Rejected,
    /// `org.bluez.Error.Canceled` — the operator canceled the flow.
    Canceled,
}

/// The shell-provided seam the agent hands each prompt to.
///
/// The shell renders the dialog and **blocks** until the operator answers (the
/// agent runs the callback off the reactor via a blocking task, so the wait
/// never wedges the bus). A test injects a fake that returns canned replies.
pub trait PairingResponder: Send + Sync {
    /// Answer one pairing prompt. Informational prompts (`Display*`, `Cancel`)
    /// still call this so the shell can update/close the dialog; their reply is
    /// ignored (return [`PairingReply::Dismiss`]).
    fn prompt(&self, prompt: AgentPrompt) -> PairingReply;
}

/// Map a reply to a `RequestPinCode` outcome. A non-PIN reply (a mis-wired
/// dialog) is refused rather than sent as a bogus PIN.
///
/// # Errors
/// [`Refusal::Rejected`] for reject/dismiss/wrong-shape, [`Refusal::Canceled`]
/// for cancel.
pub fn resolve_pin(reply: &PairingReply) -> Result<String, Refusal> {
    match reply {
        PairingReply::Pin(pin) => Ok(pin.clone()),
        PairingReply::Cancel => Err(Refusal::Canceled),
        _ => Err(Refusal::Rejected),
    }
}

/// Map a reply to a `RequestPasskey` outcome. `BlueZ` passkeys are 0–999999; an
/// out-of-range value is refused, never truncated into a wrong code.
///
/// # Errors
/// [`Refusal::Rejected`] for reject/dismiss/out-of-range/wrong-shape,
/// [`Refusal::Canceled`] for cancel.
pub const fn resolve_passkey(reply: &PairingReply) -> Result<u32, Refusal> {
    match reply {
        PairingReply::Passkey(k) if *k <= 999_999 => Ok(*k),
        PairingReply::Cancel => Err(Refusal::Canceled),
        _ => Err(Refusal::Rejected),
    }
}

/// Map a reply to a confirmation-style outcome (`RequestConfirmation`,
/// `RequestAuthorization`, `AuthorizeService`): accept → allowed, else refused.
///
/// A value-bearing reply (Pin/Passkey) to a yes/no prompt is a mis-wire and is
/// refused, not silently accepted.
///
/// # Errors
/// [`Refusal::Rejected`] for reject/dismiss/wrong-shape, [`Refusal::Canceled`]
/// for cancel.
pub const fn resolve_confirm(reply: &PairingReply) -> Result<(), Refusal> {
    match reply {
        PairingReply::Accept => Ok(()),
        PairingReply::Cancel => Err(Refusal::Canceled),
        _ => Err(Refusal::Rejected),
    }
}

/// The `org.bluez` errors the pairing state machine expects from an agent.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    /// The operator declined.
    Rejected,
    /// The operator canceled.
    Canceled,
}

impl From<Refusal> for AgentError {
    fn from(r: Refusal) -> Self {
        match r {
            Refusal::Rejected => Self::Rejected,
            Refusal::Canceled => Self::Canceled,
        }
    }
}

/// The served `org.bluez.Agent1` object. Each method turns `BlueZ`'s callback
/// into an [`AgentPrompt`], runs the responder off the reactor, and maps the
/// [`PairingReply`] back to the wire answer / error.
struct Agent1 {
    responder: Arc<dyn PairingResponder>,
}

impl Agent1 {
    /// Run the (blocking) responder off the async reactor so a slow operator
    /// never stalls the bus connection. A join failure degrades to `Reject`.
    async fn ask(&self, prompt: AgentPrompt) -> PairingReply {
        let responder = Arc::clone(&self.responder);
        tokio::task::spawn_blocking(move || responder.prompt(prompt))
            .await
            .unwrap_or(PairingReply::Reject)
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent1 {
    /// `BlueZ` unregistered us (adapter gone, another default agent). Nothing to
    /// do — the [`PairingAgent`] guard owns our lifecycle.
    async fn release(&self) {}

    async fn request_pin_code(&self, device: ObjectPath<'_>) -> Result<String, AgentError> {
        let reply = self
            .ask(AgentPrompt::RequestPin {
                device: device.to_string(),
            })
            .await;
        resolve_pin(&reply).map_err(AgentError::from)
    }

    async fn display_pin_code(&self, device: ObjectPath<'_>, pincode: String) {
        let _ = self
            .ask(AgentPrompt::DisplayPin {
                device: device.to_string(),
                pin: pincode,
            })
            .await;
    }

    async fn request_passkey(&self, device: ObjectPath<'_>) -> Result<u32, AgentError> {
        let reply = self
            .ask(AgentPrompt::RequestPasskey {
                device: device.to_string(),
            })
            .await;
        resolve_passkey(&reply).map_err(AgentError::from)
    }

    async fn display_passkey(&self, device: ObjectPath<'_>, passkey: u32, entered: u16) {
        let _ = self
            .ask(AgentPrompt::DisplayPasskey {
                device: device.to_string(),
                passkey,
                entered,
            })
            .await;
    }

    async fn request_confirmation(
        &self,
        device: ObjectPath<'_>,
        passkey: u32,
    ) -> Result<(), AgentError> {
        let reply = self
            .ask(AgentPrompt::ConfirmPasskey {
                device: device.to_string(),
                passkey,
            })
            .await;
        resolve_confirm(&reply).map_err(AgentError::from)
    }

    async fn request_authorization(&self, device: ObjectPath<'_>) -> Result<(), AgentError> {
        let reply = self
            .ask(AgentPrompt::Authorize {
                device: device.to_string(),
            })
            .await;
        resolve_confirm(&reply).map_err(AgentError::from)
    }

    async fn authorize_service(
        &self,
        device: ObjectPath<'_>,
        uuid: String,
    ) -> Result<(), AgentError> {
        let reply = self
            .ask(AgentPrompt::AuthorizeService {
                device: device.to_string(),
                uuid,
            })
            .await;
        resolve_confirm(&reply).map_err(AgentError::from)
    }

    async fn cancel(&self) {
        let _ = self.ask(AgentPrompt::Cancel).await;
    }
}

/// A registered pairing agent, alive until dropped.
///
/// [`PairingAgent::register`]
/// spawns a dedicated thread that owns the agent's own system-bus connection
/// (`BlueZ` calls the agent back on the connection that registered it), serves
/// the `org.bluez.Agent1` object, and makes it the default agent. Dropping the
/// handle unregisters it and tears the thread down.
pub struct PairingAgent {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl PairingAgent {
    /// Register `responder` as the seat's `BlueZ` pairing agent. Returns once the
    /// agent is live (or a typed error if there is no bus / no `BlueZ`) — the
    /// shell calls this once at seat start alongside the auto-reconnect pass.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when the system bus / `BlueZ` is absent (the
    /// honest headless state); [`SeatError::Backend`] if `RegisterAgent` /
    /// `RequestDefaultAgent` is refused (e.g. another default agent is bound).
    pub fn register(responder: Arc<dyn PairingResponder>) -> Result<Self, SeatError> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let (tx, rx) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("mde-seat-bt-agent".to_owned())
            .spawn(move || run_agent(&responder, &stop_thread, &tx))
            .map_err(|e| SeatError::Backend {
                backend: Backend::Bluetooth,
                reason: format!("pairing-agent thread spawn: {e}"),
            })?;
        match rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                join: Some(join),
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(e) => {
                let _ = join.join();
                Err(SeatError::Backend {
                    backend: Backend::Bluetooth,
                    reason: format!("pairing-agent setup dropped: {e}"),
                })
            }
        }
    }

    /// Unregister the agent and join its thread. Idempotent; also runs on drop.
    pub fn unregister(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for PairingAgent {
    fn drop(&mut self) {
        self.unregister();
    }
}

/// The agent thread body: build the runtime + connection, register, signal
/// setup success/failure back to [`PairingAgent::register`], then idle until the
/// stop flag flips and unregister.
fn run_agent(
    responder: &Arc<dyn PairingResponder>,
    stop: &AtomicBool,
    tx: &std::sync::mpsc::Sender<Result<(), SeatError>>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = tx.send(Err(SeatError::Backend {
                backend: Backend::Bluetooth,
                reason: format!("pairing-agent runtime: {e}"),
            }));
            return;
        }
    };
    rt.block_on(async {
        match setup_agent(Arc::clone(responder)).await {
            Ok(conn) => {
                let _ = tx.send(Ok(()));
                while !stop.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                // Best-effort unregister — a dead bus on teardown is harmless.
                let _ = conn
                    .call_method(
                        Some(BLUEZ),
                        AGENT_MANAGER_PATH,
                        Some(AGENT_MANAGER_IFACE),
                        "UnregisterAgent",
                        &(agent_path(),),
                    )
                    .await;
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        }
    });
}

/// Our agent's path as a typed `ObjectPath` — the const is always valid, so the
/// unwrap cannot fire.
fn agent_path() -> ObjectPath<'static> {
    ObjectPath::try_from(AGENT_PATH).expect("AGENT_PATH is a valid object path")
}

/// Serve the `Agent1` object, then register it + claim the default-agent slot.
async fn setup_agent(responder: Arc<dyn PairingResponder>) -> Result<zbus::Connection, SeatError> {
    let unavailable = |ctx: &str, e: zbus::Error| SeatError::Unavailable {
        backend: Backend::Bluetooth,
        reason: format!("{ctx}: {e}"),
    };
    let conn = zbus::connection::Builder::system()
        .and_then(|b| b.serve_at(AGENT_PATH, Agent1 { responder }))
        .map_err(|e| unavailable("serve Agent1", e))?
        .build()
        .await
        .map_err(|e| unavailable("system bus", e))?;

    conn.call_method(
        Some(BLUEZ),
        AGENT_MANAGER_PATH,
        Some(AGENT_MANAGER_IFACE),
        "RegisterAgent",
        &(agent_path(), AGENT_CAPABILITY),
    )
    .await
    .map_err(|e| classify_call(Backend::Bluetooth, "RegisterAgent", &e))?;

    conn.call_method(
        Some(BLUEZ),
        AGENT_MANAGER_PATH,
        Some(AGENT_MANAGER_IFACE),
        "RequestDefaultAgent",
        &(agent_path(),),
    )
    .await
    .map_err(|e| classify_call(Backend::Bluetooth, "RequestDefaultAgent", &e))?;

    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_resolves_only_from_a_pin_reply() {
        assert_eq!(
            resolve_pin(&PairingReply::Pin("0000".into())),
            Ok("0000".into())
        );
        // A cancel is a cancel; everything else that isn't a PIN is a reject —
        // we never send a fabricated or wrong-shaped PIN onto the wire.
        assert_eq!(resolve_pin(&PairingReply::Cancel), Err(Refusal::Canceled));
        assert_eq!(resolve_pin(&PairingReply::Reject), Err(Refusal::Rejected));
        assert_eq!(resolve_pin(&PairingReply::Dismiss), Err(Refusal::Rejected));
        assert_eq!(
            resolve_pin(&PairingReply::Passkey(1234)),
            Err(Refusal::Rejected)
        );
        assert_eq!(resolve_pin(&PairingReply::Accept), Err(Refusal::Rejected));
    }

    #[test]
    fn passkey_resolves_in_range_only() {
        assert_eq!(resolve_passkey(&PairingReply::Passkey(0)), Ok(0));
        assert_eq!(
            resolve_passkey(&PairingReply::Passkey(999_999)),
            Ok(999_999)
        );
        // Out of the BlueZ 0–999999 range is refused, not truncated to a wrong
        // code.
        assert_eq!(
            resolve_passkey(&PairingReply::Passkey(1_000_000)),
            Err(Refusal::Rejected)
        );
        assert_eq!(
            resolve_passkey(&PairingReply::Cancel),
            Err(Refusal::Canceled)
        );
        assert_eq!(
            resolve_passkey(&PairingReply::Reject),
            Err(Refusal::Rejected)
        );
        assert_eq!(
            resolve_passkey(&PairingReply::Pin("x".into())),
            Err(Refusal::Rejected)
        );
    }

    #[test]
    fn confirm_accepts_only_on_accept() {
        assert_eq!(resolve_confirm(&PairingReply::Accept), Ok(()));
        assert_eq!(
            resolve_confirm(&PairingReply::Reject),
            Err(Refusal::Rejected)
        );
        assert_eq!(
            resolve_confirm(&PairingReply::Dismiss),
            Err(Refusal::Rejected)
        );
        assert_eq!(
            resolve_confirm(&PairingReply::Cancel),
            Err(Refusal::Canceled)
        );
        // A value-bearing reply to a yes/no prompt is a mis-wire — refused, never
        // silently taken as a yes.
        assert_eq!(
            resolve_confirm(&PairingReply::Passkey(1)),
            Err(Refusal::Rejected)
        );
    }

    #[test]
    fn refusal_maps_to_the_matching_bluez_error_variant() {
        assert!(matches!(
            AgentError::from(Refusal::Rejected),
            AgentError::Rejected
        ));
        assert!(matches!(
            AgentError::from(Refusal::Canceled),
            AgentError::Canceled
        ));
    }

    /// A responder that records the prompt it saw and returns a scripted reply —
    /// proves the prompt→reply seam is exercised end-to-end without a bus.
    struct ScriptedResponder {
        seen: std::sync::Mutex<Vec<AgentPrompt>>,
        reply: PairingReply,
    }

    impl PairingResponder for ScriptedResponder {
        fn prompt(&self, prompt: AgentPrompt) -> PairingReply {
            self.seen.lock().unwrap().push(prompt);
            self.reply.clone()
        }
    }

    #[test]
    fn a_responder_sees_the_typed_prompt_and_its_reply_drives_the_outcome() {
        let responder = ScriptedResponder {
            seen: std::sync::Mutex::new(vec![]),
            reply: PairingReply::Passkey(4242),
        };
        // Simulate what Agent1::request_passkey does with the responder's answer.
        let reply = responder.prompt(AgentPrompt::RequestPasskey {
            device: "/org/bluez/hci0/dev_AA".into(),
        });
        assert_eq!(resolve_passkey(&reply), Ok(4242));
        assert_eq!(
            responder.seen.lock().unwrap()[0],
            AgentPrompt::RequestPasskey {
                device: "/org/bluez/hci0/dev_AA".into()
            }
        );
    }

    #[test]
    fn the_const_agent_path_is_a_valid_object_path() {
        // The unwrap in agent_path() is load-bearing — guard the const.
        assert_eq!(agent_path().as_str(), AGENT_PATH);
    }
}
