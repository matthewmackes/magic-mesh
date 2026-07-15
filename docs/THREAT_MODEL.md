# MCNF Threat Model — the mesh web browser, phone remote-input, and WebAuthn/passkeys

Scope: the sandboxed Servo browser that MCNF ships inside the desktop shell
(BOOKMARKS-5..9; design: `docs/design/mesh-bookmarks.md`). This document states
the attack surface, the confinement layers, and the **accepted residual risks**
of running a real interactive web engine on a node that also holds mesh
identity, keys and workgroup data.

It is a living document: it is the security contract the browser is packaged
against (`crates/mesh/mackesd/Cargo.toml` `generate-rpm` block + the confined
SELinux domain in `packaging/selinux/mde-web-preview.te`). Change the sandbox or
the packaging and you update this file.

This file has grown to cover MCNF's other privileged, security-relevant
`mackesd` worker surfaces alongside the browser — each below as its own
numbered section using the same shape (trust boundary → attack surface →
mitigations → accepted residual risks → out of scope): **§6, phone-to-desktop
remote-input injection** (KDC-MESH-6: `workers/seat_remote_input.rs` +
`install-helpers/seat-remote-input.py`), **§7, WebAuthn / passkey
ceremonies** (BROWSER-DD-6: `workers/browser_passkeys.rs`), **§8, the
CEF/Chromium engine's own privacy hardening** (BROWSER-DD-1:
`crates/desktop/mde-web-cef`), **§9, the CEF DevTools remote-debugging port**
(security-4), and **§10, the CEF/Chromium OS confinement** (security-1). §1-5
describe the confinement `sandbox.rs` gives the **Servo** engine specifically;
**§10** documents the EQUIVALENT OS confinement now applied to the **CEF**
engine — the shared `mde-web-sandbox` crate (factored from Servo's `sandbox.rs`),
installed on the CEF renderer before `cef_initialize`, wrapping the whole
multi-process Chromium tree. Each section is the security contract for that
worker; change its trust model and update its section.

---

## 1. What ships, and where the trust boundary is

| Component | Tier | Trust |
|-----------|------|-------|
| `mde-web-preview` (bin) | desktop-shell helper, **out-of-process** | **UNTRUSTED** — runs attacker-influenced web content (JS, layout, media). Confined (§3.1). |
| `mde-web-cef` (launcher) + `mde-web-cef-renderer` (bridge) (bins) | desktop-shell helper, **out-of-process**, **multi-process** (Chromium/CEF forks its own renderer/GPU/utility subprocesses) | **UNTRUSTED** — runs attacker-influenced web content. Confined by the SAME OS-sandbox class as `mde-web-preview` (§10), which wraps the entire subprocess tree. Chromium's OWN internal sandbox stays off (`--no-sandbox`) — see §10 for why, and why the OS sandbox is the honest confinement. |
| `mde-web-preview-client` (lib) | desktop-shell, in the shell process | Trusted. Spawns + drives the helper over a per-session socket; maps the shm frame **read-only**. |
| `mde-adblock` (lib) | services | Trusted, pure. Judges each subresource request (block-before-fetch) + builds the cosmetic stylesheet. |
| `browser_policy` worker (mackesd) | mesh service | Trusted. Fleet-wide governance: refuses to spawn on a disallowed role, forces the ad-blocker, enforces the URL allowlist. |

**The trust boundary is the process boundary.** The web engine is a separate
OS process with a distinct, confined identity. The shell never runs web content
in-process; it only receives a **read-only** shared-memory frame and a typed,
length-prefixed event stream over a per-session Unix socket. A helper compromise
must cross the OS sandbox *and* the IPC seam to reach anything of the operator's.

```
        ┌── shell process (trusted) ─────────────┐        ┌── mde-web-preview (UNTRUSTED, confined) ──┐
        │ mde-shell-egui                          │  unix  │ Servo engine  (JS on, layout, net, TLS)   │
        │  └ mde-web-preview-client               │◀──────▶│  + OS sandbox (userns/seccomp/caps/nnp/    │
        │      · spawns + drives the helper       │ socket │     cgroups/RO-rootfs, NO home/keys/data)  │
        │      · maps the shm frame READ-ONLY     │  + shm │  one process per tab                       │
        │      · mde-adblock: block-before-fetch  │ (RO)   │                                            │
        └─────────────────────────────────────────┘        └────────────────────────────────────────────┘
                    (holds mesh identity, keys, data)                    (throwaway, no identity)
```

---

## 2. Attack surface

1. **Web content → the engine.** Malicious JS / HTML / CSS / media / fonts / a
   compromised TLS peer drives the largest surface: the JS engine (SpiderMonkey
   JIT is on), layout, image/font decoders, the network stack. Assume a
   determined page can achieve arbitrary code execution *inside the helper
   process*. Everything below is designed for "when", not "if".
2. **The engine → the host.** A compromised helper trying to reach `$HOME`,
   `~/.ssh`, `/etc/nebula`, `/etc/mackesd`, `/var/lib/mackesd`, the
   mesh-storage tree, other processes, or to persist.
3. **The IPC seam.** The helper → shell socket + the shm frame. A malicious
   helper feeding oversized/lying frame headers or malformed events.
4. **The ad-filter path.** Parsing untrusted upstream filter-list text; a
   crafted list causing pathological matching.
5. **Supply chain.** The Servo crate tree + its C/C++ deps pulled at build time;
   the pinned Servo version (see the CHANGELOG).

---

## 3. Confinement — the layers

The primary confinement is an **in-process OS sandbox** the helper installs on
itself before it touches the network, plus **process-per-tab** blast-radius
isolation. SELinux is a second, orthogonal MAC layer.

### 3.1 OS sandbox (always on — `sandbox.rs`)
- **User namespace** — the tab process runs as an unprivileged uid mapped inside
  a fresh userns; it holds no real host privilege.
- **seccomp-bpf allowlist** — a syscall allowlist (Firecracker's pure-Rust
  assembler, no libseccomp), installed after setup; syscalls off the list are
  denied.
- **Capability bounding set fully dropped** — bounding + ambient + inheritable
  cleared, so the tab process holds no capability.
- **`no_new_privs`** — set, so no exec can regain privilege.
- **Read-only, minimal `pivot_root` rootfs + private tmpfs** — **NO `$HOME`, NO
  mesh keys, NO nebula certs, NO workgroup data** is visible in the mount
  namespace. (Q39/Q40.)
- **cgroup v2 memory + CPU caps per tab** (+ a layout-thread cap) — a runaway or
  hostile tab is killed with an honest "used too much", not an OOM of the node.
  (Q67.)
- **GPU** — offscreen render via EGL/GBM on the DRI render node only; no seat,
  no input devices, no framebuffer.

### 3.2 SELinux confined domain (`packaging/selinux/mde-web-preview.te`)
A confined **enforcing** domain `mde_web_preview_t` (exec type
`mde_web_preview_exec_t`): spawning `/usr/bin/mde-web-preview` auto-transitions
the child into it. It is least-privilege — GPU render node, client networking,
its own tmpfs/shm, read-only program+CA+fonts — and **everything else (the
operator's home, `~/.ssh`, `/etc/nebula`, `/etc/mackesd`, `/var/lib/*`,
mesh-storage) is denied by the SELinux default-deny**. That omission *is* the
confinement; there are no blanket `unconfined_*` grants — it is a real confined
domain, not a permissive stub.

> **Platform note.** The 2026-06-20 disabled-SELinux fleet standard is
> superseded for Quasar-cloud nodes by QC-22: shipped nodes target SELinux
> Enforcing and load the MCNF policy modules through the bounded boot-time
> policy oneshot. If a node is still kernel-disabled, has not rebooted after the
> enforcing config change, or lacks the SELinux policy toolchain, the kernel does
> not enforce this browser domain and the loader (`setup-selinux-web-preview.sh`)
> self-skips; the OS sandbox in §3.1 remains the operative confinement, and the
> primary security properties never depend on SELinux.

### 3.3 Blast radius & lifecycle
- **One sandboxed process per tab**, torn down per session (Q53). A crash or
  compromise in one tab surfaces as an honest "page crashed" state and does not
  take down the shell or sibling tabs; reload respawns a fresh sandbox.
- **No identity, no persistence** — the helper has no mesh identity and nothing
  survives the session.

### 3.4 IPC hardening
- The shell maps the received frame fd **read-only**; the helper cannot write
  into shell memory. Frame headers (`MWP1`) are bounds-checked before use.
- Events are a typed, length-prefixed protocol; a helper crash is an isolated,
  honest state per session.

### 3.5 Privacy defaults (private-by-default)
- **Zero telemetry** — no engine phone-home; only the page loads the user asked
  for (Q54).
- **System-CA TLS via rustls**, HTTPS-preferred, honest warnings on plain-HTTP /
  cert errors (Q55). **No NSS/NSPR, no Firefox** — Servo is self-contained.
- **First-party session cookies only; third-party blocked; cleared on close**
  (Q73).
- **No persistent history** — in-session back/forward only (Q74).
- **Deny-all sensitive web permissions** (geolocation/camera/mic/notifications/…),
  no prompts (Q69).
- **Host clipboard disabled** — the shipped helper is built
  `default-features = false`, dropping Servo's `clipboard` (arboard), so a page
  cannot read or write the host clipboard. (This is *stricter* than survey lock
  Q71, which had accepted a standard clipboard API; the implementation removed
  that residual risk.)
- **Generic non-identifying UA** (never leaks node/mesh); origin-only referrer;
  basic fingerprint reduction (Q75/Q76).
- **No per-URL browsing audit** (Q80) — only ad-blocker stats + policy/security
  events are logged.
- **No `file://`, no uploads; downloads → a quarantine folder** (Q56).

### 3.6 Ad-filter (network hygiene, not a security boundary)
`mde-adblock` blocks ad/tracker subresource requests **before fetch** and hides
cosmetic elements via an injected user-stylesheet, exempting mesh/overlay
domains. It reduces the malicious-ad surface but is **not** a security boundary —
the sandbox is. A fresh node blocks the worst offenders immediately from the
**bundled seed** (`crates/services/mde-adblock/seed/*.txt`, shipped in the RPM
at `/usr/share/magic-mesh/adblock/` and `include_str!`'d into the engine — one
source, no drift) until the leader replicates the full lists over Syncthing.

### 3.7 Fleet governance
The `browser_policy` worker enforces, mesh-side (not just UI): per-role
enable/disable (**refuses to spawn** on a disallowed role), forces the
ad-blocker on (a one-way ratchet), enforces the URL navigation allowlist, and
rejects out-of-policy actions. The browser is **off by default until the user
acts** (Q4).

---

## 4. Accepted residual risks

These are conscious tradeoffs; the sandbox (§3) contains everything else.

1. **Unrestricted network egress** (Q38). The ad-blocker filters requests but
   egress is otherwise open, so a compromised engine **retains network reach**
   (it could exfiltrate whatever it can see — which the sandbox limits to the
   throwaway tab's own content, no host data). Full containment would need the
   egress proxy, which was **declined**. This is the single largest accepted
   risk.
2. **Servo rendering fidelity** — Servo is a younger engine; heavy sites and some
   media may not fully render (honest degrade, not a security issue).
3. **Servo tracked monthly** (Q65) — API churn + the window between an upstream
   security fix and the next MCNF pin (see the CHANGELOG's update cadence). The
   pin makes each build reproducible + tamper-evident.
4. **Fingerprinting / anti-adblock are best-effort**, not guaranteed.
5. **SELinux policy rollout state** — Quasar-cloud nodes target Enforcing under
   QC-22, but a host that is still kernel-disabled, pre-reboot, or missing the
   policy toolchain does not get the browser confined domain (§3.2). The OS
   sandbox (§3.1) is the operative confinement meanwhile. Accepted because the
   primary security properties do not depend on SELinux.
6. **`unsafe` in the helper + the client** — confined to named FFI/sandbox/shm
   modules, each with a `// SAFETY:` note; denied workspace-wide otherwise (Q95).

---

## 5. Out of scope / non-goals

No credential/password/cookie-jar/history persistence; no uploads (downloads to
quarantine only); no browser inside VM sessions; no standalone window (shell
surface only); **no Firefox/NSS required**; no CA-signed anything; the egress
proxy is declined; perfect anti-fingerprint / anti-adblock is not attempted.

---

## 6. Phone-to-desktop remote-input injection (KDC-MESH-6)

Scope: a paired Android phone driving the active desktop seat as a touchpad +
keyboard, over KDE Connect riding the Nebula mesh overlay (design:
`docs/design/kdc-mesh.md`). The pipeline is two pieces: `kdc_host`
(`crates/mesh/mackesd/src/workers/kdc_host.rs`, pre-existing) parses a paired
phone's `kdeconnect.mousepad.request` packets into a local Bus handoff, and
the pieces documented here — `seat_remote_input`
(`crates/mesh/mackesd/src/workers/seat_remote_input.rs`) and the uinput helper
(`install-helpers/seat-remote-input.py`) — turn that handoff into real
keyboard/mouse events on the seat.

### 6.1 What ships, and where the trust boundary is

| Component | Tier | Trust |
|-----------|------|-------|
| Paired Android phone | remote mesh peer (KDE Connect + Nebula client) | Trusted **once paired** — RSA-4096 device identity, TOFU + fingerprint-pin (SEC-4), riding the Nebula overlay only (mutually authenticated, encrypted). |
| `kdc_host` worker (universal, rank 0) | mackesd, every node | Trusted. Owns the pairing store; gates `kdeconnect.mousepad.request` on `pairing.is_paired(peer)` before it ever becomes a Bus event; hash-chain audits every accepted batch (`kdc_remote_input`). |
| `seat_remote_input` worker (Workstation, rank 1) | mackesd, seated nodes | Trusted; consumes `action/seat/remote-input` as already-authorized (pairing happened upstream in `kdc_host`) and only bounds-checks shape. |
| `seat-remote-input` helper (`install-helpers/seat-remote-input.py`) | root-invoked, short-lived subprocess | Trusted **input**, but wields **root/uinput privilege** — it creates a virtual USB HID keyboard+mouse device and can inject into whatever has focus in the active session. No sandbox of its own; its only defense is the bounded JSON it receives. |

```
phone (paired, Nebula-authenticated)
   -> kdc_host            [pairing.is_paired() gate; hash-chain audit]
   -> Bus action/seat/remote-input     (local, same-host handoff)
   -> seat_remote_input    [shape/bounds validation only -- trust already spent]
   -> seat-remote-input.py
   -> /dev/uinput -> virtual HID device -> whatever has focus on the seat
```

The trust boundary is the **KDC pairing ceremony**, not the input stream
itself. Once a phone is paired, KDC-MESH-6 grants it full keyboard+mouse
control of the active desktop with **no per-action confirmation** — a
deliberate, locked design choice (`docs/design/kdc-mesh.md` decision #16,
"pairing is enough"), not an omission in this worker.

### 6.2 Attack surface

1. **The pairing ceremony itself.** TOFU-on-first-pair + a fingerprint pin is
   the entire authorization step; a spoofed pairing at enrollment, or a
   stolen/cloned already-paired phone, inherits full seat control.
2. **The Nebula overlay transport.** KDC-MESH-6 traffic rides the overlay
   only (`docs/design/kdc-mesh.md` #3) — encrypted and cert-authenticated by
   Nebula itself; a compromised overlay peer or CA is a pre-existing
   mesh-wide risk, not something this feature adds or mitigates further.
3. **The local Bus handoff.** `action/seat/remote-input` is same-host,
   same-user trust: `parse_request` checks the `op`/`source` string fields
   and bounds the payload, but has no cryptographic link back to the phone —
   anything running as the daemon's user that can write to the Bus root can
   forge a remote-input event (shared with every `action/*` worker in
   mackesd; called out here because the payload is unusually high-privilege).
4. **The uinput helper.** A JSON-driven, root-capable process
   (`seat-remote-input.py`); a parsing bug in `event_to_ops`/`create_device`
   would run with full uinput privilege. Today it fails closed on anything it
   doesn't recognize (§6.3.3).
5. **The receiving application.** An injected key/click is indistinguishable
   from physical input to whatever has focus; there is no per-window scoping
   (inherent to uinput, not unique to this implementation).

### 6.3 Mitigations — the layers

#### 6.3.1 Pairing gate (`kdc_host.rs`)
`pairing.is_paired(peer.as_str())` is checked before a
`kdeconnect.mousepad.request` packet is parsed or dispatched at all — an
unpaired phone's packets never reach the remote-input pipeline. Every
accepted batch is recorded in the hash-chained KDC audit log
(`audit_kdc_action`, `"action": "kdc_remote_input"`) — there is no per-event
confirmation, but there is always a durable record.

#### 6.3.2 Bounded, typed event parsing (`seat_remote_input.rs`)
- Move/scroll deltas: finite `f64`, `|value| <= 4096.0`.
- Text tokens: ≤ 16 chars; button clicks: 1–2; special-key codes: `0..=255`;
  phone id: ≤ 128 chars, identifier charset only (`valid_phone`).
- `classify_helper_status` maps the helper's exit code honestly: `0` →
  injected, `69`/`78` → `unavailable` (no live seat), anything else
  (including the helper's own `65` for a malformed event) → `error` — never a
  fake `injected` (proven by the worker's own
  `unavailable_injector_is_honest_state_not_fake_success` test).
- The Bus cursor prevents replaying the same already-drained message twice.

#### 6.3.3 The uinput helper fails closed
- Exits `69` when `/dev/uinput` is absent or permission is denied, `65` on
  any unsupported/malformed op (an unmapped Unicode character, an
  out-of-range delta) — before any `ioctl`/write touches `/dev/uinput`.
- A fixed key/character allowlist (`TEXT_KEYS`, `SPECIAL_KEYS`); an unmapped
  character is refused, never best-effort substituted.
- mackesd bounds the call itself with a 500 ms exec timeout
  (`INJECT_CMD_TIMEOUT`), so a hung helper can't stall the worker forever.

#### 6.3.4 Role + package gating
`seat_remote_input` is Workstation-tier (rank 1 in `worker_role.rs`) — it
idles on headless/Lighthouse nodes. The Server RPM variant deliberately does
not ship `seat-remote-input.py` at all
(`full_rpm_ships_seat_remote_input_helper_but_server_variant_does_not`) — a
box with no seat has no injector to invoke.

### 6.4 Accepted residual risks

1. **No per-action confirmation — by design.** This is
   `docs/design/kdc-mesh.md` decision #16, not an oversight: a paired phone
   drives full keyboard+mouse input (open a terminal, run anything the
   logged-in user can run) with no per-keystroke prompt. The audit log is the
   only brake. A lost, stolen, or cloned paired phone is full seat control
   until unpaired — the design doc's own risk list already names this ("a
   lost/stolen paired phone = fleet control until unpaired").
2. **One mesh-wide pairing record.** A phone pairs once and every node
   recognizes it (`docs/design/kdc-mesh.md` #5) — there's no per-node
   re-authorization, so pairing against one compromised node trusts the phone
   fleet-wide.
3. **No on-screen "a phone is driving this seat" indicator.** Nothing in
   `mde-shell-egui` surfaces phone-driven input distinctly from local input —
   a seated user has no in-the-moment visual cue. KDC-MESH-6's own WORKLIST
   entry still lists live on-seat validation as remaining work.
4. **No timestamp/freshness check.** `ts_unix_ms` is recorded but never
   compared against wall-clock time; replay protection, if any, is left
   entirely to the Nebula transport underneath.
5. **uinput is inherently seat-wide.** The helper cannot scope injected
   events to one window/app — true of any uinput-based remote input, not
   specific to this implementation.

### 6.5 Out of scope / non-goals

No phone-side code review (the Android Nebula client + KDE Connect app are
stock, out-of-repo); no per-command confirmation UI (declined by design lock
number 16); no evdev *capture* (this worker only injects, never reads the
desktop's own input); no device-binding scheme beyond the phone's
Nebula-enrolled identity.

---

## 7. WebAuthn / passkey ceremonies (BROWSER-DD-6)

Scope: a software "platform authenticator" that lets the mesh browser answer
`navigator.credentials.create()`/`.get()` calls, storing the resulting keys in
a Syncthing-mirrored, sealed mesh credential store. Tracked as BROWSER-DD-6
(`docs/WORKLIST.md`); the worker's own doc comment scopes hardware-key
ceremonies and phone-as-authenticator as separate, not-yet-built owners, and
the WORKLIST entry is still unchecked. This section covers
`crates/mesh/mackesd/src/workers/browser_passkeys.rs` — `browser_protocol.rs`,
despite the similar name, is a different, unrelated worker (BROWSER-DD-12's
`mailto:`/`magnet:` external-link handoff) with no WebAuthn role.

### 7.1 What ships, and where the trust boundary is

| Component | Tier | Trust |
|-----------|------|-------|
| Page JavaScript (`navigator.credentials`) | inside the browser engine | **UNTRUSTED** page content — confined the same as any other script (§3) when the engine is Servo. |
| The injected WebAuthn shim (`passkey_bridge_script`/`passkey_bridge_drain_script`) | engine process (Servo `mde-web-preview` or CEF `mde-web-cef`) | Intercepts `navigator.credentials.create`/`.get` **only when `options.publicKey` is present** (browser-4), calling through to the original for `{password}`/`{federated}`/`{otp}` requests; ships bounded public ceremony metadata plus a bridge-minted `client_request_id` to the shell over the existing IPC/beacon channel. CEF also checks `navigator.userActivation` before dispatch; the shell prompt below is the production presence gate. |
| `mde-shell-egui::web.rs` (`handle_passkey_event`) | shell process | Trusted. Holds each ceremony as `PendingPasskeyConsent`, renders an Approve/Deny prompt the page cannot script, rejects denial/duplicates back to the page with `CompletePasskey { error }`, and only after approval publishes `action/browser/passkey` with `user_present=true`, `shell_consent=true`, and `presence_source=browser_shell_prompt`. |
| `browser_passkeys` worker (Workstation, rank 1) | mackesd | Trusted. Validates the request shape + RP-ID/origin binding, mints or uses a P-256 keypair, signs, mirrors to the Syncthing share. |
| Sealed credential store (`credentials/sealed/*.age`, local + Syncthing-shared workgroup root) | at rest | Confidentiality rests on the **mesh-wide age identity** (`/root/.mcnf-age-key`) — the same trust root already used for VPN tunnel secrets and XCP dom0 passwords, not a per-device or per-user secret (§7.4). |
| Hardware FIDO2 keys / phone-as-authenticator | — | **Not wired to a live ceremony.** Only a HID readiness probe + CTAPHID_INIT diagnostic exist; the worker's own doc comment says CTAP2 credential commands and phone-as-authenticator "remain separate owners." |

```
page JS  navigator.credentials.create()/get()
   -> injected shim (engine process, Servo or CEF)   [metadata only; no key material]
   -> beacon/IPC -> mde-shell-egui::web.rs            [shell Approve/Deny gate]
   -> Bus action/browser/passkey                      [only after approval]
   -> browser_passkeys worker
        create: mint P-256 keypair, seal private key, mirror public+sealed record over Syncthing
        get:    locate stored credential by rp_id (+ allow_credentials), unseal, sign
   -> event back over the Bus -> shell -> shim resolves the page's Promise
```

The actual anti-phishing boundary is enforced **daemon-side**, where `rp_id`
is checked against the request's `origin` (§7.3.1) — not by the browser
sandbox. A compromised *or merely aggressive* page at its own real origin can
drive this entire pipeline; see §7.4 for what does, and does not, gate it.

### 7.2 Attack surface

1. **Any page at a matching origin calling `navigator.credentials`.** The
   page can still trigger a visible shell prompt for its own origin/RP id, so
   prompt spam/clickjacking is the main UX/security surface. The daemon only
   completes the ceremony after shell approval (§7.4).
2. **The RP-ID/origin binding logic** (`rp_matches_origin`, `valid_rp_id`,
   `origin_host`) — a bug here would be a direct cross-origin credential /
   phishing bypass. Reviewed line-by-line; found correct (§7.3.1).
3. **The sealed credential store and its Syncthing mirror** — anything that
   can read `credentials/sealed/*.age` (local disk or the shared root) *and*
   obtain the mesh age identity decrypts every mirrored private key (§7.4).
4. **The CTAP HID diagnostic path** (`probe_hardware_key_status_with_live_
   probe`) reads `/dev/hidraw*`; the live INIT exchange is opt-in only
   (`MDE_BROWSER_PASSKEY_CTAPHID_LIVE_PROBE`) specifically because polling it
   by default "can otherwise block or perturb an authenticator" (own doc
   comment) — a narrow, honestly-scoped surface.
5. **Supply chain.** Same Servo pin as §2 (supply chain), plus the
   CEF/Chromium engine lane (`mde-web-cef`) that also originates passkey
   ceremonies but is not in this document's §1 trust table at all (§7.4).

### 7.3 Mitigations — the layers

#### 7.3.1 RP-ID / origin binding is real and correct
- `origin_host()` accepts only `https://` origins (or `http://localhost` /
  `127.0.0.1`), lowercased, no embedded userinfo, no IPv6-literal shortcut.
- `rp_matches_origin` requires an exact match or a **label-boundary** suffix
  match: `origin_host.strip_suffix(rp_id)` must end in `.`. `evilexample.com`
  does **not** match `rp_id = "example.com"`; `login.example.com` does.
- **Public-suffix check (browser-6, 2026-07-10).** The label-boundary match
  alone was not the full WebAuthn "registrable-domain suffix" rule: a page at
  `attacker.github.io` could claim `rp_id = "github.io"` and pass, matching
  every `*.github.io` tenant. `parse_request` now also rejects an `rp_id` that
  is itself a **public suffix** (`is_public_suffix` / `PUBLIC_SUFFIX_RULES` in
  `browser_passkeys.rs`). Combined with the label-boundary check, requiring a
  non-public-suffix `rp_id` guarantees it covers at least the origin's
  registrable domain (eTLD+1). A `github.io` tenant can still use its own full
  `attacker.github.io` as the `rp_id`; it just cannot claim the shared suffix.
  The PSL data is an **interim curated snapshot** (common multi-tenant hosting
  suffixes + common ccTLD second-levels; single-label TLDs are covered by the
  implicit default `*` rule), not the full Mozilla list, because the airgapped
  daemon build cannot vendor a PSL crate today — refresh from
  <https://publicsuffix.org/list/> when it can. It fails **safe**: a missing
  entry is merely less restrictive on an exotic suffix, never blocks a
  legitimate registrable domain.
- Every field is bounded and charset-checked before use (host/rp_id/origin/
  challenge/credential-id lengths; b64url charset; `create` requires both a
  user handle and a name).

#### 7.3.2 Real cryptography, not a stub
- Registration mints a fresh P-256/ES256 keypair via a CSPRNG
  (`SigningKey::generate()`); the credential id is 32 CSPRNG bytes.
- Assertions sign the actual WebAuthn `authenticatorData || clientDataHash`
  payload and **self-verify** (`verifying_key.verify(...)`) before returning
  — a broken signature is caught before it reaches the page.
- Registration emits a spec-shaped CBOR `none`-format attestation object +
  COSE ES256 public key; the worker's own tests round-trip these the way a
  relying party would
  (`registration_and_assertion_outputs_verify_like_a_relying_party`).

#### 7.3.3 Sealed at rest, never plaintext on disk
Private key material is written only inside an Argon2id + XChaCha20-Poly1305
envelope (`crate::ca::backup::seal_bytes` — the same primitive the CA
disaster-recovery bundles use); `PendingPasskeyCeremony`'s own doc comment
notes it "intentionally contains no private key material, signatures,
client-data JSON, or authenticator data" — only the public record and the
sealed blob are ever mirrored. Sealed files are written `0600`.

#### 7.3.4 Honest hardware-key gating
Hardware FIDO2 ceremonies are not claimed as working: status only reports
readiness (`unknown`/`unavailable`/`present_permission_denied`/`ready`) and a
CTAP HID diagnostic — never a fake credential from a hardware key. The live
CTAPHID_INIT probe is off by default (env opt-in) precisely to avoid
perturbing a real authenticator during a routine status poll.

#### 7.3.5 Role gating + honest-absent
Workstation-tier only (idles on headless/Lighthouse nodes). No Bus root, or a
`Persist::open` failure, leaves the worker idle rather than fabricating a
credential.

### 7.4 Accepted residual risks

1. **Honest presence flags + shell consent, but no User Verification yet
   (security-2, updated 2026-07-15).** *Was:* the daemon hardcoded both the
   User Present (`UP`) and User Verified (`UV`) authenticator-data bits on
   every ceremony, and the shell forwarded page-origin requests straight to the
   Bus — so a page could silently obtain a valid, "user-verified" assertion for
   its own origin, with no click, PIN, biometric, or shell confirmation.
   *Now:*
   - **UV remains honest-`0`.** No per-ceremony PIN/biometric/user-verification
     flow exists, so `UV` is never asserted.
   - **`UP` is not hardcoded.** `authenticator_flags()` in
     `browser_passkeys.rs` sets the `UP` bit **only** when the ceremony carried
     `PasskeyRequest::user_present`; a ceremony with no presence signal signs
     `UP=0`, which a relying party rejects. Assertion flags are `0x01`
     (present) / `0x00` (absent); registration flags are `0x41` / `0x40`
     (`AT` always set).
   - **The Browser shell is now the presence gate.** `handle_passkey_event`
     holds the ceremony as `PendingPasskeyConsent` and renders an Approve/Deny
     prompt. Denial and duplicate pending requests return a page-side
     `NotAllowedError`-style completion and never reach the daemon. Approval
     stamps `user_present=true`, `shell_consent=true`, and
     `presence_source=browser_shell_prompt` before publishing to
     `action/browser/passkey`.
   - **Residual:** this is consent/presence, not verification. It does not
     defeat a user intentionally approving a malicious same-origin prompt or a
     clickjacking/social-engineering flow. Direct trusted-Bus writers also
     remain inside the daemon's trust boundary (§7.4 item 4); the shell gate
     protects the page-origin browser path, not arbitrary local privileged code.
2. **Mesh-wide key-sealing root, not a per-device secret.** `seal_private_key`
   keys its passphrase off `age_key_path()` (`/root/.mcnf-age-key` by
   default) — the same identity distributed "to leader-eligible nodes like
   the mesh SSH key" and reused for VPN tunnel secrets and XCP dom0
   passwords. Any node holding that one identity, or the identity itself if
   exfiltrated, decrypts every synced passkey private key mesh-wide, not
   just one user's. A deliberate reuse of the existing mesh trust root (real
   Argon2id/XChaCha20-Poly1305, not weakened crypto), but it trades away
   WebAuthn's usual "the private key never leaves one device" property for
   "follow me to any of my mesh nodes" — worth the same explicit accept the
   rest of this document gives its tradeoffs.
3. **Automatic discoverable-credential selection.** When `allow_credentials`
   is empty and several stored credentials match an `rp_id`,
   `find_credential` deterministically returns the alphabetically-first one
   — no account picker. Fine for a single-account site; silently chooses for
   the user on a multi-account one.
4. **The CEF/Chromium engine lane now has an OS confinement (§10), but with a
   named residual gap.** Ceremonies can originate from `engine: "servo"` **or**
   `engine: "cef"`. As of security-1, `mde-web-cef`'s renderer installs the
   shared `mde-web-sandbox` OS sandbox before `cef_initialize` (§10), so the CEF
   lane is now in §1's trust table and confined by the same class as Servo. The
   residual gap is that **Chromium's OWN internal sandbox stays off**
   (`--no-sandbox`) — see §10.3. Live `.15` runtime proof now shows the CEF
   browser child and Chromium zygote/utility children inherit MCNF's OS sandbox
   (§10.4).
5. **Local Bus trust, same caveat as §6.4's.** Anything running as the
   desktop user that can write `action/browser/passkey` can trigger a real
   signed ceremony for any `rp_id` it can also satisfy the origin check for
   — it cannot forge a credential for a domain it doesn't control the origin
   string for (§7.3.1 still applies), but it can trigger a ceremony the real
   browser never asked for, for its own reported origin.
6. **No challenge/nonce replay tracking.** The daemon does not record seen
   challenges; replay resistance is whatever the relying party's own
   challenge lifecycle provides, not an additional local check.

### 7.5 Out of scope / non-goals

CTAP2 hardware-key create/get ceremonies (readiness probe only, §7.3.4);
phone-as-authenticator (a separate future owner per the worker's own doc
comment); any change to Servo's or CEF's native credential UI (there isn't
one in this pipeline — §7.4); cross-mesh credential export/backup UX (the
sealed store mirrors silently by design, not for manual export).

---

## 8. CEF/Chromium engine — WebRTC privacy hardening (BROWSER-DD-1)

Scope: `crates/desktop/mde-web-cef`'s `chromium_privacy_switches()`
(`cef_init.rs`) and its renderer-level companion in `cef_browser.rs`. This is
narrower than §1-5: it is not a full CEF confinement audit. The CEF OS
confinement itself is now documented in **§10** (security-1: the shared
`mde-web-sandbox` sandbox applied to the renderer); this section documents one
specific, verified WebRTC privacy-hardening finding and its fix.

### 8.1 The finding (2026-07-10)

`chromium_privacy_switches()` shipped `--disable-webrtc` as part of its
privacy/telemetry-hardening bundle, applied to every CEF browser launch. This
switch **is not real** — verified directly against the live Chromium source
this pinned CEF (`149.0.6+g0d0eeb6+chromium-149.0.7827.201`, per
`packaging/browser/cef-linux64-minimal.env`) is built on:

- `content/public/common/content_switches.cc` and
  `chrome/common/chrome_switches.cc` (fetched from
  `chromium.googlesource.com`) define every WebRTC-related switch Chromium
  actually reads — `kDisableWebRtcEncryption`, `kForceWebRtcIPHandlingPolicy`,
  `kWebRtcMaxCaptureFramerate`, `kWebRtcLocalEventLogging` — and **no**
  `disable-webrtc`/`kDisableWebRtc` constant anywhere in either file.
- Chromium's `base::CommandLine` never validates switches against a registry:
  an unrecognized `--` switch is simply never read by any consuming code —
  not errored, not warned, not logged. So the switch shipped as inert.
- A live Google Chrome Enterprise support-forum thread has an administrator
  independently reporting this exact flag being ignored in a managed
  deployment (<https://support.google.com/chrome/a/thread/5939360>).
- A chromium-dev mailing-list thread has Chromium engineers confirming the
  only real way to disable WebRTC is the build-time GN flag
  `enable_webrtc=false` (used by e.g. Chromecast-audio builds) — not
  available here, since this crate links a prebuilt vendored CEF binary
  rather than building Chromium from source.
- CEF's own `cef_settings_t`/`cef_browser_settings_t` structs have no
  WebRTC-toggle field (confirmed against this crate's own pinned-offset FFI
  layout, which tracks every field it sets, and against CEF community
  guidance for embedders asking the identical question).

Net effect before the fix: **CEF's WebRTC stack (a full, standard, current
Chromium implementation — real ICE/DTLS-SRTP, `getUserMedia`,
`getDisplayMedia`, full codec negotiation) was fully reachable from any page
a CEF tab loaded**, despite the code's explicit intent to disable it — the
same local-IP-leak class of concern that motivated the Servo engine's
`dom_webrtc_enabled: false` (`mde-web-preview/src/engine.rs`) applied equally
to CEF tabs, with no working mitigation beyond the (real, but narrower)
`--force-webrtc-ip-handling-policy` switch below.

### 8.2 The fix

- **Removed** `--disable-webrtc` from `chromium_privacy_switches()` — a
  regression test (`init_paths_never_emit_the_inert_disable_webrtc_switch`)
  guards against reintroducing it.
- **Kept** `--force-webrtc-ip-handling-policy=disable_non_proxied_udp` —
  confirmed real (`kForceWebRtcIPHandlingPolicy`, backing the genuine Chrome
  enterprise policy `WebRtcIPHandling`); it constrains ICE candidate
  gathering to proxied/relayed transport, the correct mechanism for the
  local-IP-leak concern.
- **Added native CEF camera/microphone permission handling (2026-07-14).**
  `CefPermissionHandler::OnRequestMediaAccessPermission` is now pinned in the
  local ABI and bridged into the Browser's session-only permission prompt. CEF
  receives exactly the requested device audio/video capture bitmask on allow,
  or `0` on deny; desktop capture remains default-deny.
- **Made browser-page WebRTC reachable in CEF by default (2026-07-14).**
  Once the native media permission path existed, the old renderer-level WebRTC
  remover became an operational blocker for DD-9/browser compatibility. CEF now
  leaves `RTCPeerConnection`/`getUserMedia` reachable by default; the privacy
  posture is the real Chromium IP-handling policy plus the explicit
  camera/microphone permission prompt.
- **Retained the renderer-level remover as an opt-in emergency block.**
  Setting `MDE_CEF_WEBRTC_BLOCKED=1` restores the old best-effort JS API
  removal (`cef_browser::webrtc_block_script`). That script still deletes
  `window.RTCPeerConnection`, `webkitRTCPeerConnection`, `RTCDataChannel`,
  `RTCSessionDescription`, `RTCIceCandidate`, and
  `navigator.mediaDevices`/`MediaDevices.prototype`'s
  `getUserMedia`/`getDisplayMedia` (plus legacy vendor-prefixed
  `getUserMedia`), sweeping same-origin frames and late iframe insertions.

### 8.3 Accepted residual risk — defense-in-depth, not airtight

CEF WebRTC is now an enabled browser-compat feature, so the accepted residual
risk has changed:

- The local-IP-leak class is mitigated by
  `--force-webrtc-ip-handling-policy=disable_non_proxied_udp`, not by deleting
  the API. That switch remains load-bearing and must not be removed.
- Camera/microphone access is user-mediated through the Browser permission
  prompt and held session-only. A prompt denial returns `0` allowed media bits
  to CEF.
- The OS sandbox now exposes only the explicit local capture device nodes
  needed for operational media (`/dev/snd`, `/dev/videoN`) when present, in
  addition to `/dev/dri`; it still does not expose `$HOME`, `/root`, `/var`,
  SSH/Nebula/Syncthing state, or broad `/dev`.
- If `MDE_CEF_WEBRTC_BLOCKED=1` is used, the old JS remover is still not an
  airtight kill switch: it runs after the JS context exists and has no
  `OnContextCreated`-equivalent early hook. It is an emergency compatibility
  lever, not the primary privacy guarantee.

### 8.4 Out of scope

A full CEF confinement audit equivalent to §3 (Servo's `sandbox.rs`); the CEF
OS confinement now lives in **§10** (security-1). This section documents the
WebRTC-specific finding above, not that confinement layer. Full DD-9 is still
broader than browser-page WebRTC: PiP, GPU/HW decode tuning, screen share,
multi-party mesh conferencing, and product decisions around SIP/RTP reuse remain
separate work.

---

## 9. CEF/Chromium engine — the DevTools remote-debugging port (security-4)

Scope: `crates/desktop/mde-web-cef`'s `cef_init.rs` — the `cef_settings_t`
`remote_debugging_port` field and the `--remote-debugging-port` command-line
switch.

### 9.1 The finding (2026-07-10)

`CefSettings::windowless_no_sandbox()` set `remote_debugging_port = 9222`, and
`CefInitPaths::command_line_switches()` emitted `--remote-debugging-port=9222`,
**unconditionally on every CEF launch**. That opens the Chromium DevTools
Protocol (CDP) endpoint on `127.0.0.1:9222` in every shipped build. CDP is an
**unauthenticated** control channel: any local process (or anything that can
reach loopback, e.g. a mis-scoped port-forward) can attach and drive the
browser — navigate it, read cookies and the full DOM of any open tab, and run
arbitrary JavaScript in any origin's context. This runs on a node that also
holds the Nebula CA and mesh SSH keys, so it is a serious local-privilege /
credential-exposure surface, not just a debug convenience.

### 9.2 The fix

- The port is now resolved through `remote_debugging_port()`, which returns
  `0` (**disabled**) on the default/shipped path. Neither the settings field
  nor the command-line switch carries a live port unless explicitly opted in.
- Opt-in is by the **`cef-devtools` build feature** (must never be enabled in a
  release/RPM build) or the **`MDE_CEF_REMOTE_DEBUG`** environment variable at
  launch (which may also pin a specific loopback port in `1024..=65535`). The
  env decision overrides the feature default in both directions.
- Regression tests pin the default-off posture
  (`default_launch_never_exposes_the_cdp_debug_port`) and the opt-in plumbing
  (`remote_debug_env_flag_parses_the_opt_in_shapes`,
  `remote_debug_resolution_defaults_off_and_honors_overrides`).

### 9.3 Accepted residual risk

When an operator *does* opt in on a trusted host, the endpoint is still an
unauthenticated loopback CDP port for that session (the pinned CEF ABI has no
per-session-token knob); the mitigation is that it is off by default, off in
every shipped build, and reachable only by an explicit, deliberate action.

---

## 10. CEF/Chromium engine — the OS confinement (security-1)

Scope: `crates/desktop/mde-web-cef` (the launcher `/usr/bin/mde-web-cef` + the
renderer bridge `/usr/libexec/mackesd/mde-web-cef-renderer`), the shared
`crates/desktop/mde-web-sandbox` crate, and the confined SELinux domain
`packaging/selinux/mde-web-cef.te`.

### 10.1 The finding

The CEF/Chromium browser helper ran attacker-influenced web content with
Chromium's own sandbox disabled (`--no-sandbox`, `cef_settings_t.no_sandbox=1`)
**and** no OS sandbox and no SELinux domain — on a node that also holds the
Nebula CA and mesh SSH keys. A single renderer RCE (assume "when", not "if" —
§2) could read `$HOME`, `~/.ssh`, `/etc/nebula`, `/etc/mackesd`, `/var/lib/*`
and the mesh-storage tree directly. Meanwhile the Servo helper
(`mde-web-preview`) was already fully confined (§3). This was the largest gap in
the browser lane's trust story.

### 10.2 The fix — the OS sandbox, applied to the CEF renderer

The renderer installs the SAME OS-sandbox class Servo gets, **before** it
`dlopen`s `libcef.so` or calls `cef_initialize` — reusing the confinement,
factored out of Servo's `sandbox.rs` into the shared **`mde-web-sandbox`** crate
(not copy-pasted). Applied in `renderer.rs` for the top-level browser process
(`apply_os_sandbox`), it is:

- a **user + mount + IPC + UTS + cgroup + PID namespace** set (no network ns, on
  purpose — egress stays, the ad-filter is the network-hygiene layer, §3.6);
- **uid/gid maps** to a throwaway identity;
- a **`pivot_root`'d read-only rootfs** that binds ONLY the read-only system
  runtime (`/usr` etc.), the system CA bundle + DNS files, the GPU render node,
  and the vendored CEF runtime bundle (`/opt/mde/cef`, read-only, so the browser
  can load `libcef.so` and re-exec its subprocess bridge). There is **NO
  `$HOME`, no `/root`, no `/var`, no `~/.ssh`, no `/etc/nebula`, no
  `/etc/mackesd`, no mesh-storage** — they are simply absent from the new root;
- **`no_new_privs`**, a **fully-dropped capability set** (bounding + ambient +
  inheritable + permitted + effective), and the **seccomp-bpf escape denylist**
  (ptrace, the mount family, `unshare`/`setns`, module loading, `bpf`,
  `perf_event_open`, key management, `kexec`, clock-set, …);
- **cgroup v2 memory/CPU caps** (2 GiB / ~2 cores — a higher ceiling than the
  single-process Servo tab because the cap binds the WHOLE Chromium tree). The
  shipped DRM-seat unit sets `Delegate=yes` plus `DelegateSubgroup=shell` and
  exports `MDE_WEB_SANDBOX_DELEGATE_SUBGROUP=shell`, so browser helpers create
  capped sibling leaves under the delegated service root instead of under the
  busy shell-process subgroup. Ad-hoc SSH/farm session scopes that are not
  systemd-delegated log an honest degraded-cgroup warning while the
  namespace/rootfs/seccomp layers still apply.

**Multi-process reconciliation (the crux).** Chromium is multi-process: the
browser process forks + re-`exec`s the renderer bridge with `--type=renderer`
(and `--type=gpu-process`/`--type=utility`) for each subprocess. The OS sandbox
is applied ONCE, on the top-level browser process, and every subprocess inherits
it automatically — `no_new_privs` preserves the seccomp filter across `execve`,
and the namespaces + `pivot_root`'d rootfs + dropped caps + cgroup are inherited
by fork/exec. So the renderer subprocess that actually runs untrusted JS is
confined too. The renderer detects a CEF subprocess (`--type=…` present) and
does **not** re-apply the sandbox (that would `EPERM` — the very syscalls it
would need are on our own denylist). A confinement failure on the top-level
process is **fatal** (`CEF_OS_SANDBOX_FAILED`, exit 78) — the helper never runs
web content unconfined.

The SELinux domain `mde_web_cef_t` (defense-in-depth, §3.2's shape) confines
both binaries wherever a node runs SELinux Enforcing; it self-skips where
SELinux is disabled (the platform standard), leaving the OS sandbox as the
operative confinement.

### 10.3 Accepted residual risk — Chromium's OWN internal sandbox stays OFF

`--no-sandbox` is **kept**. Chromium's own internal sandbox (its
unprivileged-userns / setuid-`chrome-sandbox` layers) is **not** re-enabled, and
this is a deliberate, documented limitation, not an oversight:

- Chromium's namespace sandbox has its zygote `unshare`/`clone` a NESTED user
  namespace and then `mount`/`pivot_root`/`umount2` to build each renderer's
  restricted view. Those mount-family syscalls are **exactly** what MCNF's OS
  seccomp denylist `EPERM`s (post-setup). Re-enabling Chromium's sandbox would
  require **removing** the mount/`pivot_root`/`unshare` denials — i.e. gutting
  the OS sandbox's seccomp layer — which is a strictly worse trade.
- `no_new_privs` + seccomp block the mount family **regardless of capabilities**
  (seccomp is checked before the capability check), so even the full cap set a
  nested userns would grant does not help.
- The setuid `chrome-sandbox` helper is not installed setuid-root on these
  nodes, and would not work under the sandbox's throwaway uid map anyway.
- The pinned prebuilt CEF 149 binary cannot be rebuilt to change this
  (from-source GN flags are unavailable — this crate links a vendored payload).

**Net honest posture:** Chromium's *internal* sandbox is off, but the *entire*
Chromium process tree (browser + every renderer/GPU/utility subprocess) runs
inside MCNF's OS sandbox, so untrusted web content still cannot reach `$HOME`,
SSH keys, the Nebula CA, or mesh data. MCNF's outer seccomp filter also partly
substitutes for the per-renderer seccomp layer that `--no-sandbox` turns off
inside Chromium — it just covers the whole tree from outside rather than
per-process from inside. Do **not** read "sandboxed" as "Chromium-sandboxed"
here; it means OS-sandboxed.

### 10.4 Verification status — live `.15` OS-sandbox proof

A sandbox cannot be mathematically proven by a smoke test, but the CEF OS
confinement is now runtime-proven on a live seat for the core claims above. The
headless/unit side remains covered by crate builds and pure planner tests: the
CEF policy (`web_cef` — host, 2 GiB ceiling, distinct per-run rootfs prefix),
the seccomp denylist construction, uid/gid-map and rootfs bind plans, and the CEF-specific
extra-bind planner (`cef_extra_readonly_binds` — exposes only the runtime +
vetted extensions, never a key/home path). The seccomp denylist is the SAME one
Servo runs a full browser engine under.

**Live proof, 2026-07-15, `.15` (`Basement-Test-Workstation`, Fedora 44):**

1. A held real `/usr/bin/mde-web-cef tab` launched against the installed pinned
   `/opt/mde/cef` bundle and initialized CEF successfully. The log showed
   `CEF_OS_SANDBOX applied=1 ... home_visible=0 seccomp=1 caps_dropped=1`,
   `CEF_PRIVATE_RUNTIME_ENV ... tmpfs=1`, `CEF_INITIALIZE_OK`, and Chromium
   subprocess bridge starts for zygote and network utility processes. The
   installed verifier on the same deploy had already proven CEF render/input:
   final title `mde-browser-verify-p1-k1-tm`, 4 painted `1280x800` frames, and
   `VERIFY RESULT=PASS`.
2. The sandbox fork layout was inspected from the host. The launcher and fork
   supervisor are not the security evidence. The actual CEF browser child
   (`mde-web-cef-renderer` child PID 566774 in that run) and all observed
   Chromium zygote/utility descendants (PIDs 566787, 566788, 566808, 566817,
   566818, 566827, 566829) had `NoNewPrivs: 1`, `Seccomp: 2`, and zero
   `CapPrm`/`CapEff`/`CapBnd` masks.
3. Those CEF/Chromium children inherited the same sandbox namespaces in that
   run (`mnt:[4026532544]`, `user:[4026532535]`, `pid:[4026532552]`,
   `ipc:[4026532551]`, `uts:[4026532549]`, `cgroup:[4026532558]`). The
   `/proc/<pid>/root` view exposed `/opt/mde/cef/Release/libcef.so`, the
   renderer bridge, and the private `/tmp/mde-web-cef/{home,cache}` tree, while
   `/home`, `/root`, `/etc/nebula`, `/etc/mackesd`, `/mnt/mesh-storage`,
   `/run/user/1000/bus`, and `/run/dbus/system_bus_socket` were absent.
4. Chromium multi-process startup therefore works under the outer MCNF OS
   sandbox: zygote and utility children started and stayed alive without an
   `EPERM` crash from the shared seccomp denylist.
5. SELinux Enforcing was separately closed on the same F44 seat after the split
   Browser RPM loader fixes. Final fresh-root verifier passes ran with
   `getenforce = Enforcing`, no permissive domain marker, loaded modules
   `mde_web_cef` and `mde_web_preview`, and binary labels
   `mde_web_cef_exec_t` / `mde_web_preview_exec_t`. CEF passed with final title
   `mde-browser-verify-p1-k1-tm`, 4 painted `1280x800` frames, pointer/key/text
   input observed, and no AVCs for the final window. Servo passed with 4 painted
   `1280x800` frames and final page text `P:1 K:1 T:m`; its final window also
   had no AVCs and no leftover helper processes.
6. Delegated cgroup caps were closed on `.15` after the
   `DelegateSubgroup=shell` packaging fix. The running DRM shell moved to
   `/system.slice/mde-shell-egui.service/shell`, leaving the service root empty.
   A transient service with the same delegation contract launched the installed
   CEF verifier and created
   `/system.slice/mde-browser-cgroup-proof-cef.service/mde-web-cef-...` with
   `memory.max=2147483648`, `cpu.max=200000 100000`, and no
   `mde-web-sandbox: cgroup limits not applied` warning. The verifier returned
   `VERIFY RESULT=PASS`, final title `mde-browser-verify-p1-k1-tm`, and 4
   painted `1280x800` frames. The same delegated proof against Servo created
   `/system.slice/mde-browser-cgroup-proof-servo.service/mde-web-preview-...`
   with `memory.max=1073741824`, `cpu.max=80000 100000`, no degraded-cgroup
   warning, `VERIFY RESULT=PASS`, and final page text `P:1 K:1 T:m`.
7. Per-run sandbox rootfs mountpoints were live-proven after the fixed-root
   failure class. The installed Browser RPM payload on `.15` matched the new
   helper hashes for `mde-web-cef-renderer` and `mde-web-preview`, and
   `/usr/libexec/mackesd/browser-verify-engines --engine all --budget 30 --timeout 60s`
   passed for both engines with display/input response and process cleanup. The
   previous fixed `/tmp/.mde-web-cef-root` and `/tmp/.mde-web-preview-root`
   directories still existed during the pass, while the successful run created
   fresh `/tmp/.mde-web-*-root-<pid>-<run>` mountpoints; the proof therefore did
   not depend on manually deleting stale roots. A follow-up farm runtime proof
   tightened the lifecycle: ordinary helper exits remove the host-visible
   per-run mountpoint after successful render/input (`P:1 K:1 T:m`), so only
   hard-kill/crash residue should persist.

**Rerun triggers:** ad-hoc SSH/user-session launches are not systemd-delegated
and can honestly log `mde-web-sandbox: cgroup limits not applied ... Permission
denied`; the namespace/rootfs/seccomp/cap layers above still apply. If the CEF
pin, Servo helper, Browser sandbox rootfs plan, or DRM unit cgroup delegation
changes, rerun the Enforcing AVC audit and the delegated cgroup-cap proof; do
not assume this 2026-07-15 closure covers a new engine payload.
