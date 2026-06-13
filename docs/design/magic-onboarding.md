# Magic Onboarding — network enrollment + zero-friction join (2026-06-13)

Fixes **MESH-1**: enrollment today is QNM-Shared-mediated only (the peer writes a
CSR to the replicated volume; the lighthouse auto-signer writes the bundle back),
so a NAT'd / remote peer can't self-join — it can't reach QNM-Shared until it's on
the overlay, and it can't get on the overlay until enrolled (chicken-and-egg). The
join token already carries the lighthouse's public address but enroll never uses
it for the *signing handshake*.

This epic promotes the cert-signing exchange from a **shared filesystem** to a
**lighthouse network service**, and collapses the 5-command setup into two verbs
with a TUI — honoring §1 (Nebula control plane only; **no Tailscale/Headscale/DERP**).

## Locks (2026-06-13 survey)

| # | Decision | Lock | Why |
|---|---|---|---|
| 1 | Enroll endpoint transport | **Dedicated rustls HTTPS endpoint** on the lighthouse (POST `/enroll`) | Purpose-built, simple to reason about + firewall; rustls (§3); avoids overloading the covert NF-1.5 tunnel (which is a nebula-over-TCP pipe, not an HTTP API) |
| 2 | Lighthouse trust at enroll time | **Join token pins the endpoint's TLS cert fingerprint** | The peer has no CA yet; the single-use token already carries the bearer, so it also carries `fp=<sha256>` and the peer verifies it before sending the CSR — no trust-on-first-use MITM window |
| 3 | Onboarding command shape | **Two verbs: `found` + `join`** | `mackesd found <mesh-id>` = become the 1st lighthouse (mesh-init + enroll endpoint + serve, prints a ready-to-paste `join` line). `mackesd join <lighthouse-ip> <token>` = role-pin + network-enroll + serve, one shot |
| 4 | Enrollment TUI | **ratatui full-screen** | Operator enters lighthouse IP + token, watches live progress; works headless over SSH (lighthouses/servers have no display) |

## Architecture

### Join token v3 (extends the v2.5 shape)
`mesh:<id>@<lighthouse_ip>:<enroll_port>#<bearer>?fp=<sha256-of-endpoint-cert>`
- backward-token note: the `?fp=` suffix is additive; the v2.5 parser ignores it.
  The peer requires `fp` for the network path; absence → fall back to the legacy
  QNM-Shared flow (co-located nodes).

### Lighthouse side — the `/enroll` endpoint
- A **rustls HTTPS listener** (default `:443`, configurable) started by `found` /
  `serve` when this node is `am_lighthouse`. TLS cert is self-signed at `found`
  time; its SHA-256 fingerprint is embedded in every minted join token.
- `POST /enroll` body `{ bearer, name, pubkey_pem, role }` →
  1. validate `bearer` against the single-use ledger (reuse `enroll-token`'s
     bearer-hash ledger; reject replays/expired);
  2. sign the peer's pubkey with the mesh CA (reuse the `ca` module / the
     `cert_authority` signing path);
  3. return the nebula **bundle** JSON (ca.crt, the peer's host.crt, the
     `lighthouses` roster with public `external_addr`, `mesh_cidr`, overlay_ip).
- Reuses: the bearer ledger, the CA signer, `ca::bundle` shape. The QNM-Shared
  flow stays as the co-located fallback.

### Peer side — `join`
1. parse token → mesh_id, lighthouse addr, bearer, cert fp;
2. generate the Ed25519 keypair locally;
3. HTTPS POST the CSR to `https://<lighthouse>/enroll`, **verifying the server
   cert against the pinned fingerprint** (custom rustls `ServerCertVerifier`);
4. receive + write the bundle → materialize `/etc/nebula` (static_host_map →
   the lighthouse's **public** addr — fixes the MESH-2 class of bug by using
   `bundle.lighthouses[].external_addr`, never a local interface);
5. `serve` → nebula dials the lighthouse public IP (outbound, NAT-friendly) →
   overlay;
6. steady state: mount QNM-Shared over the overlay (ONBOARD-6).

### The two verbs
- `mackesd found <mesh-id> [--external-addr auto]` → mesh-init + start the enroll
  endpoint + serve + print the `join` line (with the embedded `fp`).
- `mackesd join <lighthouse-ip> <token>` → role-pin (default per the role chooser)
  + network-enroll + serve. `mackesd join` with no args → launch the TUI.

### TUI (`mde-enroll`, ratatui + crossterm)
Full-screen: lighthouse-IP field + token field (paste) → Join → live progress
(CSR sent → signed → /etc/nebula written → nebula up → ping lighthouse OK), error
strip in Carbon danger. Headless over SSH.

## Acceptance
- A peer on a **different network behind NAT** joins with a single `mackesd join`
  (or the TUI) — no QNM-Shared pre-mount, no manual cert shuttling.
- The peer **refuses** to send its CSR if the lighthouse cert fingerprint doesn't
  match the token's `fp` (MITM-resistant).
- A replayed/expired bearer is rejected by `/enroll`.
- `mackesd found` brings up a usable lighthouse + prints a working `join` line.
- End-to-end **bidirectional overlay ping** between a freshly-`join`ed peer and
  the lighthouse.

## Test plan (F43 constraint)
The live cloud lighthouses run an **older F43 `mackesd`** (glibc 2.42) and can't
run the F44 build (requires GLIBC_2.43). Test the new feature on the **two F44
LAN boxes**: one runs `found` (lighthouse), the other `join`s as a peer; verify
the network-enroll path + overlay ping locally. Cloud deployment waits on
ONBOARD-7 (portable build).

## Risks / out-of-scope
- Custom rustls cert-pin verifier must be correct (fail-closed on mismatch).
- Bearer single-use enforcement over the network (replay window).
- ONBOARD-6 (QNM-Shared over overlay) only affects directory/fleet features, not
  basic overlay membership.
- Multi-lighthouse roster is still out of scope (single founding lighthouse, §8).
- F43/older-glibc portable build is ONBOARD-7 (separate).
