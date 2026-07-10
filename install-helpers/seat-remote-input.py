#!/usr/bin/env python3
# KDC-MESH-6 - default phone remote-input seat helper.
#
# mackesd passes one validated JSON event argument. This helper injects that
# event through Linux uinput when the daemon can access /dev/uinput. It exits 69
# for unavailable live seat access so mackesd can publish an honest unavailable
# state instead of reporting a fake injection.

from __future__ import annotations

import argparse
import contextlib
import fcntl
import io
import json
import os
import struct
import sys
import time
from dataclasses import dataclass
from typing import Iterable


EXIT_UNAVAILABLE = 69
EXIT_UNSUPPORTED = 65

EV_SYN = 0
EV_KEY = 1
EV_REL = 2

SYN_REPORT = 0

REL_X = 0
REL_Y = 1
REL_HWHEEL = 6
REL_WHEEL = 8

BTN_LEFT = 0x110
BTN_RIGHT = 0x111
BTN_MIDDLE = 0x112

KEY_ESC = 1
KEY_1 = 2
KEY_2 = 3
KEY_3 = 4
KEY_4 = 5
KEY_5 = 6
KEY_6 = 7
KEY_7 = 8
KEY_8 = 9
KEY_9 = 10
KEY_0 = 11
KEY_MINUS = 12
KEY_EQUAL = 13
KEY_BACKSPACE = 14
KEY_TAB = 15
KEY_Q = 16
KEY_W = 17
KEY_E = 18
KEY_R = 19
KEY_T = 20
KEY_Y = 21
KEY_U = 22
KEY_I = 23
KEY_O = 24
KEY_P = 25
KEY_LEFTBRACE = 26
KEY_RIGHTBRACE = 27
KEY_ENTER = 28
KEY_LEFTCTRL = 29
KEY_A = 30
KEY_S = 31
KEY_D = 32
KEY_F = 33
KEY_G = 34
KEY_H = 35
KEY_J = 36
KEY_K = 37
KEY_L = 38
KEY_SEMICOLON = 39
KEY_APOSTROPHE = 40
KEY_GRAVE = 41
KEY_LEFTSHIFT = 42
KEY_BACKSLASH = 43
KEY_Z = 44
KEY_X = 45
KEY_C = 46
KEY_V = 47
KEY_B = 48
KEY_N = 49
KEY_M = 50
KEY_COMMA = 51
KEY_DOT = 52
KEY_SLASH = 53
KEY_LEFTALT = 56
KEY_SPACE = 57
KEY_F1 = 59
KEY_F2 = 60
KEY_F3 = 61
KEY_F4 = 62
KEY_F5 = 63
KEY_F6 = 64
KEY_F7 = 65
KEY_F8 = 66
KEY_F9 = 67
KEY_F10 = 68
KEY_SCROLLLOCK = 70
KEY_F11 = 87
KEY_F12 = 88
KEY_SYSRQ = 99
KEY_HOME = 102
KEY_UP = 103
KEY_PAGEUP = 104
KEY_LEFT = 105
KEY_RIGHT = 106
KEY_END = 107
KEY_DOWN = 108
KEY_PAGEDOWN = 109
KEY_DELETE = 111
KEY_LEFTMETA = 125
KEY_LINEFEED = 101

BUS_USB = 0x03
ABS_CNT = 0x40
UINPUT_MAX_NAME_SIZE = 80

IOC_NRBITS = 8
IOC_TYPEBITS = 8
IOC_SIZEBITS = 14
IOC_DIRBITS = 2
IOC_NRSHIFT = 0
IOC_TYPESHIFT = IOC_NRSHIFT + IOC_NRBITS
IOC_SIZESHIFT = IOC_TYPESHIFT + IOC_TYPEBITS
IOC_DIRSHIFT = IOC_SIZESHIFT + IOC_SIZEBITS
IOC_WRITE = 1
IOC_NONE = 0


def _ioc(direction: int, type_: int, nr: int, size: int) -> int:
    return (
        (direction << IOC_DIRSHIFT)
        | (type_ << IOC_TYPESHIFT)
        | (nr << IOC_NRSHIFT)
        | (size << IOC_SIZESHIFT)
    )


def _io(type_: str, nr: int) -> int:
    return _ioc(IOC_NONE, ord(type_), nr, 0)


def _iow(type_: str, nr: int, size: int) -> int:
    return _ioc(IOC_WRITE, ord(type_), nr, size)


UI_DEV_CREATE = _io("U", 1)
UI_DEV_DESTROY = _io("U", 2)
UI_SET_EVBIT = _iow("U", 100, 4)
UI_SET_KEYBIT = _iow("U", 101, 4)
UI_SET_RELBIT = _iow("U", 102, 4)


SPECIAL_KEYS = {
    1: KEY_BACKSPACE,
    2: KEY_TAB,
    3: KEY_LINEFEED,
    4: KEY_LEFT,
    5: KEY_UP,
    6: KEY_RIGHT,
    7: KEY_DOWN,
    8: KEY_PAGEUP,
    9: KEY_PAGEDOWN,
    10: KEY_HOME,
    11: KEY_END,
    12: KEY_ENTER,
    13: KEY_DELETE,
    14: KEY_ESC,
    15: KEY_SYSRQ,
    16: KEY_SCROLLLOCK,
    21: KEY_F1,
    22: KEY_F2,
    23: KEY_F3,
    24: KEY_F4,
    25: KEY_F5,
    26: KEY_F6,
    27: KEY_F7,
    28: KEY_F8,
    29: KEY_F9,
    30: KEY_F10,
    31: KEY_F11,
    32: KEY_F12,
}


TEXT_KEYS = {
    "a": (KEY_A, False),
    "b": (KEY_B, False),
    "c": (KEY_C, False),
    "d": (KEY_D, False),
    "e": (KEY_E, False),
    "f": (KEY_F, False),
    "g": (KEY_G, False),
    "h": (KEY_H, False),
    "i": (KEY_I, False),
    "j": (KEY_J, False),
    "k": (KEY_K, False),
    "l": (KEY_L, False),
    "m": (KEY_M, False),
    "n": (KEY_N, False),
    "o": (KEY_O, False),
    "p": (KEY_P, False),
    "q": (KEY_Q, False),
    "r": (KEY_R, False),
    "s": (KEY_S, False),
    "t": (KEY_T, False),
    "u": (KEY_U, False),
    "v": (KEY_V, False),
    "w": (KEY_W, False),
    "x": (KEY_X, False),
    "y": (KEY_Y, False),
    "z": (KEY_Z, False),
    "1": (KEY_1, False),
    "2": (KEY_2, False),
    "3": (KEY_3, False),
    "4": (KEY_4, False),
    "5": (KEY_5, False),
    "6": (KEY_6, False),
    "7": (KEY_7, False),
    "8": (KEY_8, False),
    "9": (KEY_9, False),
    "0": (KEY_0, False),
    " ": (KEY_SPACE, False),
    "\n": (KEY_ENTER, False),
    "\t": (KEY_TAB, False),
    "-": (KEY_MINUS, False),
    "=": (KEY_EQUAL, False),
    "[": (KEY_LEFTBRACE, False),
    "]": (KEY_RIGHTBRACE, False),
    ";": (KEY_SEMICOLON, False),
    "'": (KEY_APOSTROPHE, False),
    "`": (KEY_GRAVE, False),
    "\\": (KEY_BACKSLASH, False),
    ",": (KEY_COMMA, False),
    ".": (KEY_DOT, False),
    "/": (KEY_SLASH, False),
    "!": (KEY_1, True),
    "@": (KEY_2, True),
    "#": (KEY_3, True),
    "$": (KEY_4, True),
    "%": (KEY_5, True),
    "^": (KEY_6, True),
    "&": (KEY_7, True),
    "*": (KEY_8, True),
    "(": (KEY_9, True),
    ")": (KEY_0, True),
    "_": (KEY_MINUS, True),
    "+": (KEY_EQUAL, True),
    "{": (KEY_LEFTBRACE, True),
    "}": (KEY_RIGHTBRACE, True),
    ":": (KEY_SEMICOLON, True),
    '"': (KEY_APOSTROPHE, True),
    "~": (KEY_GRAVE, True),
    "|": (KEY_BACKSLASH, True),
    "<": (KEY_COMMA, True),
    ">": (KEY_DOT, True),
    "?": (KEY_SLASH, True),
}
for _letter, (_code, _shift) in list(TEXT_KEYS.items()):
    if _letter.isalpha() and len(_letter) == 1 and _letter.lower() == _letter:
        TEXT_KEYS[_letter.upper()] = (_code, True)


@dataclass(frozen=True)
class Op:
    type: int
    code: int
    value: int


def unavailable(message: str) -> None:
    print(f"seat-remote-input: {message}", file=sys.stderr)
    raise SystemExit(EXIT_UNAVAILABLE)


def unsupported(message: str) -> None:
    print(f"seat-remote-input: {message}", file=sys.stderr)
    raise SystemExit(EXIT_UNSUPPORTED)


def bool_field(obj: dict, key: str) -> bool:
    value = obj.get(key, False)
    if not isinstance(value, bool):
        unsupported(f"{key} must be boolean")
    return value


def modifiers(obj: dict) -> list[int]:
    raw = obj.get("modifiers", {})
    if raw is None:
        raw = {}
    if not isinstance(raw, dict):
        unsupported("modifiers must be an object")
    keys: list[int] = []
    if bool_field(raw, "ctrl"):
        keys.append(KEY_LEFTCTRL)
    if bool_field(raw, "alt"):
        keys.append(KEY_LEFTALT)
    if bool_field(raw, "shift"):
        keys.append(KEY_LEFTSHIFT)
    if bool_field(raw, "super"):
        keys.append(KEY_LEFTMETA)
    return keys


def tap_key(code: int) -> list[Op]:
    return [Op(EV_KEY, code, 1), Op(EV_SYN, SYN_REPORT, 0), Op(EV_KEY, code, 0), Op(EV_SYN, SYN_REPORT, 0)]


def with_modifiers(mods: Iterable[int], body: list[Op]) -> list[Op]:
    mod_list = list(dict.fromkeys(mods))
    ops: list[Op] = []
    for code in mod_list:
        ops.extend([Op(EV_KEY, code, 1), Op(EV_SYN, SYN_REPORT, 0)])
    ops.extend(body)
    for code in reversed(mod_list):
        ops.extend([Op(EV_KEY, code, 0), Op(EV_SYN, SYN_REPORT, 0)])
    return ops


def integer_delta(value: object, key: str, *, allow_zero: bool = False) -> int:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        unsupported(f"{key} must be numeric")
    delta = int(round(float(value)))
    if delta == 0 and not allow_zero:
        unsupported(f"{key} rounds to zero")
    if abs(delta) > 4096:
        unsupported(f"{key} is out of bounds")
    return delta


def event_to_ops(event: dict) -> list[Op]:
    if not isinstance(event, dict):
        unsupported("event must be a JSON object")
    kind = event.get("kind")
    if kind == "move":
        dx = integer_delta(event.get("dx"), "dx", allow_zero=True)
        dy = integer_delta(event.get("dy"), "dy", allow_zero=True)
        if dx == 0 and dy == 0:
            unsupported("move rounds to zero")
        return [Op(EV_REL, REL_X, dx), Op(EV_REL, REL_Y, dy), Op(EV_SYN, SYN_REPORT, 0)]
    if kind == "scroll":
        delta = integer_delta(event.get("delta"), "delta")
        return [Op(EV_REL, REL_WHEEL, delta), Op(EV_SYN, SYN_REPORT, 0)]
    if kind == "button":
        button = event.get("button")
        clicks = event.get("clicks")
        if button == "primary":
            code = BTN_LEFT
        elif button == "secondary":
            code = BTN_RIGHT
        elif button == "middle":
            code = BTN_MIDDLE
        else:
            unsupported("button must be primary, secondary, or middle")
        if isinstance(clicks, bool) or clicks not in (1, 2):
            unsupported("clicks must be 1 or 2")
        ops: list[Op] = []
        for _ in range(int(clicks)):
            ops.extend([Op(EV_KEY, code, 1), Op(EV_SYN, SYN_REPORT, 0), Op(EV_KEY, code, 0), Op(EV_SYN, SYN_REPORT, 0)])
        return ops
    if kind == "special_key":
        code = event.get("special_key")
        if not isinstance(code, int):
            unsupported("special_key must be an integer")
        keycode = SPECIAL_KEYS.get(code)
        if not keycode:
            unsupported(f"unsupported KDE Connect special_key {code}")
        return with_modifiers(modifiers(event), tap_key(keycode))
    if kind == "text":
        text = event.get("text")
        if not isinstance(text, str) or not text:
            unsupported("text must be a non-empty string")
        ops: list[Op] = []
        event_mods = modifiers(event)
        for ch in text:
            mapped = TEXT_KEYS.get(ch)
            if mapped is None:
                unsupported(f"unsupported text character U+{ord(ch):04X}")
            code, needs_shift = mapped
            char_mods = list(event_mods)
            if needs_shift and KEY_LEFTSHIFT not in char_mods:
                char_mods.append(KEY_LEFTSHIFT)
            ops.extend(with_modifiers(char_mods, tap_key(code)))
        return ops
    unsupported("unsupported event kind")
    return []


def ioctl_set(fd: int, request: int, value: int) -> None:
    fcntl.ioctl(fd, request, value)


def create_device(fd: int, ops: Iterable[Op]) -> None:
    ioctl_set(fd, UI_SET_EVBIT, EV_SYN)
    need_key = False
    need_rel = False
    key_codes = set()
    rel_codes = set()
    for op in ops:
        if op.type == EV_KEY:
            need_key = True
            key_codes.add(op.code)
        elif op.type == EV_REL:
            need_rel = True
            rel_codes.add(op.code)
    if need_key:
        ioctl_set(fd, UI_SET_EVBIT, EV_KEY)
        for code in sorted(key_codes):
            ioctl_set(fd, UI_SET_KEYBIT, code)
    if need_rel:
        ioctl_set(fd, UI_SET_EVBIT, EV_REL)
        for code in sorted(rel_codes):
            ioctl_set(fd, UI_SET_RELBIT, code)

    name = b"mackesd-seat-remote-input"
    user_dev = struct.pack(
        "80sHHHHI" + "i" * (ABS_CNT * 4),
        name[: UINPUT_MAX_NAME_SIZE - 1],
        BUS_USB,
        0x4D4D,
        0x0006,
        1,
        0,
        *([0] * (ABS_CNT * 4)),
    )
    os.write(fd, user_dev)
    fcntl.ioctl(fd, UI_DEV_CREATE)
    time.sleep(0.02)


def emit(fd: int, op: Op) -> None:
    os.write(fd, struct.pack("llHHi", 0, 0, op.type, op.code, op.value))


def inject(ops: list[Op]) -> None:
    try:
        fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
    except FileNotFoundError:
        unavailable("/dev/uinput is absent")
    except PermissionError:
        unavailable("permission denied opening /dev/uinput")
    except OSError as exc:
        unavailable(f"cannot open /dev/uinput: {exc}")

    created = False
    try:
        create_device(fd, ops)
        created = True
        for op in ops:
            emit(fd, op)
    except OSError as exc:
        unavailable(f"uinput injection failed: {exc}")
    finally:
        if created:
            try:
                fcntl.ioctl(fd, UI_DEV_DESTROY)
            except OSError:
                pass
        os.close(fd)


def op_json(ops: Iterable[Op]) -> list[dict]:
    return [{"type": op.type, "code": op.code, "value": op.value} for op in ops]


def parse_event(raw: str) -> dict:
    try:
        event = json.loads(raw)
    except json.JSONDecodeError as exc:
        unsupported(f"invalid JSON: {exc}")
    if not isinstance(event, dict):
        unsupported("event must be a JSON object")
    return event


def self_test() -> None:
    samples = [
        ({"kind": "move", "dx": 12.4, "dy": -2.0}, [(EV_REL, REL_X, 12), (EV_REL, REL_Y, -2)]),
        ({"kind": "scroll", "delta": -3}, [(EV_REL, REL_WHEEL, -3)]),
        ({"kind": "button", "button": "secondary", "clicks": 2}, [(EV_KEY, BTN_RIGHT, 1), (EV_KEY, BTN_RIGHT, 0)]),
        ({"kind": "text", "text": "Az!"}, [(EV_KEY, KEY_LEFTSHIFT, 1), (EV_KEY, KEY_A, 1), (EV_KEY, KEY_Z, 1), (EV_KEY, KEY_1, 1)]),
        ({"kind": "special_key", "special_key": 12, "modifiers": {"ctrl": True}}, [(EV_KEY, KEY_LEFTCTRL, 1), (EV_KEY, KEY_ENTER, 1)]),
    ]
    for event, expected_fragments in samples:
        ops = event_to_ops(event)
        triples = [(op.type, op.code, op.value) for op in ops]
        for fragment in expected_fragments:
            if fragment not in triples:
                raise AssertionError(f"{event} missing {fragment} in {triples}")
    with contextlib.redirect_stderr(io.StringIO()):
        try:
            event_to_ops({"kind": "text", "text": "\u2603"})
        except SystemExit as exc:
            if exc.code != EXIT_UNSUPPORTED:
                raise
        else:
            raise AssertionError("unsupported unicode text should fail")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Inject one mackesd remote-input JSON event through uinput")
    parser.add_argument("--dry-run", action="store_true", help="validate and print the uinput event plan without opening /dev/uinput")
    parser.add_argument("--self-test", action="store_true", help="run mapping self-tests without touching /dev/uinput")
    parser.add_argument("event", nargs="?", help="SeatRemoteInputEvent JSON")
    args = parser.parse_args(argv)

    if args.self_test:
        self_test()
        return 0
    if not args.event:
        parser.error("event JSON is required")
    event = parse_event(args.event)
    ops = event_to_ops(event)
    if args.dry_run:
        print(json.dumps({"ok": True, "ops": op_json(ops)}, separators=(",", ":")))
    else:
        inject(ops)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
