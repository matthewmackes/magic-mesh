#!/usr/bin/env python3
"""Fail-closed verifier for the canonical WL-CRIT-006 release gate matrix."""

from __future__ import annotations

import argparse
import copy
import json
import re
import sys
import tempfile
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MATRIX = ROOT / "install-helpers" / "release-gate-matrix.json"
REVISION_RE = re.compile(r"[0-9a-f]{40}")
ID_RE = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
EVIDENCE_RE = re.compile(r"docs/platform/release-evidence/[0-9a-f]{40}/[a-z0-9][a-z0-9.-]*\.json")
# Gate commands are recorded as one bounded command line and may contain
# parameter expansion (the canonical matrix uses ${MCNF_*}), but they must not
# carry shell control syntax that could turn an evidence claim into a second
# command when a downstream runner evaluates the field.
COMMAND_CONTROL_RE = re.compile(r"[;&|<>`]|[$][(]")

TOP_KEYS = {
    "schema_version", "kind", "source_revision", "required_gate_ids",
    "rosters", "gates",
}
ROSTER_KEYS = {"github_checks", "farm_packages", "seats", "lighthouses", "scenarios"}
GATE_KEYS = {
    "gate_id", "scope_kind", "scope_id", "categories", "owner", "command",
    "evidence_filename", "pass_condition", "revision_ref", "required",
}

EXPECTED_ROSTERS = {
    "github_checks": ["github-required"],
    "farm_packages": [
        "farm-ci", "workstation-rpm", "lighthouse-rpm",
        "workloads-rpm-transaction",
    ],
    "seats": ["dell", "seat15", "surface"],
    "lighthouses": [
        "lh-104-236-118-177", "lh-46-101-219-245", "lh-64-23-131-57",
    ],
    "scenarios": [
        "process-failure", "network-loss", "sleep-resume", "node-reboot",
        "provider-loss", "package-failure", "peer-loss-corrected-forward",
    ],
}

SCOPE_CATEGORIES = {
    "github_check": {"ci"},
    "farm_package": None,
    "seat": {"runtime", "gui", "network", "audio", "vdi", "package"},
    "lighthouse": {"runtime", "network", "package"},
    "scenario": {"recovery"},
}
FARM_CATEGORIES = {
    "farm-ci": {"farm"},
    "workstation-rpm": {"package"},
    "lighthouse-rpm": {"package"},
    "workloads-rpm-transaction": {"package"},
}
ALL_CATEGORIES = {
    "ci", "farm", "runtime", "gui", "network", "audio", "vdi", "package",
    "recovery",
}

WORKLOADS_RPM_TRANSACTION_SCOPE = "workloads-rpm-transaction"
WORKLOADS_RPM_TRANSACTION_PASS_CONDITION = (
    "the exact Workloads compute RPMs pass hard dependency headers, payload identity, "
    "repository install and upgrade transactions, and ordered retired mackesd.service "
    "to grouped mackesd.target ownership handoff"
)


class MatrixError(ValueError):
    pass


def fail(message: str) -> None:
    raise MatrixError(message)


def require_exact_keys(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} must be an object")
    actual = set(value)
    if actual != keys:
        fail(f"{label} fields differ: missing={sorted(keys - actual)} unknown={sorted(actual - keys)}")
    return value


def require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value or value != value.strip():
        fail(f"{label} must be a non-empty trimmed string")
    if any(ord(char) < 32 for char in value):
        fail(f"{label} contains control characters")
    return value


def expected_gate_ids() -> list[str]:
    return sorted(
        [f"github-{value}" for value in EXPECTED_ROSTERS["github_checks"]]
        + [f"farm-package-{value}" for value in EXPECTED_ROSTERS["farm_packages"]]
        + [f"seat-{value}" for value in EXPECTED_ROSTERS["seats"]]
        + [f"lighthouse-{value}" for value in EXPECTED_ROSTERS["lighthouses"]]
        + [f"scenario-{value}" for value in EXPECTED_ROSTERS["scenarios"]]
    )


def expected_evidence_filename(
    revision: str, scope_kind: str, scope_id: str, gate_id: str
) -> str:
    if scope_kind == "github_check":
        basename = f"{scope_id}.json"
    elif scope_kind == "farm_package":
        basename = (
            "ci-gate-status.json" if scope_id == "farm-ci" else f"{scope_id}.json"
        )
    else:
        basename = f"{gate_id}.json"
    return f"docs/platform/release-evidence/{revision}/{basename}"


def validate_matrix(matrix: Any, expected_revision: str | None = None) -> None:
    matrix = require_exact_keys(matrix, TOP_KEYS, "matrix")
    if matrix["schema_version"] != 1 or matrix["kind"] != "mcnf-release-gate-matrix":
        fail("unsupported matrix schema or kind")
    revision = require_string(matrix["source_revision"], "source_revision")
    if not REVISION_RE.fullmatch(revision):
        fail("source_revision must be exactly 40 lowercase hexadecimal characters")
    if expected_revision is not None and revision != expected_revision:
        fail(f"source_revision does not match expected revision {expected_revision}")

    rosters = require_exact_keys(matrix["rosters"], ROSTER_KEYS, "rosters")
    for roster_name, expected in EXPECTED_ROSTERS.items():
        actual = rosters[roster_name]
        if not isinstance(actual, list) or not all(isinstance(item, str) for item in actual):
            fail(f"rosters.{roster_name} must be a string array")
        if len(actual) != len(set(actual)):
            fail(f"rosters.{roster_name} contains a duplicate claim")
        if actual != expected:
            fail(f"rosters.{roster_name} is incomplete, reordered, or contains an unknown claim")

    required_ids = matrix["required_gate_ids"]
    canonical_ids = expected_gate_ids()
    if not isinstance(required_ids, list) or not all(isinstance(item, str) for item in required_ids):
        fail("required_gate_ids must be a string array")
    if required_ids != canonical_ids:
        fail("required_gate_ids must be the sorted complete canonical gate allowlist")

    gates = matrix["gates"]
    if not isinstance(gates, list):
        fail("gates must be an array")
    ids: list[str] = []
    scope_claims: set[tuple[str, str]] = set()
    evidence_claims: set[str] = set()
    scenario_claims: set[str] = set()
    for index, raw_gate in enumerate(gates):
        label = f"gates[{index}]"
        gate = require_exact_keys(raw_gate, GATE_KEYS, label)
        gate_id = require_string(gate["gate_id"], f"{label}.gate_id")
        if not ID_RE.fullmatch(gate_id):
            fail(f"{label}.gate_id is malformed")
        ids.append(gate_id)
        scope_kind = require_string(gate["scope_kind"], f"{label}.scope_kind")
        scope_id = require_string(gate["scope_id"], f"{label}.scope_id")
        if scope_kind not in SCOPE_CATEGORIES:
            fail(f"{label} uses unknown scope_kind {scope_kind!r}")
        roster_name = {
            "github_check": "github_checks", "farm_package": "farm_packages",
            "seat": "seats", "lighthouse": "lighthouses", "scenario": "scenarios",
        }[scope_kind]
        if scope_id not in EXPECTED_ROSTERS[roster_name]:
            fail(f"{label} is an implied or unknown {scope_kind} gate: {scope_id!r}")
        claim = (scope_kind, scope_id)
        if claim in scope_claims:
            fail(f"duplicate node/gate claim for {scope_kind}:{scope_id}")
        scope_claims.add(claim)
        if scope_kind == "scenario":
            if scope_id in scenario_claims:
                fail(f"duplicate scenario claim: {scope_id}")
            scenario_claims.add(scope_id)

        expected_id = {
            "github_check": f"github-{scope_id}",
            "farm_package": f"farm-package-{scope_id}",
            "seat": f"seat-{scope_id}",
            "lighthouse": f"lighthouse-{scope_id}",
            "scenario": f"scenario-{scope_id}",
        }[scope_kind]
        if gate_id != expected_id:
            fail(f"{label}.gate_id must be {expected_id!r}")
        categories = gate["categories"]
        if not isinstance(categories, list) or not categories or not all(isinstance(item, str) for item in categories):
            fail(f"{label}.categories must be a non-empty string array")
        if categories != sorted(set(categories)):
            fail(f"{label}.categories must be sorted and unique")
        if not set(categories) <= ALL_CATEGORIES:
            fail(f"{label}.categories contains an unknown category")
        expected_categories = (
            FARM_CATEGORIES[scope_id] if scope_kind == "farm_package"
            else SCOPE_CATEGORIES[scope_kind]
        )
        if set(categories) != expected_categories:
            fail(f"{label}.categories is incomplete or inapplicable for {scope_kind}:{scope_id}")

        for field in ("owner", "command", "evidence_filename", "pass_condition"):
            require_string(gate[field], f"{label}.{field}")
        if not ID_RE.fullmatch(gate["owner"]):
            fail(f"{label}.owner must be a canonical identifier")
        if (
            "\n" in gate["command"]
            or "\r" in gate["command"]
            or len(gate["command"]) > 2048
            or COMMAND_CONTROL_RE.search(gate["command"])
        ):
            fail(f"{label}.command must be one bounded command line")
        if scope_kind == "seat":
            command = gate["command"]
            if "install-helpers/test-five-seat-core.py" not in command:
                fail(f"{label}.command must use the bounded three-seat collector")
            if "--required-baseline" not in command:
                fail(f"{label}.command must explicitly select the required three-seat baseline")
            # Reject both spellings: a required gate must never smuggle an
            # optional-seat inspection into the release baseline.
            if re.search(r"(?:^|\s)--inspect-seat(?:=|\s|$)", command):
                fail(f"{label}.command cannot promote optional seat inspection to a required gate")
        if scope_kind == "farm_package" and scope_id == WORKLOADS_RPM_TRANSACTION_SCOPE:
            expected_command = (
                "install-helpers/release-evidence.sh validate "
                f"docs/platform/release-evidence/{revision}/workloads-rpm-transaction.json"
            )
            if gate["command"] != expected_command:
                fail(
                    f"{label}.command must validate the revision-bound Workloads RPM "
                    "transaction evidence"
                )
            if gate["pass_condition"] != WORKLOADS_RPM_TRANSACTION_PASS_CONDITION:
                fail(
                    f"{label}.pass_condition must require dependencies, payload, "
                    "repository transaction, and upgrade-owner handoff"
                )
        evidence = gate["evidence_filename"]
        if not EVIDENCE_RE.fullmatch(evidence) or f"/{revision}/" not in evidence:
            fail(f"{label}.evidence_filename must be a revision-bound canonical JSON filename")
        expected_evidence = expected_evidence_filename(
            revision, scope_kind, scope_id, gate_id
        )
        if evidence != expected_evidence:
            fail(
                f"{label}.evidence_filename must bind {gate_id!r} to {expected_evidence!r}"
            )
        if evidence in evidence_claims:
            fail(f"{label}.evidence_filename is reused by independent gate claims")
        evidence_claims.add(evidence)
        if gate["revision_ref"] != "source_revision":
            fail(f"{label}.revision_ref must bind the sole canonical source_revision")
        if gate["required"] is not True:
            fail(f"{label} must be explicitly required")

    if len(ids) != len(set(ids)):
        fail("duplicate gate IDs are forbidden")
    if ids != sorted(ids):
        fail("gates must be sorted by gate_id")
    if ids != required_ids:
        fail("gates do not exactly realize required_gate_ids; implied/unknown gates are forbidden")


def read_matrix(path: Path) -> Any:
    if not path.is_file() or path.is_symlink():
        fail(f"matrix is not a regular non-symlink file: {path}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"could not decode matrix JSON: {error}")


def generated_fixture(revision: str) -> dict[str, Any]:
    rosters = copy.deepcopy(EXPECTED_ROSTERS)
    gates = []
    specs = (
        [("github_check", value) for value in rosters["github_checks"]]
        + [("farm_package", value) for value in rosters["farm_packages"]]
        + [("seat", value) for value in rosters["seats"]]
        + [("lighthouse", value) for value in rosters["lighthouses"]]
        + [("scenario", value) for value in rosters["scenarios"]]
    )
    for scope_kind, scope_id in specs:
        prefix = {
            "github_check": "github", "farm_package": "farm-package", "seat": "seat",
            "lighthouse": "lighthouse", "scenario": "scenario",
        }[scope_kind]
        categories = FARM_CATEGORIES[scope_id] if scope_kind == "farm_package" else SCOPE_CATEGORIES[scope_kind]
        gate_id = f"{prefix}-{scope_id}"
        workloads_transaction = (
            scope_kind == "farm_package"
            and scope_id == WORKLOADS_RPM_TRANSACTION_SCOPE
        )
        gates.append({
            "gate_id": gate_id, "scope_kind": scope_kind, "scope_id": scope_id,
            "categories": sorted(categories), "owner": "self-test-owner",
            "command": (
                "install-helpers/release-evidence.sh validate "
                f"docs/platform/release-evidence/{revision}/"
                "workloads-rpm-transaction.json"
                if workloads_transaction
                else "install-helpers/test-five-seat-core.py --required-baseline"
                if scope_kind == "seat"
                else f"self-test --gate {gate_id}"
            ),
            "evidence_filename": expected_evidence_filename(
                revision, scope_kind, scope_id, gate_id
            ),
            "pass_condition": (
                WORKLOADS_RPM_TRANSACTION_PASS_CONDITION
                if workloads_transaction else "typed result is pass"
            ),
            "revision_ref": "source_revision",
            "required": True,
        })
    gates.sort(key=lambda gate: gate["gate_id"])
    return {
        "schema_version": 1, "kind": "mcnf-release-gate-matrix",
        "source_revision": revision, "required_gate_ids": expected_gate_ids(),
        "rosters": rosters, "gates": gates,
    }


def self_test() -> None:
    revision = "0123456789abcdef0123456789abcdef01234567"
    valid = generated_fixture(revision)
    validate_matrix(valid, revision)
    mutations = []

    def case(name: str, mutate) -> None:
        fixture = copy.deepcopy(valid)
        mutate(fixture)
        mutations.append((name, fixture))

    case("duplicate gate id", lambda value: value["gates"].__setitem__(1, copy.deepcopy(value["gates"][0])))
    case("unknown implied gate", lambda value: value["gates"][0].__setitem__("scope_id", "invented"))
    case("missing owner", lambda value: value["gates"][0].__setitem__("owner", ""))
    case("missing command", lambda value: value["gates"][0].__setitem__("command", ""))
    case(
        "shell control command",
        lambda value: value["gates"][0].__setitem__(
            "command", "self-test --gate github-required; touch /tmp/forged"
        ),
    )
    case(
        "command substitution",
        lambda value: value["gates"][0].__setitem__(
            "command", "self-test --gate github-required $(touch /tmp/forged)"
        ),
    )
    case("missing evidence", lambda value: value["gates"][0].__setitem__("evidence_filename", ""))
    case(
        "reused evidence claim",
        lambda value: value["gates"][1].__setitem__(
            "evidence_filename", value["gates"][0]["evidence_filename"]
        ),
    )

    def swap_seat_evidence(value: dict[str, Any]) -> None:
        seats = [gate for gate in value["gates"] if gate["scope_kind"] == "seat"]
        seats[0]["evidence_filename"], seats[1]["evidence_filename"] = (
            seats[1]["evidence_filename"],
            seats[0]["evidence_filename"],
        )

    case("cross-wired evidence claims", swap_seat_evidence)
    case(
        "duplicate node claim",
        lambda value: value["gates"][next(
            index for index, gate in enumerate(value["gates"])
            if gate["scope_kind"] == "seat" and gate["scope_id"] == "surface"
        )].update(scope_id="dell", gate_id="seat-dell"),
    )
    case(
        "duplicate scenario claim",
        lambda value: value["gates"][next(
            index for index, gate in enumerate(value["gates"])
            if gate["scope_kind"] == "scenario" and gate["scope_id"] == "sleep-resume"
        )].update(scope_id="network-loss", gate_id="scenario-network-loss"),
    )
    case("malformed revision", lambda value: value.__setitem__("source_revision", "abc123"))
    case("incomplete seat roster", lambda value: value["rosters"]["seats"].pop())
    case(
        "optional seat inspection promoted to required gate",
        lambda value: value["gates"][next(
            index for index, gate in enumerate(value["gates"])
            if gate["scope_kind"] == "seat"
        )].__setitem__("command", "install-helpers/test-five-seat-core.py --inspect-seat eagle"),
    )
    case(
        "implicit required seat baseline",
        lambda value: value["gates"][next(
            index for index, gate in enumerate(value["gates"])
            if gate["scope_kind"] == "seat"
        )].__setitem__("command", "install-helpers/test-five-seat-core.py"),
    )
    case(
        "equals-form optional seat inspection promoted to required gate",
        lambda value: value["gates"][next(
            index for index, gate in enumerate(value["gates"])
            if gate["scope_kind"] == "seat"
        )].__setitem__(
            "command", "install-helpers/test-five-seat-core.py --required-baseline --inspect-seat=eagle"
        ),
    )
    case("unknown category", lambda value: value["gates"][0]["categories"].append("implied"))
    case("optional required gate", lambda value: value["gates"][0].__setitem__("required", False))
    case("unbound revision", lambda value: value["gates"][0].__setitem__("revision_ref", "HEAD"))
    case(
        "incomplete Workloads RPM transaction",
        lambda value: next(
            gate for gate in value["gates"]
            if gate["scope_id"] == WORKLOADS_RPM_TRANSACTION_SCOPE
        ).__setitem__(
            "pass_condition", "the Workloads RPM file exists and is below the size limit"
        ),
    )

    rejected = 0
    with tempfile.TemporaryDirectory(prefix="mcnf-release-gate-matrix-self-test-") as temp:
        for name, fixture in mutations:
            path = Path(temp) / f"fixture-{rejected}.json"
            path.write_text(json.dumps(fixture, sort_keys=True), encoding="utf-8")
            try:
                validate_matrix(read_matrix(path), revision)
            except MatrixError:
                rejected += 1
            else:
                fail(f"self-test hostile fixture was accepted: {name}")
    print(f"verify-release-gate-matrix: self-test PASS (1 valid, {rejected} hostile fixtures rejected)")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("matrix", nargs="?", type=Path, default=DEFAULT_MATRIX)
    parser.add_argument("--expected-revision")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.self_test:
            self_test()
            return 0
        if args.expected_revision is not None and not REVISION_RE.fullmatch(args.expected_revision):
            fail("--expected-revision must be exactly 40 lowercase hexadecimal characters")
        validate_matrix(read_matrix(args.matrix), args.expected_revision)
        print(f"verify-release-gate-matrix: PASS {args.matrix} ({len(expected_gate_ids())} explicit required gates)")
        return 0
    except MatrixError as error:
        print(f"verify-release-gate-matrix: FAIL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
