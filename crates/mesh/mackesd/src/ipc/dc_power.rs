//! DATACENTER-16 (action layer) — `action/dc/wol` → Wake-on-LAN, the
//! power-orchestration primitive that turns a sleeping/powered-off machine
//! back on.
//!
//! Companion to the host power responder ([`crate::ipc::host_ops`]): where that
//! enters/leaves maintenance and reboots an already-running dom0 over SSH, this
//! brings a machine up from cold by broadcasting the standard Wake-on-LAN
//! "magic packet" on the LAN. Same dedicated-OS-thread, `action/dc/<verb>`
//! Bus-RPC shape.
//!
//! Request body `{ "mac": "aa:bb:cc:dd:ee:ff" }`:
//!   * `mac` MUST be six hex octets separated by `:` or `-`
//!     ([`build_magic_packet`]); anything else is rejected without a send.
//! The 102-byte magic packet (6×`0xFF` then the 6-byte MAC repeated 16×) is
//! sent as a UDP broadcast to `255.255.255.255:9` (the classic discard-port WoL
//! target).
//! Reply `{"ok":true}` once the packet is sent, `{"error":"<message>"}` otherwise.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The Wake-on-LAN responder — rooted at the shared workgroup root (carried for
/// parity with the other action services; no per-host config is read here, the
/// packet is built purely from the request's MAC).
#[derive(Debug, Clone)]
pub struct DcPowerService {
    // Carried for parity with the other action services and the
    // `new(workgroup_root)` spawn contract; WoL needs no rooted state, so this
    // isn't read here.
    #[allow(dead_code)]
    workgroup_root: PathBuf,
}

impl DcPowerService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 1] = ["wol"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Parse a 6-octet MAC and build the 102-byte Wake-on-LAN magic packet. PURE.
///
/// The MAC must be six hexadecimal octets separated by `:` or `-`
/// (e.g. `aa:bb:cc:dd:ee:ff` or `AA-BB-CC-DD-EE-FF`); any other shape — wrong
/// octet count, a non-hex digit, an octet that isn't exactly two hex chars, or
/// mixed/empty separators — is rejected.
///
/// The returned packet is the standard WoL frame: six `0xFF` sync bytes
/// followed by the 6-byte target MAC repeated 16 times (`6 + 6*16 = 102`).
///
/// # Errors
/// Returns `Err` with a human-readable message for any MAC that isn't exactly
/// six valid hex octets.
pub fn build_magic_packet(mac: &str) -> Result<Vec<u8>, String> {
    // Accept ':' or '-' as the separator, but not a mix (split on both then
    // require the separator count to be consistent by re-checking each piece).
    let parts: Vec<&str> = if mac.contains(':') && !mac.contains('-') {
        mac.split(':').collect()
    } else if mac.contains('-') && !mac.contains(':') {
        mac.split('-').collect()
    } else {
        return Err(format!("invalid mac: {mac}"));
    };

    if parts.len() != 6 {
        return Err(format!(
            "invalid mac: expected 6 octets, got {}",
            parts.len()
        ));
    }

    let mut octets = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        if part.len() != 2 || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("invalid mac octet: {part}"));
        }
        octets[i] =
            u8::from_str_radix(part, 16).map_err(|e| format!("invalid mac octet {part}: {e}"))?;
    }

    // 6 sync bytes of 0xFF, then the MAC repeated 16 times = 102 bytes.
    let mut packet = Vec::with_capacity(102);
    packet.extend_from_slice(&[0xFF; 6]);
    for _ in 0..16 {
        packet.extend_from_slice(&octets);
    }
    Ok(packet)
}

/// Send a Wake-on-LAN magic packet as a UDP broadcast to `255.255.255.255:9`.
///
/// # Errors
/// Returns `Err` if the socket can't be bound, broadcast can't be enabled, or
/// the datagram can't be sent.
fn send_magic_packet(packet: &[u8]) -> Result<(), String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("bind failed: {e}"))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("set_broadcast failed: {e}"))?;
    socket
        .send_to(packet, "255.255.255.255:9")
        .map_err(|e| format!("send failed: {e}"))?;
    Ok(())
}

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(_svc: &DcPowerService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    if verb != "wol" {
        return err("unknown dc verb".into());
    }
    let Some(body) = req_body else {
        return err("wol: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("wol: bad json: {e}")),
    };
    let mac = req
        .get("mac")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let packet = match build_magic_packet(mac) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    match send_magic_packet(&packet) {
        Ok(()) => json!({ "ok": true }).to_string(),
        Err(e) => err(e),
    }
}

/// Run the dc-power Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DcPowerService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &DcPowerService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc-power responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "dc-power responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("wol"), "action/dc/wol");
        assert!(ACTION_VERBS.contains(&"wol"));
    }

    #[test]
    fn magic_packet_is_102_bytes() {
        let p = build_magic_packet("aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(p.len(), 102);
    }

    #[test]
    fn magic_packet_first_six_are_ff() {
        let p = build_magic_packet("aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(&p[0..6], &[0xFF; 6]);
    }

    #[test]
    fn magic_packet_carries_the_mac_sixteen_times() {
        let mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let p = build_magic_packet("aa:bb:cc:dd:ee:ff").unwrap();
        // Bytes 6..12 are the first MAC copy.
        assert_eq!(&p[6..12], &mac);
        // All 16 repetitions match.
        for i in 0..16 {
            let start = 6 + i * 6;
            assert_eq!(&p[start..start + 6], &mac, "repetition {i} mismatch");
        }
    }

    #[test]
    fn dash_separator_is_accepted() {
        let p = build_magic_packet("AA-BB-CC-DD-EE-FF").unwrap();
        assert_eq!(p.len(), 102);
        assert_eq!(&p[6..12], &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    }

    #[test]
    fn uppercase_and_lowercase_hex_both_parse() {
        let lower = build_magic_packet("01:23:45:67:89:ab").unwrap();
        let upper = build_magic_packet("01:23:45:67:89:AB").unwrap();
        assert_eq!(lower, upper);
        assert_eq!(&lower[6..12], &[0x01, 0x23, 0x45, 0x67, 0x89, 0xab]);
    }

    #[test]
    fn too_few_octets_rejected() {
        assert!(build_magic_packet("aa:bb:cc:dd:ee").is_err());
    }

    #[test]
    fn too_many_octets_rejected() {
        assert!(build_magic_packet("aa:bb:cc:dd:ee:ff:00").is_err());
    }

    #[test]
    fn non_hex_digit_rejected() {
        assert!(build_magic_packet("aa:bb:cc:dd:ee:zz").is_err());
        assert!(build_magic_packet("gg:bb:cc:dd:ee:ff").is_err());
    }

    #[test]
    fn wrong_octet_width_rejected() {
        // A single-digit octet is not a valid two-char hex octet.
        assert!(build_magic_packet("a:bb:cc:dd:ee:ff").is_err());
        // A three-digit octet is rejected too.
        assert!(build_magic_packet("aaa:bb:cc:dd:ee:ff").is_err());
    }

    #[test]
    fn missing_or_mixed_separator_rejected() {
        assert!(build_magic_packet("aabbccddeeff").is_err());
        assert!(build_magic_packet("aa:bb-cc:dd:ee:ff").is_err());
        assert!(build_magic_packet("").is_err());
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = DcPowerService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "wol", None).contains("missing request body"));
    }

    #[test]
    fn bad_json_and_bad_mac_error() {
        let s = DcPowerService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "wol", Some("not json")).contains("bad json"));
        let body = json!({ "mac": "nope" }).to_string();
        let r = build_reply(&s, "wol", Some(&body));
        assert!(r.contains("error"), "{r}");
        assert!(r.contains("invalid mac"), "{r}");
    }
}
