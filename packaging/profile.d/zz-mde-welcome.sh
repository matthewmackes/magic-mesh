# MESHSHELL SHELL-3 — Magic Mesh welcome greeting (interactive bash only).
[ -n "$BASH_VERSION" ] || return 0
case $- in *i*) ;; *) return 0 ;; esac
[ -x /usr/libexec/mackesd/mesh-welcome ] && /usr/libexec/mackesd/mesh-welcome 2>/dev/null || true
