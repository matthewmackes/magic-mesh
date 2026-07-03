//! MEDIA-17: **casting** — throw the current playback at a mesh node, a DLNA/UPnP
//! renderer, or a Chromecast.
//!
//! Where [`party`](crate::party) keeps several *seats* in sync (the propagation is
//! real — sync-play events over the mesh), casting hands the stream to an *external*
//! renderer. Mirroring the [`ytdlp`](crate::ytdlp) / [`capture`](crate::capture) seams
//! and the `mesh_mount` gating idiom, this module is **discovery + an honest live
//! gate**:
//!
//! * **Discovery is real.** [`mesh_render_targets`] folds the replicated mesh peer
//!   roster into the media-capable nodes; [`parse_ssdp_responses`] projects an SSDP
//!   `M-SEARCH` reply into DLNA/UPnP [`CastTarget`]s; [`parse_chromecast_records`]
//!   projects an mDNS `_googlecast._tcp` listing into Chromecast targets. All three are
//!   pure + fixture-tested, and the live probes ([`MeshRoster`] / [`SsdpProbe`]) return
//!   an **empty** list when nothing answers — never a fabricated renderer.
//! * **The throw is honest-gated.** [`NetworkCaster`] casts to a live **DLNA/UPnP**
//!   renderer for real (a `SetAVTransportURI` + `Play` SOAP push over `std::net`, built
//!   by the pure [`soap_set_av_transport_uri`] / [`http_request`] helpers and parsed by
//!   [`parse_http_status`]). When no renderer is present it is [`CastError::NoRenderers`];
//!   an unreachable target is [`CastError::Unreachable`]; and the protocols whose live
//!   launch is genuinely out of this crate (Chromecast's CASTV2 handshake, the mesh
//!   cast-receiver worker on the target node) return a typed [`CastError::Gated`] that
//!   **names exactly what a live cast needs** — never a faked success (§7), exactly the
//!   `LiveServiceApply`/`IntegrationGated` posture.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use mackes_mesh_types::peers::{default_mesh_home, peers_dir, read_peers, PeerRecord};

/// The default Subsonic/media control port a mesh media node answers on when its
/// descriptor advertises no explicit port (the `music.mesh` Navidrome port).
pub const MESH_MEDIA_DEFAULT_PORT: u16 = 4533;

/// The per-connection timeout for a live cast probe / push — kept short so an absent
/// renderer fails fast to the honest gate rather than hanging the surface.
pub const CAST_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

// ── the discovered target ──────────────────────────────────────────────────────

/// The kind of renderer a [`CastTarget`] is — the three destinations the acceptance
/// names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastKind {
    /// Another mesh node running a media service (from the replicated peer roster).
    MeshNode,
    /// A DLNA/UPnP `MediaRenderer` (discovered over SSDP).
    DlnaUpnp,
    /// A Google Chromecast (discovered over mDNS `_googlecast._tcp`).
    Chromecast,
}

impl CastKind {
    /// A short human label for the surface.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MeshNode => "Mesh node",
            Self::DlnaUpnp => "DLNA / UPnP",
            Self::Chromecast => "Chromecast",
        }
    }
}

/// A discovered renderer the current playback can be thrown to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CastTarget {
    /// Which kind of renderer this is.
    pub kind: CastKind,
    /// A stable id (the peer hostname / the `UPnP` `USN` / the Chromecast id) — the key
    /// the surface selects on.
    pub id: String,
    /// A friendly display name.
    pub name: String,
    /// Where to reach it: a mesh node's `host:port`, a `UPnP` device-description URL, or
    /// a Chromecast `host:port`.
    pub location: String,
}

/// What to cast — the media URL plus the display title + resume position, so a
/// renderer that supports it can pick up where the seat left off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CastRequest {
    /// The direct media URL the renderer will open (an `http(s)` stream a DLNA /
    /// Chromecast device can fetch, or a mesh path a node can resolve).
    pub media_url: String,
    /// The display title, if known.
    pub title: Option<String>,
    /// The position (seconds) to start the cast at.
    pub position_secs: f64,
}

/// A successful cast — the target the stream was thrown to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastOutcome {
    /// The renderer the stream is now playing on.
    pub target: CastTarget,
}

/// A failure from the cast seam.
///
/// Every variant is honest: [`NoRenderers`](Self::NoRenderers) is the "no cast target
/// present" gate (the mirror of `mesh_mount`'s absent-mount state); [`Gated`](Self::Gated)
/// names exactly what a live cast to that renderer would need (never a fake success).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum CastError {
    /// No renderer was discovered — the honest "nothing to cast to" gate.
    #[error("no cast renderer discovered on this network")]
    NoRenderers,
    /// A discovered target could not be reached (offline / firewalled).
    #[error("cast target unreachable: {0}")]
    Unreachable(String),
    /// The renderer answered, but rejected the cast (bad media / unsupported).
    #[error("cast rejected by the renderer: {0}")]
    Rejected(String),
    /// The live launch for this target kind needs infrastructure outside this crate —
    /// named honestly so the surface reports what is missing rather than faking a cast.
    #[error("live cast to {kind} is gated: needs {needs}")]
    Gated {
        /// The renderer kind whose live launch is not wired here.
        kind: &'static str,
        /// Exactly what a live cast to it requires.
        needs: &'static str,
    },
    /// The request itself was unusable (e.g. no media URL to cast).
    #[error("nothing to cast: {0}")]
    Invalid(String),
}

// ── discovery (the real, testable folds) ────────────────────────────────────────

/// The injectable renderer-discovery seam. Production impls probe the network
/// ([`MeshRoster`] / [`SsdpProbe`]); tests inject a canned list, so the surface's cast
/// picker is exercised with no real network.
pub trait RendererDiscovery {
    /// Every renderer this source currently sees (empty when none — the honest gate).
    fn discover(&self) -> Vec<CastTarget>;
}

/// Fold the replicated mesh **peer roster** into the media-capable cast targets.
///
/// Pure (§6 — no re-derivation of discovery): a peer is a mesh render target when it
/// carries a reachable [`overlay_ip`](PeerRecord::overlay_ip) and is media-capable —
/// either the `Lighthouse_Media` [`media`](PeerRecord::media) capability tag, or a
/// [`ServiceDescriptors::media`](mackes_mesh_types::peers::ServiceDescriptors::media)
/// service. The target `location` is the overlay `ip:port` (the first advertised media
/// service's port, else [`MESH_MEDIA_DEFAULT_PORT`]).
#[must_use]
pub fn mesh_render_targets(peers: &[PeerRecord]) -> Vec<CastTarget> {
    let mut out = Vec::new();
    for peer in peers {
        let Some(ip) = peer
            .overlay_ip
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let media_service = peer.descriptors.as_ref().and_then(|d| d.media.first());
        if !peer.media && media_service.is_none() {
            continue; // not a media-capable node
        }
        let port = media_service.map_or(MESH_MEDIA_DEFAULT_PORT, |s| s.port);
        out.push(CastTarget {
            kind: CastKind::MeshNode,
            id: peer.hostname.clone(),
            name: peer.hostname.clone(),
            location: format!("{ip}:{port}"),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Project an SSDP `M-SEARCH` reply (one or more blank-line-separated response blocks)
/// into DLNA/UPnP [`CastTarget`]s.
///
/// Pure + tolerant: it reads each block's `LOCATION` (the device-description URL — the
/// target `location`), `USN` (the stable id), and `SERVER` (a friendly name), keeping
/// only blocks whose `ST`/`NT` names a `MediaRenderer` (the devices that can actually
/// play a URL). Malformed blocks are skipped.
#[must_use]
pub fn parse_ssdp_responses(raw: &str) -> Vec<CastTarget> {
    let mut out = Vec::new();
    for block in raw.split("\r\n\r\n").flat_map(|b| b.split("\n\n")) {
        let mut location = None;
        let mut usn = None;
        let mut server = None;
        let mut is_renderer = false;
        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_uppercase();
            let value = value.trim();
            match key.as_str() {
                "LOCATION" => location = Some(value.to_owned()),
                "USN" => usn = Some(value.to_owned()),
                "SERVER" => server = Some(value.to_owned()),
                "ST" | "NT" => {
                    if value.contains("MediaRenderer") {
                        is_renderer = true;
                    }
                }
                _ => {}
            }
        }
        if !is_renderer {
            continue;
        }
        if let Some(location) = location {
            let id = usn.unwrap_or_else(|| location.clone());
            out.push(CastTarget {
                kind: CastKind::DlnaUpnp,
                id: id.clone(),
                name: server.unwrap_or(id),
                location,
            });
        }
    }
    out
}

/// Project an mDNS `_googlecast._tcp` listing into Chromecast [`CastTarget`]s.
///
/// Pure + tolerant: each record is the service line plus its `TXT`/`A` key=value pairs
/// (`id=`, `fn=` friendly name, and the resolved `host:port`), one record per blank-line
/// group. `avahi-browse -rp` and `dns-sd -L` style output both fold here.
#[must_use]
pub fn parse_chromecast_records(raw: &str) -> Vec<CastTarget> {
    let mut out = Vec::new();
    for block in raw.split("\r\n\r\n").flat_map(|b| b.split("\n\n")) {
        let mut id = None;
        let mut name = None;
        let mut host = None;
        let mut port = None;
        for line in block.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            match key.as_str() {
                "id" => id = Some(value),
                "fn" => name = Some(value),
                "host" | "address" => host = Some(value),
                "port" => port = value.parse::<u16>().ok(),
                _ => {}
            }
        }
        let (Some(id), Some(host)) = (id, host) else {
            continue;
        };
        let port = port.unwrap_or(8009); // the Chromecast control port
        out.push(CastTarget {
            kind: CastKind::Chromecast,
            id: id.clone(),
            name: name.unwrap_or(id),
            location: format!("{host}:{port}"),
        });
    }
    out
}

/// Discover mesh render targets from the replicated peer roster.
///
/// [`mesh_render_targets`] over [`read_peers`]. Always compiles (std +
/// `mackes-mesh-types`); honest — an unmounted / empty roster yields no targets.
#[derive(Debug, Clone, Default)]
pub struct MeshRoster;

impl RendererDiscovery for MeshRoster {
    fn discover(&self) -> Vec<CastTarget> {
        mesh_render_targets(&read_peers(&peers_dir(&default_mesh_home())))
    }
}

/// Discover DLNA/UPnP renderers over a live SSDP `M-SEARCH` (UDP multicast to
/// `239.255.255.250:1900`), parsed by [`parse_ssdp_responses`].
///
/// Real `std::net` discovery — no compile-time dependency, so it is always built
/// (airgap-safe). It is **honest-gated at runtime**: on a network with no renderers (or
/// no multicast egress) it collects nothing and returns an empty list, never a
/// fabricated device.
#[derive(Debug, Clone, Copy)]
pub struct SsdpProbe {
    /// How long to gather replies before giving up.
    pub timeout: Duration,
}

impl Default for SsdpProbe {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(2),
        }
    }
}

/// The SSDP `M-SEARCH` datagram that asks every `MediaRenderer` on the LAN to reply.
const SSDP_MSEARCH: &str = "M-SEARCH * HTTP/1.1\r\n\
Host: 239.255.255.250:1900\r\n\
Man: \"ssdp:discover\"\r\n\
MX: 2\r\n\
ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";

impl RendererDiscovery for SsdpProbe {
    fn discover(&self) -> Vec<CastTarget> {
        self.probe().unwrap_or_default()
    }
}

impl SsdpProbe {
    /// Send the `M-SEARCH` and gather replies into a single buffer that
    /// [`parse_ssdp_responses`] folds. Any socket error is the honest empty gate (the
    /// caller sees no renderers), never a panic.
    fn probe(&self) -> std::io::Result<Vec<CastTarget>> {
        use std::net::UdpSocket;
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_read_timeout(Some(self.timeout))?;
        socket.send_to(SSDP_MSEARCH.as_bytes(), "239.255.255.250:1900")?;
        let mut gathered = String::new();
        let mut buf = [0_u8; 2048];
        // Read replies until the socket read times out (the honest end of discovery).
        while let Ok((n, _addr)) = socket.recv_from(&mut buf) {
            gathered.push_str(&String::from_utf8_lossy(&buf[..n]));
            gathered.push_str("\r\n\r\n");
        }
        Ok(parse_ssdp_responses(&gathered))
    }
}

/// Fold several discovery sources into one de-duplicated target list (by `id`).
#[must_use]
pub fn discover_all(sources: &[&dyn RendererDiscovery]) -> Vec<CastTarget> {
    let mut out: Vec<CastTarget> = Vec::new();
    for source in sources {
        for target in source.discover() {
            if !out.iter().any(|t| t.id == target.id) {
                out.push(target);
            }
        }
    }
    out
}

// ── the cast seam + the live gate ───────────────────────────────────────────────

/// The injectable cast seam. [`NetworkCaster`] is the real impl; tests inject a fake
/// so the surface's cast flow is exercised with no real renderer.
pub trait Caster {
    /// Throw `req` at `target`.
    ///
    /// # Errors
    /// [`CastError`] — [`NoRenderers`](CastError::NoRenderers) is never returned here
    /// (that is the *discovery* gate); a live throw returns
    /// [`Unreachable`](CastError::Unreachable) / [`Rejected`](CastError::Rejected) /
    /// [`Gated`](CastError::Gated) / [`Invalid`](CastError::Invalid).
    fn cast(&self, target: &CastTarget, req: &CastRequest) -> Result<CastOutcome, CastError>;
}

/// The real caster over `std::net`.
///
/// * **DLNA/UPnP** is a genuine live cast: fetch the device description at the target
///   `location`, resolve the `AVTransport` control URL ([`dlna_control_url`]), and POST
///   the [`soap_set_av_transport_uri`] + [`soap_play`] actions. Unreachable → the honest
///   [`CastError::Unreachable`]; a non-2xx reply → [`CastError::Rejected`].
/// * **Chromecast** and **mesh-node** live launches need infrastructure outside this
///   crate, so they return a typed [`CastError::Gated`] naming exactly what is required
///   — never a fake success.
#[derive(Debug, Clone, Copy)]
pub struct NetworkCaster {
    /// The per-connection timeout for the live push.
    pub timeout: Duration,
}

impl Default for NetworkCaster {
    fn default() -> Self {
        Self {
            timeout: CAST_CONNECT_TIMEOUT,
        }
    }
}

impl Caster for NetworkCaster {
    fn cast(&self, target: &CastTarget, req: &CastRequest) -> Result<CastOutcome, CastError> {
        if req.media_url.trim().is_empty() {
            return Err(CastError::Invalid("no media is loaded to cast".to_owned()));
        }
        match target.kind {
            CastKind::DlnaUpnp => self.cast_dlna(target, req),
            CastKind::Chromecast => Err(CastError::Gated {
                kind: "a Chromecast",
                needs: "the CASTV2 launch handshake (protobuf over TLS :8009 to the default media receiver)",
            }),
            CastKind::MeshNode => Err(CastError::Gated {
                kind: "a mesh node",
                needs: "the mesh cast-receiver worker on the target node (a mackesd action that opens the URL in its player)",
            }),
        }
    }
}

impl NetworkCaster {
    /// The real DLNA/UPnP live cast: resolve the `AVTransport` control URL from the
    /// device description, then push `SetAVTransportURI` + `Play`.
    fn cast_dlna(&self, target: &CastTarget, req: &CastRequest) -> Result<CastOutcome, CastError> {
        let device = parse_endpoint(&target.location).ok_or_else(|| {
            CastError::Invalid(format!("bad target location: {}", target.location))
        })?;
        let description = self
            .http_roundtrip(&device, &http_request("GET", &device, &[], ""))
            .map_err(|e| CastError::Unreachable(e.to_string()))?;
        let body = http_body(&description);
        let control = dlna_control_url(body, &device).ok_or(CastError::Gated {
            kind: "a DLNA/UPnP renderer",
            needs: "an AVTransport service with a controlURL in the device description",
        })?;
        let control_ep = parse_endpoint(&control)
            .ok_or_else(|| CastError::Invalid(format!("bad control URL: {control}")))?;

        // SetAVTransportURI, then Play — the two SOAP actions that start a DLNA cast.
        let set = http_request(
            "POST",
            &control_ep,
            &soap_headers("SetAVTransportURI"),
            &soap_set_av_transport_uri(&req.media_url, req.title.as_deref().unwrap_or("")),
        );
        let set_reply = self
            .http_roundtrip(&control_ep, &set)
            .map_err(|e| CastError::Unreachable(e.to_string()))?;
        expect_2xx(&set_reply, "SetAVTransportURI")?;

        let play = http_request("POST", &control_ep, &soap_headers("Play"), &soap_play());
        let play_reply = self
            .http_roundtrip(&control_ep, &play)
            .map_err(|e| CastError::Unreachable(e.to_string()))?;
        expect_2xx(&play_reply, "Play")?;

        Ok(CastOutcome {
            target: target.clone(),
        })
    }

    /// Open a TCP connection to `endpoint`, write `request`, and read the whole reply.
    fn http_roundtrip(&self, endpoint: &Endpoint, request: &str) -> std::io::Result<String> {
        let addr = (endpoint.host.as_str(), endpoint.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no address for host")
            })?;
        let mut stream = TcpStream::connect_timeout(&addr, self.timeout)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        stream.write_all(request.as_bytes())?;
        stream.flush()?;
        let mut reply = String::new();
        // A short read to the timeout is enough to capture the status line + headers.
        let mut buf = [0_u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(n) if n > 0 => reply.push_str(&String::from_utf8_lossy(&buf[..n])),
                // EOF (`Ok(0)`) or a read timeout — we have the status line by now.
                _ => break,
            }
            if reply.len() > 64 * 1024 {
                break; // a device description can be large; cap it
            }
        }
        Ok(reply)
    }
}

/// A non-2xx SOAP reply is an honest [`CastError::Rejected`] naming the action.
fn expect_2xx(reply: &str, action: &str) -> Result<(), CastError> {
    match parse_http_status(reply) {
        Some(code) if (200..300).contains(&code) => Ok(()),
        Some(code) => Err(CastError::Rejected(format!("{action} → HTTP {code}"))),
        None => Err(CastError::Rejected(format!("{action} → no HTTP status"))),
    }
}

// ── the pure HTTP / SOAP builders + parsers (fixture-tested) ─────────────────────

/// A parsed `host` / `port` / `path` endpoint from an `http://` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The host (no scheme, no port).
    pub host: String,
    /// The TCP port (defaulting to 80 when the URL omits it).
    pub port: u16,
    /// The request path (always starts with `/`).
    pub path: String,
}

/// Parse `http://host[:port][/path]` (or a bare `host:port`) into an [`Endpoint`].
///
/// Returns [`None`] for an `https`/unsupported scheme or an unparseable authority (the
/// raw-TCP path speaks plain HTTP, the DLNA control-channel norm).
#[must_use]
pub fn parse_endpoint(url: &str) -> Option<Endpoint> {
    let rest = url.trim();
    let rest = if let Some(r) = rest.strip_prefix("http://") {
        r
    } else if rest.starts_with("https://") {
        return None;
    } else {
        rest
    };
    let (authority, path) = rest
        .find('/')
        .map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    if authority.is_empty() {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (authority, 80),
    };
    if host.is_empty() {
        return None;
    }
    Some(Endpoint {
        host: host.to_owned(),
        port,
        path: if path.is_empty() {
            "/".to_owned()
        } else {
            path.to_owned()
        },
    })
}

/// Resolve the `AVTransport` `controlURL` from a device-description XML, made absolute
/// against `base`.
///
/// Pure + tolerant (§6 — no XML dependency): it finds the `AVTransport` service block
/// and returns its `<controlURL>` — absolute if the XML gives an absolute URL, else
/// `http://<base.host:port><controlURL>`. [`None`] when the description has no
/// `AVTransport` control URL.
#[must_use]
pub fn dlna_control_url(xml: &str, base: &Endpoint) -> Option<String> {
    let anchor = xml.find("AVTransport")?;
    // The <controlURL> for AVTransport is the first one at or after the service anchor.
    let tail = &xml[anchor..];
    let raw = between(tail, "<controlURL>", "</controlURL>")?;
    let control = raw.trim();
    if control.is_empty() {
        return None;
    }
    if control.starts_with("http://") || control.starts_with("https://") {
        return Some(control.to_owned());
    }
    let path = if control.starts_with('/') {
        control.to_owned()
    } else {
        format!("/{control}")
    };
    Some(format!("http://{}:{}{path}", base.host, base.port))
}

/// The substring between `open` and the next `close`, if both are present.
fn between<'a>(haystack: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = haystack.find(open)? + open.len();
    let end = haystack[start..].find(close)? + start;
    Some(&haystack[start..end])
}

/// Build a raw HTTP/1.1 request (status line + headers + body) for `endpoint`.
///
/// Pure — the caller writes the returned bytes to a socket. `Host`, `Content-Length`,
/// and `Connection: close` are always set; `extra` carries the SOAP `Content-Type` +
/// `SOAPACTION` for a POST.
#[must_use]
pub fn http_request(
    method: &str,
    endpoint: &Endpoint,
    extra: &[(String, String)],
    body: &str,
) -> String {
    let mut req = String::new();
    req.push_str(method);
    req.push(' ');
    req.push_str(&endpoint.path);
    req.push_str(" HTTP/1.1\r\nHost: ");
    req.push_str(&endpoint.host);
    req.push(':');
    req.push_str(&endpoint.port.to_string());
    req.push_str("\r\n");
    for (key, value) in extra {
        req.push_str(key);
        req.push_str(": ");
        req.push_str(value);
        req.push_str("\r\n");
    }
    req.push_str("Content-Length: ");
    req.push_str(&body.len().to_string());
    req.push_str("\r\nConnection: close\r\n\r\n");
    req.push_str(body);
    req
}

/// The DLNA SOAP headers for an `AVTransport` `action` (`Content-Type` + `SOAPACTION`).
#[must_use]
pub fn soap_headers(action: &str) -> Vec<(String, String)> {
    vec![
        (
            "Content-Type".to_owned(),
            "text/xml; charset=\"utf-8\"".to_owned(),
        ),
        (
            "SOAPACTION".to_owned(),
            format!("\"urn:schemas-upnp-org:service:AVTransport:1#{action}\""),
        ),
    ]
}

/// The `SetAVTransportURI` SOAP envelope that points the renderer at `media_url`.
#[must_use]
pub fn soap_set_av_transport_uri(media_url: &str, title: &str) -> String {
    let media = xml_escape(media_url);
    let title = xml_escape(title);
    format!(
        "<?xml version=\"1.0\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body>\
<u:SetAVTransportURI xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">\
<InstanceID>0</InstanceID>\
<CurrentURI>{media}</CurrentURI>\
<CurrentURIMetaData>{title}</CurrentURIMetaData>\
</u:SetAVTransportURI>\
</s:Body></s:Envelope>"
    )
}

/// The `Play` SOAP envelope that starts the renderer at normal speed.
#[must_use]
pub fn soap_play() -> String {
    "<?xml version=\"1.0\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body>\
<u:Play xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">\
<InstanceID>0</InstanceID>\
<Speed>1</Speed>\
</u:Play>\
</s:Body></s:Envelope>"
        .to_owned()
}

/// Minimal XML-attribute/text escaping for the SOAP payload.
fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The HTTP status code from a raw response's status line (`HTTP/1.1 200 OK` → `200`).
#[must_use]
pub fn parse_http_status(response: &str) -> Option<u16> {
    let first = response.lines().next()?;
    let mut parts = first.split_whitespace();
    let _version = parts.next()?;
    parts.next()?.parse::<u16>().ok()
}

/// The body of a raw HTTP response (everything after the first blank line).
fn http_body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or_else(
        || response.split_once("\n\n").map_or("", |(_, b)| b),
        |(_, b)| b,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::peers::{MediaService, ServiceDescriptors};

    fn peer(hostname: &str, overlay: Option<&str>, media: bool) -> PeerRecord {
        PeerRecord {
            hostname: hostname.to_owned(),
            mde_version: None,
            last_seen_ms: 0,
            health: "healthy".to_owned(),
            descriptors: None,
            overlay_ip: overlay.map(ToOwned::to_owned),
            role: None,
            external_addr: None,
            media,
        }
    }

    // ── mesh roster fold ────────────────────────────────────────────────────────

    #[test]
    fn mesh_targets_are_media_capable_peers_with_an_overlay_ip() {
        let mut with_service = peer("nas", Some("10.42.0.7"), false);
        with_service.descriptors = Some(ServiceDescriptors {
            media: vec![MediaService {
                name: "navidrome".to_owned(),
                port: 4533,
            }],
            ..ServiceDescriptors::default()
        });
        let peers = vec![
            peer("lh-media", Some("10.42.0.1"), true), // media tag → target :4533
            peer("plain", Some("10.42.0.2"), false),   // not media-capable → skipped
            peer("no-ip", None, true),                 // no overlay ip → skipped
            with_service,                              // media descriptor → target
        ];
        let targets = mesh_render_targets(&peers);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].id, "lh-media");
        assert_eq!(targets[0].location, "10.42.0.1:4533");
        assert_eq!(targets[0].kind, CastKind::MeshNode);
        assert_eq!(targets[1].id, "nas");
        assert_eq!(targets[1].location, "10.42.0.7:4533");
    }

    #[test]
    fn mesh_targets_empty_when_nothing_media_capable() {
        assert!(mesh_render_targets(&[peer("a", Some("10.0.0.1"), false)]).is_empty());
        assert!(mesh_render_targets(&[]).is_empty());
    }

    // ── SSDP + Chromecast parsers ───────────────────────────────────────────────

    #[test]
    fn parses_ssdp_media_renderer_replies_and_skips_non_renderers() {
        let raw = "HTTP/1.1 200 OK\r\n\
LOCATION: http://192.168.1.50:8200/desc.xml\r\n\
ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\
USN: uuid:tv-1::MediaRenderer\r\n\
SERVER: Linux/3.14 UPnP/1.0 GUPnP/1.0\r\n\r\n\
HTTP/1.1 200 OK\r\n\
LOCATION: http://192.168.1.60:1900/root.xml\r\n\
ST: urn:schemas-upnp-org:device:MediaServer:1\r\n\
USN: uuid:nas-1::MediaServer\r\n\r\n";
        let targets = parse_ssdp_responses(raw);
        assert_eq!(
            targets.len(),
            1,
            "only the MediaRenderer, not the MediaServer"
        );
        assert_eq!(targets[0].kind, CastKind::DlnaUpnp);
        assert_eq!(targets[0].location, "http://192.168.1.50:8200/desc.xml");
        assert_eq!(targets[0].id, "uuid:tv-1::MediaRenderer");
        assert!(targets[0].name.contains("GUPnP"));
    }

    #[test]
    fn parses_chromecast_mdns_records() {
        let raw = "id=abc123\nfn=Living Room TV\nhost=192.168.1.70\nport=8009\n\n\
id=def456\nfn=Kitchen display\nhost=192.168.1.71\n";
        let targets = parse_chromecast_records(raw);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].kind, CastKind::Chromecast);
        assert_eq!(targets[0].name, "Living Room TV");
        assert_eq!(targets[0].location, "192.168.1.70:8009");
        // The second omits an explicit port → the default control port.
        assert_eq!(targets[1].location, "192.168.1.71:8009");
    }

    #[test]
    fn discover_all_dedupes_by_id() {
        struct Canned(Vec<CastTarget>);
        impl RendererDiscovery for Canned {
            fn discover(&self) -> Vec<CastTarget> {
                self.0.clone()
            }
        }
        let a = Canned(vec![CastTarget {
            kind: CastKind::MeshNode,
            id: "dup".to_owned(),
            name: "A".to_owned(),
            location: "x".to_owned(),
        }]);
        let b = Canned(vec![
            CastTarget {
                kind: CastKind::MeshNode,
                id: "dup".to_owned(),
                name: "B".to_owned(),
                location: "y".to_owned(),
            },
            CastTarget {
                kind: CastKind::DlnaUpnp,
                id: "other".to_owned(),
                name: "C".to_owned(),
                location: "z".to_owned(),
            },
        ]);
        let all = discover_all(&[&a, &b]);
        assert_eq!(all.len(), 2, "the duplicate id is folded once");
        assert_eq!(all[0].name, "A", "first source wins the id");
    }

    // ── endpoint + control-url parsing ──────────────────────────────────────────

    #[test]
    fn parses_endpoints_and_rejects_https() {
        assert_eq!(
            parse_endpoint("http://192.168.1.50:8200/desc.xml"),
            Some(Endpoint {
                host: "192.168.1.50".to_owned(),
                port: 8200,
                path: "/desc.xml".to_owned(),
            })
        );
        // No port defaults to 80; no path defaults to "/".
        assert_eq!(
            parse_endpoint("host.local"),
            Some(Endpoint {
                host: "host.local".to_owned(),
                port: 80,
                path: "/".to_owned(),
            })
        );
        assert_eq!(parse_endpoint("https://secure/desc"), None);
        assert_eq!(parse_endpoint(""), None);
    }

    #[test]
    fn resolves_relative_and_absolute_avtransport_control_urls() {
        let base = parse_endpoint("http://192.168.1.50:8200/desc.xml").expect("endpoint");
        let xml = "<serviceList>\
<service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType>\
<controlURL>/RenderingControl/ctrl</controlURL></service>\
<service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType>\
<controlURL>/AVTransport/ctrl</controlURL></service>\
</serviceList>";
        assert_eq!(
            dlna_control_url(xml, &base).as_deref(),
            Some("http://192.168.1.50:8200/AVTransport/ctrl")
        );
        // An absolute controlURL is kept verbatim.
        let abs = "AVTransport <controlURL>http://192.168.1.50:8200/x</controlURL>";
        assert_eq!(
            dlna_control_url(abs, &base).as_deref(),
            Some("http://192.168.1.50:8200/x")
        );
        // No AVTransport service → no control URL.
        assert_eq!(dlna_control_url("<root/>", &base), None);
    }

    // ── SOAP + HTTP builders ────────────────────────────────────────────────────

    #[test]
    fn set_av_transport_uri_escapes_and_carries_the_media_url() {
        let body = soap_set_av_transport_uri("http://mesh/a&b.mkv", "A & B");
        assert!(body.contains("<CurrentURI>http://mesh/a&amp;b.mkv</CurrentURI>"));
        assert!(body.contains("<CurrentURIMetaData>A &amp; B</CurrentURIMetaData>"));
        assert!(body.contains("SetAVTransportURI"));
    }

    #[test]
    fn http_request_sets_host_content_length_and_soap_action() {
        let ep = parse_endpoint("http://tv:8200/AVTransport/ctrl").expect("endpoint");
        let body = soap_play();
        let req = http_request("POST", &ep, &soap_headers("Play"), &body);
        assert!(req.starts_with("POST /AVTransport/ctrl HTTP/1.1\r\n"));
        assert!(req.contains("Host: tv:8200\r\n"));
        assert!(req.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert!(req.contains("SOAPACTION: \"urn:schemas-upnp-org:service:AVTransport:1#Play\""));
        assert!(req.ends_with(&body));
    }

    #[test]
    fn parses_http_status_and_body() {
        assert_eq!(parse_http_status("HTTP/1.1 200 OK\r\n\r\n"), Some(200));
        assert_eq!(parse_http_status("HTTP/1.1 500 Internal\r\n"), Some(500));
        assert_eq!(parse_http_status("garbage"), None);
        assert_eq!(
            http_body("HTTP/1.1 200 OK\r\nX: y\r\n\r\n<body/>"),
            "<body/>"
        );
    }

    #[test]
    fn expect_2xx_gates_on_the_status() {
        assert!(expect_2xx("HTTP/1.1 200 OK\r\n\r\n", "Play").is_ok());
        assert!(matches!(
            expect_2xx("HTTP/1.1 500 err\r\n\r\n", "Play"),
            Err(CastError::Rejected(_))
        ));
    }

    // ── the honest cast gate (never a fake success) ─────────────────────────────

    #[test]
    fn chromecast_and_mesh_casts_are_typed_gates_naming_what_they_need() {
        let caster = NetworkCaster::default();
        let req = CastRequest {
            media_url: "http://mesh/clip.mkv".to_owned(),
            title: None,
            position_secs: 0.0,
        };
        let cc = CastTarget {
            kind: CastKind::Chromecast,
            id: "cc".to_owned(),
            name: "TV".to_owned(),
            location: "192.168.1.70:8009".to_owned(),
        };
        assert!(matches!(
            caster.cast(&cc, &req),
            Err(CastError::Gated {
                kind: "a Chromecast",
                ..
            })
        ));
        let mesh = CastTarget {
            kind: CastKind::MeshNode,
            id: "node".to_owned(),
            name: "node".to_owned(),
            location: "10.42.0.1:4533".to_owned(),
        };
        assert!(matches!(
            caster.cast(&mesh, &req),
            Err(CastError::Gated {
                kind: "a mesh node",
                ..
            })
        ));
    }

    #[test]
    fn casting_nothing_is_an_honest_invalid_not_a_fake_success() {
        let caster = NetworkCaster::default();
        let target = CastTarget {
            kind: CastKind::DlnaUpnp,
            id: "tv".to_owned(),
            name: "TV".to_owned(),
            location: "http://192.168.1.50:8200/desc.xml".to_owned(),
        };
        let empty = CastRequest::default();
        assert!(matches!(
            caster.cast(&target, &empty),
            Err(CastError::Invalid(_))
        ));
    }

    #[test]
    fn dlna_cast_to_an_absent_renderer_is_unreachable_never_faked() {
        // 203.0.113.0/24 is TEST-NET-3 (RFC 5737) — guaranteed no renderer answers, so
        // the live leg fails to the honest Unreachable gate, never a fabricated success.
        let caster = NetworkCaster {
            timeout: Duration::from_millis(150),
        };
        let target = CastTarget {
            kind: CastKind::DlnaUpnp,
            id: "tv".to_owned(),
            name: "TV".to_owned(),
            location: "http://203.0.113.1:8200/desc.xml".to_owned(),
        };
        let req = CastRequest {
            media_url: "http://mesh/clip.mkv".to_owned(),
            title: Some("Clip".to_owned()),
            position_secs: 0.0,
        };
        assert!(matches!(
            caster.cast(&target, &req),
            Err(CastError::Unreachable(_))
        ));
    }
}
