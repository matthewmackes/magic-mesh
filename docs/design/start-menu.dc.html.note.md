# Start Menu design (APPS-STYLE-2)

Source: Claude Design project "Start menu improvement design"
(claude.ai/design/p/45e0d4a5-09a2-4531-8d5d-e6284ecae313, file `Start Menu.dc.html`),
imported 2026-06-18. Implemented in `crates/platform/mde-cosmic-applet/src/bin/mde-apps-applet.rs`.

Layout (Carbon dark mockup; implemented theme-aware via mde-theme tokens):
- Panel: Gray-90 surface, Gray-80 border, drop shadow, bottom-left over the taskbar.
- Header: grid glyph + "Applications"; Mesh Sync usage line (mono) + a thin progress bar (green fill).
- Quick links: Workbench / Files / Settings — icon over label, bordered tiles.
- Line tabs: Apps / Mesh / Workloads / Services — active = Blue underline + bright text.
- Search: leading magnifier, "Search apps, mesh, services…", clear (✕) when non-empty.
- Result rows: 32px letter avatar + Blue-40 bold title + mono subtitle + status dot;
  selected row gets a Blue left-accent + raised bg and expands an inline detail panel:
  primary (Launch/Connect/Open) + secondary action, and for apps an "Open on host"
  chip row (the mesh peers). Click toggles the expansion.
- Toast: bottom feedback bar (green accent) with dismiss.
- Footer: operator avatar + name + power button.
