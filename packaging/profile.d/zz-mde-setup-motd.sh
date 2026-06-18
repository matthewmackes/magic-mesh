# SETUP-6 — login hint for an unconfigured MCNF node. Shows only until a
# deployment role is pinned; once configured, the mesh-shell greeting (SHELL-3)
# takes over and this is silent. Sourced by /etc/profile.d for every login shell
# (incl. SSH — the console first-run unit handles tty1, this is the fallback).
if [ ! -e /var/lib/mde/role.toml ] && command -v magic-setup >/dev/null 2>&1; then
    printf '\n  \033[1;36mMCNF\033[0m — this node is not configured yet.\n'
    printf '  Run \033[1msudo magic-setup\033[0m to create or join a mesh.\n\n'
fi
