#!/usr/bin/env bash
# verify-rpm-payload.sh — the "compiles ≠ ships" + "shipped-but-dead surface" gate.
#
# WHY (two real, recurring regression classes this static gate prevents):
#
#   (a) COMPILES ≠ SHIPS. The workspace can build 100% green while the RPM omits
#       a binary that a `target/release/…` asset entry promises. This actually
#       shipped once: a strip/replace dropped the iced GUIs but never added the
#       egui shell to the `generate-rpm` assets → 11.4.0 shipped with the shell
#       binary MISSING even though every crate compiled. DoD for a strip/replace
#       is therefore `rpm -qlp <rpm> | grep <replacement-bin>` — this script is
#       that check, plus a static dry-run that needs no RPM.
#
#   (b) SHIPPED-BUT-DEAD SURFACE. A whole `mde-*-egui` surface epic (terminal,
#       media) can build green yet be UNREACHABLE — its lib compiled into no
#       shipped binary and mounted in no surface catalog, with zero launchers. That
#       is a surface that exists in the tree, passes tests, and reaches no user.
#       TERM-16 / MEDIA-18 wired the two that had regressed; this script guards
#       every future `mde-*-egui` the same way.
#
# Neither regression has an automated gate today. This one is STATIC (no build,
# no cargo, no network): it parses the RPM asset manifest out of
# `crates/mesh/mackesd/Cargo.toml` ([package.metadata.generate-rpm].assets plus
# the thin lighthouse variant) and checks the shell's surface catalog. It is fast enough to
# run on every push.
#
# ─────────────────────────────────────────────────────────────────────────────
# HOW TO RUN
#
#   install-helpers/verify-rpm-payload.sh                # dry-run BOTH checks (default)
#   install-helpers/verify-rpm-payload.sh all            # same as no args
#   install-helpers/verify-rpm-payload.sh payload        # RPM-payload check, dry-run (no RPM)
#   install-helpers/verify-rpm-payload.sh payload a.rpm  # validate a REAL built RPM's file list (also size-checks it)
#                                                        # and reject retired Timers payload
#   install-helpers/verify-rpm-payload.sh requirements            # pin source metadata for base VDI-host hard Requires
#   install-helpers/verify-rpm-payload.sh requirements a.rpm      # also inspect a built RPM's actual Requires header
#   install-helpers/verify-rpm-payload.sh size a.rpm     # size-only: fail if a.rpm exceeds the channel ceiling
#   install-helpers/verify-rpm-payload.sh candidate-payload # credential payload in base + lighthouse candidates
#   install-helpers/verify-rpm-payload.sh app-vm-payload [a.rpm] # exact App VM guest bootstrap payload
#   install-helpers/verify-rpm-payload.sh android-vm-payload [a.rpm] # exact Cuttlefish guest role payload
#   install-helpers/verify-rpm-payload.sh overlay-claims-package  # WL-CRIT-007 three-variant package/runtime shape
#   install-helpers/verify-rpm-payload.sh surfaces       # surface-reachability check only
#   install-helpers/verify-rpm-payload.sh --self-test    # exercise the parser on good+broken fixtures
#   install-helpers/verify-rpm-payload.sh --help
#
# Exit code is 0 only when every check passes; any FAIL exits non-zero so the
# script is drop-in for a gate. Output is greppable: each check prints one of
#   [OK] / [FAIL] / [WARN] / [INFO] / [SKIP]
#
# The size-only check is wired into the RPM cut paths. The static payload/surface
# checks remain useful as pre-cut gates and deliberately do NOT cut an RPM or run
# a release; their job is only to VERIFY. The real-RPM mode expects an RPM a
# gated build already produced.
#
# ─────────────────────────────────────────────────────────────────────────────
# DRY-RUN semantics (no RPM):
#   payload  : lists the expected base + thin-lighthouse asset sets;
#              for every asset SOURCE it asserts —
#              * target/…            → some workspace crate builds a bin of that
#                                      name (the static proxy for "the build will
#                                      produce it"); a name nothing builds FAILs.
#              * vendor/birthright/… → fetched+verified at build time; INFO, skipped.
#              * anything else       → the file/glob exists in the tree now; a
#                                      missing packaging source FAILs.
#              Extra hard emphasis on the base replacement bins (mde-shell-egui,
#              mackesd): each MUST appear as a target/release asset in the base
#              asset set.
#   surfaces : every mde-*-egui crate under crates/desktop (minus the shell host
#              and the documented EXEMPT list) MUST be BOTH catalog-mounted (named in
#              the shell's surfaces.rs catalog) AND shipped (a path-dep of
#              mde-shell-egui, whose binary is itself in the asset set). A surface
#              that is one but not the other FAILs.
#
# Real-RPM semantics (`payload <rpm>`): runs `rpm -qlp <rpm>` and asserts every
# expected install path is in the payload (globs are checked best-effort by dest
# prefix; the key bins are checked exactly), runs `rpm -qp --requires <rpm>` on
# the base package to prove its required KVM host packages reached the actual
# header, then ALSO size-checks the file.
#
# Size semantics (`size <rpm>`, build-deploy-12): the public dnf channel is served
# from GitHub Pages (packaging/repo/magic-mesh.repo), a git branch, so the pushed
# .rpm FILE is subject to GitHub's ~100 MiB hard per-file block. This check
# measures the COMPRESSED .rpm file (wc -c — the bytes actually pushed, not the
# uncompressed payload) and
# FAILs if it exceeds MCNF_RPM_SIZE_LIMIT_MIB (default 90 MiB — headroom under even
# the strict 100 MB=95.37 MiB reading). Both base and lighthouse RPM cuts call it so
# the channel cannot be silently broken.
#
# ─────────────────────────────────────────────────────────────────────────────
# EXEMPT surface crates — mde-*-egui crates under crates/desktop that are NOT
# launchable surfaces and so are not required to be catalog-mounted. Keep this list SHORT and
# justify every entry; the whole point of the gate is that new surfaces cannot
# silently join this set.
# (mde-shell-egui is the shell HOST itself, handled separately — never a surface.)
#
# Env overrides (mostly for --self-test; default to the live repo layout):
#   CARGO_TOML   RPM manifest         (default crates/mesh/mackesd/Cargo.toml)
#   SHELL_CARGO  shell manifest       (default crates/desktop/mde-shell-egui/Cargo.toml)
#   SURFACES_RS  surface catalog      (default crates/desktop/mde-shell-egui/src/surfaces.rs)
#   DESKTOP_DIR  surface-crate dir    (default crates/desktop)
#   REPO_ROOT    tree root for assets (default: the git worktree this script is in)
#   MCNF_FAKE_RPM_LIST  a file whose lines stand in for `rpm -qlp` (real-RPM test hook)
#   MCNF_FAKE_RPM_REQUIRES  a file whose lines stand in for `rpm -qp --requires`
#                           (self-test hook only)
#   MCNF_RPM_SIZE_LIMIT_MIB  size-gate ceiling in MiB (default 90; build-deploy-12)
set -uo pipefail
shopt -s globstar nullglob

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$HERE/.." && pwd)}"
CARGO_TOML="${CARGO_TOML:-$REPO_ROOT/crates/mesh/mackesd/Cargo.toml}"
SHELL_CARGO="${SHELL_CARGO:-$REPO_ROOT/crates/desktop/mde-shell-egui/Cargo.toml}"
SURFACES_RS="${SURFACES_RS:-$REPO_ROOT/crates/desktop/mde-shell-egui/src/surfaces.rs}"
DESKTOP_DIR="${DESKTOP_DIR:-$REPO_ROOT/crates/desktop}"

# The shell HOST (never a surface) and the justified non-surface egui crates.
readonly SHELL_HOST_CRATE="mde-shell-egui"
EXEMPT_SURFACES=()

readonly BASE_KEY_BINS=("mde-shell-egui" "mackesd")
readonly SERVER_KEY_BINS=("mackesd")
readonly LIGHTHOUSE_KEY_BINS=("mackesd")
readonly GROUPED_MACKESD_ASSETS=(
  "packaging/systemd/mackesd.target"
  "packaging/systemd/mackesd-control.service"
  "packaging/systemd/mackesd-observation.service"
  "packaging/systemd/mackesd-actions.service"
  "packaging/systemd/mackesd-data.service"
  "packaging/systemd/mackesd-compute.service"
  "packaging/systemd/mackesd-integrations.service"
)
readonly CANDIDATE_CREDENTIAL_ASSETS=(
  "install-helpers/provision-resource-publisher-credential.sh|/usr/libexec/mackesd/provision-resource-publisher-credential"
  "packaging/systemd/resource-publisher-hmac.conf|/usr/libexec/mackesd/resource-publisher-hmac.conf"
  "packaging/systemd/mcnf-resource-publisher-credential.service|/usr/lib/systemd/system/mcnf-resource-publisher-credential.service"
)
readonly BASE_VDI_HOST_REQUIRES=(
  "libvirt"
  "qemu-kvm"
  "qemu-ui-dbus"
  "libvirt-daemon-kvm"
  "libvirt-daemon-driver-storage"
)
readonly BUILT_RPM_KVM_REQUIRES=(
  "qemu-kvm"
  "qemu-ui-dbus"
  "libvirt-daemon-kvm"
)
readonly APP_VM_BOOTSTRAP_SOURCE="infra/tofu/cloud/cloud-init/mesh-join.yaml.tftpl"
readonly APP_VM_BOOTSTRAP_DEST="/usr/share/mde/iac/infra/tofu/cloud/cloud-init/mesh-join.yaml.tftpl"
readonly APP_VM_BOOTSTRAP_MARKERS=(
  "path: /etc/mackesd/app-vm/guest-profile"
  "path: /etc/systemd/system/mcnf-app-vm-runtime.service"
  "ExecStart=/usr/local/libexec/mcnf-app-vm-runtime"
  "path: /usr/local/libexec/mcnf-app-vm-runtime"
)
readonly ANDROID_VM_PAYLOAD_MEMBERS=(
  "automation/ansible/playbooks/site.yml|/usr/share/mde/iac/automation/ansible/playbooks/site.yml"
  "automation/ansible/roles/cuttlefish_host/defaults/main.yml|/usr/share/mde/iac/automation/ansible/roles/cuttlefish_host/defaults/main.yml"
  "automation/ansible/roles/cuttlefish_host/meta/main.yml|/usr/share/mde/iac/automation/ansible/roles/cuttlefish_host/meta/main.yml"
  "automation/ansible/roles/cuttlefish_host/tasks/main.yml|/usr/share/mde/iac/automation/ansible/roles/cuttlefish_host/tasks/main.yml"
)
readonly ANDROID_VM_PLAYBOOK_MARKERS=(
  "hosts: delivery_android_vm"
  "role: cuttlefish_host"
)
readonly ANDROID_VM_PROFILE_MARKERS=(
  "cuttlefish_user: cvd"
  "- kvm"
  "- cvdnetwork"
)
readonly ANDROID_VM_RUNTIME_MARKERS=(
  "path: /dev/kvm"
  "cvd version"
  "cvd start --start_vnc_server"
)

# build-deploy-12 — the RPM-size ceiling. The gh-pages dnf channel is a git branch,
# so the pushed .rpm file hits GitHub's ~100 MiB hard per-file block. Fail a cut with
# headroom (default 90 MiB) so a growth step is caught at cut time, not publish time.
RPM_SIZE_LIMIT_MIB="${MCNF_RPM_SIZE_LIMIT_MIB:-90}"

FAILS=0
ok()   { printf '[OK]   %s\n' "$*"; }
fail() { printf '[FAIL] %s\n' "$*"; FAILS=$((FAILS + 1)); }
info() { printf '[INFO] %s\n' "$*"; }
skip() { printf '[SKIP] %s\n' "$*"; }
hdr()  { printf '\n=== %s ===\n' "$*"; }

# ── parse_assets_for <cargo.toml> <table> ────────────────────────────────────
# Emit one "source<TAB>dest" line per asset in <table>.assets. Stops at the
# asset array's closing `]` so nested variant tables do not leak into each other.
parse_assets_for() {
  awk '
    $0 == "[" section "]" { main = 1; next }
    main && /^\[/          { exit }
    main && /^assets = \[/ { ina = 1; next }
    ina {
      if ($0 ~ /^[[:space:]]*\][[:space:]]*$/) { exit }
      if ($0 ~ /source[[:space:]]*=/) {
        src = $0; sub(/.*source[[:space:]]*=[[:space:]]*"/, "", src); sub(/".*/, "", src)
        dst = ""
        if ($0 ~ /dest[[:space:]]*=/) { dst = $0; sub(/.*dest[[:space:]]*=[[:space:]]*"/, "", dst); sub(/".*/, "", dst) }
        print src "\t" dst
      }
    }
  ' section="$2" "$1"
}

# Main/base, server variant, and thin lighthouse variant
# asset readers.
parse_assets() { parse_assets_for "$1" "package.metadata.generate-rpm"; }
# shellcheck disable=SC2317 # invoked indirectly by name in the variant loop
parse_server_assets() { parse_assets_for "$1" "package.metadata.generate-rpm.variants.server"; }
parse_lighthouse_assets() { parse_assets_for "$1" "package.metadata.generate-rpm.variants.lighthouse"; }
parse_all_shipped_assets() {
  parse_assets "$1"
  parse_lighthouse_assets "$1"
}

# WL-ARCH-009 — variants replace the base asset array, so every RPM shape must
# independently ship the target and all six group units. The retired monolith
# must not survive in any manifest table.
check_grouped_mackesd_assets() {
  hdr "grouped mackesd process boundary — every RPM variant"
  local label parser source dest expected expected_dest
  for label in base server lighthouse; do
    case "$label" in
      base) parser=parse_assets ;;
      server) parser=parse_server_assets ;;
      lighthouse) parser=parse_lighthouse_assets ;;
    esac
    local -A present=()
    while IFS=$'\t' read -r source dest; do
      [ -n "$source" ] && present["$source"]="$dest"
    done < <($parser "$CARGO_TOML")
    for expected in "${GROUPED_MACKESD_ASSETS[@]}"; do
      expected_dest="/usr/lib/systemd/system/${expected##*/}"
      if [ "${present["$expected"]:-}" = "$expected_dest" ]; then
        ok "$label asset    $expected -> $expected_dest"
      else
        fail "$label asset    $expected MISSING or has the wrong destination"
      fi
    done
    if [ -n "${present["packaging/systemd/mackesd.service"]:-}" ]; then
      fail "$label asset    retired packaging/systemd/mackesd.service remains"
    else
      ok "$label asset    retired mackesd.service absent"
    fi
  done

  local lifecycle_token
  for lifecycle_token in \
    'systemctl disable --now mackesd.service' \
    '/etc/systemd/system/mackesd.service.d/50-cloud-arm-credential.conf' \
    '/usr/lib/systemd/system/mackesd.service' \
    'systemctl enable mackesd.target' \
    'systemctl start mackesd.target'; do
    if [[ "$lifecycle_token" == 'systemctl enable mackesd.target' ]]; then
      # The post-install script may enable mackesd.target in the same
      # systemctl invocation as the other boot units; require the target as
      # an enabled argument, not an exact three-token command.
      if grep -Eq 'systemctl enable([^#]*[[:space:]])mackesd\.target([[:space:]]|$)' "$CARGO_TOML"; then
        ok "upgrade lifecycle contains: $lifecycle_token"
      else
        fail "upgrade lifecycle MISSING: $lifecycle_token"
      fi
    elif grep -Fq "$lifecycle_token" "$CARGO_TOML"; then
      ok "upgrade lifecycle contains: $lifecycle_token"
    else
      fail "upgrade lifecycle MISSING: $lifecycle_token"
    fi
  done
}

# WL-CRIT-006 — both production roles publish or validate governed resource
# catalog state. Variant asset arrays replace the base list, so make the
# credential materializer an explicit candidate invariant instead of relying on
# a broad file-exists scan that cannot detect a dropped lighthouse row.
check_candidate_credential_assets() {
  hdr "candidate resource-publisher credential payload — production roles"
  local label parser source dest pair expected expected_dest
  for label in base lighthouse; do
    case "$label" in
      base) parser=parse_assets ;;
      lighthouse) parser=parse_lighthouse_assets ;;
    esac
    local -A present=()
    while IFS=$'\t' read -r source dest; do
      [ -n "$source" ] && present["$source"]="$dest"
    done < <($parser "$CARGO_TOML")
    for pair in "${CANDIDATE_CREDENTIAL_ASSETS[@]}"; do
      expected="${pair%%|*}"
      expected_dest="${pair#*|}"
      if [ "${present["$expected"]:-}" = "$expected_dest" ]; then
        ok "$label candidate $expected -> $expected_dest"
      else
        fail "$label candidate $expected MISSING or has the wrong destination"
      fi
    done
  done
}

# Emit each direct key in a TOML table. This intentionally stops at the next
# table so a package present only in Recommends cannot satisfy a hard-Requires
# assertion.
parse_table_keys() {
  awk '
    $0 == "[" section "]" { in_table = 1; next }
    in_table && /^\[/ { exit }
    in_table {
      line = $0
      sub(/[[:space:]]*#.*/, "", line)
      if (line ~ /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/) {
        sub(/^[[:space:]]*/, "", line)
        sub(/[[:space:]]*=.*/, "", line)
        print line
      }
    }
  ' section="$2" "$1"
}

check_vdi_host_requires() {
  hdr "VDI host runtime — base RPM hard Requires"
  local -A seen=()
  local package
  while IFS= read -r package; do
    [ -n "$package" ] && seen["$package"]=1
  done < <(parse_table_keys "$CARGO_TOML" "package.metadata.generate-rpm.requires")

  for package in "${BASE_VDI_HOST_REQUIRES[@]}"; do
    if [ -n "${seen["$package"]:-}" ]; then
      ok "hard-requires  $package"
    else
      fail "hard-requires  $package MISSING from [package.metadata.generate-rpm.requires]"
    fi
  done
}

# WL-FUNC-018 — the App VM guest runtime is bootstrapped from one cloud-init
# template. The generate-rpm manifest deliberately ships the cloud-init
# directory through a glob, but a generic "destination has at least one file"
# check cannot detect this particular template disappearing from the source or
# a built RPM. Pin both the exact member and its runtime/profile/service
# contract so an otherwise non-empty glob cannot produce a green release gate.
check_app_vm_payload() {
  local rpm="${1:-}" source dest marker listing="" covered=0
  hdr "App VM guest bootstrap — exact release payload"

  if [ ! -f "$REPO_ROOT/$APP_VM_BOOTSTRAP_SOURCE" ]; then
    fail "app-vm source  $APP_VM_BOOTSTRAP_SOURCE MISSING"
    return
  fi

  while IFS=$'\t' read -r source dest; do
    [ -n "$source" ] || continue
    if [[ "$APP_VM_BOOTSTRAP_SOURCE" == $source ]]; then
      case "$dest" in
        */) [ "${dest}${APP_VM_BOOTSTRAP_SOURCE##*/}" = "$APP_VM_BOOTSTRAP_DEST" ] && covered=1 ;;
        *)  [ "$dest" = "$APP_VM_BOOTSTRAP_DEST" ] && covered=1 ;;
      esac
    fi
  done < <(parse_assets "$CARGO_TOML")
  if [ "$covered" -eq 1 ]; then
    ok "app-vm manifest $APP_VM_BOOTSTRAP_SOURCE -> $APP_VM_BOOTSTRAP_DEST"
  else
    fail "app-vm manifest $APP_VM_BOOTSTRAP_SOURCE is not covered at its canonical destination"
  fi

  for marker in "${APP_VM_BOOTSTRAP_MARKERS[@]}"; do
    if grep -Fq -- "$marker" "$REPO_ROOT/$APP_VM_BOOTSTRAP_SOURCE"; then
      ok "app-vm contract $marker"
    else
      fail "app-vm contract MISSING from $APP_VM_BOOTSTRAP_SOURCE: $marker"
    fi
  done

  [ -n "$rpm" ] || return
  if [ -z "${MCNF_FAKE_RPM_LIST:-}" ] && [ ! -f "$rpm" ]; then
    fail "app-vm RPM not found: $rpm"
    return
  fi
  if ! listing="$(rpm_file_list "$rpm")"; then
    fail "app-vm could not read file list from $rpm"
    return
  fi
  if grep -Fxq "$APP_VM_BOOTSTRAP_DEST" <<<"$listing"; then
    ok "app-vm payload  $APP_VM_BOOTSTRAP_DEST present in rpm -qlp"
  else
    fail "app-vm payload  $APP_VM_BOOTSTRAP_DEST MISSING from the RPM payload"
  fi
}

# WL-FUNC-020 — Android provisioning is not carried by the App VM cloud-init
# template. It is a distinct Ansible specialization: site.yml admits the
# delivery_android_vm inventory group, defaults/main.yml is the guest profile,
# and tasks/main.yml establishes nested KVM and starts cvd. These files are
# shipped through broad playbook/role globs, so a non-empty unrelated Ansible
# payload can otherwise hide their removal. Pin every exact member in both RPM
# roles that can host Android compute, plus the contract that makes the files
# operational rather than inert documentation.
check_android_vm_payload() {
  local rpm="${1:-}" pair member install_path marker listing=""
  local label parser source dest covered
  hdr "Android VM guest bootstrap/runtime/profile — exact release payload"

  for pair in "${ANDROID_VM_PAYLOAD_MEMBERS[@]}"; do
    member="${pair%%|*}"
    install_path="${pair#*|}"
    if [ -f "$REPO_ROOT/$member" ]; then
      ok "android source  $member"
    else
      fail "android source  $member MISSING"
    fi

    for label in base server; do
      case "$label" in
        base) parser=parse_assets ;;
        server) parser=parse_server_assets ;;
      esac
      covered=0
      while IFS=$'\t' read -r source dest; do
        [ -n "$source" ] || continue
        case "$member" in
          $source)
            case "$dest" in
              */playbooks/) [[ "$install_path" == "$dest"* ]] && covered=1 ;;
              */roles/) [[ "$install_path" == "$dest"* ]] && covered=1 ;;
            esac
            ;;
        esac
      done < <($parser "$CARGO_TOML")
      if [ "$covered" -eq 1 ]; then
        ok "android $label manifest covers $member -> $install_path"
      else
        fail "android $label manifest does not cover $member at $install_path"
      fi
    done
  done

  for marker in "${ANDROID_VM_PLAYBOOK_MARKERS[@]}"; do
    if grep -Fq -- "$marker" "$REPO_ROOT/automation/ansible/playbooks/site.yml"; then
      ok "android bootstrap $marker"
    else
      fail "android bootstrap marker MISSING: $marker"
    fi
  done
  for marker in "${ANDROID_VM_PROFILE_MARKERS[@]}"; do
    if grep -Fq -- "$marker" "$REPO_ROOT/automation/ansible/roles/cuttlefish_host/defaults/main.yml"; then
      ok "android profile $marker"
    else
      fail "android profile marker MISSING: $marker"
    fi
  done
  for marker in "${ANDROID_VM_RUNTIME_MARKERS[@]}"; do
    if grep -Fq -- "$marker" "$REPO_ROOT/automation/ansible/roles/cuttlefish_host/tasks/main.yml"; then
      ok "android runtime $marker"
    else
      fail "android runtime marker MISSING: $marker"
    fi
  done

  [ -n "$rpm" ] || return
  if [ -z "${MCNF_FAKE_RPM_LIST:-}" ] && [ ! -f "$rpm" ]; then
    fail "android RPM not found: $rpm"
    return
  fi
  if ! listing="$(rpm_file_list "$rpm")"; then
    fail "android could not read file list from $rpm"
    return
  fi
  for pair in "${ANDROID_VM_PAYLOAD_MEMBERS[@]}"; do
    install_path="${pair#*|}"
    if grep -Fxq "$install_path" <<<"$listing"; then
      ok "android payload $install_path present in rpm -qlp"
    else
      fail "android payload $install_path MISSING from the RPM payload"
    fi
  done
}

# Read the actual RPM Requires header (or the self-test fixture). Production
# validation intentionally uses the exact query form required by the gate.
rpm_requires_header() {
  if [ -n "${MCNF_FAKE_RPM_REQUIRES:-}" ]; then
    cat "$MCNF_FAKE_RPM_REQUIRES"
  else
    rpm -qp --requires "$1"
  fi
}

check_built_rpm_vdi_host_requires() {
  local rpm="$1"
  hdr "VDI host runtime — built RPM Requires header: $rpm"
  if [ -z "${MCNF_FAKE_RPM_REQUIRES:-}" ] && [ ! -f "$rpm" ]; then
    fail "actual-requires RPM not found: $rpm"
    return
  fi

  local requires
  if ! requires="$(rpm_requires_header "$rpm" 2>/dev/null)"; then
    fail "actual-requires could not read the built RPM Requires header with rpm -qp --requires"
    return
  fi

  local package
  for package in "${BUILT_RPM_KVM_REQUIRES[@]}"; do
    # Match the dependency name as the first RPM expression token. This accepts
    # normal versioned hard requirements (`name >= version`) but rejects a
    # similarly named capability or a package present only as a weak dependency.
    if awk -v wanted="$package" '$1 == wanted { found = 1 } END { exit !found }' <<<"$requires"; then
      ok "actual-requires $package present in rpm -qp --requires"
    else
      fail "actual-requires $package MISSING from the built RPM Requires header"
    fi
  done
}

# Does a path contain a glob metacharacter?
has_glob() { case "$1" in *[\*\?\[]*) return 0 ;; *) return 1 ;; esac; }

# Does ANY workspace crate build a binary of this exact name? (static proxy for
# "the release build will produce target/release/<name>")
crate_builds_bin() {
  grep -rlqE "name = \"$1\"" --include=Cargo.toml "$REPO_ROOT/crates" 2>/dev/null
}

# ── surface universe ─────────────────────────────────────────────────────────
# All mde-*-egui crate dirs under DESKTOP_DIR, basename only.
list_surface_crates() {
  local d
  for d in "$DESKTOP_DIR"/mde-*-egui; do
    [ -d "$d" ] && printf '%s\n' "${d##*/}"
  done
}
is_exempt() {
  local c="$1" e
  for e in "${EXEMPT_SURFACES[@]}"; do [ "$c" = "$e" ] && return 0; done
  return 1
}
# Is crate a path-dependency of the shell? (i.e. compiled INTO the shipped shell)
shell_depends_on() {
  grep -qE "^[[:space:]]*$1[[:space:]]*=[[:space:]]*\{[[:space:]]*path" "$SHELL_CARGO"
}
# Is the crate named in the canonical surface catalog?
surface_catalog_mounts() { grep -qF "$1" "$SURFACES_RS"; }

# ═════════════════════════════════════════════════════════════════════════════
# CHECK 1 — RPM payload (build-deploy-2)
# ═════════════════════════════════════════════════════════════════════════════
check_payload_dryrun() {
  hdr "payload (dry-run, static) — manifest: ${CARGO_TOML#"$REPO_ROOT"/}"
  local total=0 src dst
  # collect the parsed source set (for the key-bin emphasis at the end)
  local -A src_seen=()
  while IFS=$'\t' read -r src dst; do
    [ -n "$src" ] || continue
    total=$((total + 1))
    src_seen["$src"]=1
    case "$src" in
      target/*)
        local bin="${src##*/}"
        if crate_builds_bin "$bin"; then
          ok "build-output   $src  (a workspace crate builds '$bin')"
        else
          fail "build-output   $src  → NO workspace crate builds a binary named '$bin' (RPM would ship a bin nothing produces)"
        fi
        ;;
      vendor/birthright/*)
        info "build-fetch    $src  (fetched+sha256-verified at build by vendor-birthright-blobs.sh; skipped)"
        ;;
      *)
        if has_glob "$src"; then
          # shellcheck disable=SC2206  # intentional glob expansion (globstar/nullglob on)
          local m=( $REPO_ROOT/$src )
          if [ "${#m[@]}" -gt 0 ]; then
            ok "repo-glob      $src  (${#m[@]} match(es))"
          else
            fail "repo-glob      $src  → matches NOTHING in the tree (packaging source vanished)"
          fi
        else
          if [ -e "$REPO_ROOT/$src" ]; then
            ok "repo-file      $src"
          else
            fail "repo-file      $src  → MISSING from the tree (asset source absent)"
          fi
        fi
        ;;
    esac
  done < <(parse_all_shipped_assets "$CARGO_TOML")

  info "parsed $total asset entries from the base + thin lighthouse generate-rpm arrays"

  hdr "key replacement binaries (must be shipped)"
  local kb
  for kb in "${BASE_KEY_BINS[@]}"; do
    if [ -n "${src_seen["target/release/$kb"]:-}" ]; then
      ok "key-bin        target/release/$kb  is in the asset set"
    else
      fail "key-bin        target/release/$kb  is NOT in the asset set (the exact 'compiles ≠ ships' regression)"
    fi
  done

  check_vdi_host_requires
  check_app_vm_payload
  check_android_vm_payload
  check_grouped_mackesd_assets
  check_candidate_credential_assets
}

# Read the RPM file list (real, or a fake listing for --self-test).
rpm_file_list() {
  if [ -n "${MCNF_FAKE_RPM_LIST:-}" ]; then
    cat "$MCNF_FAKE_RPM_LIST"
  else
    rpm -qlp "$1"
  fi
}

# ── check_rpm_size <rpm> ──────────────────────────────────────────────────────
# build-deploy-12 — assert the COMPRESSED .rpm file fits under the GitHub-Pages
# channel ceiling. Measures the real file (wc -c: the bytes actually pushed), not
# the uncompressed payload. FAILs over MCNF_RPM_SIZE_LIMIT_MIB (default 90 MiB).
check_rpm_size() {
  local rpm="$1"
  hdr "payload size — ${rpm} (limit ${RPM_SIZE_LIMIT_MIB} MiB; GitHub channel ~100 MiB ceiling)"
  if [ ! -f "$rpm" ]; then
    fail "size           RPM not found for size check: $rpm"
    return
  fi
  local bytes limit_bytes mib
  bytes="$(wc -c <"$rpm" | tr -d '[:space:]')"
  limit_bytes=$(( RPM_SIZE_LIMIT_MIB * 1024 * 1024 ))
  mib="$(awk -v b="$bytes" 'BEGIN { printf "%.1f", b / 1048576 }')"
  if [ "$bytes" -le "$limit_bytes" ]; then
    ok "size           ${rpm##*/} is ${mib} MiB (within the ${RPM_SIZE_LIMIT_MIB} MiB cut limit)"
  else
    fail "size           ${rpm##*/} is ${mib} MiB — EXCEEDS the ${RPM_SIZE_LIMIT_MIB} MiB cut limit (the gh-pages channel breaks at GitHub's ~100 MiB per-file block; split the payload — see docs/design/rpm-size-split.md — or promote the sovereign channel before publishing)"
  fi
}

check_payload_rpm() {
  local rpm="$1"
  hdr "payload (real RPM) — $rpm"
  if [ -z "${MCNF_FAKE_RPM_LIST:-}" ] && [ ! -f "$rpm" ]; then
    fail "RPM not found: $rpm"
    return
  fi
  local listing
  if ! listing="$(rpm_file_list "$rpm")"; then
    fail "could not read file list from $rpm"
    return
  fi

  local shape="base"
  case "${rpm##*/}" in
    magic-mesh-server-*) shape="server" ;;
    magic-mesh-lighthouse-*) shape="lighthouse" ;;
  esac

  check_grouped_mackesd_assets
  check_candidate_credential_assets

  # Key bins: exact install-path assertions (the DoD line for a strip/replace).
  hdr "key replacement binaries present in payload"
  local kb want
  local -a key_bins=()
  case "$shape" in
    server)  key_bins=("${SERVER_KEY_BINS[@]}") ;;
    lighthouse) key_bins=("${LIGHTHOUSE_KEY_BINS[@]}") ;;
    *)       key_bins=("${BASE_KEY_BINS[@]}") ;;
  esac
  for kb in "${key_bins[@]}"; do
    want="/usr/bin/$kb"
    if grep -Fxq "$want" <<<"$listing"; then
      ok "key-bin        $want present in ${shape} rpm -qlp"
    else
      fail "key-bin        $want MISSING from the ${shape} RPM payload"
    fi
  done

  hdr "every manifest asset present in payload"
  local src dst
  local asset_stream
  case "$shape" in
    server)  asset_stream="parse_server_assets" ;;
    lighthouse) asset_stream="parse_lighthouse_assets" ;;
    *)       asset_stream="parse_assets" ;;
  esac
  while IFS=$'\t' read -r src dst; do
    if [ -z "$src" ] || [ -z "$dst" ]; then continue; fi
    if has_glob "$src"; then
      # Best-effort: assert the dest directory prefix has at least one entry.
      local pref="${dst%/}/"
      if grep -Fq "$pref" <<<"$listing"; then
        ok "glob-dest      $src → $pref (present)"
      else
        fail "glob-dest      $src → $pref has NO entries in the payload"
      fi
    else
      local want_path
      case "$dst" in
        */) want_path="${dst}${src##*/}" ;;
        *)  want_path="$dst" ;;
      esac
      if grep -Fxq "$want_path" <<<"$listing"; then
        ok "asset          $want_path"
      else
        fail "asset          $want_path MISSING (source $src)"
      fi
    fi
  done < <($asset_stream "$CARGO_TOML")

  if [ "$shape" = "base" ]; then
    check_built_rpm_vdi_host_requires "$rpm"
    check_app_vm_payload "$rpm"
    check_android_vm_payload "$rpm"
  elif [ "$shape" = "server" ]; then
    check_android_vm_payload "$rpm"
  else
    info "actual-requires KVM host check applies to the base RPM (shape is $shape)"
  fi

  # Validating a REAL RPM also asserts it fits the channel ceiling (build-deploy-12).
  # Skip under the fake-list self-test hook (no real file to measure).
  if [ -z "${MCNF_FAKE_RPM_LIST:-}" ] && [ -f "$rpm" ]; then
    check_rpm_size "$rpm"
  fi
}

check_payload() {
  if [ -n "${1:-}" ]; then
    check_payload_rpm "$1"
  else
    check_payload_dryrun
  fi
}

# WL-CRIT-007 — focused package/runtime-shape gate for the authenticated local
# claim-snapshot prerequisite. cargo-generate-rpm variant asset arrays replace
# the base array, so every role must carry all four files independently. This
# gate also prevents a packaging-only change from accidentally activating the
# post-overlay producer or the pre-Nebula guard while the typed transport blocker
# remains open.
check_overlay_claims_package() {
  hdr "overlay identity claim prerequisite — three RPM variants"
  local output rc
  output="$(python3 - "$CARGO_TOML" "$REPO_ROOT" <<'PY'
from __future__ import annotations

import pathlib
import sys
import tomllib

manifest_path = pathlib.Path(sys.argv[1])
repo = pathlib.Path(sys.argv[2])
data = tomllib.loads(manifest_path.read_text())
rpm = data["package"]["metadata"]["generate-rpm"]

expected = (
    (
        "install-helpers/mcnf-overlay-identity-claims-materializer.py",
        "/usr/libexec/mackesd/mcnf-overlay-identity-claims-materializer.py",
        "755",
    ),
    (
        "install-helpers/mcnf-overlay-identity-collision-guard.py",
        "/usr/libexec/mackesd/mcnf-overlay-identity-collision-guard.py",
        "755",
    ),
    (
        "packaging/systemd/mcnf-overlay-identity-claims-materializer.service",
        "/usr/lib/systemd/system/mcnf-overlay-identity-claims-materializer.service",
        "644",
    ),
    (
        "packaging/systemd/nebula.service.d/05-overlay-identity-collision-guard.conf",
        "/usr/lib/systemd/system/nebula.service.d/05-overlay-identity-collision-guard.conf",
        "644",
    ),
    (
        "install-helpers/verify-boot-recovery.sh",
        "/usr/libexec/mackesd/verify-boot-recovery",
        "755",
    ),
    (
        "install-helpers/mesh-peer-recovery.sh",
        "/usr/libexec/mackesd/mesh-peer-recovery",
        "755",
    ),
    (
        "install-helpers/mesh-xdg-bind-recovery.sh",
        "/usr/libexec/mackesd/mesh-xdg-bind-recovery",
        "755",
    ),
    (
        "packaging/systemd/mcnf-peer-recovery.service",
        "/usr/lib/systemd/system/mcnf-peer-recovery.service",
        "644",
    ),
    (
        "packaging/systemd/mcnf-xdg-bind-recovery.service",
        "/usr/lib/systemd/system/mcnf-xdg-bind-recovery.service",
        "644",
    ),
    (
        "packaging/systemd/mcnf-peer-recovery-sleep",
        "/usr/lib/systemd/system-sleep/mcnf-peer-recovery",
        "755",
    ),
    (
        "packaging/systemd/90-mcnf-peer-recovery",
        "/etc/NetworkManager/dispatcher.d/90-mcnf-peer-recovery",
        "755",
    ),
)
shapes = {
    "base": rpm,
    "server": rpm["variants"]["server"],
    "lighthouse": rpm["variants"]["lighthouse"],
}
errors: list[str] = []
for shape, table in shapes.items():
    assets = table.get("assets")
    if not isinstance(assets, list):
        errors.append(f"{shape}: assets is not an array")
        continue
    for source, dest, mode in expected:
        matches = [
            asset
            for asset in assets
            if isinstance(asset, dict)
            and asset.get("source") == source
            and asset.get("dest") == dest
            and str(asset.get("mode")) == mode
        ]
        if len(matches) != 1:
            errors.append(
                f"{shape}: expected exactly one {source} -> {dest} mode={mode}; "
                f"found {len(matches)}"
            )

    lifecycle = "\n".join(
        str(table.get(key, ""))
        for key in ("post_install_script", "pre_uninstall_script", "post_uninstall_script")
    )
    if "mcnf-overlay-identity-claims-materializer" in lifecycle:
        errors.append(f"{shape}: package lifecycle activates/references the disabled producer")
    if "05-overlay-identity-collision-guard" in lifecycle:
        errors.append(f"{shape}: package lifecycle mutates the systemd-owned guard drop-in")
    print(f"{shape}: eleven identity/recovery assets present exactly once; lifecycle mutation absent")

for source, _dest, _mode in expected:
    if not (repo / source).is_file():
        errors.append(f"source file missing: {source}")

materializer = (repo / expected[0][0]).read_text()
guard = (repo / expected[1][0]).read_text()
unit = (repo / expected[2][0]).read_text()
dropin = (repo / expected[3][0]).read_text()
private_snapshot = "/var/lib/mackesd/overlay-identity-claims/active-claims.json"
if private_snapshot not in materializer or private_snapshot not in guard:
    errors.append("producer and guard defaults do not share the dedicated private snapshot path")
required_unit_lines = {
    "StateDirectory=mackesd/overlay-identity-claims",
    "StateDirectoryMode=0700",
    "ReadWritePaths=/var/lib/mackesd/overlay-identity-claims /run/mackesd",
    "ExecStart=/usr/libexec/mackesd/mcnf-overlay-identity-claims-materializer.py",
}
unit_lines = set(unit.splitlines())
for line in sorted(required_unit_lines - unit_lines):
    errors.append(f"materializer unit missing exact private-state contract: {line}")
if "ReadWritePaths=/var/lib/mackesd /run/mackesd" in unit_lines:
    errors.append("materializer unit still grants write access to shared /var/lib/mackesd")
blocker = "ACTIVATION_BLOCKER=pre-nebula-current-authority-transport-unavailable"
if blocker not in unit:
    errors.append("distributed producer transport activation blocker is missing")
if any(line.strip() == "[Install]" for line in unit.splitlines()):
    errors.append("materializer unit gained an [Install] activation section")
required_dropin_lines = {
    "[Unit]",
    "Requires=network-online.target",
    "After=network-online.target",
    "Before=etcd.service syncthing.service mackesd.target",
    "[Service]",
    "ExecStartPre=/usr/libexec/mackesd/verify-boot-recovery --identity-guard",
}
dropin_lines = set(dropin.splitlines())
for line in sorted(required_dropin_lines - dropin_lines):
    errors.append(f"active local identity guard missing exact contract: {line}")
if "mcnf-overlay-identity-claims-materializer" in dropin:
    errors.append("pre-Nebula local guard depends on the post-overlay materializer")

recovery_unit = (repo / "packaging/systemd/mcnf-peer-recovery.service").read_text()
recovery_helper = (repo / "install-helpers/mesh-peer-recovery.sh").read_text()
required_recovery_unit_lines = {
    "Type=notify",
    "NotifyAccess=all",
    "ExecStart=/usr/libexec/mackesd/mesh-peer-recovery",
    "TimeoutStartSec=90",
    "RuntimeMaxSec=90",
    "TimeoutStopSec=10",
    "RuntimeDirectory=mcnf-peer-recovery",
    "Environment=MCNF_RECOVERY_LOCK=/run/mcnf-peer-recovery/recovery.lock",
    "ProtectHome=yes",
}
recovery_unit_lines = set(recovery_unit.splitlines())
for line in sorted(required_recovery_unit_lines - recovery_unit_lines):
    errors.append(f"peer recovery unit missing exact bounded-runtime contract: {line}")
if "[Install]" in recovery_unit_lines:
    errors.append("event-triggered peer recovery unit must not be independently enabled")
xdg_recovery_unit = (repo / "packaging/systemd/mcnf-xdg-bind-recovery.service").read_text()
required_xdg_unit_lines = {
    "Type=oneshot",
    "ExecStart=/usr/libexec/mackesd/mesh-xdg-bind-recovery",
    "TimeoutStartSec=60",
}
xdg_recovery_unit_lines = set(xdg_recovery_unit.splitlines())
for line in sorted(required_xdg_unit_lines - xdg_recovery_unit_lines):
    errors.append(f"XDG recovery unit missing exact host-namespace contract: {line}")
for forbidden_namespace_key in (
    "ProtectSystem=", "ProtectHome=", "PrivateTmp=", "ReadOnlyPaths=", "ReadWritePaths="
):
    if any(line.startswith(forbidden_namespace_key) for line in xdg_recovery_unit_lines):
        errors.append(f"XDG recovery unit cannot observe PID 1 mounts with {forbidden_namespace_key}")
if "[Install]" in xdg_recovery_unit_lines:
    errors.append("XDG recovery subunit must be invoked only by peer recovery")
if "start mcnf-xdg-bind-recovery.service" not in recovery_helper:
    errors.append("peer recovery no longer delegates XDG mounts into PID 1's namespace")
for forbidden in ("mackesd join", "mackesd found", "mackesd leave"):
    if forbidden in recovery_helper:
        errors.append(f"peer recovery helper must not re-enroll or tear down identity: {forbidden}")
xdg_recovery_helper = (repo / "install-helpers/mesh-xdg-bind-recovery.sh").read_text()
xdg_recovery_code = "\n".join(
    line for line in xdg_recovery_helper.splitlines()
    if not line.lstrip().startswith("#")
)
if "--type=none --options=bind" not in xdg_recovery_code:
    errors.append("XDG recovery must request a directory bind mount, not a block-device bind")
if " --bind " in xdg_recovery_code:
    errors.append("XDG recovery uses unsupported systemd-mount --bind abbreviation")

sleep_hook = (repo / "packaging/systemd/mcnf-peer-recovery-sleep").read_text()
network_hook = (repo / "packaging/systemd/90-mcnf-peer-recovery").read_text()
trigger = 'start --no-block mcnf-peer-recovery.service'
if trigger not in sleep_hook or "post)" not in sleep_hook:
    errors.append("system-sleep hook lost its post-resume no-block trigger")
if trigger not in network_hook or "up|dhcp4-change|dhcp6-change|connectivity-change|reapply" not in network_hook:
    errors.append("NetworkManager hook lost its positive network-return trigger filter")

print("runtime: dedicated root 0700 state leaf declared; shared daemon state root not writable")
print("activation: local identity guard active; distributed producer remains blocked/disabled")
print("recovery: event-triggered notify service, sleep hook, and network-return hook are role-complete")
if errors:
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    raise SystemExit(1)
PY
)"
  rc=$?
  if [ "$rc" -eq 0 ]; then
    while IFS= read -r line; do
      [ -z "$line" ] || ok "$line"
    done <<<"$output"
  else
    fail "overlay identity claim package shape rejected"
    [ -z "$output" ] || printf '%s\n' "$output" >&2
  fi
}

# ═════════════════════════════════════════════════════════════════════════════
# CHECK 2 — surface reachability (test-obs-4)
# ═════════════════════════════════════════════════════════════════════════════
check_surfaces() {
  hdr "surfaces (dry-run, static) — catalog: ${SURFACES_RS#"$REPO_ROOT"/}"

  # Precompute: is the shell binary itself shipped? A surface can only "ship"
  # by being compiled into a shipped binary — and mde-shell-egui is that binary.
  # (Capture once into a var — piping awk into `grep -q` trips SIGPIPE+pipefail
  # on the large manifest, which would read as a false no-match.)
  local assets_out shell_shipped=0
  assets_out="$(parse_assets "$CARGO_TOML")"
  if grep -q "^target/release/${SHELL_HOST_CRATE}"$'\t' <<<"$assets_out"; then
    shell_shipped=1
    ok "shell-host     target/release/${SHELL_HOST_CRATE} is in the asset set (surfaces ride it)"
  else
    fail "shell-host     target/release/${SHELL_HOST_CRATE} NOT in the asset set — NO surface can ship"
  fi

  local c
  while IFS= read -r c; do
    [ -n "$c" ] || continue
    if [ "$c" = "$SHELL_HOST_CRATE" ]; then
      info "host           $c is the shell host, not a surface (skipped)"
      continue
    fi
    if is_exempt "$c"; then
      # Still report its wiring truthfully so an accidental one is visible.
      local dep="no" mnt="no"
      shell_depends_on "$c" && dep="yes"
      surface_catalog_mounts "$c" && mnt="yes"
      skip "exempt         $c (documented non-launchable surface; shell-dep=$dep catalog-ref=$mnt)"
      continue
    fi

    local is_dep=0 is_mnt=0
    shell_depends_on "$c" && is_dep=1
    surface_catalog_mounts "$c" && is_mnt=1

    if [ "$is_dep" -eq 1 ] && [ "$is_mnt" -eq 1 ] && [ "$shell_shipped" -eq 1 ]; then
      ok "surface        $c  mounted in surfaces.rs AND compiled into the shipped shell"
    else
      local why=""
      [ "$is_mnt" -eq 1 ] || why+=" NOT-mounted(no surfaces.rs catalog reference)"
      [ "$is_dep" -eq 1 ] || why+=" NOT-shipped(not a mde-shell-egui path-dep → compiled into no shipped bin)"
      [ "$shell_shipped" -eq 1 ] || why+=" shell-bin-unshipped"
      fail "surface        $c  built-but-dead:$why"
    fi
  done < <(list_surface_crates)
}

# ═════════════════════════════════════════════════════════════════════════════
# SELF-TEST — exercise the parser + classifiers on good and broken fixtures.
# ═════════════════════════════════════════════════════════════════════════════
self_test() {
  hdr "SELF-TEST"
  local tmp rc out
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  local st_fail=0

  # ---- fixture A: a KNOWN-GOOD manifest -------------------------------------
  local good="$tmp/good.toml"
  cat >"$good" <<'TOML'
[package.metadata.generate-rpm]
name = "fixture"
assets = [
    { source = "target/release/mde-shell-egui", dest = "/usr/bin/mde-shell-egui", mode = "755" },
    { source = "target/release/mackesd",        dest = "/usr/bin/mackesd",        mode = "755" },
    { source = "packaging/x.service",           dest = "/usr/lib/systemd/system/x.service", mode = "644" },
    { source = "infra/tofu/cloud/cloud-init/*", dest = "/usr/share/mde/iac/infra/tofu/cloud/cloud-init/", mode = "644" },
]
[package.metadata.generate-rpm.requires]
libvirt = "*"
qemu-kvm = "*"
qemu-ui-dbus = "*"
libvirt-daemon-kvm = "*"
libvirt-daemon-driver-storage = "*"
[package.metadata.generate-rpm.variants.server]
assets = [
    { source = "target/release/should-be-ignored", dest = "/usr/bin/should-be-ignored", mode = "755" },
]
TOML
  local n
  n="$(parse_assets "$good" | wc -l | tr -d ' ')"
  if [ "$n" -eq 4 ]; then
    ok "self-test: parser reads exactly the 4 MAIN assets (ignores the server variant)"
  else
    fail "self-test: expected 4 main assets, got $n"; st_fail=1
  fi
  if parse_assets "$good" | grep -q "^target/release/mde-shell-egui"$'\t'"/usr/bin/mde-shell-egui$"; then
    ok "self-test: parser captures source<TAB>dest correctly"
  else
    fail "self-test: parser did not capture the shell asset row"; st_fail=1
  fi
  if parse_assets "$good" | grep -q "should-be-ignored"; then
    fail "self-test: parser LEAKED a server-variant asset"; st_fail=1
  else
    ok "self-test: parser does NOT leak server-variant assets"
  fi
  out="$(CARGO_TOML="$good" bash "$0" requirements 2>&1)"; rc=$?
  if [ "$rc" -eq 0 ]; then
    ok "self-test: the complete Fedora VDI-host hard-Requires set passes"
  else
    fail "self-test: complete Fedora VDI-host hard Requires did not pass"; st_fail=1
  fi

  # A weak dependency is not sufficient: all Fedora KVM/Display1 packages remain
  # in the base RPM's hard-Requires table alongside the existing libvirt pins.
  local bad_requires="$tmp/bad-requires.toml"
  cat >"$bad_requires" <<'TOML'
[package.metadata.generate-rpm.requires]
libvirt = "*"
libvirt-daemon-driver-storage = "*"
[package.metadata.generate-rpm.recommends]
qemu-kvm = "*"
qemu-ui-dbus = "*"
libvirt-daemon-kvm = "*"
TOML
  out="$(CARGO_TOML="$bad_requires" bash "$0" requirements 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ] \
      && grep -q "qemu-kvm MISSING" <<<"$out" \
      && grep -q "qemu-ui-dbus MISSING" <<<"$out" \
      && grep -q "libvirt-daemon-kvm MISSING" <<<"$out"; then
    ok "self-test: weak-only Fedora KVM dependencies fail the hard-Requires gate"
  else
    fail "self-test: weak-only Fedora KVM dependencies were not rejected"; st_fail=1
  fi

  # Source metadata is not release evidence. Exercise the actual Requires-header
  # parser with exact and versioned package expressions, then prove similarly
  # named capabilities cannot satisfy the three mandatory KVM package names.
  local good_rpm_requires="$tmp/good-rpm-requires"
  cat >"$good_rpm_requires" <<'REQUIRES'
qemu-kvm
qemu-ui-dbus = 2:10.2.2-1.fc44
libvirt-daemon-kvm >= 10.0
rpmlib(CompressedFileNames) <= 3.0.4-1
REQUIRES
  out="$(CARGO_TOML="$good" MCNF_FAKE_RPM_REQUIRES="$good_rpm_requires" \
      bash "$0" requirements "$tmp/magic-mesh-fixture.rpm" 2>&1)"; rc=$?
  if [ "$rc" -eq 0 ] \
      && grep -q "actual-requires qemu-kvm present" <<<"$out" \
      && grep -q "actual-requires qemu-ui-dbus present" <<<"$out" \
      && grep -q "actual-requires libvirt-daemon-kvm present" <<<"$out"; then
    ok "self-test: a built RPM header with all mandatory KVM Requires passes"
  else
    fail "self-test: complete built RPM KVM Requires header did not pass"; st_fail=1
  fi

  local bad_rpm_requires="$tmp/bad-rpm-requires"
  cat >"$bad_rpm_requires" <<'REQUIRES'
qemu-kvm-helper
qemu-ui-dbus-helper
libvirt-daemon-kvm-tools
REQUIRES
  out="$(CARGO_TOML="$good" MCNF_FAKE_RPM_REQUIRES="$bad_rpm_requires" \
      bash "$0" requirements "$tmp/magic-mesh-fixture.rpm" 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ] \
      && grep -q "actual-requires qemu-kvm MISSING" <<<"$out" \
      && grep -q "actual-requires qemu-ui-dbus MISSING" <<<"$out" \
      && grep -q "actual-requires libvirt-daemon-kvm MISSING" <<<"$out"; then
    ok "self-test: similarly named capabilities cannot satisfy built RPM KVM Requires"
  else
    fail "self-test: built RPM Requires gate accepted similarly named capabilities"; st_fail=1
  fi

  # Real payload validation must invoke the header gate too; otherwise a caller
  # could get a green file-list verdict for an RPM missing one hard dependency.
  local fake_rpm_list="$tmp/fake-rpm-list"
  cat >"$fake_rpm_list" <<'LISTING'
/usr/bin/mde-shell-egui
/usr/bin/mackesd
/usr/lib/systemd/system/x.service
/usr/share/mde/iac/infra/tofu/cloud/cloud-init/mesh-join.yaml.tftpl
LISTING
  printf '%s\n' 'qemu-kvm' >"$bad_rpm_requires"
  out="$(CARGO_TOML="$good" MCNF_FAKE_RPM_LIST="$fake_rpm_list" \
      MCNF_FAKE_RPM_REQUIRES="$bad_rpm_requires" \
      bash "$0" payload "$tmp/magic-mesh-fixture.rpm" 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ] \
      && grep -q "actual-requires libvirt-daemon-kvm MISSING" <<<"$out"; then
    ok "self-test: real-RPM payload mode enforces the actual Requires header"
  else
    fail "self-test: real-RPM payload mode skipped the actual Requires header"; st_fail=1
  fi

  # A non-empty cloud-init destination is not enough: the exact App VM guest
  # bootstrap member must survive both manifest selection and rpm -qlp.
  local app_vm_missing_list="$tmp/app-vm-missing-list"
  grep -Fvx "$APP_VM_BOOTSTRAP_DEST" "$fake_rpm_list" >"$app_vm_missing_list"
  out="$(CARGO_TOML="$good" MCNF_FAKE_RPM_LIST="$app_vm_missing_list" \
      bash "$0" app-vm-payload "$tmp/magic-mesh-fixture.rpm" 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ] && grep -q "$APP_VM_BOOTSTRAP_DEST MISSING" <<<"$out"; then
    ok "self-test: non-empty cloud-init payload cannot hide a missing App VM bootstrap"
  else
    fail "self-test: missing App VM bootstrap escaped exact RPM verification"; st_fail=1
  fi

  # A populated Ansible destination is not evidence that Android can boot. The
  # Cuttlefish task file is the executable runtime boundary; prove an unrelated
  # role plus the playbook/profile/metadata cannot conceal its omission.
  local android_missing_list="$tmp/android-missing-list"
  cat >"$android_missing_list" <<'LISTING'
/usr/share/mde/iac/automation/ansible/playbooks/site.yml
/usr/share/mde/iac/automation/ansible/roles/cloud_vm/tasks/main.yml
/usr/share/mde/iac/automation/ansible/roles/cuttlefish_host/defaults/main.yml
/usr/share/mde/iac/automation/ansible/roles/cuttlefish_host/meta/main.yml
LISTING
  out="$(MCNF_FAKE_RPM_LIST="$android_missing_list" \
      bash "$0" android-vm-payload "$tmp/magic-mesh-fixture.rpm" 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ] \
      && grep -q "/usr/share/mde/iac/automation/ansible/roles/cuttlefish_host/tasks/main.yml MISSING" <<<"$out"; then
    ok "self-test: non-empty Ansible payload cannot hide a missing Cuttlefish runtime"
  else
    fail "self-test: missing Cuttlefish runtime escaped exact RPM verification"; st_fail=1
  fi

  # ---- fixture B: a SYNTHETICALLY-BROKEN manifest ---------------------------
  # Drops the shell key-bin, adds a bin nothing builds, adds a missing file.
  local bad="$tmp/bad.toml"
  cat >"$bad" <<'TOML'
[package.metadata.generate-rpm]
name = "fixture"
assets = [
    { source = "target/release/mackesd",         dest = "/usr/bin/mackesd",         mode = "755" },
    { source = "target/release/mde-ghost-bin",    dest = "/usr/bin/mde-ghost-bin",    mode = "755" },
    { source = "packaging/definitely-missing.service", dest = "/usr/lib/systemd/system/definitely-missing.service", mode = "644" },
]
TOML
  # Run the dry-run against the broken fixture; it MUST fail and name the issues.
  out="$(CARGO_TOML="$bad" FAILS=0 bash "$0" payload 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ]; then
    ok "self-test: broken manifest makes the payload check EXIT NON-ZERO ($rc)"
  else
    fail "self-test: broken manifest did NOT fail the payload check"; st_fail=1
  fi
  if grep -q "mde-ghost-bin" <<<"$out" && grep -q "NO workspace crate builds" <<<"$out"; then
    ok "self-test: flags target/release/mde-ghost-bin (bin nothing builds)"
  else
    fail "self-test: did not flag the un-buildable ghost binary"; st_fail=1
  fi
  if grep -q "definitely-missing.service" <<<"$out" && grep -q "MISSING from the tree" <<<"$out"; then
    ok "self-test: flags the missing packaging source"; else
    fail "self-test: did not flag the missing packaging source"; st_fail=1
  fi
  if grep -q "target/release/mde-shell-egui  is NOT in the asset set" <<<"$out"; then
    ok "self-test: flags the dropped mde-shell-egui key bin (the real 11.4.0 regression)"
  else
    fail "self-test: did not flag the dropped shell key-bin"; st_fail=1
  fi

  # ---- fixture C: surface reachability on a synthetic desktop tree ----------
  local dt="$tmp/desktop"
  mkdir -p "$dt/mde-shell-egui/src" "$dt/mde-good-egui" "$dt/mde-orphan-egui"
  local scargo="$dt/mde-shell-egui/Cargo.toml"
  local ssurfaces="$dt/mde-shell-egui/src/surfaces.rs"
  cat >"$scargo" <<'TOML'
[dependencies]
mde-good-egui = { path = "../mde-good-egui" }
TOML
  cat >"$ssurfaces" <<'RS'
const EMBEDDED_SURFACE_CRATES: &[&str] = &["mde-good-egui"];
RS
  # good manifest that ships the shell so surfaces can ride it
  local scmani="$tmp/surf.toml"
  cat >"$scmani" <<'TOML'
[package.metadata.generate-rpm]
assets = [
    { source = "target/release/mde-shell-egui", dest = "/usr/bin/mde-shell-egui", mode = "755" },
]
TOML
  out="$(DESKTOP_DIR="$dt" SHELL_CARGO="$scargo" SURFACES_RS="$ssurfaces" CARGO_TOML="$scmani" bash "$0" surfaces 2>&1)"; rc=$?
  if grep -q "surface        mde-good-egui  mounted in surfaces.rs AND compiled" <<<"$out"; then
    ok "self-test: a properly wired surface PASSES"
  else
    fail "self-test: wired surface did not pass"; st_fail=1
  fi
  if grep -q "mde-orphan-egui  built-but-dead" <<<"$out" && [ "$rc" -ne 0 ]; then
    ok "self-test: an unmounted+unshipped surface FAILS (the term/media regression)"
  else
    fail "self-test: orphan surface was not caught"; st_fail=1
  fi

  # ---- fixture D: the RPM-size gate (build-deploy-12) ------------------------
  # A real RPM can't be cut on the airgapped tree, so exercise the byte-threshold
  # logic on a fixed-size file (5 MiB of zeros stands in for the .rpm).
  local fakerpm="$tmp/fake.rpm"
  head -c $((5 * 1024 * 1024)) /dev/zero >"$fakerpm"
  # under a generous limit → PASS + exit 0
  out="$(MCNF_RPM_SIZE_LIMIT_MIB=90 bash "$0" size "$fakerpm" 2>&1)"; rc=$?
  if [ "$rc" -eq 0 ] && grep -q "within the 90 MiB cut limit" <<<"$out"; then
    ok "self-test: a 5 MiB RPM passes the 90 MiB size gate"
  else
    fail "self-test: small RPM did not pass the size gate"; st_fail=1
  fi
  # over a tight limit → FAIL + exit non-zero
  out="$(MCNF_RPM_SIZE_LIMIT_MIB=2 bash "$0" size "$fakerpm" 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ] && grep -q "EXCEEDS the 2 MiB cut limit" <<<"$out"; then
    ok "self-test: a 5 MiB RPM fails a 2 MiB size gate (the channel-ceiling guard fires)"
  else
    fail "self-test: oversize RPM was not caught by the size gate"; st_fail=1
  fi
  # a missing file FAILs cleanly (not a crash)
  out="$(bash "$0" size "$tmp/nonexistent.rpm" 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ] && grep -q "RPM not found for size check" <<<"$out"; then
    ok "self-test: a missing RPM path fails the size gate cleanly"
  else
    fail "self-test: missing-RPM size check did not fail cleanly"; st_fail=1
  fi

  hdr "SELF-TEST RESULT"
  if [ "$st_fail" -eq 0 ]; then
    ok "self-test: all assertions passed"
    return 0
  fi
  fail "self-test: $st_fail assertion group(s) failed"
  return 1
}

usage() {
  sed -n '2,60p' "$0" | sed 's/^# \{0,1\}//'
}

main() {
  local cmd="${1:-all}"
  case "$cmd" in
    -h|--help|help) usage; exit 0 ;;
    --self-test|self-test) self_test; exit $? ;;
    payload)
      shift
      check_payload "${1:-}"
      if [ -n "${1:-}" ]; then
        "$REPO_ROOT/install-helpers/lint-clock-cutover.sh" --rpm "$1" || FAILS=$((FAILS + 1))
      else
        "$REPO_ROOT/install-helpers/lint-clock-cutover.sh" || FAILS=$((FAILS + 1))
      fi
      ;;
    requirements)
      shift
      check_vdi_host_requires
      [ -z "${1:-}" ] || check_built_rpm_vdi_host_requires "$1"
      ;;
    overlay-claims-package) check_overlay_claims_package ;;
    grouped-process) check_grouped_mackesd_assets ;;
    candidate-payload) check_candidate_credential_assets ;;
    app-vm-payload) shift; check_app_vm_payload "${1:-}" ;;
    android-vm-payload) shift; check_android_vm_payload "${1:-}" ;;
    size)     shift; check_rpm_size "${1:?usage: verify-rpm-payload.sh size <rpm>}" ;;
    surfaces) check_surfaces ;;
    all|"")
      check_payload_dryrun
      check_surfaces
      "$REPO_ROOT/install-helpers/lint-clock-cutover.sh" || FAILS=$((FAILS + 1))
      ;;
    *) printf 'unknown command: %s\n\n' "$cmd" >&2; usage >&2; exit 2 ;;
  esac

  hdr "SUMMARY"
  if [ "$FAILS" -eq 0 ]; then
    ok "all checks passed"
    exit 0
  fi
  fail "$FAILS check(s) failed"
  exit 1
}

main "$@"
