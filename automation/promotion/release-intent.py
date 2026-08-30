#!/usr/bin/env python3
"""ReleaseIntentV1 and ReleaseStateV1 contracts for the 13.0.0 coordinator.

Unsigned drafts are not authorization. This helper never invents dests,
never flips production_admitted, and never admits Android/Cuttlefish.
Credential fields are names only.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path


SCHEMA_INTENT = "ReleaseIntentV1"
SCHEMA_STATE = "ReleaseStateV1"
VERSION = "13.0.0"
ROLES = (
    "workstation-rpm",
    "server-rpm",
    "lighthouse-rpm",
    "browser-vm",
    "app-vm",
    "bootc-image",
)
TARGETS = ("dell", "seat-15", "surface", "lh1", "lh2", "lh3")
DESTRUCTIVE = frozenset({"none", "lighthouse-one-at-a-time", "seat-wave"})
REVISION = re.compile(r"[0-9a-f]{40}\Z")
EPOCH = re.compile(r"[1-9][0-9]{0,11}\Z")
CREDENTIAL_NAME = re.compile(r"[A-Za-z][A-Za-z0-9._+-]{0,63}\Z")
FORBIDDEN_KEYS = frozenset(
    {
        "mesh_id",
        "bearer",
        "overlay_ip",
        "etcd_endpoints",
        "token",
        "private_key",
        "password",
        "secret",
    }
)
REFUSED_MARKERS = ("android", "cuttlefish")
MAX_DOC = 64 * 1024


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            refuse(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def load_object(path: Path) -> dict[str, object]:
    if path.is_symlink() or not path.is_file():
        refuse(f"{path} must be a regular non-symlink file")
    raw = path.read_bytes()
    if not 0 < len(raw) <= MAX_DOC:
        refuse("document exceeds the bounded size")
    try:
        parsed = json.loads(raw, object_pairs_hook=unique_object)
    except json.JSONDecodeError as exc:
        refuse(f"document is not object JSON: {exc}")
    if not isinstance(parsed, dict):
        refuse("document must be a JSON object")
    return parsed


def refuse_forbidden(obj: dict[str, object]) -> None:
    for key in obj:
        lowered = key.lower()
        if key in FORBIDDEN_KEYS or any(marker in lowered for marker in REFUSED_MARKERS):
            refuse(f"{key} is not a release-contract field")


def refuse_markers(*values: object) -> None:
    blob = json.dumps(values).lower()
    for marker in REFUSED_MARKERS:
        if marker in blob:
            refuse(f"{marker} is deferred and is not a 13.0.0 release input")


def validate_intent(obj: dict[str, object]) -> dict[str, object]:
    refuse_forbidden(obj)
    if obj.get("schema") != SCHEMA_INTENT:
        refuse("schema must be ReleaseIntentV1")
    if obj.get("version") != VERSION:
        refuse("version must be 13.0.0")
    revision = obj.get("source_revision")
    if not isinstance(revision, str) or not REVISION.fullmatch(revision):
        refuse("source_revision must be a 40-character lowercase hex SHA")
    epoch = obj.get("epoch")
    if not isinstance(epoch, str) or not EPOCH.fullmatch(epoch):
        refuse("epoch must be a positive decimal second")
    roles = obj.get("roles")
    if not isinstance(roles, list) or tuple(roles) != ROLES:
        refuse("roles must be exactly the six-role set")
    targets = obj.get("targets")
    if not isinstance(targets, list) or tuple(targets) != TARGETS:
        refuse("targets must be Dell, Seat 15, Surface, and the three lighthouses")
    names = obj.get("credential_names")
    if not isinstance(names, list) or not names:
        refuse("credential_names must name at least one systemd/mde-seal input")
    for name in names:
        if not isinstance(name, str) or not CREDENTIAL_NAME.fullmatch(name):
            refuse("credential fields are names only")
    scope = obj.get("destructive_scope")
    if scope not in DESTRUCTIVE:
        refuse("destructive_scope is outside the bounded set")
    if obj.get("production_admitted") is not False:
        refuse("production_admitted must stay false; do not flip it here")
    admitted = obj.get("admitted")
    signature = obj.get("signature")
    if admitted is True:
        if not isinstance(signature, str) or not re.fullmatch(r"[0-9a-f]{64,256}", signature):
            refuse("admitted requires a hex signature from the named release credential")
    elif admitted is not False:
        refuse("admitted must be a boolean")
    elif signature not in (None, ""):
        refuse("signature is refused on an unadmitted draft")
    refuse_markers(roles, targets, names, scope)
    return obj


def draft_intent(source_revision: str, epoch: str, credential_names: list[str]) -> dict[str, object]:
    obj = {
        "schema": SCHEMA_INTENT,
        "version": VERSION,
        "source_revision": source_revision,
        "epoch": epoch,
        "roles": list(ROLES),
        "targets": list(TARGETS),
        "credential_names": credential_names,
        "destructive_scope": "lighthouse-one-at-a-time",
        "production_admitted": False,
        "admitted": False,
        "signature": None,
    }
    return validate_intent(obj)


def validate_state(obj: dict[str, object], intent: dict[str, object]) -> dict[str, object]:
    refuse_forbidden(obj)
    if obj.get("schema") != SCHEMA_STATE:
        refuse("schema must be ReleaseStateV1")
    if obj.get("source_revision") != intent["source_revision"]:
        refuse("state source_revision must match the bound intent")
    generation = obj.get("generation")
    if not isinstance(generation, int) or generation < 1:
        refuse("generation must be a positive integer")
    status = obj.get("status")
    if status not in {"blocked", "ready", "running", "passed"}:
        refuse("status is outside the bounded set")
    if status == "passed" and intent.get("admitted") is not True:
        refuse("state cannot pass without an admitted signed intent")
    return obj


def cas_state(path: Path, incoming: dict[str, object], intent: dict[str, object]) -> dict[str, object]:
    validate_state(incoming, intent)
    if path.exists():
        current = validate_state(load_object(path), intent)
        expected = incoming.get("expected_generation")
        if expected != current["generation"]:
            refuse("state compare-and-swap lost; retry from the current generation")
        if incoming["generation"] != current["generation"] + 1:
            refuse("state generation must advance by one")
    elif incoming.get("generation") != 1:
        refuse("first state generation must be 1")
    stored = {key: value for key, value in incoming.items() if key != "expected_generation"}
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(stored, separators=(",", ":"), sort_keys=True) + "\n")
    tmp.replace(path)
    return stored


def write_json(path: Path, obj: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(obj, separators=(",", ":"), sort_keys=True) + "\n")
    tmp.replace(path)


def self_test() -> None:
    revision = "a" * 40
    names = ["rpm-signer"]
    intent = draft_intent(revision, "1787450205", names)
    if intent["admitted"] or intent["production_admitted"]:
        refuse("draft must remain unadmitted")
    bad_roles = dict(intent)
    bad_roles["roles"] = [*ROLES, "cuttlefish-image"]
    try:
        validate_intent(bad_roles)
    except Refusal:
        pass
    else:
        refuse("Cuttlefish roles must refuse")
    dest = dict(intent)
    dest["mesh_id"] = "invented"
    try:
        validate_intent(dest)
    except Refusal:
        pass
    else:
        refuse("invented dest fields must refuse")
    admitted = dict(intent)
    admitted["admitted"] = True
    admitted["signature"] = "ab"
    try:
        validate_intent(admitted)
    except Refusal:
        pass
    else:
        refuse("short signature must refuse admitted")
    with tempfile.TemporaryDirectory() as tmp:
        state_path = Path(tmp) / "state.json"
        first = {
            "schema": SCHEMA_STATE,
            "source_revision": revision,
            "stage": "inputs",
            "status": "blocked",
            "generation": 1,
        }
        cas_state(state_path, first, intent)
        second = dict(first)
        second["generation"] = 2
        second["expected_generation"] = 1
        second["status"] = "ready"
        cas_state(state_path, second, intent)
        stale = dict(second)
        stale["generation"] = 3
        stale["expected_generation"] = 1
        try:
            cas_state(state_path, stale, intent)
        except Refusal:
            pass
        else:
            refuse("stale generation must refuse")
        passed = dict(second)
        passed["generation"] = 3
        passed["expected_generation"] = 2
        passed["status"] = "passed"
        try:
            cas_state(state_path, passed, intent)
        except Refusal:
            pass
        else:
            refuse("unadmitted pass must refuse")
    print("release-intent: ALL PASS")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--validate-intent")
    parser.add_argument("--write-draft")
    parser.add_argument("--source-revision")
    parser.add_argument("--epoch")
    parser.add_argument("--credential-name", action="append", default=[])
    args = parser.parse_args(argv)
    try:
        if args.self_test:
            self_test()
            return 0
        if args.validate_intent:
            validate_intent(load_object(Path(args.validate_intent)))
            print("release-intent: PASS: intent")
            return 0
        if args.write_draft:
            if not args.source_revision or not args.epoch or not args.credential_name:
                refuse("write-draft needs --source-revision, --epoch, and --credential-name")
            write_json(
                Path(args.write_draft),
                draft_intent(args.source_revision, args.epoch, args.credential_name),
            )
            print(f"release-intent: PASS: wrote unadmitted draft to {args.write_draft}")
            return 0
        refuse("choose --self-test, --validate-intent, or --write-draft")
    except Refusal as exc:
        print(f"release-intent: REFUSE: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
