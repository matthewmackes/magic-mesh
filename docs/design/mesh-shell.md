# MESHSHELL — Bash mesh prompt, welcome greeting, and cheat sheet

Operator-locked 2026-06-16 (15-question survey). A Carbon-styled bash experience
on every mesh node: a starship prompt listing online nodes, a per-shell welcome
greeting with live mesh state, and a `mesh-help` cheat sheet.

## Locked decisions
| # | Topic | Lock |
|---|-------|------|
| 1 | Prompt engine | **starship** (fast, async, themeable; bash `eval "$(starship init bash)"`). |
| 2 | Prompt content | **Full online-hostname list** rendered in the prompt (Carbon-colored), via a custom starship module. |
| 3 | Prompt data | **Cached snapshot** at `/run/mde/mesh-status.json`, refreshed by a background timer (~30s). Prompt never queries live (no lag). |
| 4 | Shells | **Bash** only. |
| 5 | Greeting trigger | **Every new shell** (profile.d). |
| 6 | Greeting freshness | **Live query, bounded** — kick a live refresh with a hard ~1.5s timeout; fall back to the snapshot so a shell never hangs. |
| 7 | Health display | **Color-coded per-node list** (hostname + overlay IP + green/yellow/red presence dot) **+ summary line** ("4/4 healthy"). |
| 8 | Services display | **Per-node service matrix** (node × {Bus, etcd, Syncthing, Nebula, DNS, Voice, Music, KDC, Workbench}). (Was a single LizardFS column pre-SUBSTRATE-6; the substrate is now etcd coordination + Syncthing files.) |
| 9 | Updates display | **Per-node update list** (installed magic-mesh version + "update available" flag). |
| 10 | Cheat sheet location | Separate **`mesh-help`** command (greeting shows a one-line hint). |
| 11 | Cheat sheet source | **Auto-generated** from verb `--help` (mackesd/meshctl/mde-bus) + curated extras. |
| 12 | Cheat sheet scope | Enrollment & join · Status & health · Storage (Syncthing / `/mnt/mesh-storage`) · Services & logs · **Fedora system commands** (dnf/systemctl/journalctl). |
| 13 | Roles | **All roles** (most useful on the headless lighthouses over SSH). |
| 14 | Delivery | Applied **automatically / one-way** (no toggle). Shipped as its own RPM component enabled on all roles (NOT gated behind the Workstation-only BRAND service); styled to match the Carbon branding. |
| 15 | Style | **Carbon ANSI palette** (Gray + Blue-60 accent) + **Mackes ASCII wordmark** header + Unicode status dots. |

## Data plane
A single snapshot `/run/mde/mesh-status.json` is the source for both the prompt
(cached read) and the greeting (snapshot + bounded live refresh). It carries:
`nodes:[{hostname, overlay_ip, presence}]`, `services:{<node>:{bus,etcd,syncthing,
nebula,dns,voice,music,kdc,workbench}}` (the `lizardfs` flag was replaced by
`etcd`+`syncthing` at SUBSTRATE-6), `versions:{<node>:{installed, latest,
update}}`, `generated_ms`. Written by a small refresher (a `mackesd` tick or a
oneshot+timer) that reads the replicated directory + `mackesd healthz` + the
package channel. All roles run the refresher.

## Components (→ WORKLIST SHELL-1..5)
1. **Snapshot refresher + timer** — writes `/run/mde/mesh-status.json` (~30s), all roles.
2. **starship config + prompt module** — `/etc/starship.toml` custom `mesh` module reads the snapshot, prints the online-hostname list; profile.d does `starship init bash`.
3. **Welcome greeting** — `/etc/profile.d/zz-mde-welcome.sh`: Carbon ASCII wordmark + color-coded health + service matrix + update list, bounded live refresh.
4. **`mesh-help`** — auto-generated cheat sheet from verb `--help` + curated Fedora/system commands, grouped by task.
5. **RPM packaging** — ship 1–4 + the starship binary dependency; enable the timer on all roles in post_install. One-way.

## Acceptance (runtime-observable)
- Opening a bash shell on any node shows the Carbon greeting (wordmark, node
  health, service matrix, updates) within ~1.5s, and a starship prompt listing
  the online hostnames; `mesh-help` prints the grouped cheat sheet.
- The prompt never lags (snapshot-backed); a down mesh never hangs the shell.

## Out of scope
- zsh/fish (bash only). A revert/toggle (one-way).
