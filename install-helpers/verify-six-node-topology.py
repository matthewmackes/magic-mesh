#!/usr/bin/env python3
"""Fail-closed verifier for the production six-node topology evidence.

This helper validates an evidence bundle; it is not a live probe and never
turns a missing node, an operator assertion, or a farm fixture into live proof.
The promotion path can use ``--require-live`` to require that every observation
was collected from the six-node testbed rather than from a farm-only fixture.

Evidence schema (version 1)::

    {
      "schema": 1,
      "revision": "<source revision>",
      "generated_at_ms": 1760000000000,
      "nodes": [{
        "id": "lighthouse-1",
        "role": "lighthouse",
        "source": "live",
        "scenarios": {
          "join": {"status": "pass", "observed_at_ms": 1760000000000,
                   "command": "...", "artifact": "...",
                   "sha256": "<64 hex>"},
          "steady_state": ...,
          "loss": ...,
          "failover": ...,
          "re_enrollment": ...,
          "corrected_forward_recovery": ...
        },
        "live_attestation": {
          "status": "pass", "observed_at_ms": 1760000000000,
          "command": "ssh ...", "artifact": "evidence/.../live-attestation.json",
          "sha256": "<64 hex>", "node_id": "lighthouse-1", "transport": "ssh"
        },
        "recovery": {
          "node_id": "lighthouse-1",
          "states": [
            {"state": "healthy", "node_id": "lighthouse-1", ...},
            {"state": "degraded", "node_id": "lighthouse-1", ...},
            {"state": "recovering", "node_id": "lighthouse-1", ...},
            {"state": "healthy", "node_id": "lighthouse-1", ...}
          ],
          "failover": {
            "failed_lighthouse_id": "lighthouse-1",
            "active_lighthouse_id": "lighthouse-2",
            "automatic": true, "node_id": "lighthouse-1", ...
          },
          "corrected_forward": {
            "previous_revision": "old-revision",
            "forward_revision": "<source revision>",
            "re_enrolled": true, "rollback": false,
            "node_id": "lighthouse-1", ...
          }
        }
      }]
    }

The six scenario records are intentionally per-node: a single aggregate
"passed" field cannot prove that every node survived every drill.
"source": "farm" is useful for deterministic pre-live validation but is not
accepted when ``--require-live`` is supplied. A node marked ``live`` must also
carry an artifact-bound ``live_attestation`` record; the verifier never treats
the source label alone as live proof.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tempfile
import time
from pathlib import Path
from typing import Any


SCHEMA = 1
ROLES = {"lighthouse": 3, "workstation": 3}
SCENARIOS = (
    "join",
    "steady_state",
    "loss",
    "failover",
    "re_enrollment",
    "corrected_forward_recovery",
)
RECOVERY_STATES = ("healthy", "degraded", "recovering", "healthy")
SOURCES = {"farm", "live"}
SHA256 = re.compile(r"^[0-9a-f]{64}$")
LIVE_ATTESTATION_KIND = "mcnf-six-node-live-attestation-v1"
LIVE_SCENARIO_KIND = "mcnf-six-node-scenario-observation-v2"
LIVE_RECOVERY_KIND = "mcnf-six-node-recovery-observation-v2"
# Evidence artifacts are summaries/markers, not raw logs. Keep validation
# bounded even when an otherwise plausible path and digest are supplied.
MAX_ARTIFACT_BYTES = 8 * 1024 * 1024


class EvidenceError(ValueError):
    """The supplied evidence cannot support the requested claim."""


def _text(value: Any, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise EvidenceError(f"{field} must be a non-empty string")
    return value.strip()


def _integer(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise EvidenceError(f"{field} must be a positive integer")
    return value


def _digest(value: Any, field: str) -> str:
    digest = _text(value, field).lower()
    if not SHA256.fullmatch(digest):
        raise EvidenceError(f"{field} must be 64 lowercase hex characters")
    return digest


def _candidate(
    record: Any,
    *,
    node_id: str,
    role: str,
    revision: str,
    manifest_sha256: str,
) -> dict[str, Any]:
    """Validate the installed-candidate identity emitted by the live collector."""

    if not isinstance(record, dict) or set(record) != {
        "binaries",
        "manifest_sha256",
        "package",
        "package_payload_sha256",
        "revision",
    }:
        raise EvidenceError(f"{node_id}/candidate must contain exactly its binding fields")
    if record["revision"] != revision:
        raise EvidenceError(f"{node_id}/candidate.revision must match the bundle revision")
    if _digest(record["manifest_sha256"], f"{node_id}/candidate.manifest_sha256") != manifest_sha256:
        raise EvidenceError(f"{node_id}/candidate.manifest_sha256 must match the bundle")
    _text(record["package"], f"{node_id}/candidate.package")
    _digest(record["package_payload_sha256"], f"{node_id}/candidate.package_payload_sha256")
    binaries = record["binaries"]
    expected_binaries = {"mackesd"} | ({"mde-shell-egui"} if role == "workstation" else set())
    if not isinstance(binaries, dict) or set(binaries) != expected_binaries:
        raise EvidenceError(
            f"{node_id}/candidate.binaries must contain exactly the {role} runtime payload"
        )
    for name, digest in binaries.items():
        _digest(digest, f"{node_id}/candidate.binaries.{name}")
    return record


def _live_claim_artifact(
    record: dict[str, Any],
    *,
    node_id: str,
    revision: str,
    generated_at_ms: int,
    artifact_root: Path,
    candidate: dict[str, Any],
    kind: str,
    claim_name: str,
    recovery_kind: str | None = None,
) -> None:
    """Require a live claim to be a typed collector artifact, not arbitrary bytes."""

    artifact_path = (artifact_root.resolve() / record["artifact"]).resolve()
    try:
        artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"{node_id}/{claim_name}.artifact must contain typed JSON") from exc
    common_keys = {
        "candidate",
        "collected_at_ms",
        "kind",
        "node_id",
        "outcome",
        "revision",
        "schema_version",
    }
    expected_keys = common_keys | (
        {"hostname", "machine_id_sha256", "scenario"}
        if kind == LIVE_SCENARIO_KIND
        else {"recovery_kind"}
    )
    if not isinstance(artifact, dict) or set(artifact) != expected_keys:
        raise EvidenceError(f"{node_id}/{claim_name}.artifact has an unsupported collector schema")
    if artifact["schema_version"] != 1 or artifact["kind"] != kind:
        raise EvidenceError(f"{node_id}/{claim_name}.artifact has an unsupported collector kind")
    if artifact["node_id"] != node_id or artifact["revision"] != revision:
        raise EvidenceError(f"{node_id}/{claim_name}.artifact identity does not match the claim")
    if artifact["candidate"] != candidate:
        raise EvidenceError(f"{node_id}/{claim_name}.artifact candidate does not match the node")
    if artifact["collected_at_ms"] != generated_at_ms:
        raise EvidenceError(f"{node_id}/{claim_name}.artifact collection time does not match the bundle")
    if kind == LIVE_SCENARIO_KIND and artifact["scenario"] != claim_name:
        raise EvidenceError(f"{node_id}/{claim_name}.artifact scenario does not match the claim")
    if recovery_kind is not None and artifact["recovery_kind"] != recovery_kind:
        raise EvidenceError(f"{node_id}/{claim_name}.artifact recovery kind does not match the claim")
    outcome = artifact["outcome"]
    if not isinstance(outcome, dict):
        raise EvidenceError(f"{node_id}/{claim_name}.artifact outcome must be an object")
    compared_fields = {"command", "observed_at_ms", "status"} | (
        set(record) - {"artifact", "sha256"}
    )
    for field in compared_fields:
        if outcome.get(field) != record.get(field):
            raise EvidenceError(
                f"{node_id}/{claim_name}.artifact outcome.{field} does not match the claim"
            )


def _scenario(
    record: Any,
    node_id: str,
    name: str,
    now_ms: int,
    max_age_ms: int,
    artifact_root: Path,
) -> None:
    if not isinstance(record, dict):
        raise EvidenceError(f"{node_id}/{name} must be an object")
    if record.get("status") != "pass":
        raise EvidenceError(f"{node_id}/{name} is not a pass")
    observed = _integer(record.get("observed_at_ms"), f"{node_id}/{name}.observed_at_ms")
    age = now_ms - observed
    if age < 0 or age > max_age_ms:
        raise EvidenceError(f"{node_id}/{name} is stale or from the future (age_ms={age})")
    _text(record.get("command"), f"{node_id}/{name}.command")
    artifact = _text(record.get("artifact"), f"{node_id}/{name}.artifact")
    if Path(artifact).is_absolute() or ".." in Path(artifact).parts:
        raise EvidenceError(f"{node_id}/{name}.artifact must be a relative path")
    digest = _text(record.get("sha256"), f"{node_id}/{name}.sha256").lower()
    if not SHA256.fullmatch(digest):
        raise EvidenceError(f"{node_id}/{name}.sha256 must be 64 lowercase hex characters")
    root = artifact_root.resolve()
    artifact_path = (root / artifact).resolve()
    try:
        artifact_path.relative_to(root)
    except ValueError as exc:
        raise EvidenceError(f"{node_id}/{name}.artifact escapes the evidence bundle") from exc
    if artifact_path.is_symlink() or not artifact_path.is_file():
        raise EvidenceError(f"{node_id}/{name}.artifact is missing or not a regular file")
    try:
        artifact_size = artifact_path.stat().st_size
    except OSError as exc:
        raise EvidenceError(f"{node_id}/{name}.artifact cannot be stat'ed") from exc
    if artifact_size > MAX_ARTIFACT_BYTES:
        raise EvidenceError(
            f"{node_id}/{name}.artifact exceeds the {MAX_ARTIFACT_BYTES}-byte bound"
        )
    actual = hashlib.sha256(artifact_path.read_bytes()).hexdigest()
    if actual != digest:
        raise EvidenceError(
            f"{node_id}/{name}.artifact digest mismatch (declared={digest}, actual={actual})"
        )


def _live_attestation(
    record: Any,
    node_id: str,
    revision: str,
    now_ms: int,
    max_age_ms: int,
    artifact_root: Path,
) -> None:
    if not isinstance(record, dict):
        raise EvidenceError(f"{node_id}/live_attestation must be an object")
    expected_keys = {
        "artifact",
        "command",
        "node_id",
        "observed_at_ms",
        "sha256",
        "status",
        "transport",
    }
    if set(record) != expected_keys:
        raise EvidenceError(
            f"{node_id}/live_attestation must contain exactly the capture fields"
        )
    if record["node_id"] != node_id:
        raise EvidenceError(f"{node_id}/live_attestation.node_id must match the node id")
    transport = _text(record["transport"], f"{node_id}/live_attestation.transport")
    if transport not in {"ssh", "console"}:
        raise EvidenceError(
            f"{node_id}/live_attestation.transport must be ssh or console"
        )
    _scenario(record, node_id, "live_attestation", now_ms, max_age_ms, artifact_root)

    artifact_path = (artifact_root.resolve() / record["artifact"]).resolve()
    try:
        marker = json.loads(artifact_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise EvidenceError(
            f"{node_id}/live_attestation.artifact must contain a JSON live marker"
        ) from exc
    expected_marker = {
        "kind": LIVE_ATTESTATION_KIND,
        "node_id": node_id,
        "observed_at_ms": record["observed_at_ms"],
        "revision": revision,
        "source": "live",
        "transport": transport,
    }
    if marker != expected_marker:
        raise EvidenceError(
            f"{node_id}/live_attestation.artifact must exactly bind the live "
            "source, node, transport, revision, and timestamp"
        )


def _node_attestation(
    record: Any,
    node_id: str,
    field: str,
    now_ms: int,
    max_age_ms: int,
    artifact_root: Path,
    extra_keys: set[str],
) -> None:
    """Validate an artifact-bound event whose capture is tied to one node."""

    if not isinstance(record, dict):
        raise EvidenceError(f"{node_id}/{field} must be an object")
    expected_keys = {
        "artifact",
        "command",
        "node_id",
        "observed_at_ms",
        "sha256",
        "status",
    } | extra_keys
    if set(record) != expected_keys:
        raise EvidenceError(f"{node_id}/{field} must contain exactly its attestation fields")
    if record["node_id"] != node_id:
        raise EvidenceError(f"{node_id}/{field}.node_id must match the node id")
    _scenario(record, node_id, field, now_ms, max_age_ms, artifact_root)


def _recovery(
    record: Any,
    node_id: str,
    revision: str,
    now_ms: int,
    max_age_ms: int,
    artifact_root: Path,
) -> dict[str, str]:
    """Require an ordered, node-bound degraded/recovery and forward-repair proof."""

    if not isinstance(record, dict):
        raise EvidenceError(f"{node_id}/recovery must be an object")
    expected_keys = {"corrected_forward", "failover", "node_id", "states"}
    if set(record) != expected_keys:
        raise EvidenceError(f"{node_id}/recovery must contain exactly the recovery fields")
    if record["node_id"] != node_id:
        raise EvidenceError(f"{node_id}/recovery.node_id must match the node id")

    states = record["states"]
    if not isinstance(states, list) or len(states) != len(RECOVERY_STATES):
        raise EvidenceError(
            f"{node_id}/recovery.states must contain the ordered degraded/recovery path"
        )
    previous_observed: int | None = None
    for index, expected_state in enumerate(RECOVERY_STATES):
        state = states[index]
        if not isinstance(state, dict) or state.get("state") != expected_state:
            raise EvidenceError(
                f"{node_id}/recovery.states must transition "
                "healthy -> degraded -> recovering -> healthy"
            )
        _node_attestation(
            state,
            node_id,
            f"recovery.states[{index}]",
            now_ms,
            max_age_ms,
            artifact_root,
            {"state"},
        )
        observed = state["observed_at_ms"]
        if previous_observed is not None and observed <= previous_observed:
            raise EvidenceError(
                f"{node_id}/recovery.states must be strictly time-ordered"
            )
        previous_observed = observed

    failover = record["failover"]
    _node_attestation(
        failover,
        node_id,
        "recovery.failover",
        now_ms,
        max_age_ms,
        artifact_root,
        {"active_lighthouse_id", "automatic", "failed_lighthouse_id"},
    )
    if failover["automatic"] is not True:
        raise EvidenceError(f"{node_id}/recovery.failover must prove automatic failover")
    failed_lighthouse_id = _text(
        failover["failed_lighthouse_id"], f"{node_id}/recovery.failover.failed_lighthouse_id"
    )
    active_lighthouse_id = _text(
        failover["active_lighthouse_id"], f"{node_id}/recovery.failover.active_lighthouse_id"
    )
    if failed_lighthouse_id == active_lighthouse_id:
        raise EvidenceError(f"{node_id}/recovery.failover must change lighthouse")

    corrected_forward = record["corrected_forward"]
    _node_attestation(
        corrected_forward,
        node_id,
        "recovery.corrected_forward",
        now_ms,
        max_age_ms,
        artifact_root,
        {"forward_revision", "previous_revision", "re_enrolled", "rollback"},
    )
    if corrected_forward["re_enrolled"] is not True:
        raise EvidenceError(
            f"{node_id}/recovery.corrected_forward must prove re-enrollment"
        )
    if corrected_forward["rollback"] is not False:
        raise EvidenceError(
            f"{node_id}/recovery.corrected_forward must not use rollback"
        )
    previous_revision = _text(
        corrected_forward["previous_revision"],
        f"{node_id}/recovery.corrected_forward.previous_revision",
    )
    forward_revision = _text(
        corrected_forward["forward_revision"],
        f"{node_id}/recovery.corrected_forward.forward_revision",
    )
    if previous_revision == forward_revision:
        raise EvidenceError(
            f"{node_id}/recovery.corrected_forward must advance the revision"
        )
    if forward_revision != revision:
        raise EvidenceError(
            f"{node_id}/recovery.corrected_forward.forward_revision must match revision"
        )
    return {
        "failed_lighthouse_id": failed_lighthouse_id,
        "active_lighthouse_id": active_lighthouse_id,
    }


def validate(
    bundle: Any,
    *,
    now_ms: int,
    max_age_ms: int,
    require_live: bool,
    artifact_root: Path,
    expected_revision: str | None = None,
) -> dict[str, Any]:
    if not isinstance(bundle, dict):
        raise EvidenceError("evidence root must be an object")
    if bundle.get("schema") != SCHEMA:
        raise EvidenceError(f"schema must be {SCHEMA}")
    revision = _text(bundle.get("revision"), "revision")
    if expected_revision is not None and revision != _text(expected_revision, "expected_revision"):
        raise EvidenceError(
            f"revision does not match expected source revision "
            f"(evidence={revision}, expected={expected_revision})"
        )
    generated = _integer(bundle.get("generated_at_ms"), "generated_at_ms")
    generated_age = now_ms - generated
    if generated_age < 0 or generated_age > max_age_ms:
        raise EvidenceError(f"bundle is stale or from the future (age_ms={generated_age})")
    nodes = bundle.get("nodes")
    if not isinstance(nodes, list) or len(nodes) != 6:
        raise EvidenceError("nodes must contain exactly six records")
    has_live_nodes = any(isinstance(node, dict) and node.get("source") == "live" for node in nodes)
    manifest_sha256 = ""
    if has_live_nodes:
        if set(bundle) != {
            "candidate_manifest_sha256",
            "generated_at_ms",
            "nodes",
            "revision",
            "schema",
        }:
            raise EvidenceError("live evidence root must contain exactly the collector fields")
        manifest_sha256 = _digest(
            bundle.get("candidate_manifest_sha256"), "candidate_manifest_sha256"
        )

    seen: set[str] = set()
    role_counts = {role: 0 for role in ROLES}
    normalized_nodes: list[dict[str, Any]] = []
    recovery_records: list[dict[str, str]] = []
    artifact_claims: dict[Path, tuple[str, str]] = {}
    role_candidates: dict[str, dict[str, Any]] = {}

    def claim_artifact(record: dict[str, Any], node_id: str, claim: str) -> None:
        """Prevent one capture from being presented as multiple acceptance events."""

        artifact_path = (artifact_root.resolve() / record["artifact"]).resolve()
        previous = artifact_claims.get(artifact_path)
        if previous is not None:
            raise EvidenceError(
                f"{claim}.artifact reuses evidence already claimed by "
                f"{previous[0]} at {previous[1]}"
            )
        artifact_claims[artifact_path] = (node_id, claim)

    for node in nodes:
        if not isinstance(node, dict):
            raise EvidenceError("each node must be an object")
        node_id = _text(node.get("id"), "node.id")
        if node_id in seen:
            raise EvidenceError(f"duplicate node id: {node_id}")
        seen.add(node_id)
        role = _text(node.get("role"), f"{node_id}.role")
        if role not in ROLES:
            raise EvidenceError(f"{node_id} has unsupported role {role!r}")
        role_counts[role] += 1
        source = _text(node.get("source"), f"{node_id}.source")
        if source not in SOURCES:
            raise EvidenceError(f"{node_id} has unsupported source {source!r}")
        if require_live and source != "live":
            raise EvidenceError(f"{node_id} is {source} evidence; live evidence is required")
        candidate: dict[str, Any] | None = None
        if source == "live":
            if "live_attestation" not in node:
                raise EvidenceError(f"{node_id}/live_attestation is required for live evidence")
            if set(node) != {
                "candidate",
                "id",
                "live_attestation",
                "recovery",
                "role",
                "scenarios",
                "source",
            }:
                raise EvidenceError(f"{node_id} must contain exactly the live collector fields")
            candidate = _candidate(
                node.get("candidate"),
                node_id=node_id,
                role=role,
                revision=revision,
                manifest_sha256=manifest_sha256,
            )
            previous_candidate = role_candidates.get(role)
            if previous_candidate is not None and candidate != previous_candidate:
                raise EvidenceError(
                    f"{node_id}/candidate does not match the other {role} candidate payload"
                )
            role_candidates[role] = candidate
            _live_attestation(
                node.get("live_attestation"),
                node_id,
                revision,
                now_ms,
                max_age_ms,
                artifact_root,
            )
        scenarios = node.get("scenarios")
        if not isinstance(scenarios, dict) or set(scenarios) != set(SCENARIOS):
            raise EvidenceError(f"{node_id} must provide exactly the six required scenarios")
        for scenario in SCENARIOS:
            _scenario(scenarios[scenario], node_id, scenario, now_ms, max_age_ms, artifact_root)
            if candidate is not None:
                _live_claim_artifact(
                    scenarios[scenario],
                    node_id=node_id,
                    revision=revision,
                    generated_at_ms=generated,
                    artifact_root=artifact_root,
                    candidate=candidate,
                    kind=LIVE_SCENARIO_KIND,
                    claim_name=scenario,
                )
            claim_artifact(scenarios[scenario], node_id, f"{node_id}/{scenario}")
        if source == "live":
            claim_artifact(
                node["live_attestation"], node_id, f"{node_id}/live_attestation"
            )
        recovery = node.get("recovery")
        recovery_records.append(
            _recovery(
                recovery,
                node_id,
                revision,
                now_ms,
                max_age_ms,
                artifact_root,
            )
        )
        for index, state in enumerate(recovery["states"]):
            if candidate is not None:
                _live_claim_artifact(
                    state,
                    node_id=node_id,
                    revision=revision,
                    generated_at_ms=generated,
                    artifact_root=artifact_root,
                    candidate=candidate,
                    kind=LIVE_RECOVERY_KIND,
                    claim_name=f"recovery.states[{index}]",
                    recovery_kind="state",
                )
            claim_artifact(state, node_id, f"{node_id}/recovery.states[{index}]")
        if candidate is not None:
            _live_claim_artifact(
                recovery["failover"],
                node_id=node_id,
                revision=revision,
                generated_at_ms=generated,
                artifact_root=artifact_root,
                candidate=candidate,
                kind=LIVE_RECOVERY_KIND,
                claim_name="recovery.failover",
                recovery_kind="failover",
            )
            _live_claim_artifact(
                recovery["corrected_forward"],
                node_id=node_id,
                revision=revision,
                generated_at_ms=generated,
                artifact_root=artifact_root,
                candidate=candidate,
                kind=LIVE_RECOVERY_KIND,
                claim_name="recovery.corrected_forward",
                recovery_kind="corrected_forward",
            )
        claim_artifact(
            recovery["failover"], node_id, f"{node_id}/recovery.failover"
        )
        claim_artifact(
            recovery["corrected_forward"],
            node_id,
            f"{node_id}/recovery.corrected_forward",
        )
        normalized_nodes.append({"id": node_id, "role": role, "source": source})

    for role, expected in ROLES.items():
        if role_counts[role] != expected:
            raise EvidenceError(f"topology requires {expected} {role}s, found {role_counts[role]}")
    lighthouse_ids = {
        node["id"] for node in normalized_nodes if node["role"] == "lighthouse"
    }
    for recovery in recovery_records:
        if recovery["failed_lighthouse_id"] not in lighthouse_ids:
            raise EvidenceError(
                "recovery.failover.failed_lighthouse_id must name a lighthouse"
            )
        if recovery["active_lighthouse_id"] not in lighthouse_ids:
            raise EvidenceError(
                "recovery.failover.active_lighthouse_id must name a lighthouse"
            )
    return {
        "schema": SCHEMA,
        "revision": revision,
        "node_count": len(normalized_nodes),
        "role_counts": role_counts,
        "sources": sorted({node["source"] for node in normalized_nodes}),
        "scenarios_per_node": len(SCENARIOS),
        "recovery_states": list(RECOVERY_STATES),
        "recovery_nodes": len(recovery_records),
        "live_required": require_live,
    }


def _fixture(source: str = "farm", *, now_ms: int = 1_760_000_000_000) -> dict[str, Any]:
    nodes = []
    for role, prefix in (("lighthouse", "lh"), ("workstation", "ws")):
        for index in range(1, 4):
            scenarios = {
                name: {
                    "status": "pass",
                    "observed_at_ms": now_ms,
                    "command": f"six-node-drill {name} --node {prefix}-{index}",
                    "artifact": f"evidence/{prefix}-{index}/scenarios/{name}.json",
                    "sha256": hashlib.sha256(f"{prefix}-{index}-{name}".encode()).hexdigest(),
                }
                for name in SCENARIOS
            }
            node_id = f"{prefix}-{index}"
            node = {"id": node_id, "role": role, "source": source, "scenarios": scenarios}
            recovery_states = []
            for state_index, state_name in enumerate(RECOVERY_STATES):
                observed_at_ms = now_ms - (len(RECOVERY_STATES) - state_index - 1) * 1_000
                recovery_states.append(
                    {
                        "state": state_name,
                        "status": "pass",
                        "observed_at_ms": observed_at_ms,
                        "command": (
                            f"six-node-recovery {state_name} --node {node_id}"
                        ),
                        "artifact": f"evidence/{node_id}/recovery/state-{state_index}.json",
                        "sha256": hashlib.sha256(
                            f"{node_id}-recovery-state-{state_index}".encode()
                        ).hexdigest(),
                        "node_id": node_id,
                    }
                )
            node["recovery"] = {
                "node_id": node_id,
                "states": recovery_states,
                "failover": {
                    "status": "pass",
                    "observed_at_ms": now_ms,
                    "command": f"six-node-failover --node {node_id}",
                    "artifact": f"evidence/{node_id}/recovery/failover.json",
                    "sha256": hashlib.sha256(f"{node_id}-failover".encode()).hexdigest(),
                    "node_id": node_id,
                    "failed_lighthouse_id": "lh-1",
                    "active_lighthouse_id": "lh-2",
                    "automatic": True,
                },
                "corrected_forward": {
                    "status": "pass",
                    "observed_at_ms": now_ms,
                    "command": f"six-node-corrected-forward --node {node_id}",
                    "artifact": f"evidence/{node_id}/recovery/corrected-forward.json",
                    "sha256": hashlib.sha256(
                        f"{node_id}-corrected-forward".encode()
                    ).hexdigest(),
                    "node_id": node_id,
                    "previous_revision": "previous-revision",
                    "forward_revision": "test-revision",
                    "re_enrolled": True,
                    "rollback": False,
                },
            }
            if source == "live":
                manifest_sha256 = hashlib.sha256(b"test-candidate-manifest").hexdigest()
                binaries = {"mackesd": hashlib.sha256(f"{role}-mackesd".encode()).hexdigest()}
                if role == "workstation":
                    binaries["mde-shell-egui"] = hashlib.sha256(
                        b"workstation-mde-shell-egui"
                    ).hexdigest()
                node["candidate"] = {
                    "revision": "test-revision",
                    "manifest_sha256": manifest_sha256,
                    "package": f"magic-mesh-{role} 12.1.6-1.x86_64",
                    "package_payload_sha256": hashlib.sha256(
                        f"{role}-rpm-payload".encode()
                    ).hexdigest(),
                    "binaries": binaries,
                }
                node["live_attestation"] = {
                    "status": "pass",
                    "observed_at_ms": now_ms,
                    "command": f"ssh {node_id} capture-six-node-attestation",
                    "artifact": f"evidence/{node_id}/live-attestation.json",
                    "sha256": hashlib.sha256(f"{node_id}-live-attestation".encode()).hexdigest(),
                    "node_id": node_id,
                    "transport": "ssh",
                }
            nodes.append(node)
    bundle = {
        "schema": SCHEMA,
        "revision": "test-revision",
        "generated_at_ms": now_ms,
        "nodes": nodes,
    }
    if source == "live":
        bundle["candidate_manifest_sha256"] = hashlib.sha256(
            b"test-candidate-manifest"
        ).hexdigest()
    return bundle


def _materialize_fixture(bundle: dict[str, Any], root: Path) -> None:
    # Release-evidence's fixture probe may bind the synthetic bundle to a
    # different source revision between _fixture() and materialization. Keep
    # that test-only fixture internally coherent without relaxing validate().
    revision = _text(bundle["revision"], "revision")
    for node in bundle["nodes"]:
        if node["source"] == "live":
            node["candidate"]["revision"] = revision
        for name, record in node["scenarios"].items():
            artifact = root / record["artifact"]
            artifact.parent.mkdir(parents=True, exist_ok=True)
            if node["source"] == "live":
                contents = {
                    "schema_version": 1,
                    "kind": LIVE_SCENARIO_KIND,
                    "node_id": node["id"],
                    "hostname": node["id"],
                    "machine_id_sha256": hashlib.sha256(node["id"].encode()).hexdigest(),
                    "revision": revision,
                    "scenario": name,
                    "candidate": node["candidate"],
                    "outcome": {
                        key: value
                        for key, value in record.items()
                        if key not in {"artifact", "sha256"}
                    },
                    "collected_at_ms": bundle["generated_at_ms"],
                }
                artifact_bytes = json.dumps(
                    contents, sort_keys=True, separators=(",", ":")
                ).encode()
                artifact.write_bytes(artifact_bytes)
                record["sha256"] = hashlib.sha256(artifact_bytes).hexdigest()
            else:
                artifact.write_bytes(f"{node['id']}-{name}".encode())
        recovery = node["recovery"]
        recovery["corrected_forward"]["forward_revision"] = revision
        for state_index, record in enumerate(recovery["states"]):
            artifact = root / record["artifact"]
            artifact.parent.mkdir(parents=True, exist_ok=True)
            if node["source"] == "live":
                contents = {
                    "schema_version": 1,
                    "kind": LIVE_RECOVERY_KIND,
                    "node_id": node["id"],
                    "revision": revision,
                    "recovery_kind": "state",
                    "candidate": node["candidate"],
                    "outcome": {
                        key: value
                        for key, value in record.items()
                        if key not in {"artifact", "sha256"}
                    },
                    "collected_at_ms": bundle["generated_at_ms"],
                }
                artifact_bytes = json.dumps(
                    contents, sort_keys=True, separators=(",", ":")
                ).encode()
                artifact.write_bytes(artifact_bytes)
                record["sha256"] = hashlib.sha256(artifact_bytes).hexdigest()
            else:
                artifact.write_bytes(f"{node['id']}-recovery-state-{state_index}".encode())
        for record, contents in (
            (recovery["failover"], f"{node['id']}-failover"),
            (recovery["corrected_forward"], f"{node['id']}-corrected-forward"),
        ):
            artifact = root / record["artifact"]
            artifact.parent.mkdir(parents=True, exist_ok=True)
            if node["source"] == "live":
                recovery_kind = (
                    "failover" if record is recovery["failover"] else "corrected_forward"
                )
                typed_contents = {
                    "schema_version": 1,
                    "kind": LIVE_RECOVERY_KIND,
                    "node_id": node["id"],
                    "revision": revision,
                    "recovery_kind": recovery_kind,
                    "candidate": node["candidate"],
                    "outcome": {
                        key: value
                        for key, value in record.items()
                        if key not in {"artifact", "sha256"}
                    },
                    "collected_at_ms": bundle["generated_at_ms"],
                }
                artifact_bytes = json.dumps(
                    typed_contents, sort_keys=True, separators=(",", ":")
                ).encode()
                artifact.write_bytes(artifact_bytes)
                record["sha256"] = hashlib.sha256(artifact_bytes).hexdigest()
            else:
                artifact.write_bytes(contents.encode())
        if "live_attestation" in node:
            record = node["live_attestation"]
            artifact = root / record["artifact"]
            artifact.parent.mkdir(parents=True, exist_ok=True)
            marker = {
                "kind": LIVE_ATTESTATION_KIND,
                "node_id": node["id"],
                "observed_at_ms": record["observed_at_ms"],
                "revision": revision,
                "source": "live",
                "transport": record["transport"],
            }
            marker_bytes = json.dumps(marker, sort_keys=True, separators=(",", ":")).encode()
            artifact.write_bytes(marker_bytes)
            record["sha256"] = hashlib.sha256(marker_bytes).hexdigest()


def self_test() -> None:
    now = 1_760_000_000_000
    positive_cases = 0
    negative_cases = 0
    with tempfile.TemporaryDirectory(prefix="six-node-verifier-") as temporary:
        root = Path(temporary)
        bundle = _fixture()
        _materialize_fixture(bundle, root)
        result = validate(
            bundle, now_ms=now, max_age_ms=60_000, require_live=False, artifact_root=root
        )
        assert result["node_count"] == 6
        assert result["recovery_states"] == list(RECOVERY_STATES)
        assert result["recovery_nodes"] == 6
        positive_cases += 1
        live_bundle = _fixture(source="live")
        _materialize_fixture(live_bundle, root)
        live_result = validate(
            live_bundle,
            now_ms=now,
            max_age_ms=60_000,
            require_live=True,
            artifact_root=root,
            expected_revision="test-revision",
        )
        assert live_result["sources"] == ["live"]
        positive_cases += 1
        mismatched_revision = _fixture(source="live")
        _materialize_fixture(mismatched_revision, root)
        try:
            validate(
                mismatched_revision,
                now_ms=now,
                max_age_ms=60_000,
                require_live=True,
                artifact_root=root,
                expected_revision="release-revision",
            )
        except EvidenceError as exc:
            assert "does not match expected source revision" in str(exc), exc
            negative_cases += 1
        else:
            raise AssertionError("topology from a different source revision unexpectedly passed")
        for mutation, expected in (
            (lambda b: b["nodes"].pop(), "exactly six"),
            (lambda b: b["nodes"][0].update({"role": "workstation"}), "workstation runtime"),
            (lambda b: b["nodes"][0]["scenarios"].pop("loss"), "exactly the six"),
            (lambda b: b["nodes"][0].update({"source": "farm"}), "live evidence"),
        ):
            candidate = json.loads(json.dumps(_fixture(source="live")))
            _materialize_fixture(candidate, root)
            mutation(candidate)
            try:
                validate(
                    candidate,
                    now_ms=now,
                    max_age_ms=60_000,
                    require_live=(expected == "live evidence"),
                    artifact_root=root,
                )
            except EvidenceError as exc:
                assert expected in str(exc), (expected, exc)
                negative_cases += 1
            else:
                raise AssertionError(f"mutation unexpectedly passed: {expected}")
        missing_attestation = json.loads(json.dumps(_fixture(source="live")))
        del missing_attestation["nodes"][0]["live_attestation"]
        _materialize_fixture(missing_attestation, root)
        try:
            validate(
                missing_attestation,
                now_ms=now,
                max_age_ms=60_000,
                require_live=False,
                artifact_root=root,
            )
        except EvidenceError as exc:
            assert "live_attestation" in str(exc), exc
            negative_cases += 1
        else:
            raise AssertionError("live evidence without attestation unexpectedly passed")
        unbound_marker = json.loads(json.dumps(_fixture(source="live")))
        _materialize_fixture(unbound_marker, root)
        marker_record = unbound_marker["nodes"][0]["live_attestation"]
        marker_path = root / marker_record["artifact"]
        marker = json.loads(marker_path.read_text(encoding="utf-8"))
        marker["source"] = "farm"
        marker_bytes = json.dumps(marker, sort_keys=True, separators=(",", ":")).encode()
        marker_path.write_bytes(marker_bytes)
        marker_record["sha256"] = hashlib.sha256(marker_bytes).hexdigest()
        try:
            validate(
                unbound_marker,
                now_ms=now,
                max_age_ms=60_000,
                require_live=True,
                artifact_root=root,
            )
        except EvidenceError as exc:
            assert "exactly bind the live source" in str(exc), exc
            negative_cases += 1
        else:
            raise AssertionError("a farm marker relabeled as live unexpectedly passed")
        untyped_live_claim = _fixture(source="live")
        _materialize_fixture(untyped_live_claim, root)
        untyped_record = untyped_live_claim["nodes"][0]["scenarios"]["join"]
        untyped_path = root / untyped_record["artifact"]
        untyped_path.write_bytes(b"operator asserted pass")
        untyped_record["sha256"] = hashlib.sha256(untyped_path.read_bytes()).hexdigest()
        try:
            validate(
                untyped_live_claim,
                now_ms=now,
                max_age_ms=60_000,
                require_live=True,
                artifact_root=root,
            )
        except EvidenceError as exc:
            assert "must contain typed JSON" in str(exc), exc
            negative_cases += 1
        else:
            raise AssertionError("arbitrary bytes were accepted as a live drill claim")
        split_candidate = _fixture(source="live")
        _materialize_fixture(split_candidate, root)
        split_candidate["nodes"][4]["candidate"]["package"] = (
            "magic-mesh-workstation 12.1.7-1.x86_64"
        )
        try:
            validate(
                split_candidate,
                now_ms=now,
                max_age_ms=60_000,
                require_live=True,
                artifact_root=root,
            )
        except EvidenceError as exc:
            assert "does not match the other workstation candidate payload" in str(exc), exc
            negative_cases += 1
        else:
            raise AssertionError("a split workstation candidate was accepted as one fleet revision")
        reused_artifact = _fixture()
        _materialize_fixture(reused_artifact, root)
        first_join = reused_artifact["nodes"][0]["scenarios"]["join"]
        second_join = reused_artifact["nodes"][1]["scenarios"]["join"]
        second_join["artifact"] = first_join["artifact"]
        second_join["sha256"] = first_join["sha256"]
        try:
            validate(
                reused_artifact,
                now_ms=now,
                max_age_ms=60_000,
                require_live=False,
                artifact_root=root,
            )
        except EvidenceError as exc:
            assert "reuses evidence already claimed by lh-1" in str(exc), exc
            negative_cases += 1
        else:
            raise AssertionError("one artifact satisfying multiple claims unexpectedly passed")
        reused_same_node_artifact = _fixture()
        _materialize_fixture(reused_same_node_artifact, root)
        join = reused_same_node_artifact["nodes"][0]["scenarios"]["join"]
        loss = reused_same_node_artifact["nodes"][0]["scenarios"]["loss"]
        loss["artifact"] = join["artifact"]
        loss["sha256"] = join["sha256"]
        try:
            validate(
                reused_same_node_artifact,
                now_ms=now,
                max_age_ms=60_000,
                require_live=False,
                artifact_root=root,
            )
        except EvidenceError as exc:
            assert "reuses evidence already claimed by lh-1 at lh-1/join" in str(exc), exc
            negative_cases += 1
        else:
            raise AssertionError(
                "one capture satisfying multiple same-node drills unexpectedly passed"
            )
        tampered = json.loads(json.dumps(bundle))
        first_artifact = root / tampered["nodes"][0]["scenarios"]["join"]["artifact"]
        first_artifact.write_bytes(b"tampered")
        try:
            validate(tampered, now_ms=now, max_age_ms=60_000, require_live=False, artifact_root=root)
        except EvidenceError as exc:
            assert "digest mismatch" in str(exc)
            negative_cases += 1
        else:
            raise AssertionError("tampered artifact unexpectedly passed")
        oversized = json.loads(json.dumps(bundle))
        oversized_record = oversized["nodes"][0]["scenarios"]["join"]
        oversized_path = root / oversized_record["artifact"]
        oversized_path.write_bytes(b"x" * (MAX_ARTIFACT_BYTES + 1))
        oversized_record["sha256"] = hashlib.sha256(oversized_path.read_bytes()).hexdigest()
        try:
            validate(oversized, now_ms=now, max_age_ms=60_000, require_live=False, artifact_root=root)
        except EvidenceError as exc:
            assert "exceeds the" in str(exc), exc
            negative_cases += 1
        else:
            raise AssertionError("oversized artifact unexpectedly passed")
        missing = json.loads(json.dumps(bundle))
        (root / missing["nodes"][0]["scenarios"]["join"]["artifact"]).unlink()
        try:
            validate(missing, now_ms=now, max_age_ms=60_000, require_live=False, artifact_root=root)
        except EvidenceError as exc:
            assert "missing" in str(exc)
            negative_cases += 1
        else:
            raise AssertionError("missing artifact unexpectedly passed")
        stale = _fixture()
        _materialize_fixture(stale, root)
        stale["generated_at_ms"] = now - 60_001
        try:
            validate(stale, now_ms=now, max_age_ms=60_000, require_live=False, artifact_root=root)
        except EvidenceError as exc:
            assert "stale" in str(exc)
            negative_cases += 1
        else:
            raise AssertionError("stale evidence unexpectedly passed")
        for mutation, expected in (
            (lambda b: b["nodes"][0].pop("recovery"), "/recovery"),
            (
                lambda b: b["nodes"][0]["recovery"]["states"][2].update(
                    {"state": "healthy"}
                ),
                "must transition",
            ),
            (
                lambda b: b["nodes"][0]["recovery"]["states"][1].update(
                    {"node_id": "ws-1"}
                ),
                "node_id must match",
            ),
            (
                lambda b: b["nodes"][0]["recovery"]["failover"].update(
                    {"active_lighthouse_id": "lh-1"}
                ),
                "must change lighthouse",
            ),
            (
                lambda b: b["nodes"][0]["recovery"]["corrected_forward"].update(
                    {"rollback": True}
                ),
                "must not use rollback",
            ),
        ):
            candidate = json.loads(json.dumps(_fixture()))
            _materialize_fixture(candidate, root)
            mutation(candidate)
            try:
                validate(
                    candidate,
                    now_ms=now,
                    max_age_ms=60_000,
                    require_live=False,
                    artifact_root=root,
                )
            except EvidenceError as exc:
                assert expected in str(exc), (expected, exc)
                negative_cases += 1
            else:
                raise AssertionError(f"recovery mutation unexpectedly passed: {expected}")
    print(
        "verify-six-node-topology: self-test passed "
        f"({positive_cases} positive, {negative_cases} negative cases)"
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, help="six-node evidence JSON bundle")
    parser.add_argument("--max-age-seconds", type=int, default=86_400)
    parser.add_argument("--require-live", action="store_true", help="reject farm-only evidence")
    parser.add_argument(
        "--expected-revision",
        help="require the evidence bundle revision to equal this source revision",
    )
    parser.add_argument("--now-ms", type=int, help="test clock override")
    parser.add_argument("--json", action="store_true", help="emit the validated summary as JSON")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.self_test:
        self_test()
        return 0
    if args.evidence is None:
        print("verify-six-node-topology: --evidence is required (no live evidence is synthesized)", file=sys.stderr)
        return 2
    if args.max_age_seconds < 0:
        print("verify-six-node-topology: --max-age-seconds must be non-negative", file=sys.stderr)
        return 2
    try:
        bundle = json.loads(args.evidence.read_text(encoding="utf-8"))
        summary = validate(
            bundle,
            now_ms=args.now_ms if args.now_ms is not None else time.time_ns() // 1_000_000,
            max_age_ms=args.max_age_seconds * 1000,
            require_live=args.require_live,
            artifact_root=args.evidence.parent,
            expected_revision=args.expected_revision,
        )
    except (OSError, json.JSONDecodeError, EvidenceError) as exc:
        print(f"verify-six-node-topology: BLOCKED: {exc}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(
            "verify-six-node-topology: PASS — "
            f"{summary['role_counts']['lighthouse']} lighthouses + "
            f"{summary['role_counts']['workstation']} workstations; "
            f"sources={','.join(summary['sources'])}; live_required={summary['live_required']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
