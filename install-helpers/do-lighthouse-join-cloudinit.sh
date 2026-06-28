#!/bin/bash
# do-lighthouse-join-cloudinit.sh — DigitalOcean cloud-init user-data that turns a
# fresh Fedora droplet into a MCNF lighthouse that JOINS an EXISTING mesh (#13,
# turn-key `mackesd lighthouse add`). Sister to do-lighthouse-cloudinit.sh, which
# `found`s a NEW mesh (§8: one founding lighthouse per mesh); this instead runs
# `mackesd join '<token>' --role lighthouse`, so the droplet becomes a full
# lighthouse of the EXISTING mesh:
#   * `join` pins role=lighthouse, network-enrolls against the token's /enroll
#     endpoint, and brings up nebula + mackesd;
#   * the daemon's auto-etcd-membership (cmd_join, #11) adds it to the quorum;
#   * a lighthouse-scoped bearer (add-peer --role lighthouse, #12) ships the CA
#     key so it is a full SIGNING lighthouse;
#   * the supervisor roster reconcile flips it to am_lighthouse:true and
#     propagates it to every peer — no manual roster edit.
#
# It is a TEMPLATE: `do-lighthouse-join.sh` substitutes the @PLACEHOLDERS@ and
# passes the result as the droplet's --user-data. DO runs it as root once.
# All output also lands in /var/log/cloud-init-output.log for debugging.
set -euo pipefail

# ---- substituted by do-lighthouse-join.sh ---------------------------------
JOIN_TOKEN="@JOIN_TOKEN@"            # v3 token from `mackesd add-peer --role lighthouse`
REPO_BASEURL="@REPO_BASEURL@"       # gh-pages dnf channel base (no trailing /)
RPM_URL="@RPM_URL@"                 # optional direct RPM URL (overrides the repo)
# ---------------------------------------------------------------------------

STATUS_FILE="/root/mesh-join-status.txt"
log() { echo "[magic-lighthouse-join] $*"; }
fail() { echo "FAILED: $*" >"$STATUS_FILE"; log "FATAL: $*"; exit 1; }

# 1. Install magic-mesh (+ the nebula control plane it Requires) — same as the
#    found path (do-lighthouse-cloudinit.sh step 2).
if [ -n "$RPM_URL" ] && [ "$RPM_URL" != "@RPM_URL@" ]; then
    log "installing magic-mesh from $RPM_URL"
    dnf install -y "$RPM_URL" || fail "dnf install of $RPM_URL failed"
else
    RELEASEVER="$(rpm -E %fedora)"
    log "installing magic-mesh from $REPO_BASEURL (fedora-$RELEASEVER)"
    cat >/etc/yum.repos.d/magic-mesh.repo <<EOF
[magic-mesh]
name=MCNF
baseurl=$REPO_BASEURL/fedora-$RELEASEVER-x86_64/
enabled=1
gpgcheck=1
gpgkey=$REPO_BASEURL/RPM-GPG-KEY-magic-mesh
EOF
    dnf install -y magic-mesh || fail "dnf install magic-mesh failed (is there a fedora-$RELEASEVER channel dir? else pass --rpm-url a portable build)"
fi
command -v mackesd >/dev/null || fail "mackesd not on PATH after install"

# 2. JOIN the existing mesh as a lighthouse (NOT found). `join --role lighthouse`
#    pins the role, network-enrolls, brings up nebula + mackesd, auto-joins the
#    etcd quorum, and installs the CA key when the bearer is lighthouse-scoped.
log "joining the existing mesh as a lighthouse"
JOIN_OUT="$(mackesd join "$JOIN_TOKEN" --role lighthouse 2>&1)" \
    || fail "mackesd join failed: $JOIN_OUT"
echo "$JOIN_OUT"

# 3. Open the lighthouse ports (DO Cloud Firewall is the real gate, applied by
#    the join-script; firewalld is the host-local belt-and-braces).
if systemctl is-active --quiet firewalld 2>/dev/null; then
    firewall-cmd --quiet --permanent --add-port=4242/udp || true   # Nebula data plane
    firewall-cmd --quiet --permanent --add-port=443/tcp || true    # covert tunnel
    firewall-cmd --quiet --reload || true
    log "firewalld: opened 4242/udp, 443/tcp"
fi

# 4. First-boot helper-binary fetches (#17 — turn-key broker). The RPM
#    post_install only `enable`s mesh-broker-setup (the ntfy notification
#    broker), mesh-netdata-setup, and mesh-shell-setup — NOT `--now`, so the dnf
#    transaction never blocks on network-online.target. On a freshly provisioned
#    lighthouse that never reboots, that leaves no ntfy broker (the mesh-wide
#    notification distribution the health watchdog feeds) until the first reboot.
#    Start them now (post-boot, the network is up). ntfy is bundled in the RPM
#    vendor dir (offline, near-instant); the others self-skip once their binary
#    exists. --no-block so a slow upstream fetch never stalls cloud-init, and
#    each oneshot is idempotent.
for unit in mesh-broker-setup.service mesh-netdata-setup.service mesh-shell-setup.service; do
    if systemctl start --no-block "$unit" 2>/dev/null; then
        log "kicked $unit (first-boot provisioning)"
    else
        log "$unit stays enabled — will run at next boot"
    fi
done

echo "OK" >"$STATUS_FILE"
log "lighthouse joined the mesh — the roster reconcile will propagate it fleet-wide."
