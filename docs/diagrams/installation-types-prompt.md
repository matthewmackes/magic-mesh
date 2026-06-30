# Image-generation prompt — MCNF Installation Types diagram

Paste the block below into ChatGPT (or any image model). For sharp labels,
prefer asking for **SVG** output over a raster image — image generators often
mangle this much small text. A ready-made Carbon SVG ships alongside this file
(`installation-types.svg`).

---

```text
You are a senior information designer. Create a single, high-resolution
horizontal diagram (16:9, suitable for a website section) that explains the
THREE installation types of a private mesh-networking platform called MCNF.
Write every label and caption at an 11th-grade reading level — plain, confident,
no jargon without a one-line plain-English gloss.

=== WHAT THE DIAGRAM MUST SHOW ===

The product ships as ONE installer. At install time you pick a ROLE, and the
roles NEST — each bigger one includes everything the smaller one has, then adds
more:  LIGHTHOUSE  ⊂  HEADLESS SERVER  ⊂  WORKSTATION.

Show this as three vertical "stacks" side by side, left to right, growing taller:
Lighthouse (shortest), Headless Server (medium), Workstation (tallest). Each
stack is built from horizontal LAYER blocks. The SHARED layers sit at the BOTTOM
and align across all three stacks (same color = shared); each role's EXTRA layers
stack on top in a distinct accent color. The visual takeaway: same foundation,
more capability as you move up and to the right. Put a small count badge on each
role — Lighthouse "22 background services", Server "+3", Workstation "+5"
(~30 total at the top).

LAYERS, bottom to top:

1. SHARED FOUNDATION — present in ALL THREE (color these identically):
   • Encryption Library — rustls/ring for all TLS, never OpenSSL.
     Caption: "Handles all the locking and key-checking."
   • Coordination + Shared Disk — etcd (agreement) + Syncthing (a copy of the
     shared files on every node). Caption: "Keeps every machine in sync."
   • Secure Overlay (the wire) — a Nebula encrypted network; every node talks
     over end-to-end encryption (Ed25519 identities, AES-256-GCM).
     Caption: "The private, encrypted network all machines share."

2. LIGHTHOUSE adds (Server + Workstation inherit it):
   • Control Plane Core — the always-on anchor: the "mackesd" daemon, Nebula
     RELAY, Certificate Authority + enrollment, leader election, and
     health/scan/metrics/alerts (22 background services). Caption: "The
     lighthouse: the always-on anchor that relays traffic, hands out
     certificates, and watches the mesh's health. Runs on a small cloud server
     with no screen."

3. HEADLESS SERVER adds (Workstation inherits it):
   • Fleet Automation + Jobs — "magic-fleet": any node publishes a desired
     configuration and every peer converges itself; saved jobs run on a
     schedule. Caption: "Automation engine — set the rules once, every machine
     fixes itself."
   • Storage Replica — a full Syncthing copy of the shared volume.
     Caption: "A full, always-on copy of the shared files (like a NAS)."
   Note under this stack: "Headless = no desktop. An always-on box or NAS."

4. WORKSTATION adds (top of the stack):
   • Message Bus (IPC) — "mde-bus", a file-backed pub/sub + RPC plane the apps
     use to talk to the daemon. Caption: "Lets the apps and the engine talk."
   • Look Stack — strict IBM Carbon design system, single-sourced (mde-theme).
     Caption: "The shared visual style for every app."
   • COSMIC Desktop + Apps — the full graphical experience: Workbench, Files,
     Music, Voice/SIP, the panel applet, first-run role chooser.
     Caption: "The full desktop and every app — the daily-driver laptop."

=== CALLOUTS ===
Footer line: "One installer, three roles. Roles nest — each adds to the one
before. No fixed center: every node can lead, enforce, and relay." Label typical
hardware under each stack: Lighthouse = "cloud VPS, headless" · Headless Server =
"always-on box / NAS" · Workstation = "laptop / desktop".

=== LEGEND ===
A compact legend distinguishing "Shared by all" vs each role's added layers, with
each layer's one-line caption kept readable.

=== VISUAL STYLE — STRICT IBM CARBON DESIGN SYSTEM (v11), DARK THEME ===
This matches the product's real UI (single-sourced in its `mde-theme`).
• Typeface: IBM Plex Sans for all text; IBM Plex Mono for code-like labels.
• Carbon "Gray 100" dark theme:
    - Page background:     Gray 100  #161616
    - Layer/card surfaces: Gray 90   #262626, 1px Gray 80 #393939 border
    - Primary text:        Gray 10   #f4f4f4 ; secondary text: Gray 50 #8d8d8d
• Accent tokens (use sparingly), one per layer-group as a left edge bar:
    - Shared foundation: Blue 60   #0f62fe
    - Lighthouse:        Teal 50    #009d9a
    - Headless Server:   Purple 60  #8a3ffc
    - Workstation:       Green 50   #24a148
• Carbon rules (strict): flat — NO shadows, gradients, or glow; sharp 0px
  corners; align to an 8px (2x) grid with generous, even whitespace; simple
  geometric Carbon line icons (2px stroke, no fill, one per layer is enough);
  clear type hierarchy; high contrast and fully legible; restrained,
  enterprise look — like IBM technical documentation.

=== OUTPUT ===
One clean 16:9 image, high resolution, web-ready, no watermark; text crisp and
legible at typical website widths. Do not invent features or layers beyond those
listed above.
```

---

## Light-theme variant

To render lighter than the app, swap the theme block:
background Gray 10 `#f4f4f4`, surfaces White `#ffffff` with Gray 20 `#e0e0e0`
borders, primary text Gray 100 `#161616`, secondary Gray 70 `#525252`. Keep the
same blue/teal/purple/green accents. Don't mix the two themes.

## Source notes (so the content stays accurate)

- Roles nest **Lighthouse ⊂ Server ⊂ Workstation**; one signed RPM, install-time
  role chooser (`README.md`).
- Worker split: 22 rank-0 (Lighthouse), +3 rank-1 (Server), +5 rank-2
  (Workstation) — ~30 total (`crates/mesh/mackesd/src/worker_role.rs`).
- "Server" is the canonical role name (was "headless"); use whichever label your
  website audience knows.
