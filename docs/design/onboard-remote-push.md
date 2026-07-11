# OW-15 — the onboard remote-push executor (transport decision brief)

> **Status: DECISION-PENDING (operator).** This is a `/plan` decision brief, not a
> lock — it lays out the transport options + a recommendation so the choice can be
> made in one read. Authored 2026-07-01 (autonomous prep during the `/polish /ship`
> loop) to unblock OW-7/OW-8/OW-11, all stalled on the *same* missing capability.
> Nothing is built until the transport below is chosen.

## The problem (one gate, three verbs)

Three `onboard` verbs deliver a pure planner + an injectable `LiveX` seam, and all
three `LiveX` impls return the **same** `IntegrationGated` for the **same** reason —
there is no way to *reach a target node and apply a bounded set of actions to it*:

| Verb | `LiveX` seam | What it must push to the target |
|---|---|---|
| **OW-7** `spawn-lighthouse` | `LiveProvisioner::{push_enroll, migrate_ca}` | run `enroll` on a fresh box; move/cross-sign the CA |
| **OW-8** `first-desktop` | `LiveFirstDesktop` | open a broker session via Nova cloud placement (`LiveNovaPlacement`) + Bus |
| **OW-11** `service-add music` | `LiveServiceApply::provision_music` | pin the **Media role** + seal the `media-spaces` secret on a lighthouse (then its `navidrome_supervisor`/`media_registry` do the rest — [[worklist E12-POLISH]] proven live on LH1+LH2) |

So OW-15 is **one shared executor**: `reach(target) → apply([RoleP­in | SecretSeal |
RunEnroll | OpenBroker …])`, that every onboard verb's `LiveX` calls instead of
returning gated.

## The load-bearing distinction: bootstrap vs. day-2

The target's mesh-membership state decides what transports are even *possible*:

- **Bootstrap targets** (OW-3/4/5 join, OW-7 fresh lighthouse) are **not on the mesh
  yet** — no Nebula cert, not on the Bus. You can only reach them out-of-band (SSH
  over the LAN/public IP + the enroll bearer). This is exactly OW-7's existing model:
  `push_enroll` is documented as *"SSH in and run enroll."*
- **Day-2 targets** (OW-11's media-lighthouse, any existing peer) are **already full
  mesh members** — reachable over the Nebula overlay, running `mackesd`, on the Bus.
  For these, §9 applies in full: *remote execution is typed verbs + signed job
  bundles only, no raw shell.*

Forcing one transport across both is the mistake. A fresh box **can't** use the Bus
(chicken-and-egg: enrollment is what puts it on the Bus); an enrolled peer
**shouldn't** be driven by raw SSH (§9).

## The options

**A. Raw SSH everywhere.** One SSH executor for both bootstrap + day-2.
*Pro:* simplest; matches OW-7's `push_enroll` today. *Con:* **violates §9** for day-2
(raw shell to an enrolled peer); ambient SSH key management is a second trust root
next to the Nebula CA; host-key pinning is on us.

**B. Typed Bus verb + signed job bundle everywhere** (§9-native). A `mackesd`
`onboard-apply` action over the overlay Bus; the target's worker validates the
signed bundle + applies the allow-listed actions.
*Pro:* fully §9-compliant, auditable (hash-chain), reuses the Bus + the CA trust
root, no second credential. *Con:* **can't reach a bootstrap target** (not on the
Bus yet) — OW-7/join still need an out-of-band path.

**C. Hybrid (recommended).** **Bootstrap → mesh-CA-scoped SSH; day-2 → typed Bus
verb + signed bundle.** One `RemotePush` trait, two impls behind the same allow-list:
- `SshBootstrap` — reaches a not-yet-enrolled box over SSH **gated by the single-use
  enroll bearer** (the same token OW-4 mints), runs only the enroll/`role-provision`
  step, then the box is a mesh member and never touched by SSH again. This is OW-7's
  accepted model, scoped to bootstrap only.
- `BusApply` — for an enrolled peer (OW-11, day-2 OW-7 CA-migrate, OW-8 broker), a
  typed `action/onboard/apply` verb carrying a **signed job bundle** (§9); the
  target's `onboard_apply` worker validates signer + freshness + the action
  allow-list, applies (`RolePin`/`SecretSeal`/…), and replies with observed-state.

## Recommendation: **C (hybrid)**

It's the only option that honors §9 *and* actually reaches a fresh box. It keeps the
raw-SSH surface to the **bootstrap instant only** (bearer-scoped, single action, then
gone), and makes all steady-state remote config §9-native + auditable. It matches how
the platform already thinks: enrollment is the trust boundary; after it, everything is
typed Bus verbs (§9) over the CA-rooted overlay.

## Acceptance (each runtime-observable)

1. **Day-2 (OW-11):** from node A, `onboard service-add music` pins the Media role +
   seals `media-spaces` on a *different* live lighthouse B **over the Bus (no SSH)**;
   B's `navidrome_supervisor`/`media_registry` bring Navidrome + `music.mesh` up.
2. **Bootstrap (OW-7):** from A, `spawn-lighthouse` reaches a fresh box over
   bearer-scoped SSH, runs enroll, and the box joins — with **no ambient SSH key**,
   only the single-use enroll bearer; SSH is never used to it again.
3. The executor **refuses any action outside the typed allow-list** (RolePin /
   SecretSeal / RunEnroll / OpenBroker), and a transport/auth failure surfaces a typed
   error leaving the target **unchanged** (no partial state); the day-2 path is
   hash-chain audited (§8).

## Build plan (once C is chosen)

1. `onboard/remote_push.rs`: the `RemotePush` trait + the `Action` allow-list enum +
   the signed-`JobBundle` type (reuse `ca`/signing + the SEC-6 evict signer).
2. `BusApply` impl + a `mackesd` `onboard_apply` worker (subscribe `action/onboard/apply`,
   validate, apply, publish observed-state) — reachable, leader-gated where needed.
3. `SshBootstrap` impl scoped to the enroll bearer (reuse OW-4's mint + OW-5's enroll).
4. Wire the 3 `LiveX` seams onto `RemotePush`; each stops returning `IntegrationGated`.
5. Live-verify acceptance 1 + 2 on the magic-mesh (LH1/LH2 + a throwaway node).

## Decisions (locked 2026-07-01, during the `/polish /ship` loop)

- **Transport = C (hybrid).** Operator-confirmed via the gate-lift survey. Bootstrap
  targets → bearer-scoped SSH; day-2 enrolled peers → typed Bus verb + signed bundle.
- **Bundle signer = the issuing node's mesh identity key, authorized by leadership**
  (not the Nebula CA key). The issuer signs the [`JobBundle`] with its `identity.rs`
  Ed25519 key — the **same trust primitive the SEC-6 blocklist already uses**
  (`ca/blocklist.rs` signs revocations with the node identity's `verifying_key()`).
  The target's `onboard_apply` worker: (1) resolves the *claimed issuer's* identity
  pubkey from the CA **`nodes` registry** (the mackesd SQLite `nodes` table —
  `node_id → public_key, role`, populated at enrollment from the request's
  `public_key_hex`; see `nebula_roster.rs` / `enrollment.rs` — **NOT** the
  `PeerRecord` roster, which carries no identity key), (2) verifies the signature
  against it (proves the claimant holds that identity), and (3) **authorizes** by
  requiring the issuer's registered `role` to be leader-eligible (**Lighthouse**) — so
  only a lighthouse may push a role-pin / secret-seal to a peer (§8 privileged issuer;
  a compromised workstation cannot). *Rationale:* reuses mesh-native identity + the
  enrollment registry (no new distributed authority key, no coupling to Nebula CA-key
  extraction). The bundle carries an `issuer` node-id field (signed). *Scope:* the
  registry lives on CA-holding nodes; OW-11's day-2 targets (media-lighthouses) hold
  it, so the first cut is covered — a non-CA target would need the issuer pubkey
  delivered another way (future, tracked when a non-CA day-2 target appears).
  → `process_apply` already takes the trusted `signer: &VerifyingKey` injected, so the
  worker owns resolution (roster-lookup + leader-check) and the pure core is unchanged.
- **`SshBootstrap` auth scope = the single-use enroll bearer** (OW-4's mint), matching
  OW-7's accepted `push_enroll` model — scoped to the bootstrap instant only, never a
  standing SSH key. (A mesh-CA-signed SSH cert stays a future hardening, not needed for
  the first cut.)

## Historical open questions (now resolved above)

- ~~Confirm C (hybrid) vs A vs B?~~ → C.
- ~~`SshBootstrap` auth: enroll bearer vs mesh-CA-signed SSH cert?~~ → enroll bearer.
- ~~Bundle signer: CA vs per-node key?~~ → issuing node's identity, leader-authorized.
