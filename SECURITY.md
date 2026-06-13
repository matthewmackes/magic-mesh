# Security Policy

Magic Mesh is security-sensitive infrastructure — it stands up an encrypted
overlay mesh (Nebula), a fleet control plane, and a workgroup KDC. We take
vulnerability reports seriously and appreciate responsible disclosure.

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Report privately through either:

- **GitHub Security Advisories** — the preferred channel: open a draft advisory
  at <https://github.com/matthewmackes/magic-mesh/security/advisories/new>
  (Security → Advisories → *Report a vulnerability*).
- **Email** — `matthewmackes@gmail.com` with subject `SECURITY: magic-mesh`.

Please include: affected component/crate, version or commit, a description of the
issue and its impact, and reproduction steps or a proof of concept if you have
one.

### What to expect

- **Acknowledgement** within 5 business days.
- A **triage assessment** (severity + affected versions) and a remediation plan
  once confirmed.
- Credit in the advisory and `CHANGELOG.md` for the fix, unless you ask to remain
  anonymous.

We follow **coordinated disclosure**: please give us a reasonable window to ship
a fix before any public write-up. We will keep you updated on progress.

## Supported versions

Magic Mesh is pre-1.0 and ships as a rolling release from `master`. Security
fixes land on `master` and in the next packaged release; there is no long-term
support branch yet. Always run the latest release.

## Scope

In scope: the daemon (`mackesd`), the mesh/fleet control plane (`magic-fleet`,
`meshctl`), the KDC (`mde-kdc-host`), the IPC bus (`mde-bus`), enrollment + CA
lifecycle, the network scanners, and the GUI surfaces.

Out of scope: vulnerabilities in upstream dependencies (report those upstream —
Nebula, LizardFS, rustls, libcosmic, etc.), and issues that require an attacker
who already has root/operator access on a mesh node (the trust model assumes the
operator owns their nodes — see `AI_GOVERNANCE.md` §8, the ≤8-peer flat-trust
envelope).

## Security model (summary)

- **Transport** is the Nebula overlay (Ed25519 host identities, AES-256-GCM /
  ChaCha20-Poly1305); there is no fixed center.
- **Crypto floor** (`AI_GOVERNANCE.md` §3): Ed25519 · AES-256-GCM /
  ChaCha20-Poly1305 · RSA-4096 KDC device identity · rustls, no OpenSSL.
- **Secrets** never ride argv or inherited env — passphrases/passcodes are read
  via `--*-stdin` or systemd-creds; the daemon scrubs its env at boot.
- The relay/direct introspection debug-SSH (`nebula_admin`) binds **loopback
  only** and is key-auth.

See `AI_GOVERNANCE.md` for the full architectural + security locks.
