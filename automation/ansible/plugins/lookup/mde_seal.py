# WL-ARCH-001 Phase B (item 3) — the mde-seal → Ansible secrets bridge.
#
# A lookup plugin that resolves a named secret from the mesh secret store
# (mde-seal / mcnf-secret.sh) at run time, so a play reads an age-sealed secret
# with NO Ansible Vault and NO second secret system (decided-stack #8) — the SAME
# store the OpenTofu external data source (infra/tofu/cloud/scripts/
# mde-seal-external.sh) reads.
#
# Usage in a play:
#     join_token: "{{ lookup('mde_seal', 'nebula-join-token') }}"
#     # optional: override the helper path
#     x: "{{ lookup('mde_seal', 'name', helper='/usr/bin/mcnf-secret.sh') }}"
#
# The helper resolves in order: the `helper=` kwarg, $MDE_SEAL_HELPER, then
# `mcnf-secret.sh` on $PATH. A missing secret / unreachable store raises loudly
# (never a fabricated value). Results are marked `no_log`-friendly (the caller
# should `no_log: true` the task); the plugin never prints the value itself.
from __future__ import annotations

DOCUMENTATION = r"""
name: mde_seal
author: magic-mesh
short_description: Resolve an age-sealed secret from the mesh store (mde-seal)
description:
  - Shells out to the mesh secret-store helper (C(mcnf-secret.sh get <name>)) to
    unseal a named secret at run time. No Ansible Vault; the same store the tofu
    cloud root reads.
options:
  _terms:
    description: One or more sealed-secret store names to resolve.
    required: true
  helper:
    description: Path to the secret-store helper. Defaults to $MDE_SEAL_HELPER or C(mcnf-secret.sh).
    type: string
"""

import os
import subprocess

from ansible.errors import AnsibleError
from ansible.plugins.lookup import LookupBase


def resolve_secret(name, helper=None):
    """Unseal one secret by name via the store helper. Raises on failure.

    Kept as a module-level function (not only a method) so the self-test can
    exercise the real resolution path without the full plugin loader.
    """
    if not name:
        raise ValueError("mde_seal: empty secret name")
    helper = helper or os.environ.get("MDE_SEAL_HELPER") or "mcnf-secret.sh"
    try:
        proc = subprocess.run(
            [helper, "get", name],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as exc:
        raise RuntimeError(f"mde_seal: cannot run secret-store helper {helper!r}: {exc}") from exc
    if proc.returncode != 0:
        raise RuntimeError(
            f"mde_seal: `{helper} get {name}` failed (rc={proc.returncode}) — "
            "the secret is absent or the store is unreachable"
        )
    value = proc.stdout.decode("utf-8", "replace").rstrip("\n")
    if not value:
        raise RuntimeError(f"mde_seal: /mcnf/secret/{name} decrypted to EMPTY — reseal/rotate it")
    return value


class LookupModule(LookupBase):
    def run(self, terms, variables=None, **kwargs):
        helper = kwargs.get("helper")
        out = []
        for term in terms:
            try:
                out.append(resolve_secret(str(term), helper))
            except (RuntimeError, ValueError) as exc:
                raise AnsibleError(str(exc)) from exc
        return out
