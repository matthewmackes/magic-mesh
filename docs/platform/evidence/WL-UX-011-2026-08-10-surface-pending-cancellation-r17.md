# WL-UX-011 Surface pending-action cancellation checkpoint (r17)

> **SUPERSEDED / CORRECTED FORWARD:** adversarial review after this checkpoint
> found that its Bus-backed claim/result recovery could treat cross-UID-writable
> rows as authority and had crash/publication gaps. Commit `5deafa79` replaces
> that design with a root-owned descriptor-anchored journal. Use r18 for the
> current result; r17 is retained only as the historical pre-correction record.

Date: 2026-08-10
Implementation commit: `03f9db21`

## Result

Surface enable/MOK and exact-device firmware apply now expose a separately
signed cancellation path that can claim only an exact request which the local
worker has not yet claimed for effects. A cancellation never interrupts MOK,
service, fwupd, or firmware work after execution begins, and a refused or late
cancellation does not replace the action's eventual result.

This is a runtime checkpoint, not physical Surface acceptance or production
promotion.

## Operational boundaries

- The shared bounded contract binds cancellation id, original request id,
  node, exact Pro 5/6 model, action kind, and—when applicable—the exact
  firmware device, inventory generation, release, and checksum.
- Enable and firmware workers authenticate the exact cancellation body before
  writing a durable claim and atomically consume the original action
  capability before reporting `Cancelled`.
- A durable cancellation claim can be authenticated and completed after a
  daemon crash without reviving an expired capability or accepting a modified
  body. Restart replay emits one terminal result.
- Existing action claims always win and return the closed `TooLate` refusal;
  running effects are never stopped.
- The Surface card offers cancellation only while its correlated firmware
  request remains pending. It clears the pending slot only for an exact
  `Cancelled` result and keeps the original request visible after refusal.
- The root-only `surface-mok-cancel` command mints a distinct 30-second,
  exact-target capability and publishes it to the local Bus.

## Focused farm verification

- machine 9 (`172.20.0.90`): daemon cancellation suite, 11/11 passed;
- machine 193 lane (`172.20.0.170`): shared cancellation contract, 3/3 passed;
- machine 193 lane (`172.20.0.170`): firmware-card cancellation consumer,
  1/1 passed;
- machine 194 lane (`172.20.0.50`): root CLI cancellation signing and target
  binding, 1/1 passed;
- machine 193 lane: exact-file `rustfmt --check` passed for all seven changed
  implementation files.

The daemon restart/crash test was also run alone after removing a temporary
diagnostic assertion: 1/1 passed. Machine 196 refused a duplicate contract run
because `/home` was below the farm safety threshold; no unknown slot was
deleted to manufacture capacity, and the same contract was proven on machine
193 instead.

No physical result is inferred from these tests. Release 30 installation and
canonical five-seat verification remain required.
