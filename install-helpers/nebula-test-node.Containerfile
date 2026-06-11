# OBS-1 — the nebula integration-test node image.
#
# A minimal Fedora node with the real `nebula`/`nebula-cert` binaries plus the
# tools the E2E flow shells (iproute for the overlay interface, iputils for the
# reachability ping, nfs-utils parity with the cloud bed). The freshly-built
# `mackesd` binary is NOT baked in — the test bind-mounts the cargo-built one at
# run time so the suite always exercises the current code, never a stale copy.
#
# Built once per run by the test's ensure_image() helper (podman caches layers);
# nebula's tun device needs a privileged/NET_ADMIN container on a root daemon
# (rootless userns can't open /dev/net/tun) — the test runs it that way.
FROM fedora:42
RUN dnf install -y --setopt=install_weak_deps=False nebula iproute iputils nfs-utils \
    && dnf clean all
# A predictable workdir for the bind-mounted mackesd binary + QNM-Shared mount.
RUN mkdir -p /qnm /opt/mackes
ENV PATH="/opt/mackes:${PATH}"
# nebula + mackesd are driven explicitly by the test via `podman exec`; idle by
# default so the container stays up for the multi-step enrollment flow.
CMD ["sleep", "infinity"]
