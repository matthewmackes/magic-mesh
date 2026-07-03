# MCNF Threat Model — the mesh web browser + `mde-web-preview` helper

Scope: the sandboxed Servo browser that MCNF ships inside the desktop shell
(BOOKMARKS-5..9; design: `docs/design/mesh-bookmarks.md`). This document states
the attack surface, the confinement layers, and the **accepted residual risks**
of running a real interactive web engine on a node that also holds mesh
identity, keys and workgroup data.

It is a living document: it is the security contract the browser is packaged
against (`crates/mesh/mackesd/Cargo.toml` `generate-rpm` block + the confined
SELinux domain in `packaging/selinux/mde-web-preview.te`). Change the sandbox or
the packaging and you update this file.

---

## 1. What ships, and where the trust boundary is

| Component | Tier | Trust |
|-----------|------|-------|
| `mde-web-preview` (bin) | desktop-shell helper, **out-of-process** | **UNTRUSTED** — runs attacker-influenced web content (JS, layout, media). Confined. |
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

> **Platform note.** The operator sets **SELinux disabled platform-wide**
> (2026-06-20). Where SELinux is disabled the kernel does not enforce this
> domain and the loader (`setup-selinux-web-preview.sh`) self-skips — the OS
> sandbox in §3.1 remains the operative confinement, and the primary security
> properties never depend on SELinux. The module ships + loads so that any node
> re-enabling SELinux Enforcing gains the confined domain with no extra step
> (defense-in-depth).

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
5. **SELinux disabled on the platform standard** — the confined domain (§3.2) is
   defense-in-depth that is inert until an operator enables SELinux; the OS
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
