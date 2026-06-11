# PLANES-22 (W53/W54) — the mesh-service container image (OCI).
#
# A minimal Fedora image carrying the magic-mesh RPM from the project's
# GitHub-hosted dnf repo (PKG-8, the same source the ISO kickstart uses),
# running mackesd in serve mode. Built by the image-build job
# (`mackesd images --build --kind container`) on an execution-tagged node;
# the artifact is saved as an OCI archive and its manifest recorded in the
# LizardFS image catalog via `mackesd images --record`.
#
# Build context is the repo root (the COPY below pulls the shipped repo
# file): `podman build -f packaging/containers/mackesd.Containerfile .`
FROM fedora:42

# The Magic Mesh dnf repo (PKG-8). gpgcheck stays on; dnf fetches the
# published project key from the repo's gpgkey URL at install time.
COPY packaging/repo/magic-mesh.repo /etc/yum.repos.d/magic-mesh.repo

RUN dnf install -y --setopt=install_weak_deps=False magic-mesh \
    && dnf clean all

# mackesd's local state + the replicated QNM-Shared mount point. Identity
# and enrollment are provided at run time (bind-mounted creds / env), never
# baked into the image.
RUN mkdir -p /var/lib/mackesd /qnm

# The mesh daemon: serve the Bus + the worker set.
CMD ["mackesd", "serve"]
