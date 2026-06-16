# MESHSHELL SHELL-2 — enable the Carbon starship prompt for interactive bash.
# No-op when starship isn't installed (falls back to the stock prompt).
[ -n "$BASH_VERSION" ] || return 0
case $- in *i*) ;; *) return 0 ;; esac
if command -v starship >/dev/null 2>&1; then
  export STARSHIP_CONFIG=/etc/starship.toml
  eval "$(starship init bash)"
fi
