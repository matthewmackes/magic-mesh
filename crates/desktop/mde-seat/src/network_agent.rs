//! NetworkManager's credential callback boundary.
//!
//! Profile activation is intentionally split from the read-only NetworkManager
//! client. NetworkManager asks this in-process agent for secrets only while an
//! operator-approved activation is running; the agent never persists, logs, or
//! mirrors the returned values. A host without a responder remains an honest
//! typed refusal instead of falling back to `nmcli`, shell composition, or a
//! world-readable snapshot.

#![allow(
    clippy::unused_async,
    clippy::unused_self,
    clippy::used_underscore_binding
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use zbus::zvariant::{ObjectPath, OwnedValue};

use crate::error::{classify_call, Backend, SeatError};

const NETWORK_MANAGER: &str = "org.freedesktop.NetworkManager";
const AGENT_MANAGER_PATH: &str = "/org/freedesktop/NetworkManager/AgentManager";
const AGENT_MANAGER_IFACE: &str = "org.freedesktop.NetworkManager.AgentManager";
const AGENT_PATH: &str = "/org/mde/seat/network_secret_agent";
const AGENT_IDENTIFIER: &str = "mde-seat";

/// The bounded prompt handed to the trusted local responder. No secret value
/// is present in this request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRequest {
    /// The NetworkManager profile object path.
    pub connection_path: String,
    /// The setting family requesting secrets (for example `802-11-wireless`).
    pub setting_name: String,
    /// Provider hints, bounded and never interpreted as credentials.
    pub hints: Vec<String>,
    /// NetworkManager's request flags.
    pub flags: u32,
}

/// Secret material returned only across the active D-Bus callback. Callers
/// should construct it from an interactive trusted-session prompt and drop it
/// immediately after NetworkManager completes the request.
pub type SecretSettings = HashMap<String, HashMap<String, OwnedValue>>;

/// A responder outcome. Refusal maps to NetworkManager's standard no-secrets
/// error and therefore cannot accidentally activate a profile without consent.
#[derive(Debug)]
pub enum SecretReply {
    /// Return the typed NetworkManager setting map for this request.
    Secrets(SecretSettings),
    /// The operator canceled or the provider declined to answer.
    Refused,
}

/// Shell-owned callback that supplies secrets after explicit trusted-session
/// authorization and confirmation.
pub trait NetworkSecretResponder: Send + Sync {
    /// Answer one bounded prompt. Implementations must not persist or log the
    /// returned secret map.
    fn request(&self, request: SecretRequest) -> SecretReply;
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.SecretAgent.Error")]
enum SecretAgentError {
    /// The operator canceled or declined the prompt.
    Canceled,
    /// No bounded secrets were supplied by the responder.
    NoSecrets,
    /// The responder returned a malformed or over-sized map.
    Failed,
}

impl SecretAgentError {
    fn from_reply(reply: SecretReply, request: SecretRequest) -> Result<SecretSettings, Self> {
        let SecretReply::Secrets(settings) = reply else {
            return Err(Self::Canceled);
        };
        if !valid_secret_settings(&request, &settings) {
            return Err(Self::Failed);
        }
        if settings.is_empty() {
            return Err(Self::NoSecrets);
        }
        Ok(settings)
    }
}

/// Validate only shape and bounds. Values remain opaque `OwnedValue`s and are
/// never stringified, compared, or written to a log.
fn valid_secret_settings(request: &SecretRequest, settings: &SecretSettings) -> bool {
    crate::network::safe_settings_path(&request.connection_path)
        && !request.setting_name.is_empty()
        && request.setting_name.len() <= 64
        && request
            .setting_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && request.hints.len() <= 16
        && request
            .hints
            .iter()
            .all(|hint| hint.len() <= 96 && !hint.chars().any(char::is_control))
        && settings.len() <= 16
        && settings.iter().all(|(setting, values)| {
            setting.len() <= 64
                && !setting.chars().any(char::is_control)
                && values.len() <= 32
                && values.keys().all(|key| {
                    key.len() <= 96 && !key.chars().any(char::is_control)
                })
        })
}

struct SecretAgent {
    responder: Arc<dyn NetworkSecretResponder>,
}

impl SecretAgent {
    async fn answer(&self, request: SecretRequest) -> Result<SecretSettings, SecretAgentError> {
        let responder = Arc::clone(&self.responder);
        let worker_request = request.clone();
        let reply = tokio::task::spawn_blocking(move || responder.request(worker_request))
            .await
            .unwrap_or(SecretReply::Refused);
        SecretAgentError::from_reply(reply, request)
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.SecretAgent")]
impl SecretAgent {
    async fn get_secrets(
        &self,
        _connection: HashMap<String, HashMap<String, OwnedValue>>,
        connection_path: ObjectPath<'_>,
        setting_name: String,
        hints: Vec<String>,
        flags: u32,
    ) -> Result<SecretSettings, SecretAgentError> {
        self.answer(SecretRequest {
            connection_path: connection_path.to_string(),
            setting_name,
            hints,
            flags,
        })
        .await
    }

    async fn cancel_get_secrets(
        &self,
        _connection_path: ObjectPath<'_>,
        _setting_name: String,
    ) {
    }

    async fn save_secrets(
        &self,
        _connection: HashMap<String, HashMap<String, OwnedValue>>,
        _connection_path: ObjectPath<'_>,
    ) -> Result<(), SecretAgentError> {
        Err(SecretAgentError::Failed)
    }

    async fn delete_secrets(
        &self,
        _connection_path: ObjectPath<'_>,
    ) -> Result<(), SecretAgentError> {
        Err(SecretAgentError::Failed)
    }
}

/// A registered, non-persistent NetworkManager SecretAgent.
pub struct NetworkSecretAgent {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl NetworkSecretAgent {
    /// Register the agent on the system bus. The callback is live only for the
    /// returned handle's lifetime.
    pub fn register(responder: Arc<dyn NetworkSecretResponder>) -> Result<Self, SeatError> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let (tx, rx) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("mde-seat-network-agent".to_owned())
            .spawn(move || run_agent(responder, thread_stop, tx))
            .map_err(|error| SeatError::Backend {
                backend: Backend::Network,
                reason: format!("network SecretAgent thread spawn: {error}"),
            })?;
        match rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(error) => {
                let _ = join.join();
                Err(SeatError::Backend {
                    backend: Backend::Network,
                    reason: format!("network SecretAgent setup dropped: {error}"),
                })
            }
        }
    }

    /// Unregister the agent. Idempotent and also called by `Drop`.
    pub fn unregister(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for NetworkSecretAgent {
    fn drop(&mut self) {
        self.unregister();
    }
}

fn run_agent(
    responder: Arc<dyn NetworkSecretResponder>,
    stop: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<Result<(), SeatError>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = tx.send(Err(SeatError::Backend {
                backend: Backend::Network,
                reason: format!("network SecretAgent runtime: {error}"),
            }));
            return;
        }
    };
    runtime.block_on(async move {
        let builder = match zbus::connection::Builder::system() {
            Ok(builder) => builder,
            Err(error) => {
                let _ = tx.send(Err(SeatError::Unavailable {
                    backend: Backend::Network,
                    reason: format!("system D-Bus for NetworkManager SecretAgent: {error}"),
                }));
                return;
            }
        };
        let builder = match builder.serve_at(AGENT_PATH, SecretAgent { responder }) {
            Ok(builder) => builder,
            Err(error) => {
                let _ = tx.send(Err(SeatError::Unavailable {
                    backend: Backend::Network,
                    reason: format!("serve NetworkManager SecretAgent: {error}"),
                }));
                return;
            }
        };
        let connection = match builder.build().await {
            Ok(connection) => connection,
            Err(error) => {
                let _ = tx.send(Err(SeatError::Unavailable {
                    backend: Backend::Network,
                    reason: format!("system D-Bus for NetworkManager SecretAgent: {error}"),
                }));
                return;
            }
        };
        if let Err(error) = connection
            .call_method(
                Some(NETWORK_MANAGER),
                AGENT_MANAGER_PATH,
                Some(AGENT_MANAGER_IFACE),
                "Register",
                &(AGENT_IDENTIFIER,),
            )
            .await
        {
            let _ = tx.send(Err(classify_call(Backend::Network, "AgentManager.Register", &error)));
            return;
        }
        let _ = tx.send(Ok(()));
        while !stop.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let _ = connection
            .call_method(
                Some(NETWORK_MANAGER),
                AGENT_MANAGER_PATH,
                Some(AGENT_MANAGER_IFACE),
                "Unregister",
                &(),
            )
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_shape_rejects_unsafe_request_metadata() {
        let request = SecretRequest {
            connection_path: "/tmp/not-a-profile".to_owned(),
            setting_name: "802-11-wireless".to_owned(),
            hints: Vec::new(),
            flags: 0,
        };
        assert!(!valid_secret_settings(&request, &HashMap::new()));
    }

    #[test]
    fn secret_shape_accepts_bounded_empty_value_map_for_provider_validation() {
        let request = SecretRequest {
            connection_path: "/org/freedesktop/NetworkManager/Settings/4".to_owned(),
            setting_name: "802-11-wireless".to_owned(),
            hints: vec!["psk".to_owned()],
            flags: 0,
        };
        assert!(valid_secret_settings(&request, &HashMap::new()));
    }
}
