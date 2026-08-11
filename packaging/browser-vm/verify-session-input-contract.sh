#!/usr/bin/env bash
# Verify the immutable xrdp -> Xorg -> Sway -> Chromium desktop/input chain.
set -euo pipefail
self_test_fixture=

fail() {
    echo "verify-browser-vm-session-input: $*" >&2
    exit 1
}

trimmed_last_active_line() {
    awk '
        /^[[:space:]]*($|#)/ { next }
        { line = $0 }
        END {
            sub(/^[[:space:]]+/, "", line)
            sub(/[[:space:]]+$/, "", line)
            print line
        }
    ' "$1"
}

has_active_line() {
    local path=$1 expected=$2
    awk -v expected="$expected" '
        /^[[:space:]]*#/ { next }
        {
            line = $0
            sub(/^[[:space:]]+/, "", line)
            sub(/[[:space:]]+$/, "", line)
            if (line == expected) found = 1
        }
        END { exit(found ? 0 : 1) }
    ' "$path"
}

active_code_contains() {
    local path=$1 expected=$2
    awk -v expected="$expected" '
        /^[[:space:]]*#/ { next }
        index($0, expected) { found = 1 }
        END { exit(found ? 0 : 1) }
    ' "$path"
}

verify_foreground_bootstrap() {
    local runtime=$1
    local first='if /usr/bin/dbus-run-session -- \'
    local second='/usr/local/libexec/mcnf-browser-vm-session \'
    local third='/usr/local/libexec/mcnf-browser-vm-runtime --audio-ready; then'

    FIRST="$first" SECOND="$second" THIRD="$third" awk '
        BEGIN {
            first = ENVIRON["FIRST"]
            second = ENVIRON["SECOND"]
            third = ENVIRON["THIRD"]
        }
        function trim(value) {
            sub(/^[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            return value
        }
        /^[[:space:]]*($|#)/ { next }
        {
            line = trim($0)
            if (state == 0 && line == first) {
                state = 1
            } else if (state == 1 && line == second) {
                state = 2
            } else if (state == 2 && line == third) {
                found = 1
                state = 0
            } else {
                state = (line == first) ? 1 : 0
            }
        }
        END { exit(found ? 0 : 1) }
    ' "$runtime" || fail "runtime does not keep the authenticated desktop supervisor in the foreground"
}

verify_sway_chromium_block() {
    local runtime=$1
    local header="cat > \"\$HOME/.config/sway/config\" <<'EOF'"
    local x11_enable='output X11-1 enable'
    local x11_mode='output X11-1 mode --custom 1920x1080'
    local chromium='exec @CHROMIUM_BIN@ --ozone-platform=wayland --enable-features=UseOzonePlatform --start-maximized --no-first-run --restore-last-session --hide-crash-restore-bubble --force-prefers-reduced-motion --user-data-dir=/var/lib/mcnf-browser/chromium'

    awk -v header="$header" -v x11_enable="$x11_enable" \
        -v x11_mode="$x11_mode" -v chromium="$chromium" '
        function trim(value) {
            sub(/^[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            return value
        }
        {
            line = trim($0)
            if (!inside && line == header) {
                blocks++
                inside = 1
                next
            }
            if (inside && line == "EOF") {
                closed++
                inside = 0
                next
            }
            if (inside && line == x11_enable) x11_enable_entries++
            if (inside && line == x11_mode) x11_mode_entries++
            if (inside &&
                line ~ /^exec(_always)?[[:space:]]+@CHROMIUM_BIN@([[:space:]]|$)/) {
                chromium_launch_entries++
            }
            if (inside && line == chromium) chromium_entries++
        }
        END {
            valid = blocks == 1 && closed == 1 &&
                x11_enable_entries == 1 && x11_mode_entries == 1 &&
                chromium_entries == 1 && chromium_launch_entries == 1 &&
                !inside
            exit(valid ? 0 : 1)
        }
    ' "$runtime" || fail "runtime does not generate one geometry-aligned guest-owned Chromium Sway desktop"
}

verify_desktop_chain() {
    local startwm=$1 runtime=$2 session=$3
    local path

    for path in "$startwm" "$runtime" "$session"; do
        [ -f "$path" ] || fail "desktop-chain file is missing: $path"
        [ -x "$path" ] || fail "desktop-chain file is not executable: $path"
    done

    has_active_line "$startwm" 'export WLR_BACKENDS=x11' \
        || fail "xrdp startwm does not select Sway's nested X11 backend"
    has_active_line "$startwm" 'export WLR_RENDERER=pixman' \
        || fail "xrdp startwm does not select the nested X11 renderer"
    has_active_line "$startwm" 'export MCNF_X11_PRESENT_COPY=1' \
        || fail "xrdp startwm does not enable nested-X11 present mirroring"
    has_active_line "$startwm" \
        'export LD_PRELOAD=/usr/local/lib64/libmcnf-x11-present-copy.so' \
        || fail "xrdp startwm does not preload the present-mirroring boundary"
    [ "$(trimmed_last_active_line "$startwm")" = \
        'exec /usr/local/libexec/mcnf-browser-vm-runtime' ] \
        || fail "xrdp startwm does not remain attached to the Browser runtime"

    has_active_line "$runtime" 'PATH=/usr/sbin:/usr/bin' \
        || fail "runtime does not pin executable lookup to the immutable guest image"
    has_active_line "$runtime" 'export PATH' \
        || fail "runtime does not export its image-owned executable lookup path"
    has_active_line "$runtime" 'unset MCNF_BROWSER_VM_INPUT_ROOT' \
        || fail "runtime accepts an xrdp-selected identity directory"
    has_active_line "$runtime" 'input_root=/etc/mcnf-browser-vm' \
        || fail "runtime does not use the canonical guest identity directory"
    if active_code_contains "$runtime" 'input_root=${MCNF_BROWSER_VM_INPUT_ROOT:-'; then
        fail "runtime still permits an environment-directed identity directory"
    fi
    has_active_line "$runtime" \
        'for candidate in /usr/bin/chromium /usr/bin/chromium-browser; do' \
        || fail "runtime does not select Chromium from fixed image-owned entrypoints"
    if active_code_contains "$runtime" 'command -v chromium'; then
        fail "runtime still permits environment-directed Chromium lookup"
    fi
    verify_foreground_bootstrap "$runtime"
    verify_sway_chromium_block "$runtime"
    has_active_line "$runtime" \
        'sed -i "s#@CHROMIUM_BIN@#$chromium_bin#" "$HOME/.config/sway/config"' \
        || fail "runtime does not bind the resolved Chromium binary into Sway"
    [ "$(trimmed_last_active_line "$runtime")" = \
        'exec /usr/local/libexec/mcnf-sway --unsupported-gpu --config "$HOME/.config/sway/config"' ] \
        || fail "runtime does not remain attached to Sway"
    [ "$(trimmed_last_active_line "$session")" = '"$@"' ] \
        || fail "session supervisor does not keep the Browser runtime in the foreground"
}

ini_value() {
    local path=$1 wanted_section=$2 wanted_key=$3
    awk -v wanted_section="$wanted_section" -v wanted_key="$wanted_key" '
        function trim(value) {
            sub(/^[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            return value
        }
        /^[[:space:]]*[#;]/ { next }
        /^[[:space:]]*\[/ {
            section = $0
            sub(/^[[:space:]]*\[/, "", section)
            sub(/\][[:space:]]*$/, "", section)
            next
        }
        section == wanted_section {
            equals = index($0, "=")
            if (!equals) next
            key = trim(substr($0, 1, equals - 1))
            if (key == wanted_key) {
                count++
                value = trim(substr($0, equals + 1))
            }
        }
        END {
            if (count == 1) print value
            else exit 1
        }
    ' "$path"
}

xorg_config_parameter() {
    local path=$1
    awk '
        function trim(value) {
            sub(/^[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            return value
        }
        /^[[:space:]]*[#;]/ { next }
        /^[[:space:]]*\[/ {
            section = $0
            sub(/^[[:space:]]*\[/, "", section)
            sub(/\][[:space:]]*$/, "", section)
            next
        }
        section == "Xorg" {
            equals = index($0, "=")
            if (!equals) next
            key = trim(substr($0, 1, equals - 1))
            if (key == "param") params[++count] = trim(substr($0, equals + 1))
        }
        END {
            for (i = 1; i <= count; i++) {
                if (params[i] == "-config") {
                    selectors++
                    selected = params[i + 1]
                }
            }
            if (selectors == 1 && selected != "") print selected
            else exit 1
        }
    ' "$path"
}

xorg_first_parameter() {
    local path=$1
    awk '
        function trim(value) {
            sub(/^[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            return value
        }
        /^[[:space:]]*[#;]/ { next }
        /^[[:space:]]*\[/ {
            section = $0
            sub(/^[[:space:]]*\[/, "", section)
            sub(/\][[:space:]]*$/, "", section)
            next
        }
        section == "Xorg" {
            equals = index($0, "=")
            if (!equals) next
            key = trim(substr($0, 1, equals - 1))
            if (key == "param") {
                print trim(substr($0, equals + 1))
                exit
            }
        }
    ' "$path"
}

verify_xorg_input_device() {
    local config=$1 wanted_identifier=$2 wanted_driver=$3
    awk -v wanted_identifier="$wanted_identifier" -v wanted_driver="$wanted_driver" '
        function lower(value) { return tolower(value) }
        /^[[:space:]]*#/ { next }
        /^[[:space:]]*Section[[:space:]]+"/ {
            split($0, quoted, "\"")
            section = lower(quoted[2])
            identifier = ""
            driver = ""
            next
        }
        /^[[:space:]]*EndSection/ {
            if (section == "inputdevice" && lower(identifier) == lower(wanted_identifier)) {
                matches++
                if (lower(driver) == lower(wanted_driver)) valid++
            }
            section = ""
            next
        }
        section == "inputdevice" && /^[[:space:]]*Identifier[[:space:]]+"/ {
            split($0, quoted, "\"")
            identifier = quoted[2]
            next
        }
        section == "inputdevice" && /^[[:space:]]*Driver[[:space:]]+"/ {
            split($0, quoted, "\"")
            driver = quoted[2]
        }
        END { exit(matches == 1 && valid == 1 ? 0 : 1) }
    ' "$config" || fail "Xorg does not bind $wanted_identifier to $wanted_driver exactly once"
}

verify_xorg_layout_binding() {
    local config=$1 wanted_identifier=$2 wanted_role=$3
    awk -v wanted_identifier="$wanted_identifier" -v wanted_role="$wanted_role" '
        /^[[:space:]]*#/ { next }
        /^[[:space:]]*Section[[:space:]]+"/ {
            split($0, quoted, "\"")
            section = tolower(quoted[2])
            next
        }
        /^[[:space:]]*EndSection/ { section = ""; next }
        section == "serverlayout" && /^[[:space:]]*InputDevice[[:space:]]+"/ {
            split($0, quoted, "\"")
            if (tolower(quoted[2]) == tolower(wanted_identifier) &&
                tolower(quoted[4]) == tolower(wanted_role)) matches++
        }
        END { exit(matches == 1 ? 0 : 1) }
    ' "$config" || fail "Xorg ServerLayout does not bind $wanted_identifier as $wanted_role exactly once"
}

verify_driver_module() {
    local prefix=$1 driver=$2
    local matches=() path
    shopt -s nullglob
    matches=("$prefix"/usr/lib*/xorg/modules/input/"${driver}_drv.so")
    shopt -u nullglob
    [ "${#matches[@]}" -gt 0 ] || fail "Xorg input module is missing: ${driver}_drv.so"
    for path in "${matches[@]}"; do
        [ -f "$path" ] && [ -r "$path" ] \
            || fail "Xorg input module is not a readable regular file: $path"
    done
}

verify_xorg_glamor_capture() {
    local config=$1
    awk '
        /^[[:space:]]*#/ { next }
        /^[[:space:]]*Section[[:space:]]+"/ {
            split($0, quoted, "\"")
            section = tolower(quoted[2])
            next
        }
        /^[[:space:]]*EndSection/ { section = ""; next }
        section == "module" &&
            tolower($0) ~ /^[[:space:]]*load[[:space:]]+"glamoregl"/ {
            glamor_modules++
        }
        section == "device" &&
            tolower($0) ~ /^[[:space:]]*option[[:space:]]+"drmdevice"[[:space:]]+"\/dev\/dri\/renderd128"/ {
            drm_devices++
        }
        section == "device" &&
            tolower($0) ~ /^[[:space:]]*option[[:space:]]+"drmallowlist"/ {
            allow_lists++
            line = tolower($0)
            if (line ~ /(^|[[:space:]"])(virtio_gpu)([[:space:]"]|$)/) {
                virtio_admitted++
            }
        }
        END {
            exit(glamor_modules == 1 && drm_devices == 1 &&
                 allow_lists == 1 && virtio_admitted == 1 ? 0 : 1)
        }
    ' "$config" || fail "xorgxrdp does not require glamor capture on the virtio render node"
}

verify_image_root() {
    local root=$1 prefix sesman default_wm xorg_binary config_parameter xorg_config
    local present_library sway_copy
    [ -d "$root" ] || fail "image root is not a directory: $root"
    prefix=${root%/}
    sesman=$prefix/etc/xrdp/sesman.ini
    [ -f "$sesman" ] || fail "xrdp sesman configuration is missing"

    default_wm=$(ini_value "$sesman" Globals DefaultWindowManager) \
        || fail "xrdp DefaultWindowManager is missing or ambiguous"
    [ "$default_wm" = startwm.sh ] \
        || fail "authenticated xrdp sessions do not select startwm.sh"

    xorg_binary=$(xorg_first_parameter "$sesman") \
        || fail "xrdp Xorg executable parameter is missing"
    [ "$xorg_binary" = /usr/libexec/Xorg ] \
        || fail "xrdp does not select Fedora's non-setuid Xorg executable"
    [ -x "$prefix$xorg_binary" ] \
        || fail "selected xrdp Xorg executable is unavailable: $xorg_binary"

    config_parameter=$(xorg_config_parameter "$sesman") \
        || fail "xrdp Xorg -config parameter is missing or ambiguous"
    [ "$config_parameter" = xrdp/xorg.conf ] \
        || fail "xrdp does not select the shipped xorgxrdp configuration"
    xorg_config=$prefix/etc/X11/$config_parameter
    [ -f "$xorg_config" ] || fail "selected xorgxrdp configuration is missing"

    verify_xorg_input_device "$xorg_config" xrdpMouse xrdpmouse
    verify_xorg_input_device "$xorg_config" xrdpKeyboard xrdpkeyb
    verify_xorg_layout_binding "$xorg_config" xrdpMouse CorePointer
    verify_xorg_layout_binding "$xorg_config" xrdpKeyboard CoreKeyboard
    verify_xorg_glamor_capture "$xorg_config"
    verify_driver_module "$prefix" xrdpmouse
    verify_driver_module "$prefix" xrdpkeyb
    present_library=$prefix/usr/local/lib64/libmcnf-x11-present-copy.so
    sway_copy=$prefix/usr/local/libexec/mcnf-sway
    [ -f "$present_library" ] && [ -r "$present_library" ] \
        || fail "nested-X11 present-mirroring library is missing"
    [ -f "$sway_copy" ] && [ -x "$sway_copy" ] \
        || fail "unprivileged Sway executable is missing"
    if command -v getcap >/dev/null 2>&1; then
        [ -z "$(getcap "$sway_copy")" ] \
            || fail "present-mirroring Sway executable must not carry file capabilities"
    fi
    verify_desktop_chain \
        "$prefix/usr/libexec/xrdp/startwm.sh" \
        "$prefix/usr/local/libexec/mcnf-browser-vm-runtime" \
        "$prefix/usr/local/libexec/mcnf-browser-vm-session"
}

verify_source() {
    local source=$1 container startwm runtime session present_source
    [ -d "$source" ] || fail "source directory is missing: $source"
    container=$source/Containerfile
    startwm=$source/mcnf-browser-vm-xrdp-startwm.sh
    runtime=$source/mcnf-browser-vm-runtime.sh
    session=$source/mcnf-browser-vm-session.sh
    present_source=$source/mcnf-x11-present-copy.c
    [ -f "$container" ] || fail "Browser VM Containerfile is missing"
    [ -f "$present_source" ] || fail "nested-X11 present-mirroring source is missing"

    verify_desktop_chain "$startwm" "$runtime" "$session"
    active_code_contains "$container" 'xrdp xrdp-selinux xorgxrdp-glamor' \
        || fail "Browser image does not install the glamor xrdp/Xorg input stack"
    active_code_contains "$container" \
        'Option "DRMAllowList" "amdgpu i915 msm radeon virtio_gpu"' \
        || fail "Browser image does not admit virtio_gpu for glamor capture"
    active_code_contains "$container" \
        'COPY packaging/browser-vm/verify-session-input-contract.sh /tmp/mcnf-browser-vm-verify-session-input' \
        || fail "Browser image does not copy the session-input verifier"
    active_code_contains "$container" \
        'install -D -m 0755 /tmp/mcnf-browser-vm-verify-session-input /usr/local/libexec/mcnf-browser-vm-verify-session-input' \
        || fail "Browser image does not install the session-input verifier"
    active_code_contains "$container" \
        '/usr/local/libexec/mcnf-browser-vm-verify-session-input --image-root /' \
        || fail "Browser image build does not enforce the installed session-input contract"
    active_code_contains "$container" \
        'install -D -m 0755 /tmp/mcnf-browser-vm-xrdp-startwm /usr/libexec/xrdp/startwm.sh' \
        || fail "Browser image does not replace Fedora's selected xrdp startwm entrypoint"
    active_code_contains "$container" \
        'COPY packaging/browser-vm/mcnf-x11-present-copy.c /tmp/mcnf-x11-present-copy.c' \
        || fail "Browser image does not copy the present-mirroring source"
    active_code_contains "$container" \
        '-o /usr/local/lib64/libmcnf-x11-present-copy.so' \
        || fail "Browser image does not build the present-mirroring library"
    active_code_contains "$container" \
        'install -D -m 0755 /usr/bin/sway /usr/local/libexec/mcnf-sway' \
        || fail "Browser image does not install an unprivileged Sway executable"
    active_code_contains "$container" \
        "sed -i 's/^DefaultWindowManager=.*/DefaultWindowManager=startwm.sh/' /etc/xrdp/sesman.ini" \
        || fail "Browser image does not select its authenticated desktop entrypoint"
}

expect_rejected() {
    local label=$1
    shift
    if ("$@" >/dev/null 2>&1); then
        fail "self-test accepted $label"
    fi
}

self_test() {
    local script_dir source_fixture image_fixture
    script_dir=$(cd "$(dirname "$0")" && pwd)
    self_test_fixture=$(mktemp -d)
    trap 'rm -rf -- "$self_test_fixture"' EXIT

    source_fixture=$self_test_fixture/source
    mkdir -p "$source_fixture"
    cp "$script_dir/Containerfile" "$source_fixture/Containerfile"
    cp "$script_dir/mcnf-browser-vm-xrdp-startwm.sh" \
        "$source_fixture/mcnf-browser-vm-xrdp-startwm.sh"
    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    cp "$script_dir/mcnf-browser-vm-session.sh" \
        "$source_fixture/mcnf-browser-vm-session.sh"
    cp "$script_dir/mcnf-x11-present-copy.c" \
        "$source_fixture/mcnf-x11-present-copy.c"
    verify_source "$source_fixture"

    cp "$script_dir/Containerfile" "$source_fixture/Containerfile"
    sed -i 's/xrdp xrdp-selinux xorgxrdp-glamor/xrdp xrdp-selinux/' \
        "$source_fixture/Containerfile"
    expect_rejected 'source without xorgxrdp-glamor' verify_source "$source_fixture"

    cp "$script_dir/Containerfile" "$source_fixture/Containerfile"
    sed -i 's/ radeon virtio_gpu/ radeon/' "$source_fixture/Containerfile"
    expect_rejected 'source without virtio glamor admission' \
        verify_source "$source_fixture"

    cp "$script_dir/Containerfile" "$source_fixture/Containerfile"
    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    sed -i '/^exec @CHROMIUM_BIN@ /d' "$source_fixture/mcnf-browser-vm-runtime.sh"
    expect_rejected 'source without the Chromium desktop entry' verify_source "$source_fixture"

    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    sed -i 's/ --force-prefers-reduced-motion//' \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    expect_rejected 'source with mutated Chromium runtime flags' \
        verify_source "$source_fixture"

    # Hostile regression: an xrdp-provided PATH must never redirect the guest
    # runtime to a host-provisioned Browser/helper executable after restart.
    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    sed -i 's#^PATH=/usr/sbin:/usr/bin$#PATH=/tmp/host-browser-bin:/usr/sbin:/usr/bin#' \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    expect_rejected 'runtime with a host-directed executable search path' \
        verify_source "$source_fixture"

    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    sed -i '/^exec @CHROMIUM_BIN@ /a exec @CHROMIUM_BIN@ --no-first-run --user-data-dir=/tmp/hostile-profile' \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    expect_rejected 'source with a second Chromium launch' \
        verify_source "$source_fixture"

    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    sed -i '/^output X11-1 mode --custom 1920x1080$/d' \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    expect_rejected 'source without the nested X11 custom mode' verify_source "$source_fixture"

    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    cp "$script_dir/mcnf-browser-vm-xrdp-startwm.sh" \
        "$source_fixture/mcnf-browser-vm-xrdp-startwm.sh"
    sed -i 's/^export WLR_BACKENDS=x11$/export WLR_BACKENDS=headless/' \
        "$source_fixture/mcnf-browser-vm-xrdp-startwm.sh"
    expect_rejected 'source with a non-X11 startwm backend' \
        verify_source "$source_fixture"

    cp "$script_dir/mcnf-browser-vm-xrdp-startwm.sh" \
        "$source_fixture/mcnf-browser-vm-xrdp-startwm.sh"
    sed -i 's#^exec /usr/local/libexec/mcnf-browser-vm-runtime$#exec /bin/true#' \
        "$source_fixture/mcnf-browser-vm-xrdp-startwm.sh"
    expect_rejected 'source with a detached startwm/runtime chain' \
        verify_source "$source_fixture"

    cp "$script_dir/mcnf-browser-vm-xrdp-startwm.sh" \
        "$source_fixture/mcnf-browser-vm-xrdp-startwm.sh"
    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    sed -i 's#^exec /usr/local/libexec/mcnf-sway #/usr/local/libexec/mcnf-sway #' \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    expect_rejected 'source with a detached runtime/Sway chain' \
        verify_source "$source_fixture"

    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$source_fixture/mcnf-browser-vm-runtime.sh"
    cp "$script_dir/mcnf-browser-vm-session.sh" \
        "$source_fixture/mcnf-browser-vm-session.sh"
    sed -i 's/^"\$@"$/exec true/' "$source_fixture/mcnf-browser-vm-session.sh"
    expect_rejected 'source with a detached session/runtime chain' \
        verify_source "$source_fixture"

    cp "$script_dir/mcnf-browser-vm-session.sh" \
        "$source_fixture/mcnf-browser-vm-session.sh"

    image_fixture=$self_test_fixture/image
    mkdir -p \
        "$image_fixture/etc/xrdp" \
        "$image_fixture/etc/X11/xrdp" \
        "$image_fixture/usr/libexec/xrdp" \
        "$image_fixture/usr/local/libexec" \
        "$image_fixture/usr/local/lib64" \
        "$image_fixture/usr/lib64/xorg/modules/input"
    cp "$script_dir/mcnf-browser-vm-xrdp-startwm.sh" \
        "$image_fixture/usr/libexec/xrdp/startwm.sh"
    cp "$script_dir/mcnf-browser-vm-runtime.sh" \
        "$image_fixture/usr/local/libexec/mcnf-browser-vm-runtime"
    cp "$script_dir/mcnf-browser-vm-session.sh" \
        "$image_fixture/usr/local/libexec/mcnf-browser-vm-session"
    install -m 0755 /dev/null "$image_fixture/usr/local/libexec/mcnf-sway"
    install -m 0755 /dev/null \
        "$image_fixture/usr/local/lib64/libmcnf-x11-present-copy.so"
    install -m 0755 /dev/null "$image_fixture/usr/libexec/Xorg"
    install -m 0644 /dev/null \
        "$image_fixture/usr/lib64/xorg/modules/input/xrdpmouse_drv.so"
    install -m 0644 /dev/null \
        "$image_fixture/usr/lib64/xorg/modules/input/xrdpkeyb_drv.so"
    cat >"$image_fixture/etc/xrdp/sesman.ini" <<'EOF'
[Globals]
DefaultWindowManager=startwm.sh

[Xorg]
param=/usr/libexec/Xorg
param=-config
param=xrdp/xorg.conf
param=-noreset
EOF
    cp "$image_fixture/etc/xrdp/sesman.ini" "$self_test_fixture/sesman.ini.valid"
    cat >"$image_fixture/etc/X11/xrdp/xorg.conf" <<'EOF'
Section "ServerLayout"
    Identifier "X11 Server"
    InputDevice "xrdpMouse" "CorePointer"
    InputDevice "xrdpKeyboard" "CoreKeyboard"
EndSection
Section "Module"
    Load "glamoregl"
EndSection
Section "Device"
    Identifier "Video Card (xrdpdev)"
    Driver "xrdpdev"
    Option "DRMDevice" "/dev/dri/renderD128"
    Option "DRMAllowList" "amdgpu i915 msm radeon virtio_gpu"
EndSection
Section "InputDevice"
    Identifier "xrdpKeyboard"
    Driver "xrdpkeyb"
EndSection
Section "InputDevice"
    Identifier "xrdpMouse"
    Driver "xrdpmouse"
EndSection
EOF
    cp "$image_fixture/etc/X11/xrdp/xorg.conf" "$self_test_fixture/xorg.conf.valid"
    verify_image_root "$image_fixture"

    sed -i 's/DefaultWindowManager=startwm.sh/DefaultWindowManager=unsafe.sh/' \
        "$image_fixture/etc/xrdp/sesman.ini"
    expect_rejected 'image with a foreign xrdp window manager' \
        verify_image_root "$image_fixture"
    cp "$self_test_fixture/sesman.ini.valid" "$image_fixture/etc/xrdp/sesman.ini"

    sed -i 's#param=xrdp/xorg.conf#param=xrdp/foreign.conf#' \
        "$image_fixture/etc/xrdp/sesman.ini"
    expect_rejected 'image with a foreign xorgxrdp configuration' \
        verify_image_root "$image_fixture"
    cp "$self_test_fixture/sesman.ini.valid" "$image_fixture/etc/xrdp/sesman.ini"

    rm "$image_fixture/usr/lib64/xorg/modules/input/xrdpmouse_drv.so"
    expect_rejected 'image without the xrdpmouse module' verify_image_root "$image_fixture"
    install -m 0644 /dev/null \
        "$image_fixture/usr/lib64/xorg/modules/input/xrdpmouse_drv.so"

    rm "$image_fixture/usr/lib64/xorg/modules/input/xrdpkeyb_drv.so"
    expect_rejected 'image without the xrdpkeyb module' verify_image_root "$image_fixture"
    install -m 0644 /dev/null \
        "$image_fixture/usr/lib64/xorg/modules/input/xrdpkeyb_drv.so"

    rm "$image_fixture/usr/local/lib64/libmcnf-x11-present-copy.so"
    expect_rejected 'image without nested-X11 present mirroring' \
        verify_image_root "$image_fixture"
    install -m 0755 /dev/null \
        "$image_fixture/usr/local/lib64/libmcnf-x11-present-copy.so"

    rm "$image_fixture/usr/local/libexec/mcnf-sway"
    expect_rejected 'image without unprivileged Sway' verify_image_root "$image_fixture"
    install -m 0755 /dev/null "$image_fixture/usr/local/libexec/mcnf-sway"

    sed -i 's/Driver "xrdpmouse"/Driver "libinput"/' \
        "$image_fixture/etc/X11/xrdp/xorg.conf"
    expect_rejected 'image with the wrong pointer driver' verify_image_root "$image_fixture"
    cp "$self_test_fixture/xorg.conf.valid" "$image_fixture/etc/X11/xrdp/xorg.conf"

    sed -i 's/ radeon virtio_gpu/ radeon/' \
        "$image_fixture/etc/X11/xrdp/xorg.conf"
    expect_rejected 'image without virtio glamor admission' \
        verify_image_root "$image_fixture"
    cp "$self_test_fixture/xorg.conf.valid" "$image_fixture/etc/X11/xrdp/xorg.conf"

    sed -i 's/Driver "xrdpkeyb"/Driver "libinput"/' \
        "$image_fixture/etc/X11/xrdp/xorg.conf"
    expect_rejected 'image with the wrong keyboard driver' verify_image_root "$image_fixture"
    cp "$self_test_fixture/xorg.conf.valid" "$image_fixture/etc/X11/xrdp/xorg.conf"

    sed -i '/InputDevice "xrdpMouse" "CorePointer"/d' \
        "$image_fixture/etc/X11/xrdp/xorg.conf"
    expect_rejected 'image without the core pointer layout binding' \
        verify_image_root "$image_fixture"
    cp "$self_test_fixture/xorg.conf.valid" "$image_fixture/etc/X11/xrdp/xorg.conf"

    sed -i '/InputDevice "xrdpKeyboard" "CoreKeyboard"/d' \
        "$image_fixture/etc/X11/xrdp/xorg.conf"
    expect_rejected 'image without the core keyboard layout binding' \
        verify_image_root "$image_fixture"
    cp "$self_test_fixture/xorg.conf.valid" "$image_fixture/etc/X11/xrdp/xorg.conf"

    sed -i '/^exec @CHROMIUM_BIN@ /d' \
        "$image_fixture/usr/local/libexec/mcnf-browser-vm-runtime"
    expect_rejected 'image without the authenticated Chromium desktop' \
        verify_image_root "$image_fixture"

    echo 'Browser VM session/input contract self-tests passed'
}

case "${1:-}" in
    --source)
        [ "$#" -eq 2 ] || fail '--source requires one Browser VM source directory'
        verify_source "$2"
        echo 'Browser VM source session/input contract passed'
        ;;
    --image-root)
        [ "$#" -eq 2 ] || fail '--image-root requires one image root directory'
        verify_image_root "$2"
        echo 'Browser VM installed session/input contract passed'
        ;;
    --self-test)
        [ "$#" -eq 1 ] || fail '--self-test accepts no additional arguments'
        self_test
        ;;
    *)
        fail 'usage: verify-session-input-contract.sh --source DIR | --image-root DIR | --self-test'
        ;;
esac
