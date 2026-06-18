# Start Menu redesign (APPS-STYLE-2)

Goal: **Deploy Redesigned Start menu.** Source design: Claude Design project
"Start menu improvement design" (`Start Menu.dc.html`), imported 2026-06-18.
Target: `crates/platform/mde-cosmic-applet/src/bin/mde-apps-applet.rs` (the apps
launcher applet — MCNF's Start Menu). Survey locks (10 questions, 2026-06-18):

| # | Decision | Lock |
|---|----------|------|
| 1 | Delivery | **Full one-pass** (one commit, hot-patch + review whole). |
| 2 | Launch interaction | **Click a row to expand an inline detail panel; the primary button (Launch/Connect/Open) launches.** Replaces single-click-launch + the right-click context strip. |
| 3 | "Open on host" chips | **Remote-desktop to the host** (open an RD session to the peer — existing capability), labeled honestly. |
| 4 | Menu size | **460 × 720** (wider than the 384 mockup, taller). |
| 5 | Row avatar | **Letter tile** (mono first-letter on a Carbon-gray square). |
| 6 | Secondary action | **Pin/Unpin everywhere** (universal, wired to favorites). |
| 7 | Footer power | **Full power menu** — Lock / Log out / Suspend / Restart / Shut down (loginctl/systemctl). |
| 8 | Row style | **Zebra shading AND the selected blue left-accent + raised bg** (both). |
| 9 | Theme | **Light + dark adaptive** via mde-theme tokens (follows the MDE theme pref). |
| 10 | Quick links | **Fixed Workbench / Files / Settings** tiles. |

## Layout (top → bottom)
- **Header:** grid glyph + "Applications"; QNM-Shared usage line (mono `used / total`) + a thin progress bar (success-green fill).
- **Quick links:** 3 tiles — icon glyph over label (Workbench / Files / Settings), bordered, hover-raise.
- **Tabs:** Apps / Mesh / Workloads / Services — active = accent underline + bright text.
- **Search:** leading magnifier glyph; clear (✕) when non-empty.
- **Result list (scroll):** zebra rows; each = letter avatar + accent-blue bold title + mono subtitle + status dot. Click toggles an **inline detail** (blue left-accent + raised bg): primary (Launch/Connect/Open) + secondary (Pin/Unpin), and for apps an "Open on host" chip row (mesh peers → remote desktop). Empty state per tab.
- **Toast:** bottom feedback bar (success accent) + dismiss, set on actions.
- **Footer:** operator avatar + name + power button → the power menu popover.

## Acceptance (runtime-observable)
- The menu renders the design at 460×720 through mde-theme tokens (no raw hex, §4); correct in both dark + light.
- A row click expands the detail; the primary launches (local exec / mesh RD / service endpoint) and closes the menu; host chips open an RD session to the chosen peer; Pin/Unpin toggles the favorite; actions raise a toast.
- The power button opens the 5-action power menu; each action runs its loginctl/systemctl command.
- All driven by the existing `action/apps/*` data; lib tests still green.
